---
"@servers/anydash-api": patch
---

Count requests that exceed the 60-second deadline in the new `anydash_api_timeouts_total` counter and include them in the 5xx request ratio, so the `AnyDashApiServerErrors` alert fires when requests time out instead of staying silent.
