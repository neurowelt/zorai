//! MCP-owned runtime context; deliberately independent of shell CWD selection.
use super::*;
use crate::mcp_client::{McpCallError, McpRequestContext};
use std::path::Path;

impl AgentEngine {
    /// A configured MCP integration must use the normal agent loop so its tools are
    /// advertised and executable, including status discovery while disconnected. The lightweight concierge reply path
    /// intentionally has no tool loop.
    pub(crate) fn has_mcp_servers(&self) -> bool {
        !self.mcp.catalog_snapshot().servers.is_empty()
    }

    pub(crate) async fn save_mcp_server(
        &self,
        server: zorai_protocol::McpServerConfig,
        credential: zorai_protocol::McpCredentialUpdate,
    ) -> std::result::Result<(), String> {
        let _guard = self.mcp_config_lock.lock().await;
        self.save_mcp_server_locked(server, credential).await
    }

    pub(crate) async fn remove_mcp_server(&self, id: &str) -> std::result::Result<(), String> {
        let _guard = self.mcp_config_lock.lock().await;
        let mut config = self.get_config().await;
        let previous = config.mcp_servers.clone();
        config.mcp_servers.retain(|server| server.id != id);
        if config.mcp_servers.len() == previous.len() {
            return Err("Unknown MCP server".into());
        }
        self.history
            .replace_agent_config_items(&super::config::config_to_items(&config))
            .await
            .map_err(|_| {
                "Could not remove MCP server; working configuration was retained.".to_string()
            })?;
        *self.config.write().await = config.clone();
        if let Err(error) = self.mcp.apply_desired_config(config.mcp_servers.clone()) {
            tracing::warn!(%error, "MCP reconciliation failed after removal");
        }
        if let Err(error) = self.mcp.cleanup_credentials(&previous, &config.mcp_servers) {
            tracing::warn!(%error, "MCP credential cleanup failed after removal");
        }
        self.config_notify.notify_waiters();
        Ok(())
    }

    pub(crate) async fn set_mcp_server_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> std::result::Result<(), String> {
        let _guard = self.mcp_config_lock.lock().await;
        let mut server = self
            .get_config()
            .await
            .mcp_servers
            .into_iter()
            .find(|s| s.id == id)
            .ok_or("Unknown MCP server")?;
        server.enabled = enabled;
        self.save_mcp_server_locked(server, zorai_protocol::McpCredentialUpdate::Keep)
            .await
    }

    async fn save_mcp_server_locked(
        &self,
        server: zorai_protocol::McpServerConfig,
        credential: zorai_protocol::McpCredentialUpdate,
    ) -> std::result::Result<(), String> {
        let mut config = self.get_config().await;
        let previous = config.mcp_servers.clone();
        let old = config.mcp_servers.iter().find(|s| s.id == server.id);
        let mut server = crate::mcp_client::normalize_config(server, old)?;
        self.mcp.save_credential(&mut server, credential)?;
        if let Some(old) = config.mcp_servers.iter_mut().find(|s| s.id == server.id) {
            *old = server;
        } else {
            config.mcp_servers.push(server);
        }
        self.history
            .replace_agent_config_items(&super::config::config_to_items(&config))
            .await
            .map_err(|_| {
                "Could not persist MCP settings; working configuration was retained.".to_string()
            })?;
        *self.config.write().await = config.clone();
        if let Err(error) = self.mcp.apply_desired_config(config.mcp_servers.clone()) {
            tracing::warn!(%error, "MCP config reconciliation failed after settings were saved");
        }
        if let Err(error) = self.mcp.cleanup_credentials(&previous, &config.mcp_servers) {
            tracing::warn!(%error, "MCP credential cleanup failed");
        }
        self.config_notify.notify_waiters();
        Ok(())
    }

