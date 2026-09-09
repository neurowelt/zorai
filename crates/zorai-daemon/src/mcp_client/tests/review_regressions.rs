use super::{super::*, mock::*};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn mcp_short_credentials_preserve_tool_names_schemas_and_results() {
    let mock = Mock::start(|request|async move {
        if request.body["method"] == "tools/list" {
            Reply::json(&request,json!({"tools":[{"name":"test_tool","description":"Run the test","inputSchema":{"type":"object","properties":{"test":{"type":"string"}}}}]}))
        } else if request.body["method"] == "tools/call" {
            Reply::json(&request,json!({"content":[{"type":"text","text":"test completed"}]}))
        } else {basic(&request,true)}
    }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    let mut config = config(&mock);
    config.auth = McpAuthConfig::Bearer {
        credential_ref: None,
    };
    manager
        .save_credential(&mut config, McpCredentialUpdate::Replace("test".into()))
        .unwrap();
    manager.apply_desired_config(vec![config]).unwrap();
    connected(&manager).await;
    let catalog = manager.catalog_snapshot();
    assert!(catalog.tools[0]
        .function
        .description
        .contains("Run the test"));
    assert!(catalog.tools[0].function.parameters["properties"]
        .get("test")
        .is_some());
    let result = manager
        .call(
            route(&manager, "test_tool"),
            json!({}),
            McpRequestContext::from_workspace("thread", None),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.content, "test completed");
    manager.shutdown().await;
}

#[tokio::test]
async fn mcp_discovery_rejects_total_catalog_bytes_across_pages() {
    let mock = Mock::start(|request|async move {
        if request.body["method"] == "tools/list" {
            let second = request.body["params"]["cursor"] == "next";
            let mut result = json!({"tools":[{"name":if second{"second"}else{"first"},"description":"x".repeat(2*1024*1024+100),"inputSchema":{"type":"object"}}]});
            if !second {result["nextCursor"] = json!("next");}
            Reply::json(&request,result)
        }else{basic(&request,true)}
    }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = McpManager::new(temp.path().to_owned());
    let error = manager
        .test_connection(config(&mock), McpCredentialUpdate::Keep)
        .await
        .unwrap_err();
    assert!(error.contains("4 MiB"));
    assert!(manager.catalog_snapshot().tools.is_empty());
    assert!(mock.calls().is_empty());
    assert_eq!(
        mock.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.body["method"] == "tools/list")
            .count(),
        2
    );
}

#[tokio::test]
async fn mcp_test_rejects_credential_with_no_auth_before_network() {
    let mock = Mock::start(|request| async move { basic(&request, true) }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = McpManager::new(temp.path().to_owned());
    let error = manager
        .test_connection(
            config(&mock),
            McpCredentialUpdate::Replace("test-secret".into()),
        )
        .await
        .unwrap_err();
    assert!(error.contains("authentication mode"));
    assert!(mock.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn mcp_test_connection_concurrency_is_bounded_and_permits_recover() {
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let gate = release.clone();
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let entries = entered.clone();
    let mock = Mock::start(move |request| {
        let gate = gate.clone();
        let entries = entries.clone();
        async move {
            if request.body["method"] == "initialize" {
                entries.add_permits(1);
                gate.acquire().await.unwrap().forget();
            }
            basic(&request, true)
        }
    })
    .await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let manager = manager.clone();
        let config = config(&mock);
        tasks.push(tokio::spawn(async move {
            manager
                .test_connection(config, McpCredentialUpdate::Keep)
                .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), entered.acquire_many(4))
        .await
        .unwrap()
        .unwrap()
        .forget();
    let error = manager
        .test_connection(config(&mock), McpCredentialUpdate::Keep)
        .await
        .unwrap_err();
    assert!(error.contains("already running"));
    assert_eq!(
        mock.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.body["method"] == "initialize")
            .count(),
        4
    );
    release.add_permits(5);
    for task in tasks {
        assert!(task.await.unwrap().is_ok());
    }
    assert!(manager
        .test_connection(config(&mock), McpCredentialUpdate::Keep)
        .await
        .is_ok());
}

#[tokio::test]
async fn mcp_response_ids_do_not_remove_client_cancellation_slots() {
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
    let call=serde_json::from_value(json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"echo","arguments":{}}})).unwrap();
    let call = tokio::spawn(transport.send(call));
    mock.wait_calls(1).await;
    let server_response =
        serde_json::from_value(json!({"jsonrpc":"2.0","id":7,"result":{}})).unwrap();
    tokio::time::timeout(Duration::from_secs(2), transport.send(server_response))
        .await
        .expect("responses must not wait behind requests")
        .unwrap();
    let cancel = serde_json::from_value(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}),
    )
    .unwrap();
    transport.send(cancel).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .expect("original request token must remain registered")
        .unwrap();
    assert!(result.unwrap_err().to_string().contains("MCP_CANCELLED"));
    release.notify_waiters();
    transport.close().await.unwrap();
}

