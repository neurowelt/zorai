use crate::state::mcp_settings::McpSettingsState;
use crate::theme::ThemeTokens;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zorai_protocol::{McpAdapterPolicy, McpAuthConfig};

pub(crate) const MCP_HEADER_ROWS: usize = 4;
const LABEL_WIDTH: usize = 20;

pub(crate) fn render_mcp(
    state: &McpSettingsState,
    content_width: u16,
    theme: &ThemeTokens,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled("  MCP Servers", theme.fg_active)),
        Line::from(Span::styled(
            "  Connect external tools over Streamable HTTP.",
            theme.fg_dim,
        )),
        Line::raw(""),
    ];

    if let Some(draft) = &state.draft {
        let value = |index: usize, value: String| {
            if state.editing && state.cursor == index {
                if index == 4 {
                    format!("{}█", "•".repeat(state.edit_buffer.chars().count()))
                } else {
                    format!("{}█", state.edit_buffer)
                }
            } else {
                value
            }
        };

        push_field(
            &mut lines,
            state,
            0,
            "Name:",
            value(0, draft.config.name.clone()),
            "edit",
            theme,
        );
        push_field(
            &mut lines,
            state,
            1,
            "URL:",
            value(1, draft.config.url.clone()),
            "edit",
            theme,
        );
        push_field(
            &mut lines,
            state,
            2,
            "Authentication:",
            match draft.config.auth {
                McpAuthConfig::None => "None / local peer".into(),
                McpAuthConfig::Bearer { .. } => "Bearer token".into(),
                McpAuthConfig::ApiKey { .. } => "Named API-key header".into(),
            },
            "cycle",
            theme,
        );
        push_field(
            &mut lines,
            state,
            3,
            "API-key header:",
            value(
                3,
                match &draft.config.auth {
                    McpAuthConfig::ApiKey { header, .. } => header.clone(),
                    _ => "—".into(),
                },
            ),
            "edit",
            theme,
        );
        push_field(
            &mut lines,
            state,
            4,
            "Credential:",
            value(4, state.secret_label().into()),
            "edit",
            theme,
        );
        push_action(&mut lines, state, 5, "Clear credential", theme);
        push_checkbox(&mut lines, state, 6, draft.config.enabled, "Enabled", theme);
        push_checkbox(
            &mut lines,
            state,
            7,
            draft.config.share_workspace_context,
            "Share workspace context (path and conversation ID)",
            theme,
        );
        push_field(
            &mut lines,
            state,
            8,
            "Adapter:",
            match draft.config.adapter {
                McpAdapterPolicy::Generic => "Generic".into(),
                McpAdapterPolicy::Portal => "Portal".into(),
            },
            "cycle",
            theme,
        );
        push_action(
            &mut lines,
            state,
            9,
            "Test connection (isolated draft)",
            theme,
        );
        push_action(&mut lines, state, 10, "Save", theme);
        push_action(&mut lines, state, 11, "Reconnect saved server", theme);
        push_action(&mut lines, state, 12, "Back / discard draft", theme);

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  Available Tools",
            theme.fg_active,
        )));
        if let Some(status) = state.status() {
            if status.tools.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No tools were advertised by this server.",
                    theme.fg_dim,
                )));
            } else {
                for (index, tool) in status.tools.iter().enumerate() {
                    push_tool(
                        &mut lines,
                        state,
                        index,
                        &tool.name,
                        &tool.description,
                        content_width,
                        theme,
                    );
                }
            }
        } else {
            lines.push(Line::from(Span::styled(
                "  Test or reconnect to discover this server's tools.",
                theme.fg_dim,
            )));
        }
    } else {
        for (index, server) in state.servers.iter().enumerate() {
            let selected = state.selected == index;
            let marker_style = if selected {
                theme.accent_primary
            } else {
                theme.fg_dim
            };
            let name_style = if selected {
                theme.accent_primary
            } else if server.config.enabled {
                theme.fg_active
            } else {
                theme.fg_dim
            };
            let state_style = if server.state == "connected" {
                theme.accent_success
            } else if matches!(server.state.as_str(), "connecting" | "discovering") {
                theme.accent_secondary
            } else {
                theme.accent_danger
            };
            lines.push(Line::from(vec![
                Span::styled(if selected { "> " } else { "  " }, marker_style),
                Span::styled(
                    if server.config.enabled {
                        "[x] "
                    } else {
                        "[ ] "
                    },
                    if server.config.enabled {
                        theme.accent_success
                    } else {
                        theme.fg_dim
                    },
                ),
                Span::styled(server.config.name.clone(), name_style),
                Span::styled("  ·  ", theme.fg_dim),
                Span::styled(server.state.clone(), state_style),
                Span::styled(format!("  ·  {} tools", server.tools.len()), theme.fg_dim),
            ]));
        }
        let selected = state.selected == state.servers.len();
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "> " } else { "  " },
                if selected {
                    theme.accent_primary
                } else {
                    theme.fg_dim
                },
            ),
            Span::styled(
                "[Add MCP server]",
                if selected {
                    theme.accent_primary
                } else {
                    theme.fg_active
                },
            ),
        ]));
    }

    if let Some((success, message)) = &state.result {
        lines.push(Line::raw(""));
        push_wrapped_text(
            &mut lines,
            "  ",
            message,
            content_width,
            if *success {
                theme.accent_success
            } else {
                theme.accent_danger
            },
        );
    }
    lines
}