    pub(crate) async fn mcp_workspace_binding(&self, thread_id: &str) -> Option<String> {
        if let Some(root) = self.mcp_bindings.read().await.get(thread_id).cloned() {
            return Some(root);
        }
        let raw = self
            .history
            .thread_metadata_json(thread_id)
            .await
            .ok()
            .flatten()?;
        let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let root = value.get("mcp_workspace")?.as_str()?.to_string();
        self.mcp_bindings
            .write()
            .await
            .entry(thread_id.to_string())
            .or_insert(root.clone());
        Some(root)
    }

    pub(crate) async fn bind_mcp_workspace(&self, thread_id: &str, root: String) {
        if self.mcp_workspace_binding(thread_id).await.is_none() {
            self.mcp_bindings
                .write()
                .await
                .entry(thread_id.to_string())
                .or_insert(root);
        }
    }

    pub(crate) async fn mcp_metadata_json(
        &self,
        thread_id: &str,
        raw: Option<String>,
    ) -> Option<String> {
        let mut value = raw
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(root) = self.mcp_workspace_binding(thread_id).await {
            value["mcp_workspace"] = root.into();
        }
        Some(value.to_string())
    }

    pub(crate) async fn resolve_mcp_context(
        &self,
        thread_id: &str,
        session: Option<zorai_protocol::SessionId>,
    ) -> McpRequestContext {
        if let Some(id) = session {
            let sessions = self.session_manager.list().await;
            return context_from_execution_session(thread_id, id, &sessions);
        }
        let mut current = thread_id.to_string();
        let mut visited = std::collections::HashSet::new();
        while visited.insert(current.clone()) && visited.len() <= 32 {
            let workspace = self.get_thread_workspace_context(&current).await;
            let binding = self.mcp_workspace_binding(&current).await;
            if let Some(workspace) = workspace {
                if let Some(binding) = binding.as_deref() {
                    let a = std::fs::canonicalize(&workspace.root);
                    let b = std::fs::canonicalize(binding);
                    if !Path::new(binding).is_absolute() || a.is_err() || b.is_err() {
                        return mcp_context_error(thread_id, "MCP_WORKSPACE_INVALID", "A saved workspace binding is invalid or no longer exists. Start a new workspace-bound conversation. No request was sent; daemon CWD was not used.");
                    }
                    if a.ok() != b.ok() {
                        return mcp_context_error(thread_id, "MCP_WORKSPACE_CONFLICT", "Thread workspace and MCP binding disagree. Start a new conversation from the intended workspace. No request was sent; daemon CWD was not used.");
                    }
                }
                return McpRequestContext::from_workspace(
                    thread_id,
                    Some(Path::new(&workspace.root)),
                );
            }
            if let Some(binding) = binding {
                return McpRequestContext::from_workspace(thread_id, Some(Path::new(&binding)));
            }
            let identity = self
                .thread_identity_metadata
                .read()
                .await
                .get(&current)
                .cloned();
            let parent = match identity {
                Some(identity) => identity.parent_thread_id,
                None => self
                    .persisted_thread_metadata(&current)
                    .await
                    .and_then(|m| m.identity)
                    .and_then(|i| i.parent_thread_id),
            };
            let Some(parent) = parent else { break };
            current = parent;
        }
        McpRequestContext::from_workspace(thread_id, None)
    }

    pub(crate) fn effective_tools(
        &self,
        config: &AgentConfig,
        topology: bool,
    ) -> Vec<ToolDefinition> {
        self.effective_tools_from_mcp_snapshot(config, topology, &self.mcp.catalog_snapshot())
    }

    pub(crate) fn effective_tools_from_mcp_snapshot(
        &self,
        config: &AgentConfig,
        topology: bool,
        snapshot: &crate::mcp_client::McpCatalogSnapshot,
    ) -> Vec<ToolDefinition> {
        let mut tools = tool_executor::get_available_tools(config, &self.data_dir, topology);
        let mut names: std::collections::HashSet<String> =
            tools.iter().map(|t| t.function.name.clone()).collect();
        tools.extend(
            snapshot
                .tools
                .iter()
                .filter(|tool| names.insert(tool.function.name.clone()))
                .cloned(),
        );
        tools
    }

