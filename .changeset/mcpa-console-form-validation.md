---
"@apps/mcpa": patch
---

Validate console forms against the server's input limits before sending a request: call key, resource and credential names (UTF-8 bytes), descriptions, model allowlist entries (1-200 bytes, no whitespace or control characters) and https-only API-key base URLs. Accept typed budget limits of up to 12 integer and 6 fraction digits, send an untouched prefilled limit back unchanged, and show a full budget meter for an exhausted zero limit. Promote the byte unit when display rounding reaches 1024.

<!-- public-changelog:start -->

Forms in the console now check names, descriptions, model names and API-key base URLs before you submit and explain what to fix, instead of failing after the request. Budget limits accept up to six decimal places, a budget with a zero limit shows a full meter, and sizes such as 1,048,575 bytes now read `1.0 MiB` instead of `1024 KiB`.

<!-- public-changelog:end -->