#[tokio::test]
async fn mcp_shutdown_awaits_session_delete_completion_with_shared_call_handle() {
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let gate = release.clone();
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let entries = entered.clone();
    let mock = Mock::start(move |request| {
        let gate = gate.clone();
        let entries = entries.clone();
        async move {
            if request.method == "DELETE" {
                entries.add_permits(1);
                gate.acquire().await.unwrap().forget();
            }
            if request.body["method"] == "tools/call" {
                std::future::pending::<()>().await;
            }
            basic(&request, true)
        }
    })
    .await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    manager.apply_desired_config(vec![config(&mock)]).unwrap();
    connected(&manager).await;
    let caller = manager.clone();
    let tool = route(&manager, "echo");
    let call = tokio::spawn(async move {
        caller
            .call(
                tool,
                json!({}),
                McpRequestContext::from_workspace("thread", None),
                CancellationToken::new(),
            )
            .await
    });
    mock.wait_calls(1).await;
    let owner = manager.clone();
    let shutdown = tokio::spawn(async move { owner.shutdown().await });
    tokio::time::timeout(Duration::from_secs(2), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert!(
        !shutdown.is_finished(),
        "shutdown must wait for transport close"
    );
    assert!(manager.catalog_snapshot().tools.is_empty());
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .unwrap()
        .unwrap()
        .is_err());
}

