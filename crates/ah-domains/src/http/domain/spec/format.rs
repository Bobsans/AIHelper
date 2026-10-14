//! The `.http.yaml` spec file as it is written on disk, and reading one.

use super::*;

#[derive(Debug, Deserialize)]
pub(crate) struct HttpSpec {
    pub(super) version: u32,
    #[serde(default)]
    pub(super) defaults: SpecDefaults,
    #[serde(default)]
    pub(super) vars: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) cases: Vec<SpecCase>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SpecDefaults {
    pub(super) base_url: Option<String>,
    pub(super) timeout_secs: Option<u64>,
    pub(super) max_response_bytes: Option<usize>,
    #[serde(default)]
    pub(super) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) query: BTreeMap<String, String>,
    pub(super) bearer: Option<String>,
    pub(super) basic: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SpecCase {
    pub(super) name: String,
    pub(super) request: SpecRequest,
    #[serde(default)]
    pub(super) expect: SpecExpect,
    #[serde(default)]
    pub(super) extract: BTreeMap<String, SpecExtractRule>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct SpecRequest {
    pub(super) method: Option<String>,
    pub(super) path: Option<String>,
    pub(super) url: Option<String>,
    #[serde(default)]
    pub(super) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) query: BTreeMap<String, String>,
    pub(super) timeout_secs: Option<u64>,
    pub(super) max_response_bytes: Option<usize>,
    pub(super) bearer: Option<String>,
    pub(super) basic: Option<String>,
    pub(super) json: Option<Value>,
    pub(super) json_file: Option<String>,
    pub(super) body: Option<String>,
    pub(super) body_file: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct SpecExpect {
    pub(super) status: Option<SpecStatusValue>,
    #[serde(default)]
    pub(super) headers: BTreeMap<String, String>,
    pub(super) body_contains: Option<OneOrManyStrings>,
    #[serde(default)]
    pub(super) json: Vec<SpecJsonCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum SpecStatusValue {
    Number(u16),
    Text(String),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum OneOrManyStrings {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
pub(super) struct SpecJsonCheck {
    pub(super) path: String,
    pub(super) eq: Option<Value>,
    pub(super) contains: Option<Value>,
    pub(super) exists: Option<bool>,
    #[serde(rename = "match")]
    pub(super) regex: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SpecExtractRule {
    pub(super) json: Option<String>,
    pub(super) header: Option<String>,
    pub(super) text: Option<SpecTextExtract>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpecTextExtract {
    pub(super) regex: String,
    #[serde(default = "default_extract_group")]
    pub(super) group: usize,
}

pub(super) fn default_extract_group() -> usize {
    1
}

pub(crate) fn read_spec_file(path: &Path) -> Result<HttpSpec, AppError> {
    let raw = crate::http::io::read_to_string(path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if extension == "json" {
        serde_json::from_str(&raw).map_err(|error| {
            AppError::invalid_argument(format!(
                "failed to parse spec JSON '{}': {error}",
                path.display()
            ))
        })
    } else {
        yaml_serde::from_str(&raw).map_err(|error| {
            AppError::invalid_argument(format!(
                "failed to parse spec YAML '{}': {error}",
                path.display()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_yaml(input: &str) -> Result<HttpSpec, AppError> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scenario.http.yaml");
        std::fs::write(&path, input).unwrap();
        read_spec_file(&path)
    }

    #[test]
    fn yaml_specs_preserve_scalar_types_and_aliases() {
        let spec = read_yaml(
            r#"
version: 1
vars:
  country: NO
  switch: on
  count: 42
  leading: 0123
  message: "Привет: # YAML"
defaults:
  base_url: &origin https://example.test
  headers: &headers
    X-Flag: off
    X-Count: 42
cases:
  - name: scalar-contract
    request:
      url: *origin
      headers: *headers
      body: |
        line one
        line two
      json:
        plain: on
        boolean: true
        quoted: "true"
        leading: 0123
        binary: 0b11
        maximum: 18446744073709551615
        overflow: 1e999
        list: [null, false, "NO"]
    expect:
      status: "2xx"
      body_contains: [ready, "Привет"]
      json:
        - path: boolean
          eq: true
          exists: false
"#,
        )
        .unwrap();

        assert_eq!(spec.version, 1);
        assert_eq!(spec.vars["country"], "NO");
        assert_eq!(spec.vars["switch"], "on");
        assert_eq!(spec.vars["count"], "42");
        assert_eq!(spec.vars["leading"], "0123");
        assert_eq!(spec.vars["message"], "Привет: # YAML");
        assert_eq!(spec.cases.len(), 1);
        let case = &spec.cases[0];
        assert_eq!(case.request.url, spec.defaults.base_url);
        assert_eq!(case.request.headers, spec.defaults.headers);
        assert_eq!(case.request.headers["X-Count"], "42");
        assert_eq!(case.request.headers["X-Flag"], "off");
        assert_eq!(case.request.body.as_deref(), Some("line one\nline two\n"));
        assert_eq!(
            case.request.json,
            Some(serde_json::json!({
                "plain": "on",
                "boolean": true,
                "quoted": "true",
                "leading": "0123",
                "binary": 3,
                "maximum": u64::MAX,
                "overflow": "1e999",
                "list": [null, false, "NO"],
            }))
        );
        assert!(matches!(
            case.expect.status.as_ref(),
            Some(SpecStatusValue::Text(value)) if value == "2xx"
        ));
        assert_eq!(case.expect.json[0].eq, Some(Value::Bool(true)));
        assert_eq!(case.expect.json[0].exists, Some(false));
    }

    #[test]
    fn yaml_specs_keep_last_duplicate_map_value() {
        let spec = read_yaml("version: 1\nvars:\n  token: first\n  token: second\n").unwrap();
        assert_eq!(spec.vars["token"], "second");
    }

    #[test]
    fn yaml_specs_reject_invalid_documents_without_changing_error_code() {
        for input in [
            "version: 1\ncases: [\n",
            "version: 1\nversion: 2\n",
            "version: 1\n---\nversion: 1\n",
            "version: 1\nvars:\n  ? [one, two]\n  : value\n",
            "version: 1\ncases:\n  - name: tagged\n    request:\n      json: !Custom value\n",
            "version: 1\ncases:\n  - name: invalid-extract\n    request: {}\n    extract:\n      token:\n        json: $.token\n        unknown: true\n",
        ] {
            let error = read_yaml(input).unwrap_err();
            assert_eq!(error.code(), "INVALID_ARGUMENT", "{input}");
            assert!(error.detail_message().contains("failed to parse spec YAML"));
        }
    }
}
