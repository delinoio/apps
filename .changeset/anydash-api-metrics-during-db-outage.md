---
"@servers/anydash-api": patch
---

Keep serving `/metrics` when the database-backed gauges cannot be refreshed. The scrape returns 200 with the last gauge values and increments the new `anydash_gauge_refresh_failures_total` counter, so request error counters stay visible during a database outage and `AnyDashApiServerErrors` can fire.
