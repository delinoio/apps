---
"@servers/mcpa-api": patch
---

Leave a 401 `invalid_call_key` rejection that is raised after the call state exists (a permission denial, or a resource that is gone when the allowlist is read) unrecorded, like the rejections before the call state exists. The response still carries `X-Mcpa-Request-Id` and the request keeps its HTTP request metric and a log line, but the call record, outbox row, aggregate and `mcpa_calls_total` sample are skipped, and neither the final nor a periodic metering checkpoint bills its response bytes. A requested model name longer than 200 bytes, from the body or from the Gemini path, is rejected with `model_not_allowed` and the message `the model name is too long` before the allowlist read, and the call state and log line carry the name truncated to 200 bytes on a UTF-8 boundary. Names of up to 200 bytes, other rejection codes and their recorded status are unchanged.

<!-- public-changelog:start -->

A request that is rejected with `invalid_call_key` because the key is no longer valid is no longer recorded as a call or billed for its response. A request that names a model longer than 200 bytes is now rejected with `model_not_allowed` without echoing the name.

<!-- public-changelog:end -->
