//! Configuration tests use disabled, isolated MCP servers; no connection is attempted.
use super::*;
use zorai_protocol::{McpAdapterPolicy, McpAuthConfig, McpCredentialUpdate, McpServerConfig};

fn disabled_mcp_server(id: &str, auth: McpAuthConfig) -> McpServerConfig {
    McpServerConfig {
        id: id.into(),
        name: format!("Isolated {id}"),
        url: "http://127.0.0.1:1/mcp".into(),
        enabled: false,
        share_workspace_context: false,
        auth,
        adapter: McpAdapterPolicy::Generic,
        ..Default::default()
    }
}

async fn persisted_mcp_config(engine: &AgentEngine) -> AgentConfig {
    load_config_from_items(engine.history.list_agent_config_items().await.unwrap())
        .expect("MCP config projection should rehydrate")
}

#[tokio::test]
async fn mcp_default_configuration_has_no_servers_or_tools() {
    let root = tempdir().unwrap();
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
    assert!(engine.get_config().await.mcp_servers.is_empty());
    assert!(engine.mcp.statuses().is_empty());
    assert!(engine.mcp.catalog_snapshot().tools.is_empty());
}

#[tokio::test]
async fn mcp_authenticated_config_projection_roundtrips_both_auth_modes_without_secrets() {
    let root = tempdir().unwrap();
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
    for (id, auth) in [
        (
            "bearer",
            McpAuthConfig::Bearer {
                credential_ref: None,
            },
        ),
        (
            "api-key",
            McpAuthConfig::ApiKey {
                header: "X-Mock-Key".into(),
                credential_ref: None,
            },
        ),
    ] {
        let mut server = disabled_mcp_server(id, auth);
        server.share_workspace_context = true;
        engine
            .save_mcp_server(
                server,
                McpCredentialUpdate::Replace(format!("isolated-{id}-secret")),
            )
            .await
            .expect("dedicated settings save");
    }
    let saved = engine.get_config().await;
    let items = engine.history.list_agent_config_items().await.unwrap();
    let serialized = serde_json::to_string(&items).unwrap();
    assert!(!serialized.contains("isolated-bearer-secret"));
    assert!(!serialized.contains("isolated-api-key-secret"));
    let reloaded = load_config_from_items(items).unwrap();
    assert_eq!(
        reloaded.mcp_servers, saved.mcp_servers,
        "authenticated enums must survive config key normalization"
    );
    assert_eq!(reloaded.mcp_servers.len(), 2);
    for server in &reloaded.mcp_servers {
        assert!(!server.enabled);
        assert!(server.share_workspace_context);
        match &server.auth {
            McpAuthConfig::Bearer { credential_ref }
            | McpAuthConfig::ApiKey { credential_ref, .. } => {
                assert!(credential_ref
                    .as_deref()
                    .is_some_and(|value| uuid::Uuid::parse_str(value).is_ok()));
            }
            McpAuthConfig::None => panic!("authenticated config must not fall back to defaults"),
        }
    }
    let status_json = serde_json::to_string(&engine.mcp.statuses()).unwrap();
    assert!(!status_json.contains("isolated-bearer-secret"));
    assert!(!status_json.contains("isolated-api-key-secret"));
    assert!(engine.mcp.catalog_snapshot().tools.is_empty());
    // Keep preserves the existing reference without requiring a credential read or connection.
    for server in reloaded.mcp_servers {
        engine
            .save_mcp_server(server, McpCredentialUpdate::Keep)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn mcp_unrelated_config_item_updates_preserve_servers_and_saved_consent() {
    let root = tempdir().unwrap();
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
    let mut server = disabled_mcp_server(
        "saved",
        McpAuthConfig::ApiKey {
            header: "X-Mock-Key".into(),
            credential_ref: None,
        },
    );
    server.share_workspace_context = true;
    engine
        .save_mcp_server(
            server,
            McpCredentialUpdate::Replace("isolated-preserved-secret".into()),
        )
        .await
        .unwrap();
    let expected = engine.get_config().await.mcp_servers;
    engine
        .set_config_item_json("/system_prompt", r#""MCP projection regression test""#)
        .await
        .unwrap();
    engine
        .set_config_item_json("/message_loop_delay_ms", "123")
        .await
        .unwrap();
    assert_eq!(engine.get_config().await.mcp_servers, expected);
    let persisted = persisted_mcp_config(&engine).await;
    assert_eq!(persisted.mcp_servers, expected);
    assert_eq!(persisted.message_loop_delay_ms, 123);
    assert!(engine.mcp.catalog_snapshot().tools.is_empty());
}

#[tokio::test]
async fn mcp_generic_config_writes_cannot_grant_or_restore_revoked_consent() {
    let root = tempdir().unwrap();
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
    let mut server = disabled_mcp_server("consent", McpAuthConfig::None);
    server.share_workspace_context = true;
    let mut config = engine.get_config().await;
    config.mcp_servers = vec![server.clone()];
    engine.set_config(config).await;
    assert!(
        engine.get_config().await.mcp_servers.is_empty(),
        "whole-config writes must preserve the dedicated MCP subtree"
    );
    let error = engine
        .set_config_item_json(
            "/mcp_servers",
            &serde_json::to_string(&vec![server.clone()]).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Use Settings → MCP"));
    assert!(engine.get_config().await.mcp_servers.is_empty());
    assert!(persisted_mcp_config(&engine).await.mcp_servers.is_empty());
    engine
        .save_mcp_server(server.clone(), McpCredentialUpdate::Keep)
        .await
        .unwrap();
    assert!(engine.get_config().await.mcp_servers[0].share_workspace_context);
    let stale_config = engine.get_config().await;
    server.share_workspace_context = false;
    engine
        .save_mcp_server(server, McpCredentialUpdate::Keep)
        .await
        .unwrap();
    engine.set_config(stale_config).await;
    assert!(
        !engine.get_config().await.mcp_servers[0].share_workspace_context,
        "stale generic snapshots cannot restore revoked permission"
    );
    assert!(!persisted_mcp_config(&engine).await.mcp_servers[0].share_workspace_context);
}

#[tokio::test]
async fn mcp_endpoint_changes_reset_consent_and_saved_auth_reference() {
    let root = tempdir().unwrap();
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
    let mut server = disabled_mcp_server(
        "endpoint",
        McpAuthConfig::Bearer {
            credential_ref: None,
        },
    );
    server.share_workspace_context = true;
    engine
        .save_mcp_server(
            server,
            McpCredentialUpdate::Replace("isolated-endpoint-secret".into()),
        )
        .await
        .unwrap();
    let mut changed = engine.get_config().await.mcp_servers.remove(0);
    changed.url = "http://127.0.0.1:2/mcp".into();
    engine
        .save_mcp_server(changed, McpCredentialUpdate::Keep)
        .await
        .unwrap();
    let changed = engine.get_config().await.mcp_servers.remove(0);
    assert!(!changed.share_workspace_context);
    assert!(matches!(
        &changed.auth,
        McpAuthConfig::Bearer {
            credential_ref: None
        }
    ));
    assert_eq!(
        persisted_mcp_config(&engine).await.mcp_servers,
        vec![changed.clone()]
    );
    // A later explicit save for this endpoint can grant consent. Generic endpoint
    // updates must be rejected without modifying that saved endpoint or permission.
    let mut explicit = changed;
    explicit.share_workspace_context = true;
    engine
        .save_mcp_server(explicit, McpCredentialUpdate::Keep)
        .await
        .unwrap();
    let expected = engine.get_config().await.mcp_servers;
    let mut updated = expected.clone();
    updated[0].url = "http://127.0.0.1:3/mcp".into();
    let error = engine
        .set_config_item_json("/mcp_servers", &serde_json::to_string(&updated).unwrap())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Use Settings → MCP"));
    assert_eq!(engine.get_config().await.mcp_servers, expected);
    assert_eq!(persisted_mcp_config(&engine).await.mcp_servers, expected);
}

#[tokio::test]
async fn mcp_prepared_unrelated_item_cannot_overwrite_newer_saved_server_settings() {
    let root = tempdir().unwrap();
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
    let (prepared, value) = engine
        .prepare_config_item_json("/message_loop_delay_ms", "77")
        .await
        .unwrap();
    let mut server = disabled_mcp_server("newer", McpAuthConfig::None);
    server.share_workspace_context = true;
    engine
        .save_mcp_server(server, McpCredentialUpdate::Keep)
        .await
        .unwrap();
    let expected = engine.get_config().await.mcp_servers;
    engine
        .persist_prepared_config_item_json("/message_loop_delay_ms", &value, prepared)
        .await
        .unwrap();
    assert_eq!(engine.get_config().await.mcp_servers, expected);
    assert_eq!(persisted_mcp_config(&engine).await.mcp_servers, expected);
}

#[tokio::test]
async fn mcp_enabled_toggle_uses_latest_settings_without_regranting_revoked_consent() {
    let root = tempdir().unwrap();
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
    let mut server = disabled_mcp_server("toggle", McpAuthConfig::None);
    server.share_workspace_context = true;
    engine
        .save_mcp_server(server.clone(), McpCredentialUpdate::Keep)
        .await
        .unwrap();
    server.share_workspace_context = false;
    server.name = "Updated while settings list was open".into();
    server.url = "http://127.0.0.1:2/mcp".into();
    engine
        .save_mcp_server(server.clone(), McpCredentialUpdate::Keep)
        .await
        .unwrap();
    // An old list row submits only the ID and enabled flag. Disabling remains fully offline.
    engine
        .set_mcp_server_enabled("toggle", false)
        .await
        .unwrap();
    assert_eq!(engine.get_config().await.mcp_servers, vec![server.clone()]);
    assert_eq!(
        persisted_mcp_config(&engine).await.mcp_servers,
        vec![server]
    );
    assert!(engine.mcp.catalog_snapshot().tools.is_empty());
}

fn mcp_credential_path(root: &std::path::Path, server: &McpServerConfig) -> std::path::PathBuf {
    let reference = match &server.auth {
        McpAuthConfig::Bearer { credential_ref } | McpAuthConfig::ApiKey { credential_ref, .. } => {
            credential_ref.as_ref().expect("saved credential reference")
        }
        McpAuthConfig::None => panic!("expected authenticated MCP server"),
    };
    root.join("agent")
        .join("mcp-credentials")
        .join(format!("{reference}.enc"))
}

#[tokio::test]
async fn mcp_prebuilt_generic_snapshot_rejected_without_losing_new_server_or_credential() {
    let root = tempdir().unwrap();
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
    let stale = engine.get_config().await;
    let mut server = disabled_mcp_server(
        "latest-with-key",
        McpAuthConfig::Bearer {
            credential_ref: None,
        },
    );
    server.share_workspace_context = true;
    engine
        .save_mcp_server(
            server,
            McpCredentialUpdate::Replace("isolated-latest-secret".into()),
        )
        .await
        .unwrap();
    let expected = engine.get_config().await.mcp_servers;
    let path = mcp_credential_path(root.path(), &expected[0]);
    let encrypted_before = std::fs::read(&path).unwrap();
    let error = engine
        .persist_prepared_config_item_json("/mcp_servers", &serde_json::json!([]), stale)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Use Settings → MCP"));
    assert_eq!(engine.get_config().await.mcp_servers, expected);
    assert_eq!(persisted_mcp_config(&engine).await.mcp_servers, expected);
    assert_eq!(
        std::fs::read(path).unwrap(),
        encrypted_before,
        "rejected stale snapshot must not delete or replace the saved credential"
    );
}

#[tokio::test]
async fn mcp_missing_or_corrupt_vault_entry_allows_disable_and_sharing_revocation() {
    for corrupt in [false, true] {
        let root = tempdir().unwrap();
        let manager = SessionManager::new_test(root.path()).await;
        let engine = AgentEngine::new_test(manager, AgentConfig::default(), root.path()).await;
        let mut server = disabled_mcp_server(
            "broken-vault",
            McpAuthConfig::Bearer {
                credential_ref: None,
            },
        );
        server.share_workspace_context = true;
        engine
            .save_mcp_server(
                server,
                McpCredentialUpdate::Replace("isolated-broken-secret".into()),
            )
            .await
            .unwrap();
        let saved = engine.get_config().await.mcp_servers.remove(0);
        let path = mcp_credential_path(root.path(), &saved);
        if corrupt {
            std::fs::write(&path, b"invalid encrypted credential").unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        // The ID-only disable operation uses Keep. It must work even when the vault is
        // unreadable; the server stays disabled throughout this test, so no I/O is submitted.
        engine
            .set_mcp_server_enabled(&saved.id, false)
            .await
            .expect("broken credentials must not prevent disabling a server");
        assert_eq!(engine.get_config().await.mcp_servers, vec![saved.clone()]);
        let mut revoked = saved;
        revoked.share_workspace_context = false;
        engine
            .save_mcp_server(revoked.clone(), McpCredentialUpdate::Keep)
            .await
            .expect("broken credentials must not prevent revoking sharing");
        assert_eq!(engine.get_config().await.mcp_servers, vec![revoked.clone()]);
        assert_eq!(
            persisted_mcp_config(&engine).await.mcp_servers,
            vec![revoked]
        );
        assert!(engine.mcp.catalog_snapshot().tools.is_empty());
    }
}

#[tokio::test]
async fn mcp_empty_history_load_is_successful_default_configuration() {
    let root = tempdir().unwrap();
    let history = crate::history::HistoryStore::new_test_store(root.path())
        .await
        .unwrap();
    assert!(history.list_agent_config_items().await.unwrap().is_empty());
    let loaded = crate::agent::load_config_from_history(&history)
        .await
        .expect("empty history is not a storage failure");
    assert_eq!(
        serde_json::to_value(loaded).unwrap(),
        serde_json::to_value(AgentConfig::default()).unwrap()
    );
}
