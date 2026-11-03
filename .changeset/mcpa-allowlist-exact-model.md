---
"@servers/mcpa-api": patch
---

Match the resource model allowlist against the exact requested model string. A name with a thinking suffix, such as `model(high)`, no longer passes because its base name `model` is listed; it is rejected with `model_not_allowed` before any provider contact unless that exact string is listed. Credential routing and empty-allowlist behavior are unchanged.

<!-- public-changelog:start -->

A resource's model allowlist now allows only the exact model names it lists. A suffixed name such as `model(high)` is rejected unless it is listed itself, so a request can no longer reach a listed model through an unlisted name.

<!-- public-changelog:end -->
