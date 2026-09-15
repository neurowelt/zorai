# MCP tool approvals

MCP tools use WELES governance and the existing Approval Center. If WELES blocks a
remote tool, including when its review is unavailable, the daemon requests an
operator decision and pauses that call. No remote request is sent before approval.

In the TUI, press **Ctrl+A** from chat and select the pending MCP request:

- **Approve Once** runs the waiting call with its original arguments and context.
- **Approve Session** permits that tool for the current conversation until the daemon restarts.
- **Always Approve** saves a per-tool rule using the existing task approval rules.
  It applies to future arguments for that tool and connection. Revoke the rule in
  the Approval Center to require review again.
- **Deny** returns a denied result without calling the remote server.

There is no automatic server-wide allowance on installation. Approve a concrete
call first, and select Always Approve if you want to permit subsequent calls.
Other tools on the same server still require their own review or allowance.

Rules include the server identity and a fingerprint of its connection settings.
Changing the endpoint, credentials, workspace-sharing setting, or other saved
connection settings requires fresh permission. Reconnecting an unchanged server
does not invalidate a saved rule. A waiting call cannot execute against a removed
or reconfigured connection.

Approvals do not override task/responder tool restrictions, workspace-sharing
requirements, or MCP connection checks. Cancelling the operation abandons its
waiting approval. WELES internal review tasks retain their existing restrictions.
No lower security level or separate reviewer UI is required for this approval path.
