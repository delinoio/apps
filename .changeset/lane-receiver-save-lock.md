---
"@apps/lane": patch
---

Lock the webhook receiver name, repository choices and Save while a receiver save is pending, so edits cannot be silently discarded when the editor closes. Failure retains the draft for retry.

<!-- public-changelog:start -->

Webhook receiver fields are now locked while a save is in progress, so changes made during the save are no longer lost when the editor closes.
<!-- public-changelog:end -->
