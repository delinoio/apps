---
"@actions/git-tracker": patch
---

Include the Connect error code and a bounded single-line message from the API response in Action failure and warning text, next to the status and correlation ID. Retries, transient classification and warn-on-api-error are unchanged. The committed action.cjs bundle is rebuilt.

<!-- public-changelog:start -->

When the Git Tracker API rejects an upload or completion, the Action log now shows the reason given by the API, such as a conflicting measurement, together with the status and correlation ID.
<!-- public-changelog:end -->
