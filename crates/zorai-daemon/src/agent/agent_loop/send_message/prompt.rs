use super::*;

impl<'a> SendMessageRunner<'a> {
    pub(super) async fn maybe_rebuild_prompt_after_memory_flush(&mut self) -> Result<()> {
        if !self
            .engine
            .maybe_run_pre_compaction_memory_flush(
                &self.tid,
                self.task_id,
                &self.config,
                &self.provider_config,
                &self.system_prompt,
                self.preferred_session_id,
                self.retry_strategy,
                &mut self.last_pre_compaction_flush_signature,
            )
            .await?
        {
            return Ok(());
        }

        let memory = self.engine.current_memory_snapshot().await;
        let causal_guidance = self.engine.build_causal_guidance_summary().await;
        let sub_agents = self.engine.list_sub_agents().await;
        let structured_memory_summary =
            crate::agent::memory_context::build_structured_memory_summary(
                &memory,
                &self.memory_paths,
                self.continuity_summary.as_deref(),
                self.negative_constraints_context.as_deref(),
            );
        let existing_memory_injection_state = self
            .engine
            .get_thread_memory_injection_state(&self.tid)
            .await;
        let client_surface = self.engine.get_thread_client_surface(&self.tid).await;
        let identity_is_spawned = self
            .engine
            .thread_identity_metadata
            .read()
            .await
            .get(&self.tid)
            .is_some_and(|identity| identity.is_spawned_subagent());
        self.system_prompt = build_system_prompt(
            &self.config,
            &self.base_prompt,
            &memory,
            &self.memory_paths,
            &self.agent_scope_id,
            &sub_agents,
            super::setup::spawned_subagent_turn(
                self.current_task_snapshot.as_ref(),
                identity_is_spawned,
            ),
            self.operator_model_summary.as_deref(),
            self.operational_context.as_deref(),
            causal_guidance.as_deref(),
            self.learned_patterns.as_deref(),
            None,
            self.continuity_summary.as_deref(),
            self.negative_constraints_context.as_deref(),
            client_surface,
        );
        self.system_prompt.push_str("\n\n");
        self.system_prompt.push_str(&build_runtime_identity_prompt(
            &self.runtime_agent_name,
            &self.active_provider_id,
            &self.provider_config.model,
        ));
        if let Some(injection_state) =
            crate::agent::memory_context::append_structured_memory_summary_if_needed(
                &mut self.system_prompt,
                existing_memory_injection_state.as_ref(),
                &structured_memory_summary,
                true,
                true,
            )
        {
            self.engine
                .set_thread_memory_injection_state(&self.tid, injection_state)
                .await;
        }
        if let Some(memory_palace_context) = self
            .engine
            .build_memory_palace_prompt_context(&self.tid, self.task_id)
            .await
        {
            self.system_prompt.push_str("\n\n");
            self.system_prompt.push_str(&memory_palace_context);
        }
        if let Some(recall) = self.onecontext_bootstrap.as_deref() {
            self.system_prompt.push_str("\n\n## OneContext Recall\n");
            self.system_prompt
                .push_str("Use this as historical context from prior sessions when relevant:\n");
            self.system_prompt.push_str(recall);
        }
        if let Some(skill_preflight) = self.skill_preflight.as_ref() {
            self.system_prompt.push_str("\n\n## Preloaded Skills\n");
            self.system_prompt.push_str(&skill_preflight.prompt_context);
        }
        self.system_prompt.push_str(&self.mcp_prompt_context);
        Ok(())
    }
}
