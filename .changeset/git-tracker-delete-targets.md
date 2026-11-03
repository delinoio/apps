---
"@servers/git-tracker-api": patch
---

Make `Delete` check its target. A dataset deletion now marks only the executions whose measurements held the dataset as having lost it (extracted into `State::delete_dataset`), so executions that never held it keep accepting it and compare it as new. Deleting an unknown execution, or a dataset that no execution holds, fails with not-found before the state is written, so a mistyped ID no longer leaves a tombstone or marker or writes a `measurements_deleted` audit row. Repeating the deletion of a deleted execution stays idempotent without a second audit row, and deleting an empty repository writes no audit row. Add unit and PostgreSQL integration coverage and update the server and protocol contracts.

<!-- public-changelog:start -->

Deleting an execution or a dataset that does not exist now returns a not-found error instead of reporting success, and deleting a dataset no longer blocks other executions from uploading it.

<!-- public-changelog:end -->
