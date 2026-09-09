use super::*;
use crate::mcp_client::McpRequestContext;
use crate::session_manager::SessionManager;
use std::path::Path;

async fn engine_at(root: &Path) -> Arc<AgentEngine> {
    let manager = SessionManager::new_test(root).await;
    AgentEngine::new_test(manager, AgentConfig::default(), root).await
}

async fn insert_thread(engine: &AgentEngine, id: &str) {
    engine.threads.write().await.insert(
        id.into(),
        AgentThread {
            id: id.into(),
            agent_name: None,
            title: "MCP context test".into(),
            messages: vec![],
            pinned: false,
            upstream_thread_id: None,
            upstream_transport: None,
            upstream_provider: None,
            upstream_model: None,
            upstream_assistant_id: None,
            created_at: 1,
            updated_at: 1,
            total_input_tokens: 0,
            total_output_tokens: 0,
        },
    );
}

async fn bind_parent(engine: &AgentEngine, child: &str, parent: &str) {
    engine
        .set_thread_identity_metadata(
            child,
            ThreadIdentityMetadata {
                thread_id: child.into(),
                goal_run_id: None,
                goal_id: None,
                task_id: None,
                parent_task_id: None,
                parent_thread_id: Some(parent.into()),
                source: Some("subagent".into()),
                reserved_at: None,
            },
        )
        .await;
}

fn assert_workspace(context: &McpRequestContext, thread: &str, root: &Path) {
    assert!(
        context.error.is_none(),
        "unexpected context error: {:?}",
        context.error
    );
    assert_eq!(context.thread_id, thread);
    assert_eq!(
        context
            .workspace_uri
            .as_ref()
            .unwrap()
            .to_file_path()
            .unwrap(),
        root.canonicalize().unwrap()
    );
}

fn assert_context_error(context: &McpRequestContext, code: &str) {
    assert!(context.workspace_uri.is_none());
    let error = context
        .error
        .as_ref()
        .expect("invalid context must fail closed");
    assert_eq!(error.code, code);
    assert!(error.message.contains("No request was sent"));
}

#[tokio::test]
async fn mcp_first_pending_binding_survives_initial_thread_save_and_reopen() {
    let root = tempdir().unwrap();
    let other = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    let thread = "mcp-first-turn";
    engine
        .bind_mcp_workspace(thread, root.path().to_string_lossy().into_owned())
        .await;
    assert_workspace(
        &engine.resolve_mcp_context(thread, None).await,
        thread,
        root.path(),
    );
    insert_thread(&engine, thread).await;
    engine.persist_thread_by_id(thread).await;
    engine
        .bind_mcp_workspace(thread, other.path().to_string_lossy().into_owned())
        .await;
    engine.persist_thread_by_id(thread).await;
    let raw = engine
        .history
        .thread_metadata_json(thread)
        .await
        .unwrap()
        .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        metadata["mcp_workspace"],
        root.path().to_string_lossy().as_ref()
    );
    assert!(
        metadata["workspace_context"].is_null(),
        "MCP binding must not select a shell workspace"
    );
    let restarted = engine_at(root.path()).await;
    restarted
        .bind_mcp_workspace(thread, other.path().to_string_lossy().into_owned())
        .await;
    assert_workspace(
        &restarted.resolve_mcp_context(thread, None).await,
        thread,
        root.path(),
    );
}

#[tokio::test]
async fn mcp_binding_survives_unrelated_metadata_patch_and_snapshot_rebuild() {
    let root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    let thread = "mcp-metadata-patch";
    insert_thread(&engine, thread).await;
    engine
        .bind_mcp_workspace(thread, root.path().to_string_lossy().into_owned())
        .await;
    engine.persist_thread_by_id(thread).await;
    engine
        .set_thread_client_surface(thread, zorai_protocol::ClientSurface::Tui)
        .await;
    engine.persist_thread_by_id(thread).await;
    engine.mcp_bindings.write().await.clear();
    assert_workspace(
        &engine.resolve_mcp_context(thread, None).await,
        thread,
        root.path(),
    );
    assert_eq!(
        engine.get_thread_client_surface(thread).await,
        Some(zorai_protocol::ClientSurface::Tui)
    );
}

#[tokio::test]
async fn mcp_child_inherits_parent_workspace_but_keeps_own_conversation_id() {
    let root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    insert_thread(&engine, "parent").await;
    insert_thread(&engine, "child").await;
    engine
        .bind_mcp_workspace("parent", root.path().to_string_lossy().into_owned())
        .await;
    bind_parent(&engine, "child", "parent").await;
    assert_workspace(
        &engine.resolve_mcp_context("child", None).await,
        "child",
        root.path(),
    );
    engine.persist_thread_by_id("parent").await;
    engine.persist_thread_by_id("child").await;
    let restarted = engine_at(root.path()).await;
    assert_workspace(
        &restarted.resolve_mcp_context("child", None).await,
        "child",
        root.path(),
    );
}

#[tokio::test]
async fn mcp_distinct_child_workspace_overrides_inherited_parent_binding() {
    let root = tempdir().unwrap();
    let child_root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    insert_thread(&engine, "child").await;
    engine
        .bind_mcp_workspace("parent", root.path().to_string_lossy().into_owned())
        .await;
    bind_parent(&engine, "child", "parent").await;
    assert!(
        engine
            .set_thread_workspace_context(
                "child",
                Some(ThreadWorkspaceContext {
                    root: child_root.path().to_string_lossy().into_owned(),
                    ..Default::default()
                })
            )
            .await
    );
    assert_workspace(
        &engine.resolve_mcp_context("child", None).await,
        "child",
        child_root.path(),
    );
}

