use crate::{MAX_MANIFEST_BYTES, ManifestError, ReleaseManifest};

impl ReleaseManifest {
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate()?;
        serialize_canonical(self)
    }
}

pub(crate) fn decode_canonical_untrusted(input: &[u8]) -> Result<ReleaseManifest, ManifestError> {
    if input.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::InputTooLarge {
            actual: input.len(),
            maximum: MAX_MANIFEST_BYTES,
        });
    }

    let manifest = serde_json::from_slice::<ReleaseManifest>(input).map_err(|error| {
        ManifestError::MalformedJson {
            detail: error.to_string(),
        }
    })?;
    if serialize_canonical(&manifest)? != input {
        return Err(ManifestError::NonCanonicalEncoding);
    }
    Ok(manifest)
}

fn serialize_canonical(manifest: &ReleaseManifest) -> Result<Vec<u8>, ManifestError> {
    serde_json::to_vec(manifest).map_err(|error| ManifestError::MalformedJson {
        detail: format!("canonical serialization failed: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/windows-x64.manifest.json");

    #[test]
    fn decodes_canonical_bytes_without_semantic_validation() {
        let parsed = decode_canonical_untrusted(FIXTURE).unwrap();
        assert_eq!(parsed.schema_version, 1);

        let mut semantically_invalid = parsed;
        semantically_invalid.schema_version = 2;
        semantically_invalid.files.swap(0, 1);
        let raw = serde_json::to_vec(&semantically_invalid).unwrap();

        let decoded = decode_canonical_untrusted(&raw).unwrap();
        assert_eq!(decoded.schema_version, 2);
        assert!(decoded.validate().is_err());
    }

    #[test]
    fn rejects_input_over_the_byte_limit_before_parsing() {
        let oversized = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert_eq!(
            decode_canonical_untrusted(&oversized),
            Err(ManifestError::InputTooLarge {
                actual: MAX_MANIFEST_BYTES + 1,
                maximum: MAX_MANIFEST_BYTES,
            })
        );
    }

    #[test]
    fn rejects_non_canonical_textual_encodings() {
        let canonical = std::str::from_utf8(FIXTURE).unwrap();
        let release = "\"release\":{\"version\":\"1.2.0\",\"target\":\"x86_64-pc-windows-msvc\",\"architecture\":\"x86_64\"}";
        let variants = [
            format!(" {canonical}"),
            format!("{canonical}\n"),
            serde_json::to_string_pretty(
                &serde_json::from_slice::<ReleaseManifest>(FIXTURE).unwrap(),
            )
            .unwrap(),
            canonical.replacen("ah.exe", r"ah\u002eexe", 1),
            canonical.replacen(
                &format!("\"schema_version\":1,{release}"),
                &format!("{release},\"schema_version\":1"),
                1,
            ),
        ];

        for variant in variants {
            assert_eq!(
                decode_canonical_untrusted(variant.as_bytes()),
                Err(ManifestError::NonCanonicalEncoding),
                "{variant}"
            );
        }

        assert!(matches!(
            decode_canonical_untrusted(format!("\u{feff}{canonical}").as_bytes()),
            Err(ManifestError::MalformedJson { .. })
        ));
    }
}
