---
"@servers/anydash-api": patch
---

Report the violating widget layout field in `ErrorDetail.field`, keep `PollEnrollment` database failures retryable instead of `UNAUTHENTICATED`, reject client timestamps outside years 1 to 9999 UTC (including parser `observed_at` and metric periods that were stored as absent) with `INVALID_ARGUMENT`, and give usage breakdown rows a fixed parser-state precedence (`PARTIAL` > `OK` > `NO_SOURCES` > `DISABLED`).
