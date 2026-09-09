use super::*;

fn server_config() -> McpServerConfig {
    McpServerConfig {
        id: "stable-server".into(),
        name: "Local mock".into(),
        url: "http://127.0.0.1:12345/mcp".into(),
        enabled: true,
        share_workspace_context: true,
        auth: McpAuthConfig::Bearer {
            credential_ref: Some("mcp:stable-server".into()),
        },
        adapter: McpAdapterPolicy::Portal,
    }
}

fn server_status() -> McpServerStatus {
    McpServerStatus {
        config: server_config(),
        state: "connected".into(),
        message: Some("Discovered one tool".into()),
        server_name: Some("Isolated mock".into()),
        instructions: Some("Use discovered schemas".into()),
        workspace_context_supported: true,
        tools: vec![McpToolInfo {
            name: "consult".into(),
            description: "Mock consultation".into(),
        }],
        credential_present: true,
    }
}

fn assert_client_round_trip(request: ClientMessage, index: u32) {
    assert_bincode_variant_index(&request, index);
    let expected = serde_json::to_value(&request).unwrap();
    let mut frame = BytesMut::new();
    ZoraiCodec.encode(request, &mut frame).unwrap();
    let decoded = DaemonCodec.decode(&mut frame).unwrap().unwrap();
    assert!(frame.is_empty());
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

fn assert_daemon_round_trip(response: DaemonMessage, index: u32) {
    assert_bincode_variant_index(&response, index);
    let expected = serde_json::to_value(&response).unwrap();
    let mut frame = BytesMut::new();
    DaemonCodec.encode(response, &mut frame).unwrap();
    let decoded = ZoraiCodec.decode(&mut frame).unwrap().unwrap();
    assert!(frame.is_empty());
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

#[test]
fn mcp_extension_preserves_legacy_send_binary_fixture() {
    // Frozen pre-MCP layout: discriminator 46, then the seven original fields.
    const FIXTURE: &[u8] = &[
        46, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 116, 2, 0, 0, 0, 0, 0, 0, 0, 104, 105, 1, 1, 0, 0,
        0, 0, 0, 0, 0, 115, 1, 2, 0, 0, 0, 0, 0, 0, 0, 91, 93, 1, 2, 0, 0, 0, 0, 0, 0, 0, 91, 93,
        1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 97,
    ];
    let request = ClientMessage::AgentSendMessage {
        thread_id: Some("t".into()),
        content: "hi".into(),
        session_id: Some("s".into()),
        context_messages_json: Some("[]".into()),
        content_blocks_json: Some("[]".into()),
        client_surface: Some(ClientSurface::Tui),
        target_agent_id: Some("a".into()),
    };
    assert_eq!(bincode::serialize(&request).unwrap(), FIXTURE);
    let decoded: ClientMessage = bincode::deserialize(FIXTURE).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        serde_json::to_value(&request).unwrap()
    );
    assert_client_round_trip(request, 46);
    assert_bincode_variant_index(
        &ClientMessage::AgentSetThreadExecutionProfile {
            thread_id: "t".into(),
            profile_json: "{}".into(),
        },
        248,
    );
    assert_bincode_variant_index(
        &DaemonMessage::AgentThreadExecutionProfile {
            thread_id: "t".into(),
            profile_json: "{}".into(),
        },
        186,
    );
}

#[test]
fn mcp_send_context_round_trips_without_changing_legacy_send() {
    for workspace in [None, Some("/tmp/project #1/żółć".into())] {
        assert_client_round_trip(
            ClientMessage::AgentSendMessageWithMcpContext {
                thread_id: None,
                content: "first turn".into(),
                session_id: Some("session".into()),
                context_messages_json: Some("[]".into()),
                content_blocks_json: Some("[]".into()),
                client_surface: Some(ClientSurface::Tui),
                target_agent_id: Some("agent".into()),
                mcp_workspace: workspace,
            },
            249,
        );
    }
}

#[test]
fn mcp_settings_requests_round_trip_at_append_only_tail() {
    assert_client_round_trip(ClientMessage::McpListServers, 250);
    for credential in [
        McpCredentialUpdate::Keep,
        McpCredentialUpdate::Replace("synthetic-test-token".into()),
        McpCredentialUpdate::Clear,
    ] {
        assert_client_round_trip(
            ClientMessage::McpSaveServer {
                request_id: "save-1".into(),
                revision: 42,
                config: server_config(),
                credential: credential.clone(),
            },
            251,
        );
        assert_client_round_trip(
            ClientMessage::McpTestServer {
                request_id: "test-1".into(),
                revision: 43,
                config: server_config(),
                credential,
            },
            252,
        );
    }
    for enabled in [false, true] {
        assert_client_round_trip(
            ClientMessage::McpSetServerEnabled {
                id: "stable-server".into(),
                enabled,
            },
            253,
        );
    }
    assert_client_round_trip(
        ClientMessage::McpReconnectServer {
            id: "stable-server".into(),
        },
        254,
    );
}

#[test]
fn mcp_settings_replies_round_trip_at_append_only_tail() {
    for servers in [vec![], vec![server_status()]] {
        assert_daemon_round_trip(DaemonMessage::McpServers { servers }, 187);
    }
    for success in [false, true] {
        assert_daemon_round_trip(
            DaemonMessage::McpOperationResult {
                request_id: "test-1".into(),
                revision: 43,
                success,
                message: "Connection test completed without workspace disclosure".into(),
                server: success.then(server_status),
            },
            188,
        );
    }
}

#[test]
fn mcp_settings_default_to_no_disclosure_or_authentication() {
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "id": "stable-server", "name": "Local mock", "url": "http://127.0.0.1/mcp"
    }))
    .unwrap();
    assert!(!config.share_workspace_context);
    assert!(!config.enabled);
    assert_eq!(config.auth, McpAuthConfig::None);
    assert_eq!(config.adapter, McpAdapterPolicy::Generic);
    assert!(!McpServerConfig::default().share_workspace_context);
    assert_eq!(McpCredentialUpdate::default(), McpCredentialUpdate::Keep);
}

#[test]
fn mcp_credential_replacement_is_redacted_in_enclosing_request_debug() {
    let secret = "synthetic-secret-never-in-debug";
    let credential = McpCredentialUpdate::Replace(secret.into());
    assert_eq!(format!("{credential:?}"), "Replace([redacted])");
    for request in [
        ClientMessage::McpSaveServer {
            request_id: "save".into(),
            revision: 1,
            config: server_config(),
            credential: credential.clone(),
        },
        ClientMessage::McpTestServer {
            request_id: "test".into(),
            revision: 2,
            config: server_config(),
            credential,
        },
    ] {
        let debug = format!("{request:?}");
        assert!(!debug.contains(secret));
        assert!(debug.contains("[redacted]"));
    }
}

#[test]
fn mcp_auth_modes_round_trip_as_credential_references() {
    for auth in [
        McpAuthConfig::None,
        McpAuthConfig::Bearer {
            credential_ref: None,
        },
        McpAuthConfig::ApiKey {
            header: "X-API-Key".into(),
            credential_ref: Some("mcp:stable-server".into()),
        },
    ] {
        let mut config = server_config();
        config.auth = auth;
        assert_client_round_trip(
            ClientMessage::McpSaveServer {
                request_id: "auth".into(),
                revision: 1,
                config,
                credential: McpCredentialUpdate::Keep,
            },
            251,
        );
    }
}
