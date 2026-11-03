---
"@servers/mcpa-api": patch
---

Reject an MCP authorization `state` that is empty, longer than 512 bytes or not printable ASCII. An invalid non-empty state now gets a 400 `invalid_request` response without a redirect or echo, instead of a storage failure reported as `temporarily_unavailable`.
