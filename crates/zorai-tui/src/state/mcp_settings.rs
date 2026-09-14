//! MCP editor state is separate from ordinary config: credentials never enter config patches.
use zorai_protocol::{
    McpAdapterPolicy, McpAuthConfig, McpCredentialUpdate, McpServerConfig, McpServerStatus,
};

#[derive(Default)]
pub struct McpSettingsState {
    pub servers: Vec<McpServerStatus>,
    pub last_refresh: Option<std::time::Instant>,
    pub selected: usize,
    pub draft: Option<McpDraft>,
    pub cursor: usize,
    pub close_after_save: bool,
    pub removing: bool,
    pub expanded_tool: Option<usize>,
    pub editing: bool,
    pub edit_buffer: String,
    pub revision: u64,
    pub pending: Option<(String, u64, bool)>, // request ID, revision, save (rather than test)
    pub result: Option<(bool, String)>,
    pub test_status: Option<McpServerStatus>,
}

pub struct McpDraft {
    pub config: McpServerConfig,
    pub credential: McpCredentialUpdate,
    pub credential_present: bool,
}

impl McpSettingsState {
    pub const FIELDS: usize = 14;

    pub fn is_saved(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| self.servers.iter().any(|s| s.config.id == draft.config.id))
    }

    pub fn visible_fields(&self) -> Vec<usize> {
        (0..Self::FIELDS)
            .filter(|index| match index {
                3 => self
                    .draft
                    .as_ref()
                    .is_some_and(|d| matches!(d.config.auth, McpAuthConfig::ApiKey { .. })),
                5 => false,
                13 => self.is_saved(),
                _ => true,
            })
            .collect()
    }

    pub fn move_cursor(&mut self, forward: bool) {
        let mut fields = self.visible_fields();
        fields.extend(Self::FIELDS..Self::FIELDS + self.tool_count());
        if let Some(next) = if forward {
            fields.into_iter().find(|i| *i > self.cursor)
        } else {
            fields.into_iter().rev().find(|i| *i < self.cursor)
        } {
            self.cursor = next;
        }
    }

    pub fn open(&mut self, server: Option<McpServerStatus>) {
        self.changed();
        self.draft = Some(match server {
            Some(server) => McpDraft {
                config: server.config,
                credential: McpCredentialUpdate::Keep,
                credential_present: server.credential_present,
            },
            None => McpDraft {
                config: McpServerConfig {
                    id: uuid::Uuid::new_v4().to_string(),
                    enabled: true,
                    ..Default::default()
                },
                credential: McpCredentialUpdate::Keep,
                credential_present: false,
            },
        });
        self.cursor = 0;
        self.expanded_tool = None;
        self.editing = false;
        self.edit_buffer.clear();
    }

    pub fn close_draft(&mut self) {
        self.changed();
        self.draft = None;
        self.expanded_tool = None;
        self.editing = false;
        self.edit_buffer.clear();
    }

    pub fn changed(&mut self) {
        self.close_after_save = false;
        self.removing = false;
        self.revision = self.revision.wrapping_add(1);
        self.pending = None;
        self.result = None;
        self.test_status = None;
        self.expanded_tool = None;
    }

    pub fn begin_edit(&mut self) {
        let Some(draft) = &self.draft else {
            return;
        };
        self.edit_buffer = match self.cursor {
            0 => draft.config.name.clone(),
            1 => draft.config.url.clone(),
            3 => match &draft.config.auth {
                McpAuthConfig::ApiKey { header, .. } => header.clone(),
                _ => return,
            },
            // Never initialize an editable token with a masked value or an existing secret.
            4 if !matches!(draft.config.auth, McpAuthConfig::None) => String::new(),
            _ => return,
        };
        self.changed();
        self.editing = true;
    }

    pub fn commit_edit(&mut self) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        match self.cursor {
            0 => draft.config.name = self.edit_buffer.trim().to_string(),
            1 => {
                let url = self.edit_buffer.trim().to_string();
                if draft.config.url != url {
                    draft.config.share_workspace_context = false;
                    // An edited endpoint must never inherit a stored credential silently.
                    draft.credential = McpCredentialUpdate::Clear;
                    draft.credential_present = false;
                }
                draft.config.url = url;
            }
            3 => {
                if let McpAuthConfig::ApiKey { header, .. } = &mut draft.config.auth {
                    *header = self.edit_buffer.trim().to_string();
                }
            }
            4 => {
                if !self.edit_buffer.is_empty() {
                    draft.credential = McpCredentialUpdate::Replace(self.edit_buffer.clone());
                } else {
                    draft.credential = McpCredentialUpdate::Clear;
                }
            }
            _ => {}
        }
        self.edit_buffer.clear();
        self.editing = false;
        self.changed();
    }

    pub fn toggle(&mut self) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        match self.cursor {
            2 => {
                draft.config.auth = match draft.config.auth {
                    McpAuthConfig::None => McpAuthConfig::Bearer {
                        credential_ref: None,
                    },
                    McpAuthConfig::Bearer { .. } => McpAuthConfig::None,
                    McpAuthConfig::ApiKey { .. } => McpAuthConfig::None,
                };
                draft.credential = McpCredentialUpdate::Clear;
                draft.credential_present = false;
            }
            5 => draft.credential = McpCredentialUpdate::Clear,
            6 => draft.config.enabled = !draft.config.enabled,
            7 => draft.config.share_workspace_context = !draft.config.share_workspace_context,
            8 => {
                draft.config.adapter = match draft.config.adapter {
                    McpAdapterPolicy::Generic => McpAdapterPolicy::Portal,
                    McpAdapterPolicy::Portal => McpAdapterPolicy::Generic,
                }
            }
            _ => return,
        }
        self.changed();
    }

    pub fn request(&mut self, save: bool) -> Option<zorai_protocol::ClientMessage> {
        if self.editing {
            return None;
        }
        let draft = self.draft.as_ref()?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let revision = self.revision;
        let config = draft.config.clone();
        let credential = draft.credential.clone();
        self.pending = Some((request_id.clone(), revision, save));
        self.result = Some((
            true,
            if save {
                "Saving…"
            } else {
                "Testing isolated connection: initializing and discovering tools…"
            }
            .into(),
        ));
        self.test_status = None;
        Some(if save {
            zorai_protocol::ClientMessage::McpSaveServer {
                request_id,
                revision,
                config,
                credential,
            }
        } else {
            zorai_protocol::ClientMessage::McpTestServer {
                request_id,
                revision,
                config,
                credential,
            }
        })
    }

    /// Returns whether a successful save should refresh the saved server list.
    pub fn complete(
        &mut self,
        request_id: String,
        revision: u64,
        success: bool,
        message: String,
        server: Option<McpServerStatus>,
    ) -> bool {
        let Some((expected_id, expected_revision, save)) = &self.pending else {
            return false;
        };
        if *expected_id != request_id || *expected_revision != revision || self.revision != revision
        {
            return false;
        }
        let saved = *save && success;
        if saved && self.removing {
            if let Some(draft) = &self.draft {
                self.servers.retain(|s| s.config.id != draft.config.id);
            }
            self.selected = self.selected.min(self.servers.len());
            self.close_draft();
            self.result = Some((true, message));
            return true;
        }
        self.pending = None;
        self.result = Some((success, message));
        if saved {
            if let Some(status) = &server {
                self.draft = Some(McpDraft {
                    config: status.config.clone(),
                    credential: McpCredentialUpdate::Keep,
                    credential_present: status.credential_present,
                });
            } else if let Some(draft) = &mut self.draft {
                draft.credential_present = match draft.credential {
                    McpCredentialUpdate::Replace(_) => true,
                    McpCredentialUpdate::Clear => false,
                    McpCredentialUpdate::Keep => draft.credential_present,
                };
                draft.credential = McpCredentialUpdate::Keep;
            }
        }
        self.test_status = if saved { None } else { server };
        if saved && self.close_after_save {
            self.close_draft();
        } else {
            self.close_after_save = false;
            self.removing = false;
        }
        saved
    }

    pub fn status(&self) -> Option<&McpServerStatus> {
        self.test_status.as_ref().or_else(|| {
            self.draft.as_ref().and_then(|draft| {
                self.servers
                    .iter()
                    .find(|server| server.config == draft.config)
            })
        })
    }

    pub fn tool_count(&self) -> usize {
        self.status().map_or(0, |status| status.tools.len())
    }

    pub fn last_cursor(&self) -> usize {
        Self::FIELDS
            .checked_add(self.tool_count())
            .and_then(|count| count.checked_sub(1))
            .unwrap_or(Self::FIELDS - 1)
    }

    pub fn selected_tool(&self) -> Option<usize> {
        self.cursor
            .checked_sub(Self::FIELDS)
            .filter(|index| *index < self.tool_count())
    }

    pub fn toggle_selected_tool_description(&mut self) -> bool {
        let Some(index) = self.selected_tool() else {
            return false;
        };
        self.expanded_tool = (self.expanded_tool != Some(index)).then_some(index);
        true
    }

    pub fn secret_label(&self) -> &'static str {
        match self.draft.as_ref() {
            Some(McpDraft {
                config:
                    McpServerConfig {
                        auth: McpAuthConfig::None,
                        ..
                    },
                ..
            }) => "Choose authentication first",
            Some(McpDraft {
                credential: McpCredentialUpdate::Replace(_),
                ..
            }) => "••••••••",
            Some(McpDraft {
                credential: McpCredentialUpdate::Clear,
                ..
            }) => "Not set",
            Some(McpDraft {
                credential_present: true,
                ..
            }) => "••••••••",
            _ => "Not set",
        }
    }
}

#[cfg(test)]
#[path = "tests/mcp_settings.rs"]
mod tests;
