//! What a plugin says it can do: commands, their declared effects, their risk,
//! and the secrets they need.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandEffect {
    FilesystemRead,
    FilesystemWrite,
    FilesystemDelete,
    ProcessSpawn,
    NetworkRead,
    NetworkWrite,
    ConfigurationRead,
    ConfigurationWrite,
    ExternalRead,
    ExternalWrite,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandEffects {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
    pub effects: Vec<CommandEffect>,
    pub risk: RiskLevel,
    pub impact: String,
    pub reversibility: Reversibility,
}

impl CommandEffects {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        read_only: bool,
        destructive: bool,
        idempotent: bool,
        open_world: bool,
        effects: Vec<CommandEffect>,
        risk: RiskLevel,
        impact: impl Into<String>,
        reversibility: Reversibility,
    ) -> Self {
        Self {
            read_only,
            destructive,
            idempotent,
            open_world,
            effects,
            risk,
            impact: impact.into(),
            reversibility,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandExample {
    pub description: String,
    pub arguments: serde_json::Value,
}

impl CommandExample {
    pub fn new(description: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            description: description.into(),
            arguments,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretSlot {
    pub name: String,
    pub accepted_kinds: Vec<String>,
    pub required: bool,
    pub description: String,
}

impl SecretSlot {
    pub fn optional(
        name: impl Into<String>,
        accepted_kinds: impl IntoIterator<Item = impl Into<String>>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            accepted_kinds: accepted_kinds.into_iter().map(Into::into).collect(),
            required: false,
            description: description.into(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedSecret {
    pub id: String,
    pub kind: String,
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandDescriptor {
    pub id: String,
    pub title: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub effects: CommandEffects,
    #[serde(default)]
    pub examples: Vec<CommandExample>,
    #[serde(default)]
    pub secret_slots: Vec<SecretSlot>,
}

impl CommandDescriptor {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        output_schema: serde_json::Value,
        effects: CommandEffects,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: description.into(),
            input_schema,
            output_schema,
            effects,
            examples: Vec::new(),
            secret_slots: Vec::new(),
        }
    }

    pub fn with_example(mut self, example: CommandExample) -> Self {
        self.examples.push(example);
        self
    }

    pub fn with_secret_slot(mut self, slot: SecretSlot) -> Self {
        self.secret_slots.push(slot);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandCatalog {
    pub plugin_name: String,
    pub domain: String,
    pub commands: Vec<CommandDescriptor>,
}

impl CommandCatalog {
    pub fn new(
        plugin_name: impl Into<String>,
        domain: impl Into<String>,
        commands: Vec<CommandDescriptor>,
    ) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            domain: domain.into(),
            commands,
        }
    }
}
