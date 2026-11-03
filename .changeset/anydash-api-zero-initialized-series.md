---
"@servers/anydash-api": patch
---

Expose the worker job run counters for every job and outcome, and the collection snapshot counters for every provider and status, at 0 from startup, so the first retention or deletion job failure is visible to Prometheus `increase()` and the worker failure alert fires. Add a rule test for the server-emitted series shape.
