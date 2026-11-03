---
"@servers/mcpa-api": patch
---

Assign the X-Mcpa-Request-Id trace ID before routing so that ServeMux-generated 404 and 405 responses carry it; per-route instrumentation reuses the same ID and the call surface still replaces it with the call record ID.
