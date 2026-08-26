//! Who the plugin is, which API version it was built against, and the manual it
//! ships.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl PluginApiVersion {
    pub fn current() -> Self {
        Self {
            major: AH_PLUGIN_API_MAJOR_VERSION,
            minor: AH_PLUGIN_API_MINOR_VERSION,
        }
    }

    pub fn is_compatible_with_host(&self) -> bool {
        let host = Self::current();
        self.major == host.major && self.minor <= host.minor
    }
}

impl Default for PluginApiVersion {
    fn default() -> Self {
        Self::current()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCompatibility {
    #[serde(default)]
    pub api_version: PluginApiVersion,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl PluginCompatibility {
    pub fn current() -> Self {
        Self::default()
    }

    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities
            .iter()
            .any(|candidate| candidate == capability)
    }

    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        let capability = capability.into();
        if !self.supports(&capability) {
            self.capabilities.push(capability);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub plugin_name: String,
    pub domain: String,
    pub description: String,
    pub abi_version: u32,
    #[serde(default)]
    pub required_tools: Vec<RequiredTool>,
    #[serde(default)]
    pub compatibility: PluginCompatibility,
}

impl PluginMetadata {
    pub fn supports_capability(&self, capability: &str) -> bool {
        self.compatibility.supports(capability)
    }

    pub fn is_api_compatible_with_host(&self) -> bool {
        self.compatibility.api_version.is_compatible_with_host()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RequiredTool {
    pub name: String,
    pub check_args: Vec<String>,
    pub reason: String,
}

impl RequiredTool {
    pub fn new(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            check_args: vec!["--version".to_owned()],
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ManualExample {
    pub description: String,
    pub argv: Vec<String>,
}

impl ManualExample {
    /// Every manual entry builds its examples the same way, from a description
    /// and a borrowed argv.
    pub fn new(description: &str, argv: &[&str]) -> Self {
        Self {
            description: description.to_owned(),
            argv: argv.iter().map(|item| (*item).to_owned()).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ManualCommand {
    pub name: String,
    pub summary: String,
    pub usage: String,
    pub examples: Vec<ManualExample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PluginManual {
    pub plugin_name: String,
    pub domain: String,
    pub description: String,
    pub commands: Vec<ManualCommand>,
    pub notes: Vec<String>,
}
