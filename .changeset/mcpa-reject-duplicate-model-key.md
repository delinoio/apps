---
"@servers/mcpa-api": patch
---

Reject OpenAI-format and Claude-format call bodies whose top-level object names `model` more than once, before the allowlist check and before any provider contact. The gate read the first occurrence while the body was forwarded unchanged, so a decoder that reads another occurrence could serve a model the allowlist never approved. The rejection is `model_not_allowed` (`CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED`) in the client error format, does not echo the body, applies to empty allowlists too, and bills only response bytes. Nested `model` keys and single-key bodies are unaffected.

<!-- public-changelog:start -->

A request body that names the `model` field more than once at the top level is now rejected with `model_not_allowed`. Send exactly one top-level `model` field.

<!-- public-changelog:end -->
