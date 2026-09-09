//! Daemon-owned outbound MCP. No process CWD or live configuration is consulted here.
mod catalog;
mod config;
mod context;
mod credentials;
mod manager;
mod result;
mod transport;

pub use catalog::{McpCatalogSnapshot, McpToolRoute};
pub use config::normalize_config;
pub use context::{McpCallError, McpRequestContext};
pub use manager::McpManager;
pub use result::McpCallOutcome;
pub use zorai_protocol::{
    McpAdapterPolicy, McpAuthConfig, McpCredentialUpdate, McpServerConfig, McpServerStatus,
};

#[cfg(test)]
pub(crate) mod tests;
