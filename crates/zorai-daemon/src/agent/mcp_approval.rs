//! MCP uses the existing Approval Center and exact-match saved approval rules.
//! The original execution stays suspended, preserving its route, scope and context.
use super::*;
use std::collections::{HashMap, HashSet};
use tokio::sync::oneshot;
use zorai_protocol::ApprovalDecision;

#[derive(Default)]
pub(super) struct McpApprovalState {
    inner: std::sync::Mutex<McpApprovalInner>,
}
#[derive(Default)]
struct McpApprovalInner {
    pending: HashMap<String, PendingMcpApproval>,
    session_grants: HashSet<(String, String)>,
}
struct PendingMcpApproval {
    command: String,
    thread_id: String,
    response: oneshot::Sender<ApprovalDecision>,
}
struct PendingGuard<'a> {
    state: &'a McpApprovalState,
    id: String,
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.state.inner.lock().unwrap().pending.remove(&self.id);
    }
}

impl AgentEngine {
    pub(super) fn has_live_mcp_approval(&self, id: &str) -> bool {
        self.mcp_approval_state
            .inner
            .lock()
            .unwrap()
            .pending
            .get(id)
            .is_some_and(|pending| !pending.response.is_closed())
    }

    pub(super) fn resolve_mcp_approval(&self, id: &str, decision: ApprovalDecision) -> bool {
        let mut state = self.mcp_approval_state.inner.lock().unwrap();
        let Some(pending) = state.pending.remove(id) else {
            return false;
        };
        if pending.response.send(decision).is_err() {
            return false;
        }
        if matches!(decision, ApprovalDecision::ApproveSession) {
            state
                .session_grants
                .insert((pending.thread_id, pending.command));
        }
        true
    }

    pub(in crate::agent) async fn mcp_has_approval(&self, command: &str, thread_id: &str) -> bool {
        if self
            .mcp_approval_state
            .inner
            .lock()
            .unwrap()
            .session_grants
            .contains(&(thread_id.to_string(), command.to_string()))
        {
            return true;
        }
        self.mark_task_approval_rule_used(command).await
    }

    pub(in crate::agent) async fn request_mcp_approval(
        &self,
        command: String,
        thread_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        review: &WelesReviewMeta,
        cancel: CancellationToken,
    ) -> bool {
        let id = format!("mcp-approval-{}", Uuid::new_v4());
        let (response, receive) = oneshot::channel();
        self.mcp_approval_state
            .inner
            .lock()
            .unwrap()
            .pending
            .insert(
                id.clone(),
                PendingMcpApproval {
                    command: command.clone(),
                    thread_id: thread_id.to_string(),
                    response,
                },
            );
        let _guard = PendingGuard {
            state: &self.mcp_approval_state,
            id: id.clone(),
        };
        let mut reasons = review.reasons.clone();
        reasons.push("Approve Once permits this call. Approve Session permits this tool in this conversation. Always Approve permits future calls to this tool with these connection settings; revoke it in Approval Center.".into());
        let pending = ToolPendingApproval {
            approval_id: id.clone(),
            execution_id: format!("mcp-exec-{}", Uuid::new_v4()),
            command: command.clone(),
            rationale: format!(
                "Permission to call {tool_name}. Arguments: {}",
                summarize_text(&crate::scrub::scrub_sensitive(&args.to_string()), 2000)
            ),
            risk_level: "high".into(),
            blast_radius: "remote MCP tool execution".into(),
            reasons: reasons.clone(),
            session_id: None,
        };
        self.remember_pending_approval_command(&pending).await;
        let _ = self.record_operator_approval_requested(&pending).await;
        let _ = self.event_tx.send(AgentEvent::ApprovalRequired {
            thread_id: thread_id.to_string(),
            approval_id: id.clone(),
            command,
            rationale: Some(pending.rationale),
            reasons,
            risk_level: pending.risk_level,
            blast_radius: pending.blast_radius,
        });
        let decision = tokio::select! {
            biased;
            _ = cancel.cancelled() => ApprovalDecision::Deny,
            result = receive => result.unwrap_or(ApprovalDecision::Deny),
        };
        self.forget_pending_approval_command(&id).await;
        // Resolution is recorded by the existing Approval Center handler.
        self.pending_operator_approvals.write().await.remove(&id);
        !matches!(decision, ApprovalDecision::Deny) && !cancel.is_cancelled()
    }
}