fn push_field(
    lines: &mut Vec<Line<'static>>,
    state: &McpSettingsState,
    index: usize,
    label: &str,
    value: String,
    action: &str,
    theme: &ThemeTokens,
) {
    let selected = state.cursor == index;
    let editing = selected && state.editing;
    let mut spans = vec![
        Span::styled(
            if selected { "> " } else { "  " },
            if selected {
                theme.accent_primary
            } else {
                theme.fg_dim
            },
        ),
        Span::styled(format!("{label:<LABEL_WIDTH$}"), theme.fg_dim),
        Span::styled(
            value,
            if selected {
                theme.accent_primary
            } else {
                theme.fg_active
            },
        ),
    ];
    if selected && !editing {
        spans.push(Span::styled(format!("  [Enter: {action}]"), theme.fg_dim));
    }
    lines.push(Line::from(spans));
}

fn push_checkbox(
    lines: &mut Vec<Line<'static>>,
    state: &McpSettingsState,
    index: usize,
    checked: bool,
    label: &str,
    theme: &ThemeTokens,
) {
    let selected = state.cursor == index;
    let mut spans = vec![
        Span::styled(
            if selected { "> " } else { "  " },
            if selected {
                theme.accent_primary
            } else {
                theme.fg_dim
            },
        ),
        Span::styled(
            if checked { "[x] " } else { "[ ] " },
            if checked {
                theme.accent_success
            } else {
                theme.fg_dim
            },
        ),
        Span::styled(
            label.to_string(),
            if selected {
                theme.accent_primary
            } else {
                theme.fg_active
            },
        ),
    ];
    if selected {
        spans.push(Span::styled("  [Enter: toggle]", theme.fg_dim));
    }
    lines.push(Line::from(spans));
}

fn push_action(
    lines: &mut Vec<Line<'static>>,
    state: &McpSettingsState,
    index: usize,
    label: &str,
    theme: &ThemeTokens,
) {
    let selected = state.cursor == index;
    lines.push(Line::from(vec![
        Span::styled(
            if selected { "> " } else { "  " },
            if selected {
                theme.accent_primary
            } else {
                theme.fg_dim
            },
        ),
        Span::styled(
            format!("[{label}]"),
            if selected {
                theme.accent_primary
            } else {
                theme.fg_active
            },
        ),
    ]));
}