#[tokio::test]
async fn mcp_missing_context_does_not_use_daemon_workspace_root() {
    let root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    let context = engine.resolve_mcp_context("unbound", None).await;
    assert!(context.workspace_uri.is_none());
    assert!(
        context.error.is_none(),
        "ordinary context-free tools can still be called"
    );
    assert_eq!(context.thread_id, "unbound");
}

#[tokio::test]
async fn mcp_invalid_binding_and_stale_explicit_session_never_fall_back() {
    let root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    for (thread, binding) in [
        ("relative", "relative/path".to_string()),
        (
            "missing",
            root.path()
                .join("deleted-workspace")
                .to_string_lossy()
                .into_owned(),
        ),
    ] {
        engine.bind_mcp_workspace(thread, binding).await;
        assert_context_error(
            &engine.resolve_mcp_context(thread, None).await,
            "MCP_WORKSPACE_INVALID",
        );
    }
    engine
        .bind_mcp_workspace("bound", root.path().to_string_lossy().into_owned())
        .await;
    assert_context_error(
        &engine
            .resolve_mcp_context("bound", Some(uuid::Uuid::new_v4()))
            .await,
        "MCP_WORKSPACE_INVALID",
    );
}

#[tokio::test]
async fn mcp_unrelated_persisted_workspace_and_binding_are_a_conflict() {
    let root = tempdir().unwrap();
    let other = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    insert_thread(&engine, "conflict").await;
    engine
        .bind_mcp_workspace("conflict", other.path().to_string_lossy().into_owned())
        .await;
    assert!(
        engine
            .set_thread_workspace_context(
                "conflict",
                Some(ThreadWorkspaceContext {
                    root: root.path().to_string_lossy().into_owned(),
                    ..Default::default()
                })
            )
            .await
    );
    assert_context_error(
        &engine.resolve_mcp_context("conflict", None).await,
        "MCP_WORKSPACE_CONFLICT",
    );
}

#[tokio::test]
async fn mcp_invalid_known_binding_cannot_be_hidden_by_valid_workspace() {
    let root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    insert_thread(&engine, "stale-binding").await;
    engine
        .bind_mcp_workspace(
            "stale-binding",
            root.path().join("missing").to_string_lossy().into_owned(),
        )
        .await;
    assert!(
        engine
            .set_thread_workspace_context(
                "stale-binding",
                Some(ThreadWorkspaceContext {
                    root: root.path().to_string_lossy().into_owned(),
                    ..Default::default()
                })
            )
            .await
    );
    assert_context_error(
        &engine.resolve_mcp_context("stale-binding", None).await,
        "MCP_WORKSPACE_INVALID",
    );
}

#[tokio::test]
async fn mcp_parent_cycle_is_bounded_without_cwd_fallback() {
    let root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    bind_parent(&engine, "a", "b").await;
    bind_parent(&engine, "b", "a").await;
    let context = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        engine.resolve_mcp_context("a", None),
    )
    .await
    .unwrap();
    assert!(context.workspace_uri.is_none());
    assert_eq!(context.thread_id, "a");
}

#[tokio::test]
async fn mcp_no_servers_preserves_existing_tool_catalog() {
    let root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    let config = AgentConfig::default();
    let baseline = tool_executor::get_available_tools(&config, &engine.data_dir, false);
    let effective = engine.effective_tools(&config, false);
    assert_eq!(
        serde_json::to_value(effective).unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
    assert!(engine.mcp.catalog_snapshot().tools.is_empty());
    assert!(!engine.has_mcp_tools());
}

#[tokio::test]
async fn mcp_connected_catalog_selects_the_tool_capable_agent_loop() {
    use crate::mcp_client::tests::mock::{basic, config, connected, Mock};

    let root = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    let mock = Mock::start(|request| async move { basic(&request, true) }).await;
    engine
        .mcp
        .apply_desired_config(vec![config(&mock)])
        .unwrap();
    connected(&engine.mcp).await;

    assert!(engine.has_mcp_tools());
    assert_eq!(engine.mcp.catalog_snapshot().tools.len(), 3);
    engine.mcp.shutdown().await;
}

#[tokio::test]
async fn mcp_deleted_thread_releases_cached_binding() {
    let root = tempdir().unwrap();
    let other = tempdir().unwrap();
    let engine = engine_at(root.path()).await;
    insert_thread(&engine, "deleted-mcp").await;
    engine
        .bind_mcp_workspace("deleted-mcp", root.path().to_string_lossy().into_owned())
        .await;
    engine.persist_thread_by_id("deleted-mcp").await;
    assert!(engine.delete_thread("deleted-mcp").await);
    assert!(engine.mcp_workspace_binding("deleted-mcp").await.is_none());
    engine
        .bind_mcp_workspace("deleted-mcp", other.path().to_string_lossy().into_owned())
        .await;
    assert_workspace(
        &engine.resolve_mcp_context("deleted-mcp", None).await,
        "deleted-mcp",
        other.path(),
    );
}
