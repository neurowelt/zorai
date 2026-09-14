use super::*;
use zorai_protocol::ClientMessage;

fn saved_server() -> McpServerStatus {
    McpServerStatus {
        config: McpServerConfig {
            id: "stable-id".into(),
            name: "Local mock".into(),
            url: "http://127.0.0.1:9876/mcp".into(),
            enabled: true,
            share_workspace_context: true,
            auth: McpAuthConfig::Bearer {
                credential_ref: Some("mcp-stable-id".into()),
            },
            ..Default::default()
        },
        credential_present: true,
        workspace_context_supported: true,
        ..Default::default()
    }
}

#[test]
fn mcp_new_draft_sharing_is_off_even_after_successful_negotiation() {
    let mut state = McpSettingsState::default();
    state.open(None);
    assert!(!state.draft.as_ref().unwrap().config.share_workspace_context);
    let ClientMessage::McpTestServer {
        request_id,
        revision,
        ..
    } = state.request(false).unwrap()
    else {
        panic!("test request");
    };
    assert!(!state.complete(
        request_id,
        revision,
        true,
        "Connected".into(),
        Some(saved_server())
    ));
    assert!(!state.draft.as_ref().unwrap().config.share_workspace_context);
    assert!(
        state.servers.is_empty(),
        "draft test must not publish tools to saved server list"
    );
}

#[test]
fn mcp_edit_and_reopened_draft_discard_stale_tests() {
    let mut state = McpSettingsState::default();
    state.open(None);
    let ClientMessage::McpTestServer {
        request_id,
        revision,
        ..
    } = state.request(false).unwrap()
    else {
        panic!("test request");
    };
    state.begin_edit();
    state.edit_buffer = "edited name".into();
    state.commit_edit();
    state.complete(
        request_id,
        revision,
        true,
        "stale".into(),
        Some(saved_server()),
    );
    assert!(state.test_status.is_none());
    assert!(state.result.is_none());
    let ClientMessage::McpTestServer {
        request_id,
        revision,
        ..
    } = state.request(false).unwrap()
    else {
        panic!("test request");
    };
    state.close_draft();
    state.open(None);
    state.complete(
        request_id,
        revision,
        true,
        "stale reopened".into(),
        Some(saved_server()),
    );
    assert!(state.test_status.is_none());
}

#[test]
fn mcp_secret_edits_distinguish_keep_replace_clear_without_mask_values() {
    let mut state = McpSettingsState::default();
    state.open(Some(saved_server()));
    assert_eq!(state.secret_label(), "••••••••");
    assert!(matches!(
        state.draft.as_ref().unwrap().credential,
        McpCredentialUpdate::Keep
    ));
    state.cursor = 4;
    state.begin_edit();
    assert!(state.edit_buffer.is_empty());
    state.commit_edit();
    assert!(matches!(
        state.draft.as_ref().unwrap().credential,
        McpCredentialUpdate::Clear
    ));
    state.begin_edit();
    state.edit_buffer = "new-token".into();
    state.commit_edit();
    let ClientMessage::McpSaveServer {
        request_id,
        revision,
        credential,
        ..
    } = state.request(true).unwrap()
    else {
        panic!("save request");
    };
    assert_eq!(credential, McpCredentialUpdate::Replace("new-token".into()));
    assert!(!format!("{credential:?}").contains("new-token"));
    assert!(state.complete(
        request_id,
        revision,
        true,
        "Saved".into(),
        Some(saved_server())
    ));
    assert_eq!(
        state.draft.as_ref().unwrap().credential,
        McpCredentialUpdate::Keep
    );
    state.cursor = 4;
    state.begin_edit();
    state.commit_edit();
    let ClientMessage::McpSaveServer { credential, .. } = state.request(true).unwrap() else {
        panic!("save request");
    };
    assert_eq!(credential, McpCredentialUpdate::Clear);
}

#[test]
fn mcp_endpoint_change_resets_consent_and_credential_but_name_edit_keeps_identity() {
    let mut state = McpSettingsState::default();
    state.open(Some(saved_server()));
    state.begin_edit();
    state.edit_buffer = "Renamed".into();
    state.commit_edit();
    let draft = state.draft.as_ref().unwrap();
    assert_eq!(draft.config.id, "stable-id");
    assert!(draft.config.share_workspace_context);
    state.cursor = 1;
    state.begin_edit();
    state.edit_buffer = "http://127.0.0.1:9877/mcp".into();
    state.commit_edit();
    let draft = state.draft.as_ref().unwrap();
    assert!(!draft.config.share_workspace_context);
    assert_eq!(draft.credential, McpCredentialUpdate::Clear);
    assert!(!draft.credential_present);
}

#[test]
fn mcp_save_error_retains_edit_for_correction_and_close_drops_secrets() {
    let mut state = McpSettingsState::default();
    state.open(None);
    state.cursor = 2;
    state.toggle();
    state.cursor = 4;
    state.begin_edit();
    state.edit_buffer = "retry-token".into();
    state.commit_edit();
    let ClientMessage::McpSaveServer {
        request_id,
        revision,
        ..
    } = state.request(true).unwrap()
    else {
        panic!("save request");
    };
    assert!(!state.complete(request_id, revision, false, "Invalid URL".into(), None));
    assert_eq!(
        state.draft.as_ref().unwrap().credential,
        McpCredentialUpdate::Replace("retry-token".into())
    );
    state.close_draft();
    assert!(state.draft.is_none());
    assert!(state.edit_buffer.is_empty());
}

#[test]
fn mcp_credential_edit_requires_an_authentication_mode() {
    let mut state = McpSettingsState::default();
    state.open(None);
    state.cursor = 4;
    let revision = state.revision;
    state.begin_edit();
    assert!(!state.editing);
    assert!(state.edit_buffer.is_empty());
    assert_eq!(state.revision, revision);
    assert_eq!(
        state.draft.as_ref().unwrap().credential,
        McpCredentialUpdate::Keep
    );

    for auth in [
        McpAuthConfig::Bearer {
            credential_ref: None,
        },
        McpAuthConfig::ApiKey {
            header: "X-API-Key".into(),
            credential_ref: None,
        },
    ] {
        state.draft.as_mut().unwrap().config.auth = auth;
        state.begin_edit();
        assert!(state.editing);
        state.edit_buffer = "allowed-secret".into();
        state.commit_edit();
        assert_eq!(
            state.draft.as_ref().unwrap().credential,
            McpCredentialUpdate::Replace("allowed-secret".into())
        );
    }
}

#[test]
fn mcp_tool_selection_is_collapsed_by_default_and_uses_an_accordion() {
    let mut status = saved_server();
    status.tools = vec![
        zorai_protocol::McpToolInfo {
            name: "first".into(),
            description: "First description".into(),
        },
        zorai_protocol::McpToolInfo {
            name: "second".into(),
            description: "Second description".into(),
        },
    ];
    let mut state = McpSettingsState::default();
    state.servers = vec![status.clone()];
    state.open(Some(status));

    assert_eq!(state.last_cursor(), McpSettingsState::FIELDS + 1);
    assert_eq!(state.expanded_tool, None);
    state.cursor = McpSettingsState::FIELDS;
    assert!(state.toggle_selected_tool_description());
    assert_eq!(state.expanded_tool, Some(0));
    state.cursor += 1;
    assert!(state.toggle_selected_tool_description());
    assert_eq!(state.expanded_tool, Some(1));
    assert!(state.toggle_selected_tool_description());
    assert_eq!(state.expanded_tool, None);
}
