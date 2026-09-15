use super::*;
use zorai_protocol::{ClientSurface, DaemonMessage, McpServerStatus};

#[test]
fn mcp_ordinary_and_resumed_sends_carry_local_workspace_in_appended_variant() {
    let (event_tx, _) = mpsc::channel(8);
    let client = DaemonClient::new(event_tx);
    let mut requests = client.request_rx.lock().unwrap().take().unwrap();
    for thread in [None, Some("restored-thread".to_string())] {
        client
            .send_message(
                thread.clone(),
                "hello".into(),
                Some("[]".into()),
                None,
                Some("svar".into()),
            )
            .unwrap();
        match drain_request(&mut requests) {
            ClientMessage::AgentSendMessageWithMcpContext {
                thread_id,
                content,
                content_blocks_json,
                client_surface,
                target_agent_id,
                mcp_workspace,
                ..
            } => {
                assert_eq!(thread_id, thread);
                assert_eq!(content, "hello");
                assert_eq!(content_blocks_json.as_deref(), Some("[]"));
                assert_eq!(client_surface, Some(ClientSurface::Tui));
                assert_eq!(target_agent_id.as_deref(), Some("svar"));
                assert_eq!(
                    mcp_workspace,
                    std::env::current_dir()
                        .ok()
                        .and_then(|path| path.into_os_string().into_string().ok())
                );
                assert!(std::path::Path::new(mcp_workspace.as_ref().unwrap()).is_absolute());
            }
            other => panic!("expected context-bearing message, got {other:?}"),
        }
    }
    assert!(
        requests.try_recv().is_err(),
        "never retry via the legacy context-free variant"
    );
}

#[tokio::test]
async fn mcp_daemon_settings_replies_reach_editor_with_revision_intact() {
    let (tx, mut rx) = mpsc::channel(8);
    assert!(
        dispatch_for_test(
            DaemonMessage::McpServers {
                servers: vec![McpServerStatus::default()]
            },
            &tx
        )
        .await
    );
    assert!(
        matches!(rx.recv().await, Some(ClientEvent::Mcp(DaemonMessage::McpServers { servers })) if servers.len() == 1)
    );
    assert!(
        dispatch_for_test(
            DaemonMessage::McpOperationResult {
                request_id: "draft-test-1".into(),
                revision: 42,
                success: false,
                message: "protocol failure".into(),
                server: None
            },
            &tx
        )
        .await
    );
    assert!(
        matches!(rx.recv().await, Some(ClientEvent::Mcp(DaemonMessage::McpOperationResult { revision: 42, request_id, .. })) if request_id == "draft-test-1")
    );
}
