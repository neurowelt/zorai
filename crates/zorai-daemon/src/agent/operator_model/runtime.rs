use super::*;
use crate::agent::learning::traces::hash_context_blob;
use crate::agent::tool_executor::execute_tool;

impl AgentEngine {
    async fn persist_implicit_feedback_signal(
        &self,
        session_id: &str,
        signal_type: &str,
        weight: f64,
        timestamp_ms: u64,
        context_snapshot: serde_json::Value,
    ) -> Result<()> {
        self.history
            .insert_implicit_signal(&crate::history::ImplicitSignalRow {
                id: format!("implicit_{}", uuid::Uuid::new_v4()),
                session_id: session_id.to_string(),
                signal_type: signal_type.to_string(),
                weight,
                timestamp_ms,
                context_snapshot_json: Some(context_snapshot.to_string()),
            })
            .await
    }

    async fn persist_operator_satisfaction_snapshot(
        &self,
        session_id: &str,
        computed_at_ms: u64,
        model: &OperatorModel,
    ) -> Result<()> {
        let signal_count = model.implicit_feedback.tool_hesitation_count
            + model.implicit_feedback.revision_message_count
            + model.implicit_feedback.correction_message_count
            + model.implicit_feedback.fast_denial_count
            + model.implicit_feedback.rapid_revert_count
            + model.implicit_feedback.session_abandon_count
            + model.attention_topology.rapid_switch_count;
        self.history
            .insert_satisfaction_score(&crate::history::SatisfactionScoreRow {
                id: format!("satisfaction_{}", uuid::Uuid::new_v4()),
                session_id: session_id.to_string(),
                score: model.operator_satisfaction.score,
                computed_at_ms,
                label: model.operator_satisfaction.label.clone(),
                signal_count,
            })
            .await
    }

    pub(crate) async fn persist_intent_prediction_if_present(&self, item: &AnticipatoryItem) {
        let Some(payload) = item.intent_prediction.as_ref() else {
            return;
        };
        let session_id = item
            .thread_id
            .clone()
            .unwrap_or_else(|| "global".to_string());
        let context_state_hash = hash_context_blob(&format!(
            "{}|{}|{}|{}",
            session_id,
            item.kind,
            payload.primary_action,
            item.preferred_attention_surface
                .as_deref()
                .unwrap_or_default()
        ));
        let _ = self
            .history
            .insert_intent_prediction(&crate::history::IntentPredictionRow {
                id: format!("intent_prediction_{}", uuid::Uuid::new_v4()),
                session_id,
                context_state_hash,
                predicted_action: payload.primary_action.clone(),
                confidence: payload.confidence,
                actual_action: None,
                was_correct: None,
                created_at_ms: item.created_at,
            })
            .await;
    }

    pub(crate) async fn persist_system_outcome_prediction_if_present(
        &self,
        item: &AnticipatoryItem,
    ) {
        if item.kind != "system_outcome_foresight" {
            return;
        }
        let session_id = item
            .thread_id
            .clone()
            .unwrap_or_else(|| "global".to_string());
        let prediction_type = item
            .bullets
            .iter()
            .find_map(|bullet| bullet.strip_prefix("prediction_type="))
            .unwrap_or("unknown")
            .to_string();
        let predicted_outcome = if prediction_type == "stale_context" {
            "stale context".to_string()
        } else {
            "build/test failure".to_string()
        };
        let _ = self
            .history
            .insert_system_outcome_prediction(&crate::history::SystemOutcomePredictionRow {
                id: format!("system_outcome_prediction_{}", uuid::Uuid::new_v4()),
                session_id,
                prediction_type,
                predicted_outcome,
                confidence: item.confidence,
                actual_outcome: None,
                was_correct: None,
                created_at_ms: item.created_at,
            })
            .await;
    }

    pub(crate) async fn resolve_system_outcome_prediction_feedback(
        &self,
        thread_id: &str,
        observed_outcome: &str,
    ) {
        let _ = self
            .history
            .resolve_latest_system_outcome_prediction(thread_id, observed_outcome)
            .await;
    }

