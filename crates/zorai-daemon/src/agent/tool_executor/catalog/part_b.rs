use super::*;
pub(crate) fn add_available_tools_part_b(
    tools: &mut Vec<ToolDefinition>,
    config: &AgentConfig,
    _agent_data_dir: &std::path::Path,
    has_workspace_topology: bool,
) {
    tools.push(tool_def(
        tool_names::SEARCH_HISTORY,
        "Search command execution history. Returns matching commands with timestamps and exit codes.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "integer", "description": "Max results (default: 20)" }
            },
            "required": ["query"]
        }),
    ));
    tools.push(tool_def(
        tool_names::FETCH_GATEWAY_HISTORY,
        "Fetch a paged recent-message window from the current gateway thread, with paging metadata and has_more flag.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "description": "Number of messages to fetch (default: 10, max: 100)" },
                "count": { "type": "integer", "description": "Deprecated alias for limit" },
                "offset": { "type": "integer", "description": "Newest messages to skip before limit (default: 0)" }
            }
        }),
    ));
    tools.push(tool_def(
        tool_names::SESSION_SEARCH,
        "Search prior sessions, transcripts, cognitive traces, and operational history for relevant past context.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "integer", "description": "Max results (default: 8)" }
            },
            "required": ["query"]
        }),
    ));
    if config.enable_honcho_memory && !config.honcho_api_key.trim().is_empty() {
        tools.push(tool_def(
            tool_names::AGENT_QUERY_MEMORY,
            "Query Honcho cross-session memory for long-term user, workspace, or assistant context.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Question to ask Honcho memory." }
                },
                "required": ["query"]
            }),
        ));
    }
    tools.push(tool_def(
        tool_names::ONECONTEXT_SEARCH,
        "Search Aline OneContext history for related prior sessions/events/turns.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "scope": { "type": "string", "enum": ["session", "event", "turn"], "description": "Search scope (default: session)" },
                "no_regex": { "type": "boolean", "description": "Treat query as plain text (default: true)" },
                "timeout_seconds": { "type": "integer", "minimum": 0, "maximum": 600, "description": "Max wait (default: 300, max: 600)" }
            },
            "required": ["query"]
        }),
    ));

    if has_workspace_topology {
        tools.push(tool_def(
            tool_names::LIST_SESSIONS,
            "List frontend-reported workspace sessions and panes when workspace topology is available.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ));
    }

    tools.push(tool_def(
        tool_names::NOTIFY_USER,
        "Send a proactive notification to the user via configured channels.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "Notification title" },
                "message": { "type": "string", "description": "Notification body" },
                "severity": { "type": "string", "enum": ["info", "warning", "alert", "error"] }
            },
            "required": ["title", "message"]
        }),
    ));

    tools.push(tool_def(
        tool_names::UPDATE_TODO,
        "Replace the current todo list for this conversation.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "Ordered todo items for the current plan",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string", "description": "Short todo item text" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "blocked"], "description": "Current execution state" },
                            "step_index": { "type": "integer", "description": "Optional step label for non-goal displays; never binds goal steps" }
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        }),
    ));

    tools.push(tool_def(
        tool_names::GET_TODOS,
        "Fetch the current planner todos for a thread (thread-scoped lookup).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "thread_id": { "type": "string", "description": "Optional; defaults to the current thread" },
                "task_id": { "type": "string", "description": "Optional task ID for goal-run context validation" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::UPDATE_MEMORY,
        "Update curated persistent memory. Use this only for durable operator preferences or stable project facts, not temporary task state.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "target": { "type": "string", "enum": ["memory", "user", "soul"], "description": "Memory file to update (default: memory)" },
                "mode": { "type": "string", "enum": ["replace", "append", "remove"], "description": "How to apply the content (default: replace)" },
                "content": { "type": "string", "description": "Markdown content or exact fragment for remove mode" }
            },
            "required": ["content"]
        }),
    ));

    tools.push(tool_def(
        tool_names::READ_MEMORY,
        "Read MEMORY.md plus structured memory layers; skips base markdown already injected in the prompt.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "include_already_injected": { "type": "boolean", "description": "Include base markdown even if already injected (default: false)" },
                "include_base_markdown": { "type": "boolean", "description": "Include MEMORY.md (default: true)" },
                "include_operator_profile_json": { "type": "boolean", "description": "Include operator profile JSON (default: true)" },
                "include_operator_model_summary": { "type": "boolean", "description": "Include operator-model summary (default: true)" },
                "include_thread_structural_memory": { "type": "boolean", "description": "Include thread structural memory (default: true)" },
                "limit_per_layer": { "type": "integer", "description": "Max items per list layer (default: 5, max: 25)" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::READ_USER,
        "Read USER.md plus structured operator layers; skips base markdown already injected in the prompt.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "include_already_injected": { "type": "boolean", "description": "Include base markdown even if already injected (default: false)" },
                "include_base_markdown": { "type": "boolean", "description": "Include USER.md (default: true)" },
                "include_operator_profile_json": { "type": "boolean", "description": "Include operator profile JSON (default: true)" },
                "include_operator_model_summary": { "type": "boolean", "description": "Include operator-model summary (default: true)" },
                "include_thread_structural_memory": { "type": "boolean", "description": "Include thread structural memory (default: false)" },
                "limit_per_layer": { "type": "integer", "description": "Max items per list layer (default: 5, max: 25)" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::READ_SOUL,
        "Read SOUL.md plus structured context layers; skips base markdown already injected in the prompt.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "include_already_injected": { "type": "boolean", "description": "Include base markdown even if already injected (default: false)" },
                "include_base_markdown": { "type": "boolean", "description": "Include SOUL.md (default: true)" },
                "include_operator_profile_json": { "type": "boolean", "description": "Include operator profile JSON (default: true)" },
                "include_operator_model_summary": { "type": "boolean", "description": "Include operator-model summary (default: true)" },
                "include_thread_structural_memory": { "type": "boolean", "description": "Include thread structural memory (default: false)" },
                "limit_per_layer": { "type": "integer", "description": "Max items per list layer (default: 5, max: 25)" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::SEARCH_MEMORY,
        "Search MEMORY.md plus structured memory layers; skips base markdown already injected in the prompt.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "integer", "description": "Max matches (default: 5, max: 25)" },
                "include_already_injected": { "type": "boolean", "description": "Allow injected MEMORY.md content in matches (default: false)" },
                "include_base_markdown": { "type": "boolean", "description": "Search MEMORY.md (default: true)" },
                "include_operator_profile_json": { "type": "boolean", "description": "Search operator profile JSON (default: true)" },
                "include_operator_model_summary": { "type": "boolean", "description": "Search operator-model summary (default: true)" },
                "include_thread_structural_memory": { "type": "boolean", "description": "Search thread structural memory (default: true)" }
            },
            "required": ["query"]
        }),
    ));

    tools.push(tool_def(
        tool_names::SEARCH_USER,
        "Search USER.md plus structured operator layers; skips base markdown already injected in the prompt.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "integer", "description": "Max matches (default: 5, max: 25)" },
                "include_already_injected": { "type": "boolean", "description": "Allow injected USER.md content in matches (default: false)" },
                "include_base_markdown": { "type": "boolean", "description": "Search USER.md (default: true)" },
                "include_operator_profile_json": { "type": "boolean", "description": "Search operator profile JSON (default: true)" },
                "include_operator_model_summary": { "type": "boolean", "description": "Search operator-model summary (default: true)" },
                "include_thread_structural_memory": { "type": "boolean", "description": "Search thread structural memory (default: false)" }
            },
            "required": ["query"]
        }),
    ));

    tools.push(tool_def(
        tool_names::SEARCH_SOUL,
        "Search SOUL.md plus structured context layers; skips base markdown already injected in the prompt.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "integer", "description": "Max matches (default: 5, max: 25)" },
                "include_already_injected": { "type": "boolean", "description": "Allow injected SOUL.md content in matches (default: false)" },
                "include_base_markdown": { "type": "boolean", "description": "Search SOUL.md (default: true)" },
                "include_operator_profile_json": { "type": "boolean", "description": "Search operator profile JSON (default: true)" },
                "include_operator_model_summary": { "type": "boolean", "description": "Search operator-model summary (default: true)" },
                "include_thread_structural_memory": { "type": "boolean", "description": "Search thread structural memory (default: false)" }
            },
            "required": ["query"]
        }),
    ));

    tools.push(tool_def(
        tool_names::LIST_SKILLS,
        "List local skills from ~/.zorai/skills (built-in, generated, community, plugin-bundled).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Optional name/path filter for relevant skills" },
                "limit": { "type": "integer", "description": "Max skills to return (default: 20)" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::GET_CURRENT_DATETIME,
        "Fetch the current local and UTC datetime from the daemon host, including a Unix timestamp in milliseconds.",
        serde_json::json!({
            "type": "object",
            "properties": {}
        }),
    ));

    tools.push(tool_def(
        tool_names::SEMANTIC_QUERY,
        "Query local workspace manifests, compose services, import relationships, conventions, and temporal history.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["summary", "packages", "dependencies", "dependents", "services", "service_dependencies", "service_dependents", "imports", "imported_by", "conventions", "temporal"], "description": "Query mode (default: summary)" },
                "target": { "type": "string", "description": "Package, service, path fragment, or module name, per mode" },
                "path": { "type": "string", "description": "Workspace root (default: active session cwd)" },
                "limit": { "type": "integer", "description": "Max results for list modes (default: 20)" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::LIST_MCP_SERVERS,
        "List configured MCP integrations, including disconnected servers, by server_id, name, aliases, connection state, permitted tool count, and optional workflow skill. Use for live integration discovery when a user names an MCP such as Portal or Companions; session history does not describe current connections.",
        serde_json::json!({"type": "object", "properties": {}}),
    ));

    tools.push(tool_def(
        tool_names::LIST_TOOLS,
        "List built-in and connected MCP tools, including descriptions and argument schemas. Results are paginated: the first page is not the full catalog. Set server_id from list_mcp_servers to list only that integration's permitted tools.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "description": "Max tools to return (default: 20)" },
                "server_id": { "type": "string", "description": "Exact MCP server_id from list_mcp_servers; excludes built-in tools when set" },
                "offset": { "type": "integer", "description": "Zero-based pagination offset (default: 0)" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::TOOL_SEARCH,
        "Search built-in and connected MCP tools by tool name, server name or aliases, description, and parameter names. Set server_id from list_mcp_servers to search only that integration. Use the returned exact name to call an MCP tool directly.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Capability or action you are looking for" },
                "server_id": { "type": "string", "description": "Exact MCP server_id from list_mcp_servers; excludes built-in tools when set" },
                "limit": { "type": "integer", "description": "Max matches (default: 10)" },
                "offset": { "type": "integer", "description": "Zero-based pagination offset (default: 0)" }
            },
            "required": ["query"]
        }),
    ));

    tools.push(tool_def(
        tool_names::LOAD_TOOLS,
        "Activate deferred tools so they become callable this turn. Only a core set is shown by default; use `tool_search` to find a tool, then `load_tools` with its exact name(s) before calling it.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "names": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Exact tool names to activate (from tool_search results)"
                }
            },
            "required": ["names"]
        }),
    ));

    tools.push(tool_def(
        tool_names::DISCOVER_GUIDELINES,
        "Find matching local guidelines when a workflow orchestrator would help. Optional; skip unless this turn needs one.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Brief intent query, 3-6 words." },
                "limit": { "type": "integer", "description": "Max ranked candidates (default: 3)" },
                "session": { "type": "string", "description": "Optional terminal session UUID for workspace-aware ranking" }
            },
            "required": ["query"]
        }),
    ));

    tools.push(tool_def(
        tool_names::READ_GUIDELINE,
        "Read a local guideline when you will follow it. Optional. Accepts a guideline name, relative path, or filename.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "guideline": { "type": "string", "description": "Guideline name, file stem, or relative path" },
                "max_lines": { "type": "integer", "description": "Max lines to read (default: 200)" }
            },
            "required": ["guideline"]
        }),
    ));

    tools.push(tool_def(
        tool_names::LIST_GUIDELINES,
        "List local guidelines available above the skill layer.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Optional guideline name or path filter" },
                "limit": { "type": "integer", "description": "Maximum guidelines to return (default: 20)" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::DISCOVER_SKILLS,
        "Find matching local skills when a playbook would help. Optional; skip unless this turn needs one.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Brief intent query, 3-6 words." },
                "limit": { "type": "integer", "description": "Max ranked candidates (default: 3)" },
                "session": { "type": "string", "description": "Optional terminal session UUID for workspace-aware ranking" }
            },
            "required": ["query"]
        }),
    ));

    tools.push(tool_def(
        tool_names::READ_SKILL,
        "Read one or more local skill documents when you will follow them. Accepts skill names, relative paths, or generated skill filenames.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill": { "type": "string", "description": "Skill name, file stem, or relative path" },
                "skills": {
                    "type": "array",
                    "description": "Multiple skill names or paths to read in one call",
                    "items": { "type": "string" }
                },
                "max_lines": { "type": "integer", "description": "Max lines to read (default: 200)" }
            }
        }),
    ));

    tools.push(tool_def(
        tool_names::ASK_QUESTIONS,
        "Show a blocking multiple-choice question to the operator and wait for one compact token answer. Full prompt and answer text go in `content`; `options` are short ordered tokens like A/B/C or 1/2/3.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "Full question text including each option's answer text" },
                "options": {
                    "type": "array",
                    "description": "Compact button tokens, e.g. [\"A\", \"B\", \"C\"]",
                    "items": { "type": "string" }
                },
                "session": { "type": "string", "description": "Optional terminal session UUID to target one surface" }
            },
            "required": ["content", "options"]
        }),
    ));

    tools.push(tool_def(
        tool_names::JUSTIFY_SKILL_SKIP,
        "Record a rationale for proceeding without a local skill recommendation.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "rationale": { "type": "string", "description": "Why no installed local skill is suitable for the current request" }
            },
            "required": ["rationale"]
        }),
    ));
}
