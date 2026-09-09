use super::{super::*, mock::*};
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn mcp_required_refusals_send_zero_calls_and_optional_omits_context() {
    for supported in [false, true] {
        for sharing in [false, true] {
            let mock = Mock::start(move |request| async move { basic(&request, supported) }).await;
            let temp = tempfile::tempdir().unwrap();
            let manager = Arc::new(McpManager::new(temp.path().to_owned()));
            let mut config = config(&mock);
            config.share_workspace_context = sharing;
            config.adapter = McpAdapterPolicy::Portal;
            manager.apply_desired_config(vec![config]).unwrap();
            connected(&manager).await;
            let context = McpRequestContext::from_workspace("private-thread", None);
            let error = manager
                .call(
                    route(&manager, "consult"),
                    json!({}),
                    context.clone(),
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(error.message.contains("No request was sent"));
            assert!(mock.calls().is_empty());
            assert_eq!(
                error.code,
                if !supported {
                    "MCP_CONTEXT_UNSUPPORTED"
                } else if !sharing {
                    "MCP_CONTEXT_SHARING_DISABLED"
                } else {
                    "MCP_WORKSPACE_MISSING"
                }
            );
            manager
                .call(
                    route(&manager, "echo"),
                    json!({}),
                    context,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            let call = mock.calls().pop().unwrap();
            assert!(call.body["params"]["_meta"]
                .get("ai.zorai/workspace")
                .is_none());
            assert_eq!(
                call.body["params"]["_meta"]
                    .get("ai.zorai/conversation")
                    .is_some(),
                supported && sharing
            );
            manager.shutdown().await;
        }
    }
}

#[tokio::test]
async fn mcp_config_revoke_disable_rename_and_invalid_edit() {
    let mock = Mock::start(|request| async move { basic(&request, true) }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    let mut config = config(&mock);
    manager.apply_desired_config(vec![config.clone()]).unwrap();
    connected(&manager).await;
    let old = route(&manager, "consult");
    let catalog = manager.catalog_snapshot();
    config.name = "Renamed".into();
    manager.apply_desired_config(vec![config.clone()]).unwrap();
    assert_eq!(old, route(&manager, "consult"));
    assert_eq!(
        catalog.tools[0].function.name,
        manager.catalog_snapshot().tools[0].function.name
    );
    let mut bad = config.clone();
    bad.url = "file:///invalid".into();
    assert!(manager.apply_desired_config(vec![bad]).is_err());
    assert_eq!(manager.statuses()[0].state, "connected");
    config.share_workspace_context = false;
    manager.apply_desired_config(vec![config.clone()]).unwrap();
    let error = manager
        .call(
            old.clone(),
            json!({}),
            McpRequestContext::from_workspace("t", Some(temp.path())),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "MCP_CONTEXT_SHARING_DISABLED");
    assert!(mock.calls().is_empty());
    config.enabled = false;
    manager.apply_desired_config(vec![config]).unwrap();
    assert!(manager.catalog_snapshot().tools.is_empty());
    assert!(manager
        .call(
            old,
            json!({}),
            McpRequestContext::from_workspace("t", Some(temp.path())),
            CancellationToken::new()
        )
        .await
        .is_err());
    assert!(mock.calls().is_empty());
    manager.shutdown().await;
}

#[tokio::test]
async fn mcp_draft_test_isolated_authentication_and_redirect_refusal() {
    let mock = Mock::start(|request| async move { basic(&request, true) }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    let mut draft = config(&mock);
    draft.auth = McpAuthConfig::ApiKey {
        header: "X-Test-Key".into(),
        credential_ref: None,
    };
    let status = manager
        .test_connection(
            draft,
            McpCredentialUpdate::Replace("mock-only-secret".into()),
        )
        .await
        .unwrap();
    assert_eq!(status.tools.len(), 3);
    assert!(status.workspace_context_supported);
    assert!(manager.catalog_snapshot().tools.is_empty());
    assert!(manager.statuses().is_empty());
    assert!(mock.calls().is_empty());
    assert!(!serde_json::to_string(&status)
        .unwrap()
        .contains("mock-only-secret"));
    assert!(mock.requests.lock().unwrap().iter().any(|r| r
        .headers
        .get("x-test-key")
        .is_some_and(|v| v == "mock-only-secret")));
    let destination = Mock::start(|request| async move { basic(&request, true) }).await;
    let url = destination.url.clone();
    let redirect = Mock::start(move |_request| {
        let url = url.clone();
        async move {
            let mut reply = Reply::status(307);
            reply.headers.push(("Location".into(), url));
            reply
        }
    })
    .await;
    let error = manager
        .test_connection(config(&redirect), McpCredentialUpdate::Keep)
        .await
        .unwrap_err();
    assert!(error.contains("redirect refused"));
    assert!(destination.requests.lock().unwrap().is_empty());
    let unauthorized = Mock::start(|_request| async move { Reply::status(401) }).await;
    assert!(manager
        .test_connection(config(&unauthorized), McpCredentialUpdate::Keep)
        .await
        .unwrap_err()
        .contains("authentication failed"));
}

#[tokio::test]
async fn mcp_portal_collection_budget_exact_routes_and_action_states() {
    let pending = Arc::new(AtomicBool::new(true));
    let flag = pending.clone();
    let mock=Mock::start(move|request|{let flag=flag.clone();async move{if request.body["method"]=="tools/call"{Reply::json(&request,json!({"content":[],"structuredContent":{"status":if flag.load(Ordering::SeqCst){"pending"}else{"needs_reply"},"job_id":"job-a"}}))}else{basic(&request,true)}}}).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    let mut config = config(&mock);
    config.adapter = McpAdapterPolicy::Portal;
    manager.apply_desired_config(vec![config]).unwrap();
    connected(&manager).await;
    let route = route(&manager, "get_answer");
    let args = json!({"job_id":"job-a"});
    assert!(!manager.allow_pending_poll(&route, "t", &args));
    for _ in 0..20 {
        manager
            .call(
                route.clone(),
                args.clone(),
                McpRequestContext::from_workspace("t", None),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    assert!(!manager.allow_pending_poll(&route, "t", &args));
    let error = manager
        .call(
            route.clone(),
            json!({"job_id":"job-a","extra":"changed"}),
            McpRequestContext::from_workspace("t", None),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "MCP_POLL_LIMIT");
    assert_eq!(mock.calls().len(), 20);
    manager.reset_pending_polls("t");
    pending.store(false, Ordering::SeqCst);
    manager
        .call(
            route.clone(),
            args.clone(),
            McpRequestContext::from_workspace("t", None),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!manager.allow_pending_poll(&route, "t", &args));
    manager.shutdown().await;
}

#[tokio::test]
async fn mcp_queued_transport_cancellation_never_submits_late_mutation() {
    use rmcp::transport::Transport;
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let mock = Mock::start(move |request| {
        let gate = gate.clone();
        async move {
            if request.body["method"] == "tools/call" {
                gate.notified().await;
            }
            basic(&request, true)
        }
    })
    .await;
    let mut transport = super::super::transport::HttpTransport::new(&config(&mock), None, None)
        .unwrap()
        .with_request_limit(1);
    let first=serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo","arguments":{}}})).unwrap();
    let second=serde_json::from_value(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{}}})).unwrap();
    let first = tokio::spawn(transport.send(first));
    mock.wait_calls(1).await;
    let second = tokio::spawn(transport.send(second));
    let cancel=serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"thread stopped"}})).unwrap();
    transport.send(cancel).await.unwrap();
    assert!(second
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("MCP_CANCELLED"));
    release.notify_waiters();
    first.await.unwrap().unwrap();
    assert_eq!(mock.calls().len(), 1);
    transport.close().await.unwrap();
}