    pub(crate) async fn mcp_tool_scope_denial(
        &self,
        name: &str,
        task_id: Option<&str>,
    ) -> Option<String> {
        self.mcp_scope_filter(task_id).await(name)
    }

    pub(crate) async fn mcp_scope_filter(
        &self,
        task_id: Option<&str>,
    ) -> impl Fn(&str) -> Option<String> + use<> {
        use subagent::tool_filter::ToolFilter;
        let config = self.get_config().await;
        let local_names: std::collections::HashSet<String> =
            tool_executor::get_available_tools(&config, &self.data_dir, true)
                .into_iter()
                .map(|tool| tool.function.name)
                .collect();
        let task_filter = if let Some(task_id) = task_id {
            match tool_executor::task_by_id_for_tool_scope(self, task_id).await {
                Some(task) => ToolFilter::new(task.tool_whitelist, task.tool_blacklist)
                    .map_err(|_| "MCP execution tool scope is invalid"),
                None => Err("MCP execution task no longer exists"),
            }
        } else {
            Ok(ToolFilter::allow_all())
        };
        let scope = agent_identity::current_agent_scope_id();
        let (sub_agents, _) = config::effective_sub_agents_from_config(&config);
        let responder_filter = match sub_agents.into_iter().find(|def| def.id == scope) {
            Some(def) => ToolFilter::new(def.tool_whitelist, def.tool_blacklist)
                .map_err(|_| "MCP responder tool scope is invalid"),
            None => Ok(ToolFilter::allow_all()),
        };
        move |name: &str| {
            if local_names.contains(name) {
                return Some("MCP route collides with a local tool name".into());
            }
            for filter in [&task_filter, &responder_filter] {
                match filter {
                    Ok(filter) => {
                        if let Some(reason) = filter.deny_reason(name) {
                            return Some(reason);
                        }
                    }
                    Err(reason) => return Some((*reason).into()),
                }
            }
            None
        }
    }
}

fn mcp_context_error(thread_id: &str, code: &'static str, message: &str) -> McpRequestContext {
    McpRequestContext {
        thread_id: thread_id.into(),
        workspace_uri: None,
        error: Some(McpCallError {
            code,
            message: message.into(),
        }),
    }
}

fn context_from_execution_session(
    thread: &str,
    id: zorai_protocol::SessionId,
    sessions: &[zorai_protocol::SessionInfo],
) -> McpRequestContext {
    match sessions.iter().find(|session| session.id == id && session.is_alive) {
        Some(session) => McpRequestContext::from_workspace(thread, session.cwd.as_deref().map(Path::new)),
        None => mcp_context_error(thread, "MCP_WORKSPACE_INVALID", "The bound execution session no longer exists. Start a new workspace-bound conversation. No request was sent; daemon CWD was not used."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mcp_explicit_child_session_uses_its_worktree_and_never_first_session() {
        let parent = tempfile::tempdir().unwrap();
        let child = tempfile::tempdir().unwrap();
        let session = |root: &Path| zorai_protocol::SessionInfo {
            id: uuid::Uuid::new_v4(),
            title: None,
            cwd: Some(root.to_string_lossy().into()),
            cols: 80,
            rows: 24,
            created_at: 1,
            workspace_id: None,
            exit_code: None,
            is_alive: true,
            active_command: None,
        };
        let sessions = vec![session(parent.path()), session(child.path())];
        let context = context_from_execution_session("child", sessions[1].id, &sessions);
        assert_eq!(
            context.workspace_uri.unwrap().to_file_path().unwrap(),
            child.path().canonicalize().unwrap()
        );
        assert_eq!(context.thread_id, "child");
        let context = context_from_execution_session("child", uuid::Uuid::nil(), &sessions);
        assert!(context.workspace_uri.is_none());
        assert_eq!(context.error.unwrap().code, "MCP_WORKSPACE_INVALID");
    }
}
