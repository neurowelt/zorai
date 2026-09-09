//! Settings operations use a separate draft client; they never submit business calls.
use super::*;

pub(crate) async fn dispatch_mcp(
    msg: &ClientMessage,
    agent: &Arc<crate::agent::AgentEngine>,
    framed: &mut ConnectionWriter,
) -> anyhow::Result<bool> {
    match msg {
        ClientMessage::McpListServers => {
            framed
                .send(DaemonMessage::McpServers {
                    servers: agent.mcp.statuses(),
                })
                .await?;
        }
        ClientMessage::McpTestServer {
            request_id,
            revision,
            config,
            credential,
        } => {
            let agent = agent.clone();
            let mut writer = framed.clone();
            let (request_id, revision, config, credential) = (
                request_id.clone(),
                *revision,
                config.clone(),
                credential.clone(),
            );
            tokio::spawn(async move {
                let result = agent.mcp.test_connection(config, credential).await;
                let (success, message, server) = match result {
                    Ok(status) => (
                        true,
                        "Initialization and complete tool discovery succeeded. No tool was called."
                            .into(),
                        Some(status),
                    ),
                    Err(error) => (false, error, None),
                };
                let _ = writer
                    .send(DaemonMessage::McpOperationResult {
                        request_id,
                        revision,
                        success,
                        message,
                        server,
                    })
                    .await;
            });
        }
        ClientMessage::McpSaveServer {
            request_id,
            revision,
            config,
            credential,
        } => {
            let result = agent
                .save_mcp_server(config.clone(), credential.clone())
                .await;
            let server = if result.is_ok() {
                agent
                    .mcp
                    .statuses()
                    .into_iter()
                    .find(|s| s.config.id == config.id)
            } else {
                None
            };
            framed
                .send(DaemonMessage::McpOperationResult {
                    request_id: request_id.clone(),
                    revision: *revision,
                    success: result.is_ok(),
                    message: result.err().unwrap_or_else(|| {
                        "Saved. Connection status refreshes in MCP settings.".into()
                    }),
                    server,
                })
                .await?;
        }
        ClientMessage::McpSetServerEnabled { id, enabled } => {
            if let Err(message) = agent.set_mcp_server_enabled(id, *enabled).await {
                framed.send(DaemonMessage::Error { message }).await?;
            }
            framed
                .send(DaemonMessage::McpServers {
                    servers: agent.mcp.statuses(),
                })
                .await?;
        }
        ClientMessage::McpReconnectServer { id } => {
            if let Err(message) = agent.mcp.reconnect(id).await {
                framed.send(DaemonMessage::Error { message }).await?;
            }
            framed
                .send(DaemonMessage::McpServers {
                    servers: agent.mcp.statuses(),
                })
                .await?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mcp_failed_save_does_not_attach_previous_server_status() {
        let root = tempfile::tempdir().unwrap();
        let sessions = SessionManager::new_test(root.path()).await;
        let agent = crate::agent::AgentEngine::new_test(
            sessions,
            crate::agent::types::AgentConfig::default(),
            root.path(),
        )
        .await;
        let config = zorai_protocol::McpServerConfig {
            id: "saved".into(),
            name: "Saved server".into(),
            url: "http://127.0.0.1:1/mcp".into(),
            enabled: false,
            share_workspace_context: false,
            auth: zorai_protocol::McpAuthConfig::None,
            adapter: zorai_protocol::McpAdapterPolicy::Generic,
        };
        agent
            .save_mcp_server(config.clone(), zorai_protocol::McpCredentialUpdate::Keep)
            .await
            .unwrap();
        let mut draft = config.clone();
        draft.url = "invalid-url".into();
        let (tx, mut rx) = mpsc::channel(2);
        let mut writer = ConnectionWriter::new(tx);
        dispatch_mcp(
            &ClientMessage::McpSaveServer {
                request_id: "rejected-draft".into(),
                revision: 7,
                config: draft,
                credential: zorai_protocol::McpCredentialUpdate::Keep,
            },
            &agent,
            &mut writer,
        )
        .await
        .unwrap();
        match rx.recv().await.unwrap() {
            DaemonMessage::McpOperationResult {
                success,
                server,
                request_id,
                revision,
                ..
            } => {
                assert!(!success);
                assert!(server.is_none());
                assert_eq!(request_id, "rejected-draft");
                assert_eq!(revision, 7);
            }
            other => panic!("unexpected response: {other:?}"),
        }
        assert_eq!(agent.get_config().await.mcp_servers, vec![config]);
    }
}
