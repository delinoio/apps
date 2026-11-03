---
"@servers/mcpa-api": patch
---

Persist a rotated Logto refresh token even when the request is canceled. Once a console session refresh or an MCP grant refresh holds its row lock, the Logto calls, the stored replacement token and the commit run detached from the request and are bounded by four times DEPENDENCY_TIMEOUT, so a disconnecting client no longer rolls back a consumed refresh token and ends the session or grant. Waiting for the lock still ends with the request, an MCP exchange whose client left keeps the presented handle active, and a canceled but completed refresh logs a warning with request_cancelled=true.
