use super::super::{execute_tool, get_available_tools};
use crate::agent::{
    types::{AgentConfig, ToolCall, ToolFunction},
    AgentEngine,
};
use crate::session_manager::SessionManager;
use tempfile::tempdir;
use tokio::sync::broadcast;

#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;

#[tokio::test]
async fn deferred_tool_gate_withholds_niche_tools_but_keeps_core_and_meta() {
    use super::super::partition_deferred_tools;
    use zorai_protocol::tool_names as tn;

    let config = AgentConfig::default();
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let mut tools = crate::agent::agent_identity::run_with_agent_scope(
        crate::agent::agent_identity::MAIN_AGENT_ID.to_string(),
        async { get_available_tools(&config, temp_dir.path(), true) },
    )
    .await;

    let full_count = tools.len();
    let pool = partition_deferred_tools(&mut tools);

    let has = |set: &[super::super::ToolDefinition], name: &str| {
        set.iter().any(|tool| tool.function.name == name)
    };

    // Deferral must actually withhold a meaningful chunk of the catalog,
    // otherwise the per-turn token saving this exists for does not happen.
    assert!(!pool.is_empty(), "expected some tools to be deferred");
    assert_eq!(full_count, tools.len() + pool.len());

    // Discovery/activation meta-tools must always remain callable, or the
    // agent could never reach a withheld tool.
    for meta in [tn::TOOL_SEARCH, tn::LIST_TOOLS, tn::LOAD_TOOLS] {
        assert!(has(&tools, meta), "meta tool {meta} must stay available");
        assert!(!has(&pool, meta), "meta tool {meta} must not be deferred");
    }

    // Everyday tools stay; niche long-tail tools are withheld.
    assert!(has(&tools, tn::READ_FILE), "read_file must stay core");
    assert!(has(&tools, tn::BASH_COMMAND), "bash must stay core");
    assert!(has(&tools, tn::SEARCH_FILES), "search_files must stay core");
    assert!(
        has(&pool, tn::CREATE_ROUTINE),
        "create_routine should defer"
    );
    assert!(has(&pool, tn::RUN_DEBATE), "run_debate should defer");
}

#[tokio::test]
async fn workspace_task_tools_are_exposed_to_all_agent_scopes() {
    let config = AgentConfig::default();
    let temp_dir = tempfile::tempdir().expect("tempdir");

    let svarog_tools = crate::agent::agent_identity::run_with_agent_scope(
        crate::agent::agent_identity::MAIN_AGENT_ID.to_string(),
        async { get_available_tools(&config, temp_dir.path(), false) },
    )
    .await;
    assert!(svarog_tools
        .iter()
        .any(|tool| tool.function.name == "workspace_list_tasks"));
    assert!(svarog_tools
        .iter()
        .any(|tool| tool.function.name == "workspace_create_task"));
    assert!(svarog_tools
        .iter()
        .any(|tool| tool.function.name == "workspace_submit_completion"));

    let rarog_tools = crate::agent::agent_identity::run_with_agent_scope(
        crate::agent::agent_identity::CONCIERGE_AGENT_ID.to_string(),
        async { get_available_tools(&config, temp_dir.path(), false) },
    )
    .await;
    assert!(rarog_tools
        .iter()
        .any(|tool| tool.function.name == "workspace_list_tasks"));
    assert!(rarog_tools
        .iter()
        .any(|tool| tool.function.name == "workspace_submit_review"));
    assert!(rarog_tools
        .iter()
        .any(|tool| tool.function.name == "workspace_submit_completion"));
    assert!(rarog_tools
        .iter()
        .any(|tool| tool.function.name == "workspace_create_task"));
    assert!(rarog_tools
        .iter()
        .any(|tool| tool.function.name == "workspace_move_task"));
}

#[tokio::test]
async fn workspace_create_task_tool_persists_task_for_svarog_scope() {
    let root = tempdir().expect("tempdir should succeed");
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager.clone(), AgentConfig::default(), root.path()).await;
    let (event_tx, _) = broadcast::channel(8);
    let tool_call = ToolCall::with_default_weles_review(
        "tool-workspace-create-task".to_string(),
        ToolFunction {
            name: "workspace_create_task".to_string(),
            arguments: serde_json::json!({
                "workspace_id": "main",
                "task_type": "thread",
                "title": "Ship workspace tools",
                "description": "Expose workspace task operations to Svarog"
            })
            .to_string(),
        },
    );

    let result = crate::agent::agent_identity::run_with_agent_scope(
        crate::agent::agent_identity::MAIN_AGENT_ID.to_string(),
        execute_tool(
            &tool_call,
            &engine,
            "thread-workspace-tool",
            None,
            &manager,
            None,
            &event_tx,
            root.path(),
            &engine.http_client,
            None,
            None,
        ),
    )
    .await;

    assert!(
        !result.is_error,
        "workspace_create_task should succeed: {}",
        result.content
    );
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("tool should return JSON");
    let task_id = payload["id"].as_str().expect("task id").to_string();
    let task = engine
        .get_workspace_task(&task_id)
        .await
        .expect("load task")
        .expect("task exists");
    assert_eq!(task.title, "Ship workspace tools");
}