#[tokio::test]
async fn mcp_shutdown_joins_discovery_and_previously_removed_setup_sessions() {
    for mode in ["active", "removed", "draft"] {
        let listed = Arc::new(tokio::sync::Semaphore::new(0));
        let list_notice = listed.clone();
        let deleting = Arc::new(tokio::sync::Semaphore::new(0));
        let delete_notice = deleting.clone();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let gate = release.clone();
        let mock = Mock::start(move |request| {
            let list_notice = list_notice.clone();
            let delete_notice = delete_notice.clone();
            let gate = gate.clone();
            async move {
                if request.body["method"] == "tools/list" {
                    list_notice.add_permits(1);
                    std::future::pending::<()>().await;
                }
                if request.method == "DELETE" {
                    delete_notice.add_permits(1);
                    gate.acquire().await.unwrap().forget();
                }
                basic(&request, true)
            }
        })
        .await;
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(McpManager::new(temp.path().to_owned()));
        let draft = if mode == "draft" {
            let owner = manager.clone();
            let config = config(&mock);
            Some(tokio::spawn(async move {
                owner
                    .test_connection(config, McpCredentialUpdate::Keep)
                    .await
            }))
        } else {
            manager.apply_desired_config(vec![config(&mock)]).unwrap();
            None
        };
        tokio::time::timeout(Duration::from_secs(5), listed.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        if mode != "draft" {
            assert_eq!(manager.statuses()[0].state, "discovering");
        }
        if mode == "removed" {
            manager.apply_desired_config(Vec::new()).unwrap();
        }
        let owner = manager.clone();
        let shutdown = tokio::spawn(async move { owner.shutdown().await });
        tokio::time::timeout(Duration::from_secs(5), deleting.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        assert!(
            !shutdown.is_finished(),
            "shutdown must join setup teardown even when discovery never completed"
        );
        release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap();
        assert!(manager.statuses().is_empty());
        if let Some(draft) = draft {
            assert!(draft.await.unwrap().is_err());
        }
        assert_eq!(
            mock.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == "DELETE")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn mcp_aggregate_catalog_budget_preserves_existing_tools_and_ipc_frame() {
    let mock=Mock::start(|request|async move{
        if request.body["method"]=="tools/list" {return Reply::json(&request,json!({"tools":[{"name":"large","description":"x".repeat(3*1024*1024),"inputSchema":{"type":"object"}}]}));}
        let mut reply=basic(&request,true);
        if request.body["method"]=="initialize" {
            let mut body:serde_json::Value=serde_json::from_str(&reply.body).unwrap();body["result"]["serverInfo"]["name"]=json!("n".repeat(8192));reply.body=body.to_string();
        }
        reply
    }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(McpManager::new(temp.path().to_owned()));
    let configs: Vec<_> = ["one", "two", "three"]
        .into_iter()
        .map(|id| {
            let mut config = config(&mock);
            config.id = id.into();
            config
        })
        .collect();
    manager.apply_desired_config(configs).unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let statuses = manager.statuses();
            if statuses.len() == 3
                && statuses
                    .iter()
                    .all(|s| s.state == "connected" || s.state == "protocol_error")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let statuses = manager.statuses();
    assert_eq!(
        statuses.iter().filter(|s| s.state == "connected").count(),
        2
    );
    let rejected = statuses
        .iter()
        .find(|s| s.state == "protocol_error")
        .unwrap();
    assert!(rejected
        .message
        .as_ref()
        .unwrap()
        .contains("8 MiB aggregate"));
    assert!(rejected.tools.is_empty());
    assert!(statuses
        .iter()
        .filter_map(|s| s.server_name.as_ref())
        .all(|name| name.chars().count() <= 256));
    assert_eq!(manager.catalog_snapshot().tools.len(), 2);
    assert!(
        zorai_protocol::validate_daemon_message_size(&zorai_protocol::DaemonMessage::McpServers {
            servers: statuses
        })
        .unwrap()
            < 16 * 1024 * 1024
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn mcp_remote_discovery_diagnostics_are_bounded() {
    let mock=Mock::start(|request|async move{
        if request.body["method"]=="tools/list" {
            let mut reply=Reply::status(200);reply.body=json!({"jsonrpc":"2.0","id":request.body["id"],"error":{"code":-32603,"message":"remote failure ".repeat(2000)}}).to_string();reply
        }else{basic(&request,true)}
    }).await;
    let temp = tempfile::tempdir().unwrap();
    let manager = McpManager::new(temp.path().to_owned());
    let error = manager
        .test_connection(config(&mock), McpCredentialUpdate::Keep)
        .await
        .unwrap_err();
    assert!(error.contains("remote failure"));
    assert!(error.chars().count() <= 4096);
}

#[tokio::test]
async fn mcp_shutdown_joins_previously_retired_established_sessions() {
    for mode in ["disabled", "removed", "reconnected"] {
        let deleting = Arc::new(tokio::sync::Semaphore::new(0));
        let notice = deleting.clone();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let gate = release.clone();
        let mock = Mock::start(move |request| {
            let notice = notice.clone();
            let gate = gate.clone();
            async move {
                if request.method == "DELETE" {
                    notice.add_permits(1);
                    gate.acquire().await.unwrap().forget();
                }
                basic(&request, true)
            }
        })
        .await;
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(McpManager::new(temp.path().to_owned()));
        manager.apply_desired_config(vec![config(&mock)]).unwrap();
        connected(&manager).await;
        match mode {
            "disabled" => {
                let mut config = config(&mock);
                config.enabled = false;
                manager.apply_desired_config(vec![config]).unwrap();
            }
            "removed" => manager.apply_desired_config(Vec::new()).unwrap(),
            _ => manager.reconnect("mock").await.unwrap(),
        }
        tokio::time::timeout(Duration::from_secs(5), deleting.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        let owner = manager.clone();
        let shutdown = tokio::spawn(async move { owner.shutdown().await });
        tokio::task::yield_now().await;
        assert!(
            !shutdown.is_finished(),
            "shutdown must await the retired session's held DELETE"
        );
        release.add_permits(4);
        tokio::time::timeout(Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap();
        assert!(manager.statuses().is_empty());
    }
}
