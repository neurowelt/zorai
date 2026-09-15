use uuid::Uuid;

pub type SessionId = Uuid;
pub type WorkspaceId = String;

pub const AGENT_ID_SWAROG: &str = "swarog";
pub const AGENT_NAME_SVAROG: &str = "Svarog";
pub const AGENT_NAME_SWAROG: &str = AGENT_NAME_SVAROG;
pub const AGENT_ID_RAROG: &str = "rarog";
pub const AGENT_NAME_RAROG: &str = "Rarog";
pub const AGENT_HANDLE_SVAROG: &str = "svarog";
pub const AGENT_HANDLE_SWAROG_LEGACY: &str = AGENT_ID_SWAROG;
pub const AGENT_HANDLE_RAROG: &str = AGENT_ID_RAROG;
pub const AGENT_HANDLE_MAIN: &str = "main";
pub const AGENT_HANDLE_CONCIERGE: &str = "concierge";
pub const GATEWAY_IPC_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq, Eq)]
pub struct GoalAgentAssignment {
    pub role_id: String,
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub inherit_from_main: bool,
}

mod mcp;
pub use mcp::*;

mod client;
mod daemon;
mod gateway;
mod support;

pub use client::ClientMessage;
pub use daemon::DaemonMessage;
pub use gateway::*;
pub use support::*;

#[cfg(test)]
mod tests;