    fn classify_observed_operator_action(content: &str) -> &'static str {
        let lowered = content.trim().to_ascii_lowercase();
        if lowered.contains("approval") {
            "review pending approval"
        } else if lowered.contains("test")
            || lowered.contains("build")
            || lowered.contains("repo")
            || lowered.contains("diff")
            || lowered.contains("inspect")
            || lowered.contains("change")
        {
            "inspect or test recent repo changes"
        } else {
            "continue the active thread"
        }
    }

    pub(crate) async fn record_rapid_revert_feedback(
        &self,
        thread_id: &str,
        path: &str,
        source_tool: &str,
        repo_root: Option<&str>,
        agent_edit_recorded_at: u64,
        detected_at: u64,
    ) -> Result<()> {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled || !settings.allow_implicit_feedback {
            return Ok(());
        }

        ensure_operator_model_file(&self.data_dir).await?;
        let age_ms = detected_at.saturating_sub(agent_edit_recorded_at);
        let model_snapshot = {
            let mut model = self.operator_model.write().await;
            model.last_updated = detected_at;
            model.implicit_feedback.rapid_revert_count += 1;
            refresh_operator_satisfaction(&mut model);
            persist_operator_model(&self.data_dir, &model)?;
            model.clone()
        };

        self.record_behavioral_event(
            "rapid_revert",
            BehavioralEventContext {
                thread_id: Some(thread_id),
                task_id: None,
                goal_run_id: None,
                approval_id: None,
            },
            serde_json::json!({
                "thread_id": thread_id,
                "path": path,
                "source_tool": source_tool,
                "repo_root": repo_root,
                "agent_edit_recorded_at": agent_edit_recorded_at,
                "detected_at": detected_at,
                "age_ms": age_ms,
            }),
        )
        .await?;

        self.persist_implicit_feedback_signal(
            thread_id,
            "rapid_revert",
            -0.20,
            detected_at,
            serde_json::json!({
                "thread_id": thread_id,
                "path": path,
                "source_tool": source_tool,
                "repo_root": repo_root,
                "agent_edit_recorded_at": agent_edit_recorded_at,
                "detected_at": detected_at,
                "age_ms": age_ms,
            }),
        )
        .await?;
        self.persist_operator_satisfaction_snapshot(thread_id, detected_at, &model_snapshot)
            .await?;
        Ok(())
    }

    pub(crate) async fn record_session_abandon_feedback(
        &self,
        thread_id: &str,
        last_assistant_message: &str,
        assistant_timestamp: u64,
        detected_at: u64,
    ) -> Result<()> {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled || !settings.allow_implicit_feedback {
            return Ok(());
        }

        ensure_operator_model_file(&self.data_dir).await?;
        let age_ms = detected_at.saturating_sub(assistant_timestamp);
        let model_snapshot = {
            let mut model = self.operator_model.write().await;
            model.last_updated = detected_at;
            model.implicit_feedback.session_abandon_count += 1;
            refresh_operator_satisfaction(&mut model);
            persist_operator_model(&self.data_dir, &model)?;
            model.clone()
        };

        self.record_behavioral_event(
            "session_abandon",
            BehavioralEventContext {
                thread_id: Some(thread_id),
                task_id: None,
                goal_run_id: None,
                approval_id: None,
            },
            serde_json::json!({
                "thread_id": thread_id,
                "last_assistant_message": last_assistant_message,
                "assistant_timestamp": assistant_timestamp,
                "detected_at": detected_at,
                "age_ms": age_ms,
            }),
        )
        .await?;

        self.persist_implicit_feedback_signal(
            thread_id,
            "session_abandon",
            -0.14,
            detected_at,
            serde_json::json!({
                "thread_id": thread_id,
                "last_assistant_message": last_assistant_message,
                "assistant_timestamp": assistant_timestamp,
                "detected_at": detected_at,
                "age_ms": age_ms,
            }),
        )
        .await?;
        self.persist_operator_satisfaction_snapshot(thread_id, detected_at, &model_snapshot)
            .await?;
        Ok(())
    }

    pub(crate) async fn learned_approval_decision(
        &self,
        command: &str,
        risk_level: &str,
    ) -> Option<ApprovalDecision> {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled || !settings.allow_approval_learning {
            return None;
        }

        let category = classify_command_category(command, risk_level);
        let model = self.operator_model.read().await;
        if model
            .risk_fingerprint
            .auto_deny_categories
            .iter()
            .any(|candidate| candidate == category)
        {
            return Some(ApprovalDecision::Deny);
        }
        if model.cognitive_style.confirmation_seeking >= 0.8 {
            return None;
        }
        if model
            .risk_fingerprint
            .auto_approve_categories
            .iter()
            .any(|candidate| candidate == category)
        {
            return Some(ApprovalDecision::ApproveOnce);
        }
        None
    }

    #[cfg(test)]
    pub(crate) async fn should_suppress_duplicate_low_value_approval_bundle(
        &self,
        pending_approval: &ToolPendingApproval,
    ) -> bool {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled || !settings.allow_approval_learning {
            return false;
        }

        let category =
            classify_command_category(&pending_approval.command, &pending_approval.risk_level);
        let is_low_value = matches!(category, "git" | "low_risk")
            && matches!(pending_approval.risk_level.as_str(), "lowest" | "yolo");
        if !is_low_value {
            return false;
        }

        let model = self.operator_model.read().await;
        if model.risk_fingerprint.avg_response_time_secs < 30.0 {
            return false;
        }
        drop(model);

        let pending = self.pending_operator_approvals.read().await;
        if pending.is_empty() {
            return false;
        }

        pending
            .values()
            .any(|existing| existing.category == category)
    }

    pub async fn resume_critique_approval_continuation(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
        session_manager: &Arc<SessionManager>,
        event_tx: &broadcast::Sender<AgentEvent>,
        agent_data_dir: &std::path::Path,
        http_client: &reqwest::Client,
    ) -> Result<ToolResult> {
        self.record_operator_approval_resolution(approval_id, decision)
            .await?;

        let continuation = self
            .critique_approval_continuations
            .lock()
            .await
            .remove(approval_id);

        if matches!(decision, ApprovalDecision::Deny) {
            return Ok(ToolResult {
                tool_call_id: approval_id.to_string(),
                name: "critique_confirmation".to_string(),
                content: "Critique confirmation denied by operator.".to_string(),
                is_error: true,
                weles_review: None,
                pending_approval: None,
            });
        }

        let continuation = continuation.ok_or_else(|| {
            anyhow::anyhow!("unknown critique approval continuation: {approval_id}")
        })?;

        Ok(execute_tool(
            &continuation.tool_call,
            self,
            &continuation.thread_id,
            None,
            session_manager,
            None,
            event_tx,
            agent_data_dir,
            http_client,
            None,
            None,
        )
        .await)
    }

    pub(crate) async fn build_operator_model_prompt_summary(&self) -> Option<String> {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled {
            return None;
        }
        let model = self.operator_model.read().await;
        if !has_operator_satisfaction_signal(&model) {
            return None;
        }

        let mut lines = Vec::new();
        if settings.allow_message_statistics && model.cognitive_style.message_count > 0 {
            lines.push(format!(
                "- Output density: {} (avg {:.1} words per message, questions {:.0}%)",
                verbosity_label(model.cognitive_style.verbosity_preference),
                model.cognitive_style.avg_message_length,
                model.cognitive_style.question_frequency * 100.0,
            ));
            if let Some(reading_pattern) = reading_pattern_summary(&model.cognitive_style) {
                lines.push(format!("- Reading pattern: {reading_pattern}"));
            }
        }
        if settings.allow_approval_learning && model.risk_fingerprint.approval_requests > 0 {
            lines.push(format!(
                "- Risk tolerance: {} ({} approvals across {} approval requests, avg response {:.1}s)",
                risk_tolerance_label(model.risk_fingerprint.risk_tolerance),
                model.risk_fingerprint.approvals,
                model.risk_fingerprint.approval_requests,
                model.risk_fingerprint.avg_response_time_secs,
            ));
            if !model.risk_fingerprint.auto_approve_categories.is_empty()
                || !model.risk_fingerprint.auto_deny_categories.is_empty()
            {
                let auto_approve = if model.risk_fingerprint.auto_approve_categories.is_empty() {
                    "none".to_string()
                } else {
                    model.risk_fingerprint.auto_approve_categories.join(", ")
                };
                let auto_deny = if model.risk_fingerprint.auto_deny_categories.is_empty() {
                    "none".to_string()
                } else {
                    model.risk_fingerprint.auto_deny_categories.join(", ")
                };
                lines.push(format!(
                    "- Learned approval shortcuts: auto-approve [{}]; auto-deny [{}]",
                    auto_approve, auto_deny
                ));
            }
        }
        if settings.allow_message_statistics {
            if let Some(hour) = model.session_rhythm.typical_start_hour_utc {
                lines.push(format!(
                    "- Session rhythm: usually starts around {:02}:00 UTC; avg observed session {:.1}m",
                    hour, model.session_rhythm.session_duration_avg_minutes,
                ));
            }
        }
        if settings.allow_attention_tracking && model.attention_topology.focus_event_count > 0 {
            let dominant_surface = model
                .attention_topology
                .dominant_surface
                .as_deref()
                .unwrap_or("unknown");
            let transitions = if model.attention_topology.top_transitions.is_empty() {
                "no stable transitions yet".to_string()
            } else {
                model.attention_topology.top_transitions.join(", ")
            };
            lines.push(format!(
                "- Attention topology: mainly {} ({} focus events, avg dwell {:.1}s, rapid switches {}); common transitions {}; deep focus {}",
                dominant_surface,
                model.attention_topology.focus_event_count,
                model.attention_topology.avg_surface_dwell_secs,
                model.attention_topology.rapid_switch_count,
                transitions,
                model.attention_topology.deep_focus_surface.as_deref().unwrap_or("unknown"),
            ));
        }
        if settings.allow_implicit_feedback
            && (model.implicit_feedback.tool_hesitation_count > 0
                || model.implicit_feedback.revision_message_count > 0
                || model.implicit_feedback.fast_denial_count > 0
                || model.implicit_feedback.rapid_revert_count > 0
                || model.implicit_feedback.session_abandon_count > 0)
        {
            let fallback = model
                .implicit_feedback
                .top_tool_fallbacks
                .first()
                .cloned()
                .unwrap_or_else(|| "none yet".to_string());
            if model.implicit_feedback.rapid_revert_count > 0
                || model.implicit_feedback.session_abandon_count > 0
            {
                lines.push(format!(
                    "- Implicit feedback: {} tool fallback(s), {} revision-style operator message(s), {} fast denial(s), {} rapid revert(s), {} session abandon(s); common fallback {}",
                    model.implicit_feedback.tool_hesitation_count,
                    model.implicit_feedback.revision_message_count,
                    model.implicit_feedback.fast_denial_count,
                    model.implicit_feedback.rapid_revert_count,
                    model.implicit_feedback.session_abandon_count,
                    fallback,
                ));
            } else {
                lines.push(format!(
                    "- Implicit feedback: {} tool fallback(s), {} revision-style operator message(s), {} fast denial(s); common fallback {}",
                    model.implicit_feedback.tool_hesitation_count,
                    model.implicit_feedback.revision_message_count,
                    model.implicit_feedback.fast_denial_count,
                    fallback,
                ));
            }
        }
        lines.push(format!(
            "- Satisfaction signal: {} ({:.2}); friction markers revisions {}, corrections {}, tool fallbacks {}, fast denials {}{}{}",
            model.operator_satisfaction.label,
            model.operator_satisfaction.score,
            model.implicit_feedback.revision_message_count,
            model.implicit_feedback.correction_message_count,
            model.implicit_feedback.tool_hesitation_count,
            model.implicit_feedback.fast_denial_count,
            if model.implicit_feedback.rapid_revert_count > 0 {
                format!(", rapid reverts {}", model.implicit_feedback.rapid_revert_count)
            } else {
                String::new()
            },
            if model.implicit_feedback.session_abandon_count > 0 {
                format!(", session abandons {}", model.implicit_feedback.session_abandon_count)
            } else {
                String::new()
            },
        ));
        lines.extend(operator_adaptation_lines(&model));
        if lines.is_empty() {
            return None;
        }

        Some(format!("## Operator Model\n{}", lines.join("\n")))
    }

    pub(crate) async fn record_operator_message(
        &self,
        thread_id: &str,
        content: &str,
        is_new_thread: bool,
    ) -> Result<()> {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled
            || (!settings.allow_message_statistics && !settings.allow_implicit_feedback)
        {
            return Ok(());
        }
        ensure_operator_model_file(&self.data_dir).await?;
        let now = now_millis();
        let word_count = count_words(content) as f64;
        let is_question = content.contains('?');
        let confirmation_like = contains_confirmation_phrase(content);
        let revision_kind = detect_revision_signal(content);
        let reading_signal = detect_reading_signal(content);
        let current_hour_utc = current_utc_hour(now);

        let thread_created_at = match self.history.thread_created_at(thread_id).await {
            Ok(created_at) => created_at,
            Err(error) => {
                tracing::warn!(
                    thread_id = %thread_id,
                    "failed to load persisted thread creation time for operator observation: {error}"
                );
                None
            }
        };

        let observed_minutes_delta = {
            let mut active_sessions = self.active_operator_sessions.write().await;
            if is_new_thread {
                active_sessions.insert(thread_id.to_string(), 0);
            }

            if let Some(created_at) = thread_created_at {
                let observed_minutes = now.saturating_sub(created_at) / 60_000;
                if let Some(previous_minutes) = active_sessions.get_mut(thread_id) {
                    if observed_minutes > *previous_minutes {
                        let delta = observed_minutes - *previous_minutes;
                        *previous_minutes = observed_minutes;
                        Some(delta)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };

        {
            let mut model = self.operator_model.write().await;
            model.last_updated = now;

            if settings.allow_message_statistics {
                let next_count = model.cognitive_style.message_count + 1;
                model.cognitive_style.avg_message_length = update_running_average(
                    model.cognitive_style.avg_message_length,
                    model.cognitive_style.message_count,
                    word_count,
                );
                model.cognitive_style.message_count = next_count;
                if is_question {
                    model.cognitive_style.question_count += 1;
                }
                if confirmation_like {
                    model.cognitive_style.confirmation_count += 1;
                }
                model.cognitive_style.question_frequency =
                    model.cognitive_style.question_count as f64 / next_count as f64;
                model.cognitive_style.confirmation_seeking =
                    model.cognitive_style.confirmation_count as f64 / next_count as f64;
                model.cognitive_style.verbosity_preference =
                    verbosity_preference_for_length(model.cognitive_style.avg_message_length);
                record_reading_signal(&mut model.cognitive_style, reading_signal);
            }
            if settings.allow_implicit_feedback {
                if revision_kind.is_revision() {
                    model.implicit_feedback.revision_message_count += 1;
                }
                if revision_kind.is_correction() {
                    model.implicit_feedback.correction_message_count += 1;
                }
            }

            if settings.allow_message_statistics {
                *model
                    .session_rhythm
                    .activity_hour_histogram
                    .entry(current_hour_utc)
                    .or_insert(0) += 1;
                model.session_rhythm.peak_activity_hours_utc =
                    top_hours(&model.session_rhythm.activity_hour_histogram, 3);

                if is_new_thread {
                    model.session_count += 1;
                    model.session_rhythm.session_count += 1;
                    *model
                        .session_rhythm
                        .start_hour_histogram
                        .entry(current_hour_utc)
                        .or_insert(0) += 1;
                    model.session_rhythm.typical_start_hour_utc =
                        most_common_hour(&model.session_rhythm.start_hour_histogram);
                }

                if let Some(delta) = observed_minutes_delta {
                    model.session_rhythm.total_observed_session_minutes += delta;
                    if model.session_rhythm.session_count > 0 {
                        model.session_rhythm.session_duration_avg_minutes =
                            model.session_rhythm.total_observed_session_minutes as f64
                                / model.session_rhythm.session_count as f64;
                    }
                }
            }

            refresh_operator_satisfaction(&mut model);
            persist_operator_model(&self.data_dir, &model)?;
        }
        self.record_behavioral_event(
            "operator_message",
            BehavioralEventContext {
                thread_id: Some(thread_id),
                task_id: None,
                goal_run_id: None,
                approval_id: None,
            },
            serde_json::json!({
                "is_new_thread": is_new_thread,
                "word_count": count_words(content),
                "is_question": is_question,
                "confirmation_like": confirmation_like,
                "revision_signal": format!("{revision_kind:?}").to_ascii_lowercase(),
            }),
        )
        .await?;

        if settings.allow_implicit_feedback && revision_kind.is_revision() {
            let signal_type = if revision_kind.is_correction() {
                "operator_correction"
            } else {
                "high_revision_rate"
            };
            let weight = if revision_kind.is_correction() {
                -0.16
            } else {
                -0.10
            };
            self.persist_implicit_feedback_signal(
                thread_id,
                signal_type,
                weight,
                now,
                serde_json::json!({
                    "thread_id": thread_id,
                    "is_new_thread": is_new_thread,
                    "revision_signal": format!("{revision_kind:?}").to_ascii_lowercase(),
                    "word_count": count_words(content),
                }),
            )
            .await?;

            let model = self.operator_model.read().await;
            self.persist_operator_satisfaction_snapshot(thread_id, now, &model)
                .await?;
        }

        let observed_action = Self::classify_observed_operator_action(content);
        let _ = self
            .history
            .resolve_latest_intent_prediction(thread_id, observed_action)
            .await;

        if let Err(error) = self.analyze_emergent_protocol_for_thread(thread_id).await {
            tracing::debug!(thread_id = %thread_id, error = %error, "emergent protocol analysis failed after operator message");
        }

        Ok(())
    }

    pub(crate) async fn record_operator_approval_requested(
        &self,
        pending_approval: &ToolPendingApproval,
    ) -> Result<()> {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled || !settings.allow_approval_learning {
            return Ok(());
        }
        ensure_operator_model_file(&self.data_dir).await?;
        let category =
            classify_command_category(&pending_approval.command, &pending_approval.risk_level);
        self.pending_operator_approvals.write().await.insert(
            pending_approval.approval_id.clone(),
            PendingApprovalObservation {
                requested_at: now_millis(),
                category: category.to_string(),
            },
        );

        let mut model = self.operator_model.write().await;
        model.last_updated = now_millis();
        model.risk_fingerprint.approval_requests += 1;
        *model
            .risk_fingerprint
            .category_requests
            .entry(category.to_string())
            .or_insert(0) += 1;
        refresh_risk_metrics(&mut model.risk_fingerprint);
        refresh_operator_satisfaction(&mut model);
        persist_operator_model(&self.data_dir, &model)?;
        self.record_behavioral_event(
            "approval_requested",
            BehavioralEventContext {
                thread_id: None,
                task_id: None,
                goal_run_id: None,
                approval_id: Some(&pending_approval.approval_id),
            },
            serde_json::json!({
                "category": category,
                "command": pending_approval.command,
                "risk_level": pending_approval.risk_level,
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn record_tool_hesitation(
        &self,
        from_tool: &str,
        to_tool: &str,
        from_error: bool,
        to_error: bool,
    ) -> Result<()> {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled || !settings.allow_implicit_feedback {
            return Ok(());
        }
        if !from_error || to_error {
            return Ok(());
        }
        let from_tool = from_tool.trim();
        let to_tool = to_tool.trim();
        if from_tool.is_empty() || to_tool.is_empty() || from_tool.eq_ignore_ascii_case(to_tool) {
            return Ok(());
        }

        ensure_operator_model_file(&self.data_dir).await?;
        let now = now_millis();
        let model_snapshot = {
            let mut model = self.operator_model.write().await;
            model.last_updated = now;
            model.implicit_feedback.tool_hesitation_count += 1;
            let pair = format!("{from_tool} -> {to_tool}");
            *model
                .implicit_feedback
                .fallback_histogram
                .entry(pair)
                .or_insert(0) += 1;
            model.implicit_feedback.top_tool_fallbacks =
                top_keys(&model.implicit_feedback.fallback_histogram, 3);
            refresh_operator_satisfaction(&mut model);
            persist_operator_model(&self.data_dir, &model)?;
            model.clone()
        };
        self.record_behavioral_event(
            "tool_fallback",
            BehavioralEventContext {
                thread_id: None,
                task_id: None,
                goal_run_id: None,
                approval_id: None,
            },
            serde_json::json!({
                "from_tool": from_tool,
                "to_tool": to_tool,
                "from_error": from_error,
                "to_error": to_error,
            }),
        )
        .await?;

        self.persist_implicit_feedback_signal(
            "global",
            "tool_fallback",
            -0.12,
            now,
            serde_json::json!({
                "from_tool": from_tool,
                "to_tool": to_tool,
                "from_error": from_error,
                "to_error": to_error,
            }),
        )
        .await?;
        self.persist_operator_satisfaction_snapshot("global", now, &model_snapshot)
            .await?;
        Ok(())
    }

    pub async fn record_attention_surface(&self, surface: &str) -> Result<()> {
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled || !settings.allow_attention_tracking {
            return Ok(());
        }
        let normalized = normalize_attention_surface(surface);
        if normalized.is_empty() {
            return Ok(());
        }

        ensure_operator_model_file(&self.data_dir).await?;
        let now = now_millis();
        let previous_attention = {
            let model = self.operator_model.read().await;
            model
                .attention_topology
                .last_surface
                .clone()
                .zip(model.attention_topology.last_surface_at)
        };
        let model_snapshot = {
            let mut model = self.operator_model.write().await;
            model.last_updated = now;
            record_attention_event(&mut model, &normalized, now);
            refresh_operator_satisfaction(&mut model);
            persist_operator_model(&self.data_dir, &model)?;
            model.clone()
        };
        self.record_behavioral_event(
            "attention_surface",
            BehavioralEventContext {
                thread_id: None,
                task_id: None,
                goal_run_id: None,
                approval_id: None,
            },
            serde_json::json!({
                "surface": normalized,
            }),
        )
        .await?;

        if let Some((previous_surface, previous_at)) = previous_attention {
            let dwell_secs = now.saturating_sub(previous_at) / 1000;
            if previous_surface != normalized && dwell_secs > 0 && dwell_secs <= 15 {
                self.persist_implicit_feedback_signal(
                    "global",
                    "short_dwell",
                    -0.03,
                    now,
                    serde_json::json!({
                        "surface": previous_surface,
                        "next_surface": normalized,
                        "dwell_secs": dwell_secs,
                        "rapid_switch_count": model_snapshot.attention_topology.rapid_switch_count,
                    }),
                )
                .await?;
                self.persist_operator_satisfaction_snapshot("global", now, &model_snapshot)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn record_operator_approval_resolution(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<()> {
        self.forget_pending_approval_command(approval_id).await;
        let settings = self.config.read().await.operator_model.clone();
        if !settings.enabled || !settings.allow_approval_learning {
            return Ok(());
        }
        ensure_operator_model_file(&self.data_dir).await?;
        let pending = self
            .pending_operator_approvals
            .write()
            .await
            .remove(approval_id);
        let now = now_millis();

        let model_snapshot = {
            let mut model = self.operator_model.write().await;
            model.last_updated = now;
            if matches!(
                decision,
                ApprovalDecision::ApproveOnce | ApprovalDecision::ApproveSession
            ) {
                model.risk_fingerprint.approvals += 1;
            } else {
                model.risk_fingerprint.denials += 1;
            }
            if let Some(pending) = pending {
                let category = pending.category.clone();
                if matches!(
                    decision,
                    ApprovalDecision::ApproveOnce | ApprovalDecision::ApproveSession
                ) {
                    *model
                        .risk_fingerprint
                        .category_approvals
                        .entry(category.clone())
                        .or_insert(0) += 1;
                }
                let response_secs = now.saturating_sub(pending.requested_at) as f64 / 1000.0;
                let responses = model.risk_fingerprint.approvals + model.risk_fingerprint.denials;
                model.risk_fingerprint.avg_response_time_secs = update_running_average(
                    model.risk_fingerprint.avg_response_time_secs,
                    responses.saturating_sub(1),
                    response_secs,
                );
                if settings.allow_implicit_feedback
                    && matches!(decision, ApprovalDecision::Deny)
                    && response_secs <= 8.0
                {
                    model.implicit_feedback.fast_denial_count += 1;
                    *model
                        .risk_fingerprint
                        .fast_denials_by_category
                        .entry(category)
                        .or_insert(0) += 1;
                }
            }
            refresh_risk_metrics(&mut model.risk_fingerprint);
            refresh_operator_satisfaction(&mut model);
            persist_operator_model(&self.data_dir, &model)?;
            model.clone()
        };
        self.record_behavioral_event(
            "approval_resolved",
            BehavioralEventContext {
                thread_id: None,
                task_id: None,
                goal_run_id: None,
                approval_id: Some(approval_id),
            },
            serde_json::json!({
                "decision": format!("{decision:?}").to_ascii_lowercase(),
            }),
        )
        .await?;

        if settings.allow_implicit_feedback && matches!(decision, ApprovalDecision::Deny) {
            if model_snapshot.implicit_feedback.fast_denial_count > 0 {
                self.persist_implicit_feedback_signal(
                    "global",
                    "fast_denial",
                    -0.18,
                    now,
                    serde_json::json!({
                        "approval_id": approval_id,
                        "decision": format!("{decision:?}").to_ascii_lowercase(),
                    }),
                )
                .await?;
                self.persist_operator_satisfaction_snapshot("global", now, &model_snapshot)
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn status_polling_diagnostics_snapshot(&self) -> serde_json::Value {
        let sync_state = match super::operator_profile::user_sync::current_user_sync_state() {
            super::operator_profile::user_sync::UserProfileSyncState::Clean => "clean",
            super::operator_profile::user_sync::UserProfileSyncState::Dirty => "dirty",
            super::operator_profile::user_sync::UserProfileSyncState::Reconciling => "reconciling",
        };

        let aline_summary = self.aline_startup_last_summary().await;
        let aline_available = aline_summary
            .as_ref()
            .map(|summary| summary.aline_available)
            .unwrap_or_else(|| self.aline_startup_is_available());
        let watcher_state = aline_summary
            .as_ref()
            .and_then(|summary| {
                if summary.watcher_started {
                    Some("running")
                } else {
                    summary
                        .watcher_initial_state
                        .as_ref()
                        .map(|state| match state {
                            crate::agent::WatcherState::Running => "running",
                            crate::agent::WatcherState::Stopped => "stopped",
                            crate::agent::WatcherState::Unknown => "unknown",
                        })
                }
            })
            .unwrap_or("unknown");
        let skill_mesh_backend = self
            .config
            .read()
            .await
            .skill_recommendation
            .discovery_backend
            .clone();
        let skill_mesh_state = if skill_mesh_backend.eq_ignore_ascii_case("mesh") {
            "fresh"
        } else {
            "legacy"
        };
        let active_skill_gate_state = self
            .thread_skill_discovery_states
            .read()
            .await
            .values()
            .filter(|state| !state.compliant)
            .max_by_key(|state| state.updated_at)
            .cloned();
        let active_skill_gate = if let Some(state) = active_skill_gate_state {
            let capability_family = fallback_skill_gate_family(state.recommended_skill.as_deref());
            serde_json::json!({
                "recommended_skill": state.recommended_skill,
                "recommended_action": state.recommended_action,
                "requires_approval": state.mesh_requires_approval,
                "skill_read_completed": state.skill_read_completed,
                "mesh_next_step": state.mesh_next_step,
                "rationale": cached_skill_gate_rationale(&state),
                "capability_family": capability_family,
            })
            .into()
        } else {
            None
        };

        serde_json::json!({
            "operator_profile_sync_state": sync_state,
            "operator_profile_sync_dirty": sync_state != "clean",
            "operator_profile_scheduler_fallback": false,
            "aline": {
                "available": aline_available,
                "watcher_state": watcher_state,
                "watcher_started": aline_summary.as_ref().map(|summary| summary.watcher_started).unwrap_or(false),
                "discovered_count": aline_summary.as_ref().map(|summary| summary.discovered_count).unwrap_or(0),
                "selected_count": aline_summary.as_ref().map(|summary| summary.selected_count).unwrap_or(0),
                "imported_count": aline_summary.as_ref().map(|summary| summary.imported_count).unwrap_or(0),
                "generated_count": aline_summary.as_ref().map(|summary| summary.generated_count).unwrap_or(0),
                "skipped_recently_imported_count": aline_summary.as_ref().map(|summary| summary.skipped_recently_imported_count).unwrap_or(0),
                "budget_exhausted": aline_summary.as_ref().map(|summary| summary.budget_exhausted).unwrap_or(false),
                "failure_stage": aline_summary.as_ref().and_then(|summary| summary.failure_stage.clone()),
                "failure_message": aline_summary.as_ref().and_then(|summary| summary.failure_message.clone()),
                "short_circuit_reason": aline_summary
                    .as_ref()
                    .and_then(|summary| summary.short_circuit_reason.map(|reason| reason.as_str())),
            },
            "skill_mesh": {
                "backend": skill_mesh_backend,
                "state": skill_mesh_state,
                "active_gate": active_skill_gate,
            },
        })
    }

    #[cfg(test)]
    pub(crate) async fn status_diagnostics_snapshot(&self) -> serde_json::Value {
        let sync_state = match super::operator_profile::user_sync::current_user_sync_state() {
            super::operator_profile::user_sync::UserProfileSyncState::Clean => "clean",
            super::operator_profile::user_sync::UserProfileSyncState::Dirty => "dirty",
            super::operator_profile::user_sync::UserProfileSyncState::Reconciling => "reconciling",
        };

        let aline_summary = self.aline_startup_last_summary().await;
        let aline_available = aline_summary
            .as_ref()
            .map(|summary| summary.aline_available)
            .unwrap_or_else(|| self.aline_startup_is_available());
        let watcher_state = aline_summary
            .as_ref()
            .and_then(|summary| {
                if summary.watcher_started {
                    Some("running")
                } else {
                    summary
                        .watcher_initial_state
                        .as_ref()
                        .map(|state| match state {
                            crate::agent::WatcherState::Running => "running",
                            crate::agent::WatcherState::Stopped => "stopped",
                            crate::agent::WatcherState::Unknown => "unknown",
                        })
                }
            })
            .unwrap_or("unknown");
        let skill_mesh_backend = self
            .config
            .read()
            .await
            .skill_recommendation
            .discovery_backend
            .clone();
        let skill_mesh_state = if skill_mesh_backend.eq_ignore_ascii_case("mesh") {
            "fresh"
        } else {
            "legacy"
        };
        let active_skill_gate_state = self
            .thread_skill_discovery_states
            .read()
            .await
            .values()
            .filter(|state| !state.compliant)
            .max_by_key(|state| state.updated_at)
            .cloned();
        let active_skill_gate = if let Some(state) = active_skill_gate_state {
            let capability_family = fallback_skill_gate_family(state.recommended_skill.as_deref());
            serde_json::json!({
                "recommended_skill": state.recommended_skill,
                "recommended_action": state.recommended_action,
                "requires_approval": state.mesh_requires_approval,
                "skill_read_completed": state.skill_read_completed,
                "mesh_next_step": state.mesh_next_step,
                "rationale": cached_skill_gate_rationale(&state),
                "capability_family": capability_family,
            })
            .into()
        } else {
            None
        };
        let operator_model = self.operator_model.read().await.clone();
        let recent_implicit_signals = self
            .history
            .list_implicit_signals("global", 5)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id,
                    "session_id": row.session_id,
                    "signal_type": row.signal_type,
                    "weight": row.weight,
                    "timestamp_ms": row.timestamp_ms,
                    "context_snapshot_json": row.context_snapshot_json,
                })
            })
            .collect::<Vec<_>>();
        let recent_satisfaction_scores = self
            .history
            .list_satisfaction_scores("global", 5)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id,
                    "session_id": row.session_id,
                    "score": row.score,
                    "computed_at_ms": row.computed_at_ms,
                    "label": row.label,
                    "signal_count": row.signal_count,
                })
            })
            .collect::<Vec<_>>();
        let recent_memory_distillation = self
            .history
            .list_memory_distillation_log(5)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id,
                    "source_thread_id": row.source_thread_id,
                    "source_message_range": row.source_message_range,
                    "distilled_fact": row.distilled_fact,
                    "target_file": row.target_file,
                    "category": row.category,
                    "confidence": row.confidence,
                    "created_at_ms": row.created_at_ms,
                    "applied_to_memory": row.applied_to_memory,
                    "agent_id": row.agent_id,
                })
            })
            .collect::<Vec<_>>();
        let recent_forge_passes = self
            .history
            .list_forge_pass_log(5)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id,
                    "agent_id": row.agent_id,
                    "period_start_ms": row.period_start_ms,
                    "period_end_ms": row.period_end_ms,
                    "traces_analyzed": row.traces_analyzed,
                    "patterns_found": row.patterns_found,
                    "hints_applied": row.hints_applied,
                    "hints_logged": row.hints_logged,
                    "completed_at_ms": row.completed_at_ms,
                })
            })
            .collect::<Vec<_>>();
        let emergent_protocols = {
            let mut pending_proposals = Vec::new();
            for row in self
                .history
                .list_thread_protocol_candidates()
                .await
                .unwrap_or_default()
            {
                let Ok(store) = serde_json::from_value::<
                    crate::agent::emergent_protocol::types::ProtocolCandidateStore,
                >(row.state_json) else {
                    continue;
                };
                pending_proposals.extend(
                    store
                        .candidates
                        .into_iter()
                        .filter(|candidate| {
                            candidate.state
                                == crate::agent::emergent_protocol::types::ProtocolCandidateState::Proposed
                        })
                        .map(|candidate| {
                            serde_json::json!({
                                "thread_id": candidate.thread_id,
                                "candidate_id": candidate.id,
                                "signal_kind": candidate.kind.as_str(),
                                "trigger_phrase": candidate.trigger_phrase,
                                "normalized_pattern": candidate.normalized_pattern,
                                "confidence": candidate.confidence,
                                "observation_count": candidate.observation_count,
                                "last_seen_at_ms": candidate.last_seen_at_ms,
                            })
                        }),
                );
            }
            let mut accepted_protocols = self
                .history
                .list_emergent_protocols()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|row| {
                    let signal_kind =
                        crate::agent::emergent_protocol::types::ProtocolSignalKind::from_str(
                            &row.signal_kind,
                        )
                        .map(|kind| kind.as_str().to_string())
                        .unwrap_or_else(|| row.signal_kind.clone());
                    serde_json::json!({
                        "thread_id": row.thread_id,
                        "protocol_id": row.protocol_id,
                        "token": row.token,
                        "signal_kind": signal_kind,
                        "normalized_pattern": row.normalized_pattern,
                        "usage_count": row.usage_count,
                        "success_rate": row.success_rate,
                        "last_used_ms": row.last_used_at,
                        "source_candidate_id": row.source_candidate_id,
                    })
                })
                .collect::<Vec<_>>();

            pending_proposals.sort_by(|left, right| {
                right
                    .get("last_seen_at_ms")
                    .and_then(|value| value.as_u64())
                    .cmp(&left.get("last_seen_at_ms").and_then(|value| value.as_u64()))
            });
            accepted_protocols.sort_by(|left, right| {
                right
                    .get("last_used_ms")
                    .and_then(|value| value.as_u64())
                    .cmp(&left.get("last_used_ms").and_then(|value| value.as_u64()))
                    .then(
                        right
                            .get("usage_count")
                            .and_then(|value| value.as_u64())
                            .cmp(&left.get("usage_count").and_then(|value| value.as_u64())),
                    )
            });

            if pending_proposals.is_empty() && accepted_protocols.is_empty() {
                None
            } else {
                Some(serde_json::json!({
                    "proposal_count": pending_proposals.len(),
                    "proposals": pending_proposals,
                    "protocol_count": accepted_protocols.len(),
                    "protocols": accepted_protocols,
                }))
            }
        };
        let routing_decision = self
            .history
            .list_recent_handoff_routing(1)
            .await
            .unwrap_or_default()
            .into_iter()
            .next()
            .map(|row| {
                let capability_tags = row
                    .capability_tags_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
                    .unwrap_or_default();
                serde_json::json!({
                    "handoff_log_id": row.id,
                    "specialist_id": row.to_specialist_id,
                    "capability_tags": capability_tags,
                    "routing_method": row.routing_method,
                    "routing_score": row.routing_score,
                    "fallback_used": row.fallback_used,
                    "created_at": row.created_at,
                })
            });
        let debate_session = self
            .history
            .list_debate_sessions(1)
            .await
            .unwrap_or_default()
            .into_iter()
            .next()
            .and_then(|row| {
                serde_json::from_str::<crate::agent::debate::types::DebateSession>(
                    &row.session_json,
                )
                .ok()
                .map(|session| {
                    serde_json::json!({
                        "session_id": session.id,
                        "topic": session.topic,
                        "status": session.status,
                        "current_round": session.current_round,
                        "max_rounds": session.max_rounds,
                        "completion_reason": session.completion_reason,
                        "thread_id": session.thread_id,
                        "goal_run_id": session.goal_run_id,
                        "has_verdict": session.verdict.is_some(),
                        "updated_at": row.updated_at,
                    })
                })
            });
        let subagent_task_query = crate::history::AgentTaskListQuery {
            id: None,
            status: None,
            statuses: Vec::new(),
            source: Some("subagent".to_string()),
            thread_id: None,
            thread_ids: Vec::new(),
            goal_run_id: None,
            parent_task_id: None,
            awaiting_approval_id: None,
            supervisor_config_present: false,
            exclude_terminal_statuses: false,
            order_by_recent_activity_desc: false,
            limit: None,
            ids: Vec::new(),
            parent_task_ids: Vec::new(),
        };
        let mut subagent_tasks = match self
            .history
            .list_agent_task_subagent_hierarchy_refs_filtered(&subagent_task_query)
            .await
        {
            Ok(task_refs) => task_refs,
            Err(error) => {
                tracing::warn!(
                    "failed to query subagent task hierarchy refs for operator model diagnostics: {error}"
                );
                self.list_tasks_filtered(&subagent_task_query)
                    .await
                    .iter()
                    .map(crate::history::AgentTaskSubagentHierarchyRef::from)
                    .collect()
            }
        };
        let mut live_overrides: std::collections::HashMap<
            String,
            crate::history::AgentTaskSubagentHierarchyRef,
        > = std::collections::HashMap::new();
        let mut live_only: Vec<crate::history::AgentTaskSubagentHierarchyRef> = Vec::new();
        let persisted_ids = subagent_tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<std::collections::HashSet<_>>();
        for task in self
            .tasks
            .lock()
            .await
            .iter()
            .filter(|task| task.source == "subagent")
        {
            let live_ref = crate::history::AgentTaskSubagentHierarchyRef::from(task);
            if persisted_ids.contains(&task.id) {
                live_overrides.insert(task.id.clone(), live_ref);
            } else {
                live_only.push(live_ref);
            }
        }
        for task in subagent_tasks.iter_mut() {
            if let Some(live) = live_overrides.remove(&task.id) {
                *task = live;
            }
        }
        subagent_tasks.extend(live_only);
        let parse_subagent_containment_scope = |scope: Option<&str>| -> Option<(u8, u8)> {
            let scope = scope?.trim();
            let payload = scope.strip_prefix("subagent-depth:")?;
            let (depth, max_depth) = payload.split_once('/')?;
            let depth = depth.trim().parse::<u8>().ok()?;
            let max_depth = max_depth.trim().parse::<u8>().ok()?;
            Some((depth, max_depth))
        };
        let compute_task_delegation_depth =
            |task: &crate::history::AgentTaskSubagentHierarchyRef| -> u8 {
                let mut depth = 0u8;
                let mut current_parent_id = task.parent_task_id.as_deref();
                while let Some(parent_id) = current_parent_id {
                    depth = depth.saturating_add(1);
                    current_parent_id = subagent_tasks
                        .iter()
                        .find(|candidate| candidate.id == parent_id)
                        .and_then(|parent| parent.parent_task_id.as_deref());
                }
                depth
            };
        let effective_subagent_max_depth =
            |task: &crate::history::AgentTaskSubagentHierarchyRef| -> u8 {
                parse_subagent_containment_scope(task.containment_scope.as_deref())
                    .map(|(_, max_depth)| max_depth)
                    .unwrap_or_else(|| compute_task_delegation_depth(task).max(1))
            };
        let max_observed_depth = subagent_tasks
            .iter()
            .map(|task| compute_task_delegation_depth(task))
            .max()
            .unwrap_or(0);
        let max_observed_allowed_depth = subagent_tasks
            .iter()
            .map(|task| {
                parse_subagent_containment_scope(task.containment_scope.as_deref())
                    .map(|(_, max_depth)| max_depth)
                    .unwrap_or_else(|| effective_subagent_max_depth(task))
            })
            .max()
            .unwrap_or(0);
        let root_parent_task_ids = subagent_tasks
            .iter()
            .filter_map(|task| {
                let mut current_parent_id = task.parent_task_id.as_deref()?;
                loop {
                    let next_parent = subagent_tasks
                        .iter()
                        .find(|candidate| candidate.id == current_parent_id)
                        .and_then(|parent| parent.parent_task_id.as_deref());
                    match next_parent {
                        Some(next_parent_id) => current_parent_id = next_parent_id,
                        None => return Some(current_parent_id.to_string()),
                    }
                }
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let recursive_subagents = serde_json::json!({
            "active_subagent_count": subagent_tasks.len(),
            "max_observed_depth": max_observed_depth,
            "max_observed_allowed_depth": max_observed_allowed_depth,
            "root_parent_task_ids": root_parent_task_ids,
        });
        let mut memory_distillation_progress = self
            .history
            .list_memory_distillation_progress(5)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "source_thread_id": row.source_thread_id,
                    "last_processed_created_at_ms": row.last_processed_cursor.created_at,
                    "last_processed_message_id": row.last_processed_cursor.message_id,
                    "last_run_at_ms": row.last_run_at_ms,
                    "updated_at_ms": row.updated_at_ms,
                    "agent_id": row.agent_id,
                })
            })
            .collect::<Vec<_>>();
        memory_distillation_progress.sort_by(|left, right| {
            right
                .get("updated_at_ms")
                .and_then(|value| value.as_i64())
                .cmp(&left.get("updated_at_ms").and_then(|value| value.as_i64()))
                .then(
                    left.get("source_thread_id")
                        .and_then(|value| value.as_str())
                        .cmp(
                            &right
                                .get("source_thread_id")
                                .and_then(|value| value.as_str()),
                        ),
                )
        });
        memory_distillation_progress.truncate(5);
        let cognitive_resonance = CognitiveResonanceSnapshot::from_model(&operator_model);
        let meta_cognitive_self_model = self.meta_cognitive_self_model.read().await.clone();
        let anticipatory_runtime = self.anticipatory.read().await;
        let diagnostics_now = now_millis();
        let anticipatory_items = anticipatory_runtime.items.clone();
        let intent_prediction = anticipatory_items
            .iter()
            .filter_map(|item| {
                item.intent_prediction.as_ref().map(|prediction| {
                    let cached_prewarm_summary = item.thread_id.as_deref().and_then(|thread_id| {
                        anticipatory_runtime
                            .prewarm_cache_by_thread
                            .get(thread_id)
                            .map(|snapshot| snapshot.summary.clone())
                    });
                    serde_json::json!({
                        "thread_id": item.thread_id,
                        "primary_action": prediction.primary_action,
                        "confidence": prediction.confidence,
                        "cached_prewarm_summary": cached_prewarm_summary,
                        "ranked_actions": prediction.ranked_actions,
                    })
                })
            })
            .max_by(|left, right| {
                left.get("confidence")
                    .and_then(|value| value.as_f64())
                    .partial_cmp(&right.get("confidence").and_then(|value| value.as_f64()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let system_outcome_foresight = anticipatory_items
            .iter()
            .filter(|item| item.kind == "system_outcome_foresight")
            .map(|item| {
                let prediction_type = item
                    .bullets
                    .iter()
                    .find_map(|bullet| bullet.strip_prefix("prediction_type="))
                    .unwrap_or("unknown")
                    .to_string();
                let predicted_outcome = if prediction_type == "stale_context" {
                    "hydration-needed risk"
                } else {
                    "build/test failure"
                };
                serde_json::json!({
                    "thread_id": item.thread_id,
                    "prediction_type": prediction_type,
                    "predicted_outcome": predicted_outcome,
                    "confidence": item.confidence,
                    "summary": item.summary,
                    "bullets": item.bullets,
                })
            })
            .max_by(|left, right| {
                left.get("confidence")
                    .and_then(|value| value.as_f64())
                    .partial_cmp(&right.get("confidence").and_then(|value| value.as_f64()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let proactive_suppression = anticipatory_items
            .iter()
            .filter(|item| item.kind == "proactive_suppression")
            .map(|item| {
                serde_json::json!({
                    "thread_id": item.thread_id,
                    "confidence": item.confidence,
                    "summary": item.summary,
                    "bullets": item.bullets,
                })
            })
            .max_by(|left, right| {
                left.get("confidence")
                    .and_then(|value| value.as_f64())
                    .partial_cmp(&right.get("confidence").and_then(|value| value.as_f64()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let speculative_queue = anticipatory_runtime
            .opportunity_queue
            .iter()
            .map(|opportunity| {
                let status = match opportunity.status {
                    SpeculativeOpportunityStatus::Queued => "queued",
                    SpeculativeOpportunityStatus::Running => "running",
                    SpeculativeOpportunityStatus::Completed => "completed",
                    SpeculativeOpportunityStatus::Expired => "expired",
                    SpeculativeOpportunityStatus::Consumed => "consumed",
                    SpeculativeOpportunityStatus::Dropped => "dropped",
                };
                serde_json::json!({
                    "id": opportunity.id,
                    "thread_id": opportunity.thread_id,
                    "source_kind": opportunity.source_kind,
                    "action_kind": opportunity.action_kind,
                    "confidence": opportunity.confidence,
                    "summary": opportunity.summary,
                    "created_at_ms": opportunity.created_at_ms,
                    "expires_at_ms": opportunity.expires_at_ms,
                    "expires_in_ms": opportunity.expires_at_ms.saturating_sub(diagnostics_now),
                    "status": status,
                })
            })
            .collect::<Vec<_>>();
        let mut speculative_results = anticipatory_runtime
            .speculative_results_by_thread
            .iter()
            .flat_map(|(thread_id, results)| {
                let thread_id = thread_id.clone();
                results.iter().map(move |result| {
                    serde_json::json!({
                        "thread_id": thread_id,
                        "opportunity_id": result.opportunity_id,
                        "action_kind": result.action_kind,
                        "summary": result.summary,
                        "artifact": result.artifact,
                        "completed_at_ms": result.completed_at_ms,
                        "expires_at_ms": result.expires_at_ms,
                        "expires_in_ms": result.expires_at_ms.saturating_sub(diagnostics_now),
                        "is_expired": result.expires_at_ms <= diagnostics_now,
                        "used": result.used_at_ms.is_some(),
                        "used_at_ms": result.used_at_ms,
                        "precomputation_id": result.precomputation_id,
                    })
                })
            })
            .collect::<Vec<_>>();
        speculative_results.sort_by(|left, right| {
            right
                .get("completed_at_ms")
                .and_then(|value| value.as_u64())
                .cmp(&left.get("completed_at_ms").and_then(|value| value.as_u64()))
                .then(
                    left.get("thread_id")
                        .and_then(|value| value.as_str())
                        .cmp(&right.get("thread_id").and_then(|value| value.as_str())),
                )
        });
        let proactive_provenance = self
            .history
            .provenance_report(32)
            .map(|report| crate::agent::provenance::proactive_provenance_summary(&report, 5))
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "failed to build proactive provenance diagnostics summary");
                serde_json::json!({
                    "prepared_count": 0,
                    "used_count": 0,
                    "prepared_cache_count": 0,
                    "used_cache_count": 0,
                    "prepared_speculative_count": 0,
                    "used_speculative_count": 0,
                    "recent_event_count": 0,
                    "recent_events": [],
                })
            });
        let adaptive_carryover = self
            .history
            .provenance_report(32)
            .map(|report| {
                crate::agent::provenance::adaptive_carryover_provenance_summary(&report, 5)
            })
            .ok()
            .filter(|summary| {
                !crate::agent::provenance::adaptive_carryover_is_effectively_empty(summary)
            });

        serde_json::json!({
            "operator_profile_sync_state": sync_state,
            "operator_profile_sync_dirty": sync_state != "clean",
            "operator_profile_scheduler_fallback": false,
            "intent_prediction": intent_prediction,
            "routing_decision": routing_decision,
            "debate_session": debate_session,
            "recursive_subagents": recursive_subagents,
            "proactive_suppression": proactive_suppression,
            "system_outcome_foresight": system_outcome_foresight,
            "proactive_provenance": proactive_provenance,
            "adaptive_carryover": adaptive_carryover,
            "speculative_execution": {
                "queue_depth": speculative_queue.len(),
                "queued_opportunities": speculative_queue,
                "cached_result_count": speculative_results.len(),
                "cached_results": speculative_results,
            },
            "operator_satisfaction": {
                "label": operator_model.operator_satisfaction.label,
                "score": operator_model.operator_satisfaction.score,
                "summary": operator_model.diagnostic_summary(),
                "message_count": operator_model.cognitive_style.message_count,
                "approval_requests": operator_model.risk_fingerprint.approval_requests,
                "focus_event_count": operator_model.attention_topology.focus_event_count,
                "tool_hesitation_count": operator_model.implicit_feedback.tool_hesitation_count,
                "revision_message_count": operator_model.implicit_feedback.revision_message_count,
                "correction_message_count": operator_model.implicit_feedback.correction_message_count,
                "fast_denial_count": operator_model.implicit_feedback.fast_denial_count,
                "rapid_revert_count": operator_model.implicit_feedback.rapid_revert_count,
                "session_abandon_count": operator_model.implicit_feedback.session_abandon_count,
                "rapid_switch_count": operator_model.attention_topology.rapid_switch_count,
                "recent_implicit_signals": recent_implicit_signals,
                "recent_satisfaction_scores": recent_satisfaction_scores,
            },
            "memory_distillation": {
                "recent_activity": recent_memory_distillation,
                "progress_by_thread": memory_distillation_progress,
            },
            "forge_reflection": {
                "recent_passes": recent_forge_passes,
            },
            "emergent_protocols": emergent_protocols,
            "meta_cognitive_self_model": meta_cognitive_self_model,
            "cognitive_resonance": cognitive_resonance,
            "aline": {
                "available": aline_available,
                "watcher_state": watcher_state,
                "watcher_started": aline_summary.as_ref().map(|summary| summary.watcher_started).unwrap_or(false),
                "discovered_count": aline_summary.as_ref().map(|summary| summary.discovered_count).unwrap_or(0),
                "selected_count": aline_summary.as_ref().map(|summary| summary.selected_count).unwrap_or(0),
                "imported_count": aline_summary.as_ref().map(|summary| summary.imported_count).unwrap_or(0),
                "generated_count": aline_summary.as_ref().map(|summary| summary.generated_count).unwrap_or(0),
                "skipped_recently_imported_count": aline_summary.as_ref().map(|summary| summary.skipped_recently_imported_count).unwrap_or(0),
                "budget_exhausted": aline_summary.as_ref().map(|summary| summary.budget_exhausted).unwrap_or(false),
                "failure_stage": aline_summary.as_ref().and_then(|summary| summary.failure_stage.clone()),
                "failure_message": aline_summary.as_ref().and_then(|summary| summary.failure_message.clone()),
                "short_circuit_reason": aline_summary
                    .as_ref()
                    .and_then(|summary| summary.short_circuit_reason.map(|reason| reason.as_str())),
            },
            "skill_mesh": {
                "backend": skill_mesh_backend,
                "state": skill_mesh_state,
                "active_gate": active_skill_gate,
            },
        })
    }
}

fn cached_skill_gate_rationale(
    state: &crate::agent::types::LatestSkillDiscoveryState,
) -> Vec<String> {
    if let Some(rationale) = state
        .skip_rationale
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return vec![rationale.to_string()];
    }

    let query = state.query.trim();
    if query.is_empty() {
        Vec::new()
    } else {
        vec![format!("matched {query}")]
    }
}

fn fallback_skill_gate_family(recommended_skill: Option<&str>) -> Vec<String> {
    let Some(skill) = recommended_skill.map(|value| value.to_ascii_lowercase()) else {
        return vec!["development".to_string()];
    };
    if skill.contains("debug") {
        vec!["development".to_string(), "debugging".to_string()]
    } else if skill.contains("plan") {
        vec!["planning".to_string()]
    } else {
        vec!["development".to_string()]
    }
}

fn operator_adaptation_lines(model: &OperatorModel) -> Vec<String> {
    let mut lines = Vec::new();
    let adaptation = BehaviorAdaptationProfile::from_model(model);

    let response_mode = match model.operator_satisfaction.label.as_str() {
        "strained" => {
            "- Adaptive response mode: reduce friction aggressively: lead with the answer, keep reasoning minimal, prefer high-confidence actions, avoid repeated retries, and explain tool switches after corrections or fallbacks.".to_string()
        }
        "fragile" => {
            "- Adaptive response mode: tighten the loop: lead with the answer, keep reasoning compact, prefer proven tool paths, and acknowledge adjustments quickly when feedback appears.".to_string()
        }
        "healthy" => {
            "- Adaptive response mode: keep a normal proactive cadence: front-load the answer, keep execution deliberate, and make plan changes explicit when they help reduce friction.".to_string()
        }
        "strong" => {
            "- Adaptive response mode: trust is high, so stay proactive and exploratory when it materially helps, but keep execution disciplined and front-load the answer.".to_string()
        }
        _ => {
            "- Adaptive response mode: keep execution legible, adapt quickly to operator feedback, and avoid unnecessary retries or speculative branches.".to_string()
        }
    };
    lines.push(response_mode);

    let delivery_mode = if model.cognitive_style.prefers_summaries
        || model.cognitive_style.skips_reasoning
    {
        "- Adaptive delivery rule: default to summary-first and keep reasoning on demand unless the operator explicitly asks for detail.".to_string()
    } else if matches!(model.cognitive_style.reading_depth, ReadingDepth::Deep)
        && !adaptation.compact_response
    {
        "- Adaptive delivery rule: include fuller reasoning and step-by-step traces when they materially improve confidence or debugging speed.".to_string()
    } else if adaptation.compact_response {
        "- Adaptive delivery rule: keep the answer compact, front-load the conclusion, and add only the detail needed for the next action.".to_string()
    } else {
        "- Adaptive delivery rule: start with the conclusion, then add only the detail needed to support the next action.".to_string()
    };
    lines.push(delivery_mode);

    if adaptation.prompt_for_clarification {
        lines.push(
            "- Adaptive clarification rule: when intent is underspecified, ask one targeted question before guessing broadly.".to_string(),
        );
    }

    if model.risk_fingerprint.approval_requests > 0 {
        let avg_response_time_secs = model.risk_fingerprint.avg_response_time_secs;
        match model.risk_fingerprint.risk_tolerance {
            RiskTolerance::Aggressive if avg_response_time_secs <= 8.0 => lines.push(
                "- Adaptive approval rule: approvals resolve quickly and usually favor proceeding, so stay proactive within hard safety limits and avoid redundant confirmation loops for low-risk progress.".to_string(),
            ),
            RiskTolerance::Conservative => lines.push(
                "- Adaptive approval rule: approval behavior is conservative, so ask explicitly before ambiguous or risky actions, front-load blast radius, and avoid stacking multiple pending approvals.".to_string(),
            ),
            _ if avg_response_time_secs >= 30.0 => lines.push(
                "- Adaptive approval rule: approval responses are deliberate, so package rationale and blast radius up front when approval is needed and keep only one pending approval live at a time.".to_string(),
            ),
            _ => {}
        }
    }

    if model.implicit_feedback.tool_hesitation_count > 0 {
        lines.push(
            "- Adaptive execution rule: after a failed tool path, prefer the later successful fallback earlier and justify the switch explicitly instead of repeating the same sequence.".to_string(),
        );
    } else if model.attention_topology.rapid_switch_count >= 3 {
        lines.push(
            "- Adaptive execution rule: attention churn is elevated, so use tighter updates, bounded reads, and fewer concurrent branches.".to_string(),
        );
    }

    lines
}