#[tokio::test]
async fn mcp_queued_metadata_revocation_checked_at_actual_submission() {
    use rmcp::transport::Transport;
    let release = Arc::new(tokio::sync::Notify::new());
    let wait = release.clone();
    let mock = Mock::start(move |request| {
        let wait = wait.clone();
        async move {
            if request.body["method"] == "tools/call" {
                wait.notified().await;
            }
            basic(&request, true)
        }
    })
    .await;
    let permission = Arc::new(AtomicBool::new(true));
    let gate = permission.clone();
    let dispatch: super::super::transport::DispatchGate = Arc::new(move |_| {
        if gate.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("MCP_CONTEXT_SHARING_DISABLED: No request was sent.".into())
        }
    });
    let mut transport =
        super::super::transport::HttpTransport::new(&config(&mock), None, Some(dispatch))
            .unwrap()
            .with_request_limit(1);
    let message = |id| {
        serde_json::from_value(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"echo","arguments":{},"_meta":{"ai.zorai/conversation":"protected"}}})).unwrap()
    };
    let first = tokio::spawn(transport.send(message(1)));
    mock.wait_calls(1).await;
    let second = tokio::spawn(transport.send(message(2)));
    permission.store(false, Ordering::SeqCst);
    release.notify_waiters();
    first.await.unwrap().unwrap();
    assert!(second
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("No request was sent"));
    assert_eq!(mock.calls().len(), 1);
    transport.close().await.unwrap();
}