#[tokio::test]
async fn workspace_mutation_tool_is_rejected_outside_svarog_scope() {
    let root = tempdir().expect("tempdir should succeed");
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager.clone(), AgentConfig::default(), root.path()).await;
    let (event_tx, _) = broadcast::channel(8);
    let tool_call = ToolCall::with_default_weles_review(
        "tool-workspace-create-task-blocked".to_string(),
        ToolFunction {
            name: "workspace_create_task".to_string(),
            arguments: serde_json::json!({
                "workspace_id": "main",
                "task_type": "thread",
                "title": "Should not create",
                "description": "Non-Svarog scopes must not mutate workspace tasks"
            })
            .to_string(),
        },
    );

    let result = crate::agent::agent_identity::run_with_agent_scope(
        crate::agent::agent_identity::CONCIERGE_AGENT_ID.to_string(),
        execute_tool(
            &tool_call,
            &engine,
            "thread-workspace-tool-blocked",
            None,
            &manager,
            None,
            &event_tx,
            root.path(),
            &engine.http_client,
            None,
            None,
        ),
    )
    .await;

    assert!(result.is_error);
    assert!(result.content.contains("only available to Svarog"));
}

#[tokio::test]
async fn workspace_submit_review_tool_allows_assigned_reviewer_scope() {
    let root = tempdir().expect("tempdir should succeed");
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager.clone(), AgentConfig::default(), root.path()).await;
    let (event_tx, _) = broadcast::channel(8);
    let mut task = engine
        .create_workspace_task(
            zorai_protocol::WorkspaceTaskCreate {
                workspace_id: "main".to_string(),
                title: "Reviewable".to_string(),
                task_type: zorai_protocol::WorkspaceTaskType::Goal,
                description: "Needs review".to_string(),
                definition_of_done: None,
                priority: None,
                assignee: Some(zorai_protocol::WorkspaceActor::Agent(
                    crate::agent::agent_identity::MAIN_AGENT_ID.to_string(),
                )),
                reviewer: Some(zorai_protocol::WorkspaceActor::Subagent("qa".to_string())),
            },
            zorai_protocol::WorkspaceActor::User,
        )
        .await
        .expect("create workspace task");
    task.status = zorai_protocol::WorkspaceTaskStatus::InReview;
    engine
        .history
        .upsert_workspace_task(&task)
        .await
        .expect("persist review state");
    let tool_call = ToolCall::with_default_weles_review(
        "tool-workspace-submit-review".to_string(),
        ToolFunction {
            name: "workspace_submit_review".to_string(),
            arguments: serde_json::json!({
                "task_id": task.id,
                "verdict": "pass",
                "message": "Looks complete"
            })
            .to_string(),
        },
    );

    let result = crate::agent::agent_identity::run_with_agent_scope(
        "qa".to_string(),
        execute_tool(
            &tool_call,
            &engine,
            "thread-workspace-review-tool",
            None,
            &manager,
            None,
            &event_tx,
            root.path(),
            &engine.http_client,
            None,
            None,
        ),
    )
    .await;

    assert!(
        !result.is_error,
        "reviewer should submit review: {}",
        result.content
    );
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("tool should return JSON");
    assert_eq!(payload["status"], "done");
}

