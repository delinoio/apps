---
"@servers/mcpa-api": patch
---

Refresh group snapshots during the MCP authorization callback before the authorization code is stored, and bound that refresh as a whole by the dependency timeout. A slow Delcore no longer leaves the client a code that is already expired or nearly expired when it receives the redirect.
