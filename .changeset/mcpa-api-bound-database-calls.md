---
"@servers/mcpa-api": patch
---

Bound every request-path database call by DEPENDENCY_TIMEOUT. In serve mode the pool sets the PostgreSQL statement and lock timeouts and applies a context deadline to connection acquisition and to calls outside transactions, so lock waits, slow statements and a saturated pool fail as dependency unavailable (a dependency rejection on the call path) instead of holding requests until they end. migrate, check-schema and rotate-master-key stay unbounded, and tenant data deletion keeps its four-times bound.