#[tokio::test]
async fn workspace_submit_completion_tool_allows_assigned_assignee_scope_and_queues_review() {
    let root = tempdir().expect("tempdir should succeed");
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager.clone(), AgentConfig::default(), root.path()).await;
    let (event_tx, _) = broadcast::channel(8);
    let mut task = engine
        .create_workspace_task(
            zorai_protocol::WorkspaceTaskCreate {
                workspace_id: "main".to_string(),
                title: "Implement feature".to_string(),
                task_type: zorai_protocol::WorkspaceTaskType::Thread,
                description: "Needs delivery".to_string(),
                definition_of_done: None,
                priority: None,
                assignee: Some(zorai_protocol::WorkspaceActor::Subagent(
                    "worker".to_string(),
                )),
                reviewer: Some(zorai_protocol::WorkspaceActor::Subagent("qa".to_string())),
            },
            zorai_protocol::WorkspaceActor::User,
        )
        .await
        .expect("create workspace task");
    task.status = zorai_protocol::WorkspaceTaskStatus::InProgress;
    engine
        .history
        .upsert_workspace_task(&task)
        .await
        .expect("persist running state");
    let tool_call = ToolCall::with_default_weles_review(
        "tool-workspace-submit-completion".to_string(),
        ToolFunction {
            name: "workspace_submit_completion".to_string(),
            arguments: serde_json::json!({
                "task_id": task.id,
                "summary": "Implemented the feature and added regression coverage."
            })
            .to_string(),
        },
    );

    let result = crate::agent::agent_identity::run_with_agent_scope(
        "worker".to_string(),
        execute_tool(
            &tool_call,
            &engine,
            "thread-workspace-completion-tool",
            None,
            &manager,
            None,
            &event_tx,
            root.path(),
            &engine.http_client,
            None,
            None,
        ),
    )
    .await;

    assert!(
        !result.is_error,
        "assignee should submit completion: {}",
        result.content
    );
    let payload: serde_json::Value =
        serde_json::from_str(&result.content).expect("tool should return JSON");
    assert_eq!(payload["status"], "in_review");
    let notices = engine
        .list_workspace_notices("main", Some(payload["id"].as_str().expect("task id")))
        .await
        .expect("list notices");
    assert!(notices.iter().any(|notice| {
        notice.notice_type == "task_completion"
            && notice
                .message
                .contains("Implemented the feature and added regression coverage")
    }));
    assert!(notices.iter().any(|notice| {
        notice.notice_type == "review_requested" && notice.message.contains("queued review task")
    }));
    let tasks = engine.tasks.lock().await;
    let review_task = tasks
        .iter()
        .find(|task| task.source == "workspace_review")
        .expect("completion should queue reviewer task");
    assert_eq!(review_task.runtime, "qa");
}

#[tokio::test]
async fn workspace_submit_completion_tool_rejects_non_assignee_scope() {
    let root = tempdir().expect("tempdir should succeed");
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager.clone(), AgentConfig::default(), root.path()).await;
    let (event_tx, _) = broadcast::channel(8);
    let mut task = engine
        .create_workspace_task(
            zorai_protocol::WorkspaceTaskCreate {
                workspace_id: "main".to_string(),
                title: "Implement feature".to_string(),
                task_type: zorai_protocol::WorkspaceTaskType::Thread,
                description: "Needs delivery".to_string(),
                definition_of_done: None,
                priority: None,
                assignee: Some(zorai_protocol::WorkspaceActor::Subagent(
                    "worker".to_string(),
                )),
                reviewer: Some(zorai_protocol::WorkspaceActor::Subagent("qa".to_string())),
            },
            zorai_protocol::WorkspaceActor::User,
        )
        .await
        .expect("create workspace task");
    task.status = zorai_protocol::WorkspaceTaskStatus::InProgress;
    engine
        .history
        .upsert_workspace_task(&task)
        .await
        .expect("persist running state");
    let tool_call = ToolCall::with_default_weles_review(
        "tool-workspace-submit-completion-reject".to_string(),
        ToolFunction {
            name: "workspace_submit_completion".to_string(),
            arguments: serde_json::json!({
                "task_id": task.id,
                "summary": "Tried to complete somebody else's task."
            })
            .to_string(),
        },
    );

    let result = crate::agent::agent_identity::run_with_agent_scope(
        "other".to_string(),
        execute_tool(
            &tool_call,
            &engine,
            "thread-workspace-completion-tool-reject",
            None,
            &manager,
            None,
            &event_tx,
            root.path(),
            &engine.http_client,
            None,
            None,
        ),
    )
    .await;

    assert!(result.is_error);
    assert!(result.content.contains("assigned workspace assignee"));
}

