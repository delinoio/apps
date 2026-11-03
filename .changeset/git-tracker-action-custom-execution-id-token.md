---
"@actions/git-tracker": patch
---

Fail before any identity or API request when a custom execution-id is supplied without a repository token, because the API accepts the Actions identity only for the default execution ID. An explicit value equal to the default ID remains valid. The committed action.cjs bundle is rebuilt.

<!-- public-changelog:start -->

The Git Tracker Action now fails immediately with a clear message when execution-id is set to a custom value without a repository token, instead of failing later with a permission error. Provide the token input, or omit execution-id to use the default identity.
<!-- public-changelog:end -->
