use serde_json::{Map, Value};
use std::{collections::HashSet, path::Path};

pub const CAPABILITY: &str = "ai.zorai/workspace-context";
pub const WORKSPACE: &str = "ai.zorai/workspace";
pub const CONVERSATION: &str = "ai.zorai/conversation";

#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct McpCallError {
    pub code: &'static str,
    pub message: String,
}
impl McpCallError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct McpRequestContext {
    pub thread_id: String,
    pub workspace_uri: Option<url::Url>,
    pub error: Option<McpCallError>,
}
impl McpRequestContext {
    pub fn from_workspace(thread_id: impl Into<String>, workspace: Option<&Path>) -> Self {
        let mut context = Self {
            thread_id: thread_id.into(),
            workspace_uri: None,
            error: None,
        };
        if let Some(path) = workspace {
            let resolved = if path.is_absolute() {
                path.canonicalize().ok().filter(|p| p.is_dir())
            } else {
                None
            };
            context.workspace_uri = resolved.and_then(|p| url::Url::from_directory_path(p).ok());
            if context.workspace_uri.is_none() {
                context.error = Some(McpCallError::new("MCP_WORKSPACE_INVALID", "MCP CALL BLOCKED: The bound workspace is not an existing absolute directory. Start a new conversation from its TUI directory. No request was sent; daemon CWD was not used."));
            }
        }
        context
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ContextSupport {
    pub supported: bool,
    pub required: HashSet<String>,
}
impl ContextSupport {
    pub fn parse(capabilities: &Value) -> Self {
        let extension = &capabilities["experimental"][CAPABILITY];
        let requirements_valid = extension.get("requiredForTools").is_none_or(|value| {
            value.as_array().is_some_and(|items| {
                items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|name| !name.is_empty()))
            })
        });
        let supported = extension["version"].as_u64() == Some(1) && requirements_valid;
        // Even unsupported versions may restrict tools. They never authorize disclosure.
        let required = extension["requiredForTools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        Self {
            supported,
            required,
        }
    }
    pub fn metadata(
        &self,
        sharing: bool,
        required: bool,
        context: &McpRequestContext,
    ) -> Result<Map<String, Value>, McpCallError> {
        if required && !self.supported {
            return Err(McpCallError::new("MCP_CONTEXT_UNSUPPORTED", "MCP CALL BLOCKED: Server does not advertise workspace-context v1. Update the server and reconnect in Settings → MCP. No request was sent."));
        }
        if required && !sharing {
            return Err(McpCallError::new("MCP_CONTEXT_SHARING_DISABLED", "MCP CALL BLOCKED: Workspace sharing is off for this server. Enable Share workspace context (path and conversation ID) in Settings → MCP. No request was sent."));
        }
        let mut meta = Map::new();
        if !self.supported || !sharing {
            return Ok(meta);
        }
        if let Some(error) = &context.error {
            return Err(error.clone());
        }
        if required && (context.workspace_uri.is_none() || context.thread_id.is_empty()) {
            return Err(McpCallError::new("MCP_WORKSPACE_MISSING", "MCP CALL BLOCKED: This conversation has no known workspace or conversation ID. Start a new conversation from the workspace's TUI directory. No request was sent."));
        }
        if let Some(uri) = &context.workspace_uri {
            if uri.scheme() != "file"
                || uri
                    .host_str()
                    .is_some_and(|h| !h.is_empty() && h != "localhost")
                || uri
                    .to_file_path()
                    .ok()
                    .filter(|p| p.is_absolute() && p.is_dir())
                    .is_none()
            {
                return Err(McpCallError::new("MCP_WORKSPACE_INVALID", "MCP CALL BLOCKED: Invalid or stale local workspace URI. Start a new conversation from its TUI directory. No request was sent; daemon CWD was not used."));
            }
            meta.insert(WORKSPACE.into(), Value::String(uri.to_string()));
        }
        if !context.thread_id.is_empty() {
            meta.insert(
                CONVERSATION.into(),
                Value::String(context.thread_id.clone()),
            );
        }
        Ok(meta)
    }
}
