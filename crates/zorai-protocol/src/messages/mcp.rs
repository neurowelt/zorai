//! Native outbound MCP settings. Secrets are carried only by explicit write operations.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthConfig {
    #[default]
    None,
    Bearer {
        credential_ref: Option<String>,
    },
    ApiKey {
        header: String,
        credential_ref: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpAdapterPolicy {
    #[default]
    Generic,
    Portal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub share_workspace_context: bool,
    #[serde(default)]
    pub auth: McpAuthConfig,
    #[serde(default)]
    pub adapter: McpAdapterPolicy,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub skill: Option<String>,
}

impl McpServerConfig {
    pub fn discovery_aliases(&self) -> Vec<String> {
        let mut aliases = self.aliases.clone();
        if self.adapter == McpAdapterPolicy::Portal {
            for alias in ["portal", "companions"] {
                if !aliases
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case(alias))
                {
                    aliases.push(alias.into());
                }
            }
        }
        aliases
    }

    pub fn workflow_skill(&self) -> Option<&str> {
        self.skill
            .as_deref()
            .or_else(|| (self.adapter == McpAdapterPolicy::Portal).then_some("companions"))
    }
}

/// Agent-facing connection metadata. Deliberately excludes endpoints, credentials,
/// remote instructions, and connection diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerDirectoryEntry {
    pub server_id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub state: String,
    pub available_tool_count: usize,
    pub skill: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum McpCredentialUpdate {
    #[default]
    Keep,
    Replace(String),
    Clear,
}
impl std::fmt::Debug for McpCredentialUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Keep => "Keep",
            Self::Replace(_) => "Replace([redacted])",
            Self::Clear => "Clear",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpServerStatus {
    pub config: McpServerConfig,
    pub state: String,
    pub message: Option<String>,
    pub server_name: Option<String>,
    pub instructions: Option<String>,
    pub workspace_context_supported: bool,
    pub tools: Vec<McpToolInfo>,
    pub credential_present: bool,
}
