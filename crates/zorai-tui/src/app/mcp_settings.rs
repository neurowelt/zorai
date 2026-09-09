use super::*;
use zorai_protocol::{ClientMessage, DaemonMessage};

impl TuiModel {
    pub(crate) fn refresh_mcp_settings(&mut self) {
        self.settings.mcp.last_refresh = Some(Instant::now());
        self.send_daemon_command(DaemonCommand::Mcp(ClientMessage::McpListServers));
    }

    pub(crate) fn handle_mcp_settings_event(&mut self, message: DaemonMessage) {
        match message {
            DaemonMessage::McpServers { servers } => {
                self.settings.mcp.servers = servers;
                self.settings.mcp.selected = self
                    .settings
                    .mcp
                    .selected
                    .min(self.settings.mcp.servers.len());
            }
            DaemonMessage::McpOperationResult {
                request_id,
                revision,
                success,
                message,
                server,
            } => {
                if self
                    .settings
                    .mcp
                    .complete(request_id, revision, success, message, server)
                {
                    self.refresh_mcp_settings();
                }
            }
            _ => {}
        }
    }

    pub(crate) fn handle_mcp_settings_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        if matches!(code, KeyCode::Char(_))
            && modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return true;
        }
        let state = &mut self.settings.mcp;
        if state.editing {
            match code {
                KeyCode::Enter => state.commit_edit(),
                KeyCode::Esc => {
                    state.editing = false;
                    state.edit_buffer.clear();
                }
                KeyCode::Backspace => {
                    state.edit_buffer.pop();
                    state.changed();
                }
                KeyCode::Char(c) if !c.is_control() => {
                    state.edit_buffer.push(c);
                    state.changed();
                }
                _ => {}
            }
            return true;
        }
        if matches!(code, KeyCode::Tab | KeyCode::BackTab) {
            return false;
        }
        if matches!(code, KeyCode::PageDown | KeyCode::PageUp) {
            let scroll = if code == KeyCode::PageDown {
                self.settings_modal_scroll.saturating_add(8)
            } else {
                self.settings_modal_scroll.saturating_sub(8)
            };
            self.set_settings_modal_scroll(scroll);
            return true;
        }
        if state.draft.is_none() {
            match code {
                KeyCode::Esc => return false,
                KeyCode::Down => state.selected = (state.selected + 1).min(state.servers.len()),
                KeyCode::Up => state.selected = state.selected.saturating_sub(1),
                KeyCode::Enter => state.open(state.servers.get(state.selected).cloned()),
                KeyCode::Char('a') => state.open(None),
                KeyCode::Char('l') => self.refresh_mcp_settings(),
                KeyCode::Char('e') => {
                    if let Some(server) = state.servers.get(state.selected) {
                        let request = ClientMessage::McpSetServerEnabled {
                            id: server.config.id.clone(),
                            enabled: !server.config.enabled,
                        };
                        self.send_daemon_command(DaemonCommand::Mcp(request));
                    }
                }
                KeyCode::Char('r') => {
                    if let Some(server) = state.servers.get(state.selected) {
                        let request = ClientMessage::McpReconnectServer {
                            id: server.config.id.clone(),
                        };
                        self.send_daemon_command(DaemonCommand::Mcp(request));
                    }
                }
                _ => {}
            }
        } else {
            match code {
                KeyCode::Esc => state.close_draft(),
                KeyCode::Down => state.cursor = (state.cursor + 1).min(state.last_cursor()),
                KeyCode::Up => state.cursor = state.cursor.saturating_sub(1),
                KeyCode::Enter | KeyCode::Char(' ') if state.toggle_selected_tool_description() => {
                }
                KeyCode::Enter | KeyCode::Char(' ') => match state.cursor {
                    0 | 1 | 3 | 4 => state.begin_edit(),
                    2 | 5 | 6 | 7 | 8 => state.toggle(),
                    9 | 10 => {
                        if let Some(request) = state.request(state.cursor == 10) {
                            self.send_daemon_command(DaemonCommand::Mcp(request));
                        }
                    }
                    11 => {
                        if let Some(draft) = &state.draft {
                            if state
                                .servers
                                .iter()
                                .any(|server| server.config.id == draft.config.id)
                            {
                                let request = ClientMessage::McpReconnectServer {
                                    id: draft.config.id.clone(),
                                };
                                self.send_daemon_command(DaemonCommand::Mcp(request));
                            } else {
                                state.result = Some((false, "Save this server before reconnecting. Use Test Connection to test an unsaved draft.".into()));
                            }
                        }
                    }
                    12 => state.close_draft(),
                    _ => {}
                },
                _ => {}
            }
        }
        self.sync_settings_modal_scroll_to_selection();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zorai_protocol::{McpCredentialUpdate, McpServerStatus};

    #[test]
    fn mcp_editor_add_test_save_disable_reconnect_uses_dedicated_commands() {
        let (_event_tx, event_rx) = std::sync::mpsc::channel();
        let (daemon_tx, mut daemon_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = TuiModel::new(event_rx, daemon_tx);
        model.open_settings_tab(SettingsTab::Mcp);
        let commands: Vec<_> = std::iter::from_fn(|| daemon_rx.try_recv().ok()).collect();
        assert!(commands
            .iter()
            .any(|command| matches!(command, DaemonCommand::Mcp(ClientMessage::McpListServers))));
        model.handle_mcp_settings_key(KeyCode::Char('a'), KeyModifiers::NONE);
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        model.handle_paste("Local mock".into());
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        model.settings.mcp.cursor = 1;
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        model.handle_paste("http://127.0.0.1:12345/mcp".into());
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        model.settings.mcp.cursor = 9;
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        let DaemonCommand::Mcp(ClientMessage::McpTestServer {
            request_id,
            revision,
            config,
            credential,
        }) = daemon_rx.try_recv().unwrap()
        else {
            panic!("isolated test");
        };
        assert_eq!(config.name, "Local mock");
        assert!(!config.share_workspace_context);
        assert_eq!(credential, McpCredentialUpdate::Clear);
        let status = McpServerStatus {
            config,
            workspace_context_supported: true,
            ..Default::default()
        };
        model.handle_mcp_settings_event(DaemonMessage::McpOperationResult {
            request_id,
            revision,
            success: true,
            message: "Test succeeded".into(),
            server: Some(status.clone()),
        });
        assert!(model.settings.mcp.servers.is_empty());
        model.settings.mcp.cursor = 7;
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        model.settings.mcp.cursor = 10;
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        let DaemonCommand::Mcp(ClientMessage::McpSaveServer {
            request_id,
            revision,
            config,
            ..
        }) = daemon_rx.try_recv().unwrap()
        else {
            panic!("save");
        };
        assert!(config.share_workspace_context);
        model.handle_mcp_settings_event(DaemonMessage::McpOperationResult {
            request_id,
            revision,
            success: true,
            message: "Saved".into(),
            server: Some(McpServerStatus {
                config: config.clone(),
                ..status
            }),
        });
        assert!(matches!(
            daemon_rx.try_recv(),
            Ok(DaemonCommand::Mcp(ClientMessage::McpListServers))
        ));
        model.handle_mcp_settings_event(DaemonMessage::McpServers {
            servers: vec![McpServerStatus {
                config: config.clone(),
                ..Default::default()
            }],
        });
        model.handle_mcp_settings_key(KeyCode::Esc, KeyModifiers::NONE);
        model.handle_mcp_settings_key(KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(
            matches!(daemon_rx.try_recv(), Ok(DaemonCommand::Mcp(ClientMessage::McpSetServerEnabled { id, enabled: false })) if id == config.id)
        );
        model.handle_mcp_settings_key(KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(
            matches!(daemon_rx.try_recv(), Ok(DaemonCommand::Mcp(ClientMessage::McpReconnectServer { id })) if id == config.id)
        );
    }
    #[test]
    fn mcp_closing_settings_discards_credential_and_ignores_pending_test() {
        let (_event_tx, event_rx) = std::sync::mpsc::channel();
        let (daemon_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let mut model = TuiModel::new(event_rx, daemon_tx);
        model.open_settings_tab(SettingsTab::Mcp);
        model.settings.mcp.open(None);
        model.settings.mcp.cursor = 2;
        model.settings.mcp.toggle();
        model.settings.mcp.cursor = 4;
        model.settings.mcp.begin_edit();
        model.settings.mcp.edit_buffer = "secret".into();
        model.settings.mcp.commit_edit();
        let ClientMessage::McpTestServer {
            request_id,
            revision,
            ..
        } = model.settings.mcp.request(false).unwrap()
        else {
            panic!("test");
        };
        model.close_top_modal();
        assert!(model.settings.mcp.draft.is_none());
        model.handle_mcp_settings_event(DaemonMessage::McpOperationResult {
            request_id,
            revision,
            success: true,
            message: "stale".into(),
            server: Some(McpServerStatus::default()),
        });
        assert!(model.settings.mcp.result.is_none());
    }
    #[test]
    fn mcp_reconnect_requires_saved_server_and_reports_unsaved_draft_inline() {
        let (_event_tx, event_rx) = std::sync::mpsc::channel();
        let (daemon_tx, mut daemon_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = TuiModel::new(event_rx, daemon_tx);
        model.settings.mcp.open(None);
        model.settings.mcp.cursor = 11;
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(daemon_rx.try_recv().is_err());
        assert!(
            matches!(&model.settings.mcp.result, Some((false, message)) if message.contains("Save this server before reconnecting"))
        );
        let config = model.settings.mcp.draft.as_ref().unwrap().config.clone();
        model.settings.mcp.servers.push(McpServerStatus {
            config: config.clone(),
            ..Default::default()
        });
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(
            matches!(daemon_rx.try_recv(), Ok(DaemonCommand::Mcp(ClientMessage::McpReconnectServer { id })) if id == config.id)
        );
    }

    #[test]
    fn mcp_modifier_chords_neither_trigger_shortcuts_nor_enter_editor_text() {
        let (_event_tx, event_rx) = std::sync::mpsc::channel();
        let (daemon_tx, mut daemon_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = TuiModel::new(event_rx, daemon_tx);
        model.open_settings_tab(SettingsTab::Mcp);
        while daemon_rx.try_recv().is_ok() {}
        model.settings.mcp.servers.push(McpServerStatus::default());
        for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            for ch in ['r', 'e', 'a', 'l'] {
                model.handle_key_modal(KeyCode::Char(ch), modifiers, modal::ModalKind::Settings);
            }
        }
        assert!(daemon_rx.try_recv().is_err());
        assert!(model.settings.mcp.draft.is_none());
        model.settings.mcp.open(None);
        model.settings.mcp.cursor = 2;
        model.settings.mcp.toggle();
        for cursor in [1, 4] {
            model.settings.mcp.cursor = cursor;
            model.settings.mcp.begin_edit();
            let revision = model.settings.mcp.revision;
            for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
                model.handle_key_modal(KeyCode::Char('r'), modifiers, modal::ModalKind::Settings);
            }
            assert!(model.settings.mcp.edit_buffer.is_empty());
            assert_eq!(model.settings.mcp.revision, revision);
            model.handle_key_modal(
                KeyCode::Char('R'),
                KeyModifiers::SHIFT,
                modal::ModalKind::Settings,
            );
            assert_eq!(model.settings.mcp.edit_buffer, "R");
            model.handle_key_modal(KeyCode::Esc, KeyModifiers::NONE, modal::ModalKind::Settings);
        }
        assert!(daemon_rx.try_recv().is_err());
    }

    #[test]
    fn mcp_editor_navigates_into_tools_and_toggles_the_selected_description() {
        let (_event_tx, event_rx) = std::sync::mpsc::channel();
        let (daemon_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let mut model = TuiModel::new(event_rx, daemon_tx);
        let status = McpServerStatus {
            tools: vec![
                zorai_protocol::McpToolInfo {
                    name: "first".into(),
                    description: "First description".into(),
                },
                zorai_protocol::McpToolInfo {
                    name: "second".into(),
                    description: "Second description".into(),
                },
            ],
            ..Default::default()
        };
        model.settings.mcp.servers = vec![status.clone()];
        model.settings.mcp.open(Some(status));
        model.settings.mcp.cursor = crate::state::mcp_settings::McpSettingsState::FIELDS - 1;

        model.handle_mcp_settings_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(model.settings.mcp.selected_tool(), Some(0));
        model.handle_mcp_settings_key(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(model.settings.mcp.expanded_tool, Some(0));
        model.handle_mcp_settings_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(model.settings.mcp.selected_tool(), Some(1));
        model.handle_mcp_settings_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(model.settings.mcp.expanded_tool, Some(1));
    }
}
