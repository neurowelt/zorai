use crate::agent::types::{ToolDefinition, ToolFunctionDef};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolRoute {
    pub server_id: String,
    pub original_name: String,
    pub generation: u64,
    pub portal_collection: bool,
}
#[derive(Debug, Clone, Default)]
pub struct McpCatalogSnapshot {
    pub tools: Vec<ToolDefinition>,
    pub routes: HashMap<String, McpToolRoute>,
}

// Full original names participate in the suffix, including case and non-ASCII bytes.
// Stable IDs rather than display names prevent rename/reconnect from changing routes.
pub(super) fn public_name(server_id: &str, original: &str) -> String {
    let clean = |s: &str, max: usize| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .take(max)
            .collect::<String>()
    };
    let hash = Sha256::digest(format!("{}:{server_id}{original}", server_id.len()));
    let suffix = hash[..10]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!(
        "mcp_{}_{}_{}",
        clean(server_id, 12),
        clean(original, 24),
        suffix
    )
}
pub(super) fn definition(server: &str, tool: &rmcp::model::Tool) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".into(),
        function: ToolFunctionDef {
            name: public_name(server, &tool.name),
            description: format!(
                "External MCP tool. {}",
                tool.description.as_deref().unwrap_or("")
            ),
            parameters: serde_json::Value::Object((*tool.input_schema).clone()),
        },
    }
}