fn push_tool(
    lines: &mut Vec<Line<'static>>,
    state: &McpSettingsState,
    index: usize,
    name: &str,
    description: &str,
    content_width: u16,
    theme: &ThemeTokens,
) {
    let selected = state.cursor == McpSettingsState::FIELDS + index;
    let expanded = state.expanded_tool == Some(index);
    let hint = if expanded {
        "  [Enter: hide description]"
    } else {
        "  [Enter: show description]"
    };
    let show_hint = selected
        && 6 + UnicodeWidthStr::width(name) + UnicodeWidthStr::width(hint)
            <= usize::from(content_width);
    lines.push(Line::from(vec![
        Span::styled(
            if selected { "> " } else { "  " },
            if selected {
                theme.accent_primary
            } else {
                theme.fg_dim
            },
        ),
        Span::styled(
            if expanded { "[-] " } else { "[+] " },
            if selected {
                theme.accent_primary
            } else {
                theme.fg_dim
            },
        ),
        Span::styled(
            name.to_string(),
            if selected {
                theme.accent_primary
            } else {
                theme.accent_secondary
            },
        ),
        Span::styled(if show_hint { hint } else { "" }, theme.fg_dim),
    ]));

    if expanded {
        push_wrapped_text(
            lines,
            "      ",
            if description.is_empty() {
                "(No description provided.)"
            } else {
                description
            },
            content_width,
            theme.fg_active,
        );
    }
}

pub(crate) fn mcp_editor_hit_test(
    state: &McpSettingsState,
    rendered_row: usize,
    content_width: u16,
) -> Option<usize> {
    let fields_end = MCP_HEADER_ROWS + McpSettingsState::FIELDS;
    if (MCP_HEADER_ROWS..fields_end).contains(&rendered_row) {
        return Some(rendered_row - MCP_HEADER_ROWS);
    }

    let mut row = fields_end + 2;
    for (index, tool) in state.status()?.tools.iter().enumerate() {
        let mut rows = 1;
        if state.expanded_tool == Some(index) {
            let description = if tool.description.is_empty() {
                "(No description provided.)"
            } else {
                &tool.description
            };
            rows += wrapped_line_count(description, content_width, 6);
        }
        if (row..row + rows).contains(&rendered_row) {
            return Some(McpSettingsState::FIELDS + index);
        }
        row += rows;
    }
    None
}

fn wrapped_line_count(text: &str, content_width: u16, prefix_width: usize) -> usize {
    let available = usize::from(content_width)
        .saturating_sub(prefix_width)
        .max(1);
    wrap_chars(text, available).len()
}

fn push_wrapped_text(
    lines: &mut Vec<Line<'static>>,
    prefix: &str,
    text: &str,
    content_width: u16,
    style: Style,
) {
    let prefix_width = UnicodeWidthStr::width(prefix);
    let available = usize::from(content_width)
        .saturating_sub(prefix_width)
        .max(1);
    for chunk in wrap_chars(text, available) {
        lines.push(Line::from(vec![
            Span::raw(prefix.to_string()),
            Span::styled(chunk, style),
        ]));
    }
}

