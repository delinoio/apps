---
"@servers/mcpa-api": patch
---

Make `ListTenantGroups` use the TTL-aware group snapshot refresh and serialize refreshes of one tenant across concurrent callers and replicas with a PostgreSQL advisory lock bounded by `DEPENDENCY_TIMEOUT`, so Delcore `ListGroups` runs at most once per tenant per `MCPA_AUTHORIZE_CACHE_TTL`. A failed refresh keeps the previous snapshot and its refresh time; personal keys still read the snapshot only.
