---
"@servers/anydash-api": minor
---

Add the AnyDash Rust Connect API and background worker (`servers/anydash-api`): device enrollment and rotating device credentials, external connections with lease epochs and heartbeat failover, versioned 15-minute usage uploads with LiteLLM-based estimated cost, hourly metric snapshots, dashboards with the team role matrix and data-owner sharing, widget data queries in the selected time zone, the activity log, account data deletion, Delcore tenant lifecycle handling, quotas, rate limits and the minimum client version. Add explicit forward migrations, OCI packaging, operator monitoring assets, the load test and local development and CI registration. The unified release publishes the `anydash-api` image as part of the AnyDash release component.
