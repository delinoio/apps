---
"@servers/mcpa-api": patch
---

Add trace_id and surface to every context-aware log line written during Connect, /auth, /oauth and /mcp requests through a context-aware slog handler, and remove the manual trace_id attributes so that no line carries the key twice. Call-surface lines keep the call record ID instead of the trace ID.