#[tokio::test]
async fn workspace_submit_review_tool_rejects_non_reviewer_scope() {
    let root = tempdir().expect("tempdir should succeed");
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager.clone(), AgentConfig::default(), root.path()).await;
    let (event_tx, _) = broadcast::channel(8);
    let mut task = engine
        .create_workspace_task(
            zorai_protocol::WorkspaceTaskCreate {
                workspace_id: "main".to_string(),
                title: "Reviewable".to_string(),
                task_type: zorai_protocol::WorkspaceTaskType::Goal,
                description: "Needs review".to_string(),
                definition_of_done: None,
                priority: None,
                assignee: Some(zorai_protocol::WorkspaceActor::Agent(
                    crate::agent::agent_identity::MAIN_AGENT_ID.to_string(),
                )),
                reviewer: Some(zorai_protocol::WorkspaceActor::Subagent("qa".to_string())),
            },
            zorai_protocol::WorkspaceActor::User,
        )
        .await
        .expect("create workspace task");
    task.status = zorai_protocol::WorkspaceTaskStatus::InReview;
    engine
        .history
        .upsert_workspace_task(&task)
        .await
        .expect("persist review state");
    let tool_call = ToolCall::with_default_weles_review(
        "tool-workspace-submit-review-reject".to_string(),
        ToolFunction {
            name: "workspace_submit_review".to_string(),
            arguments: serde_json::json!({
                "task_id": task.id,
                "verdict": "pass",
                "message": "Looks complete"
            })
            .to_string(),
        },
    );

    let result = crate::agent::agent_identity::run_with_agent_scope(
        "dev".to_string(),
        execute_tool(
            &tool_call,
            &engine,
            "thread-workspace-review-tool-reject",
            None,
            &manager,
            None,
            &event_tx,
            root.path(),
            &engine.http_client,
            None,
            None,
        ),
    )
    .await;

    assert!(result.is_error);
    assert!(result.content.contains("assigned workspace reviewer"));
}

#[tokio::test]
async fn automatic_workspace_reviewer_can_complete_review_with_tool() {
    let root = tempdir().expect("tempdir should succeed");
    let manager = SessionManager::new_test(root.path()).await;
    let engine = AgentEngine::new_test(manager.clone(), AgentConfig::default(), root.path()).await;
    let (event_tx, _) = broadcast::channel(8);
    let task = engine
        .create_workspace_task(
            zorai_protocol::WorkspaceTaskCreate {
                workspace_id: "main".to_string(),
                title: "Reviewable".to_string(),
                task_type: zorai_protocol::WorkspaceTaskType::Goal,
                description: "Needs review".to_string(),
                definition_of_done: None,
                priority: None,
                assignee: Some(zorai_protocol::WorkspaceActor::Agent(
                    crate::agent::agent_identity::MAIN_AGENT_ID.to_string(),
                )),
                reviewer: Some(zorai_protocol::WorkspaceActor::Subagent("qa".to_string())),
            },
            zorai_protocol::WorkspaceActor::User,
        )
        .await
        .expect("create workspace task");

    engine
        .move_workspace_task(zorai_protocol::WorkspaceTaskMove {
            task_id: task.id.clone(),
            status: zorai_protocol::WorkspaceTaskStatus::InReview,
            sort_order: None,
        })
        .await
        .expect("move to review");
    {
        let tasks = engine.tasks.lock().await;
        let review_task = tasks
            .iter()
            .find(|task| task.source == "workspace_review")
            .expect("review task should be queued");
        assert_eq!(review_task.runtime, "qa");
        assert!(review_task.description.contains("workspace_submit_review"));
        assert!(review_task
            .description
            .contains("Complete this review task by calling workspace_submit_review"));
        assert!(review_task
            .description
            .contains("pass moves the original workspace task to done"));
        assert!(review_task
            .description
            .contains("fail moves it back to in-progress"));
        assert!(review_task
            .description
            .contains("Do not call workspace_submit_completion for this review task"));
        assert!(review_task.description.contains(&task.id));
        assert!(review_task
            .description
            .contains("Your job is to review completion of workspace task"));
        assert!(review_task.description.contains("Workspace task id:"));
        assert!(review_task
            .description
            .contains("Assignee delivery summary:"));
        assert!(review_task.description.contains("Review goal run:"));
    }

    let tool_call = ToolCall::with_default_weles_review(
        "tool-workspace-auto-review-pass".to_string(),
        ToolFunction {
            name: "workspace_submit_review".to_string(),
            arguments: serde_json::json!({
                "task_id": task.id,
                "verdict": "pass",
                "message": "Complete"
            })
            .to_string(),
        },
    );
    let result = crate::agent::agent_identity::run_with_agent_scope(
        "qa".to_string(),
        execute_tool(
            &tool_call,
            &engine,
            "thread-workspace-auto-review-tool",
            None,
            &manager,
            None,
            &event_tx,
            root.path(),
            &engine.http_client,
            None,
            None,
        ),
    )
    .await;

    assert!(!result.is_error, "review should pass: {}", result.content);
    let reviewed = engine
        .get_workspace_task(&task.id)
        .await
        .expect("load reviewed task")
        .expect("reviewed task exists");
    assert_eq!(reviewed.status, zorai_protocol::WorkspaceTaskStatus::Done);
}
