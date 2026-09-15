# MCP discovery and Companion workflows

Zorai's native agent receives a compact MCP connection directory on each turn, alongside the connected tools. The directory survives prompt rebuilding after compaction. Configured but disconnected or disabled servers remain visible, with zero available tools. Tool counts reflect the turn's tool permissions; connection metadata contains no endpoint or credential fields.

When the user names an integration, the core routing instructions direct the agent to its tools. The agent can call a known tool immediately, or discover it with:

```text
list_mcp_servers({})
list_tools({"server_id": "<id from directory>"})
tool_search({"server_id": "<id from directory>", "query": "consult"})
```

The server filter is applied before pagination and excludes built-in tools. An unknown server ID is an error, while a configured server with no permitted tools returns an empty catalog. Without a filter, search includes server display names and aliases. MCP results identify `server_id`, `server_name`, and `original_name`; their `name` remains the exact callable identifier. Renaming or changing discovery metadata does not change callable identifiers or reconnect the server.

MCP settings expose optional comma-separated **Aliases** and a **Workflow skill** name. The configuration fields are `aliases` (string array) and `skill` (optional lowercase skill name). Existing settings without these fields still load. Portal-adapter connections automatically advertise the aliases `portal` and `companions`, and default to the `companions` workflow skill. A custom skill overrides that default. Explicit aliases supplement the Portal aliases.

## Companions bundle

`skills/companions/SKILL.md` is the Zorai entry point. It and its supporting references are adapted from Portal's harness bundle at `portal/internal/assets/bundle` in the `lane-portal` checkout (source revision `31ff0f19b2e946e2b947fa53be67c5bb4080ece0`). The copy includes the main workflow, consultation protocol, client-tool declarations, perspective and brainstorm workflows, and setup/consult/discover/meet/balance prompts.

Zorai-specific adaptations resolve original MCP tool names through the live catalog, load supporting files relative to the installed skill, map client-tool requests to available host capabilities, and use Zorai's workspace bindings. The copied prompts are references, not registered slash commands. Portal's installer and harness support are unchanged.

Zorai's existing built-in skill seeding copies this repository's skill tree to the runtime skills directory (normally `~/.zorai/skills` on macOS/Linux). Once the updated daemon has seeded it, the agent can load it directly with `read_skill({"skill":"companions"})`. `zorai skill sync` downloads the published repository tree; it does not install unpublished local changes.

After building this change, restart both the daemon and TUI together: their MCP settings wire shape now includes the discovery metadata fields. No live processes or connection settings are changed merely by editing the repository.