fn wrap_chars(text: &str, max_width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if ch == '\n' || (width + ch_width > max_width && !chunk.is_empty()) {
            chunks.push(std::mem::take(&mut chunk));
            width = 0;
            if ch == '\n' {
                continue;
            }
        }
        chunk.push(ch);
        width += ch_width;
    }
    if !chunk.is_empty() || chunks.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_renderer_masks_replacement_and_edit_buffer() {
        let mut state = McpSettingsState::default();
        state.open(None);
        state.cursor = 2;
        state.toggle();
        state.cursor = 4;
        state.begin_edit();
        state.edit_buffer = "super-secret-token".into();
        let theme = ThemeTokens::default();
        let rendered = render_mcp(&state, 160, &theme)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("super-secret-token"));
        assert!(rendered.contains("path and conversation ID"));
        state.commit_edit();
        let rendered = render_mcp(&state, 160, &theme)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("super-secret-token"));
        assert!(rendered.contains("replace on save"));
    }

    #[test]
    fn mcp_render_backend_shows_styled_catalog_without_server_instructions() {
        let mut state = McpSettingsState::default();
        let status = zorai_protocol::McpServerStatus {
            config: zorai_protocol::McpServerConfig {
                name: "Mock server".into(),
                url: "http://127.0.0.1:12345/mcp".into(),
                ..Default::default()
            },
            state: "connected".into(),
            server_name: Some("mock".into()),
            instructions: Some("A very long working agreement that does not belong here".into()),
            workspace_context_supported: true,
            tools: vec![zorai_protocol::McpToolInfo {
                name: "mock_echo".into(),
                description: "Echoes input".into(),
            }],
            ..Default::default()
        };
        state.servers = vec![status.clone()];
        state.open(Some(status));
        let theme = ThemeTokens::default();
        let lines = render_mcp(&state, 160, &theme);
        let rendered = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Available Tools"));
        assert!(rendered.contains("[+] mock_echo"));
        assert!(!rendered.contains("Echoes input"));
        assert!(!rendered.contains("working agreement"));
        assert!(!rendered.contains("↑/↓ select"));
        let tool_line = lines
            .iter()
            .find(|line| line.to_string().contains("mock_echo"))
            .expect("tool line");
        assert_eq!(tool_line.spans[2].style, theme.accent_secondary);

        state.cursor = McpSettingsState::FIELDS;
        assert!(state.toggle_selected_tool_description());
        let expanded = render_mcp(&state, 160, &theme)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(expanded.contains("[-] mock_echo"));
        assert!(expanded.contains("Echoes input"));
    }

    #[test]
    fn mcp_long_remote_descriptions_remain_available_when_wrapped() {
        let mut state = McpSettingsState::default();
        let status = zorai_protocol::McpServerStatus {
            state: "connected".into(),
            tools: vec![zorai_protocol::McpToolInfo { name: "long_tool".into(), description: "A very long remote description that must stay inspectable through the final word END".into() }],
            ..Default::default()
        };
        state.servers = vec![status.clone()];
        state.open(Some(status));
        state.cursor = McpSettingsState::FIELDS;
        state.toggle_selected_tool_description();
        let theme = ThemeTokens::default();
        let lines = render_mcp(&state, 32, &theme);
        let details = &lines[MCP_HEADER_ROWS + McpSettingsState::FIELDS..];
        assert!(details
            .iter()
            .all(|line| UnicodeWidthStr::width(line.to_string().as_str()) <= 32));
        let tool_row = details
            .iter()
            .position(|line| line.to_string().contains("long_tool"))
            .expect("tool row");
        let recovered_description = details[tool_row + 1..]
            .iter()
            .take(wrapped_line_count(
                "A very long remote description that must stay inspectable through the final word END",
                32,
                6,
            ))
            .filter_map(|line| line.spans.get(1))
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(
            recovered_description,
            "A very long remote description that must stay inspectable through the final word END"
        );
    }

    #[test]
    fn mcp_tool_hit_test_accounts_for_expanded_description_rows() {
        let mut state = McpSettingsState::default();
        let status = zorai_protocol::McpServerStatus {
            tools: vec![
                zorai_protocol::McpToolInfo {
                    name: "first".into(),
                    description: "A description long enough to wrap across several rows".into(),
                },
                zorai_protocol::McpToolInfo {
                    name: "second".into(),
                    description: "Second description".into(),
                },
            ],
            ..Default::default()
        };
        state.servers = vec![status.clone()];
        state.open(Some(status));
        let first_row = MCP_HEADER_ROWS + McpSettingsState::FIELDS + 2;
        assert_eq!(mcp_editor_hit_test(&state, first_row, 24), Some(13));
        assert_eq!(mcp_editor_hit_test(&state, first_row + 1, 24), Some(14));

        state.cursor = McpSettingsState::FIELDS;
        state.toggle_selected_tool_description();
        let description_rows = wrapped_line_count(
            "A description long enough to wrap across several rows",
            24,
            6,
        );
        assert_eq!(mcp_editor_hit_test(&state, first_row + 1, 24), Some(13));
        assert_eq!(
            mcp_editor_hit_test(&state, first_row + 1 + description_rows, 24),
            Some(14)
        );
    }
}
