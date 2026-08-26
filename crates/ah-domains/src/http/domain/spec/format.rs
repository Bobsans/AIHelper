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
        serde_yaml::from_str(&raw).map_err(|error| {
            AppError::invalid_argument(format!(
                "failed to parse spec YAML '{}': {error}",
                path.display()
            ))
        })
    }
}
