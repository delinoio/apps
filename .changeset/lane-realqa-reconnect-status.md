---
"@apps/lane": patch
---

Send the Chrome bridge status notification when an approved profile reconnects with
its saved credentials, so the open RealQA settings view shows the connected state
through the existing invalidation event. Credentials, approval and reconnection
behavior are unchanged. Node fixtures cover the bridge and adapter; actual Chrome
and native UI acceptance is not performed.

<!-- public-changelog:start -->

Lane now shows the Chrome connection as connected right after an approved Chrome
profile reconnects, without needing another action in Settings.

<!-- public-changelog:end -->
