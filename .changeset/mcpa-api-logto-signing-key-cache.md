---
"@servers/mcpa-api": patch
---

Verify Logto tokens against one cached signing-key set that is refetched only for an unknown key ID, and classify failures to fetch signing keys as Logto unavailability. Authentication now answers dependency unavailable (503 on `/mcp`, `temporarily_unavailable` in the MCP authorization callback) during a key-set outage instead of rejecting valid tokens as invalid.
