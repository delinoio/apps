---
"@servers/git-tracker-api": patch
---

Reject the NUL character (U+0000) in client-supplied stored text before persistence. Upload and completion now fail with a client error that names the field, without echoing the value, for the execution ID, branch, dataset ID and name, column ID, name and unit, and row ID and name. `Delete` rejects a NUL in its ID the same way. Previously the PostgreSQL `jsonb` write failed, the request counted as a persistence outage and the Action retried it. Other control characters and non-ASCII text are unchanged. Add unit and PostgreSQL integration coverage and state the rule in the server and protocol contracts.

<!-- public-changelog:start -->

Uploads whose execution ID, branch, dataset, column or row text contains a NUL character are now rejected with a clear validation error instead of a temporary server error, so the Action no longer retries them.

<!-- public-changelog:end -->
