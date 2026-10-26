---
"@apps/lane": patch
---

Retain the request ID of Accept proposal, Reject proposal and Restore this prompt in the Lane web editor across a failed reply and a clean-draft refresh, so an unchanged retry replays the stored action instead of forking the task into a conflict.

<!-- public-changelog:start -->

Fixed a case where retrying Accept proposal, Reject proposal or Restore this prompt after a reported failure could create a second competing version and lock the task as conflicted. Retrying the same action now returns the result that was already saved.
<!-- public-changelog:end -->
