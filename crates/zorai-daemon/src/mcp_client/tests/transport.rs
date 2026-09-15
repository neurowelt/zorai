use super::{super::*, mock::*};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn mcp_http_initialize_pagination_metadata_and_session_wire() {
    let mock = Mock::start(|request| async move { basic(&request, true) }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    manager.apply_desired_config(vec![config(&mock)]).unwrap();
    connected(&manager).await;
    assert_eq!(manager.catalog_snapshot().tools.len(), 3);
    let context = McpRequestContext::from_workspace("conversation-a", Some(temp.path()));
    let result = manager
        .call(
            route(&manager, "consult"),
            json!({"cwd":"/spoofed","_meta":{"ai.zorai/workspace":"file:///spoofed"}}),
            context.clone(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    let requests = mock.requests.lock().unwrap().clone();
    let initialize = requests
        .iter()
        .find(|r| r.body["method"] == "initialize")
        .unwrap();
    assert_eq!(
        initialize.body["params"]["capabilities"]["experimental"]["ai.zorai/workspace-context"]
            ["version"],
        1
    );
    for request in requests.iter().filter(|r| r.body["method"] != "tools/call") {
        let wire = request.body.to_string();
        assert!(!wire.contains("conversation-a"));
        assert!(!wire.contains("ai.zorai/workspace\""));
    }
    let call = mock.calls().pop().unwrap();
    assert_eq!(
        call.body["params"]["_meta"]["ai.zorai/workspace"],
        context.workspace_uri.unwrap().to_string()
    );
    assert_eq!(
        call.body["params"]["_meta"]["ai.zorai/conversation"],
        "conversation-a"
    );
    assert!(call.body["params"]["_meta"].get("progressToken").is_some());
    assert!(call.body.get("_meta").is_none());
    assert_eq!(
        call.headers.get("mcp-session-id").map(String::as_str),
        Some("isolated-session")
    );
    assert_eq!(
        call.headers.get("mcp-protocol-version").map(String::as_str),
        Some("2025-06-18")
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn mcp_http_concurrent_a_b_json_calls_and_targeted_cancellation() {
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let mock = Mock::start(move |request| {
        let gate = gate.clone();
        async move {
            if request.body["method"] == "tools/call"
                && request.body["params"]["arguments"]["wait"] == true
            {
                gate.notified().await;
            }
            basic(&request, true)
        }
    })
    .await;
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    manager.apply_desired_config(vec![config(&mock)]).unwrap();
    connected(&manager).await;
    let cancel = CancellationToken::new();
    let context_a = McpRequestContext::from_workspace("thread-a", Some(&a));
    let route_a = route(&manager, "consult");
    let manager_a = manager.clone();
    let cancel_a = cancel.clone();
    let first = tokio::spawn(async move {
        manager_a
            .call(route_a, json!({"wait":true}), context_a, cancel_a)
            .await
    });
    mock.wait_calls(1).await;
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        manager.call(
            route(&manager, "consult"),
            json!({}),
            McpRequestContext::from_workspace("thread-b", Some(&b)),
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!second.is_error);
    let calls = mock.calls();
    assert_eq!(
        calls[0].body["params"]["_meta"]["ai.zorai/conversation"],
        "thread-a"
    );
    assert_eq!(
        calls[1].body["params"]["_meta"]["ai.zorai/conversation"],
        "thread-b"
    );
    assert_ne!(
        calls[0].body["params"]["_meta"]["ai.zorai/workspace"],
        calls[1].body["params"]["_meta"]["ai.zorai/workspace"]
    );
    cancel.cancel();
    assert_eq!(first.await.unwrap().unwrap_err().code, "MCP_CANCELLED");
    assert!(mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.body["method"] == "notifications/cancelled"));
    release.notify_waiters();
    assert!(manager
        .call(
            route(&manager, "echo"),
            json!({}),
            McpRequestContext::from_workspace("thread-b", Some(&b)),
            CancellationToken::new()
        )
        .await
        .is_ok());
    manager.shutdown().await;
}

#[tokio::test]
async fn mcp_http_sse_error_result_and_no_mutation_replay_on_404() {
    let mock=Mock::start(|request|async move{
        if request.body["method"]=="tools/call" {
            if request.body["params"]["arguments"]["lost"]==true{return Reply::status(404);}
            let mut reply=Reply::json(&request,json!({"content":[{"type":"text","text":"root denied"}],"structuredContent":{"code":"ROOT_DENIED","fix":"Grant root"},"isError":true}));reply.content_type="text/event-stream";reply.body=format!(": keepalive\r\nevent: message\r\ndata: {}\r\n\r\n",reply.body);reply
        }else{basic(&request,true)}
    }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    manager.apply_desired_config(vec![config(&mock)]).unwrap();
    connected(&manager).await;
    let result = manager
        .call(
            route(&manager, "echo"),
            json!({}),
            McpRequestContext::from_workspace("t", None),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("ROOT_DENIED"));
    let result = manager
        .call(
            route(&manager, "echo"),
            json!({"lost":true}),
            McpRequestContext::from_workspace("t", None),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(result.message.contains("not replayed"));
    assert_eq!(mock.calls().len(), 2);
    assert!(manager.catalog_snapshot().tools.is_empty());
    assert!(
        manager.statuses()[0].tools.is_empty(),
        "offline server must not retain an unbudgeted status catalog"
    );
    manager.reconnect("mock").await.unwrap();
    connected(&manager).await;
    assert_eq!(mock.calls().len(), 2);
    manager.shutdown().await;
}
