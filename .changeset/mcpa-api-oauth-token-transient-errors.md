---
"@servers/mcpa-api": patch
---

Answer MCP token requests with 503 `temporarily_unavailable` instead of 500 `server_error` when the database fails during an exchange or when Logto rate limits the upstream refresh. The authorization code or refresh handle stays usable, so the client retries with the same credential.
