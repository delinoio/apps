---
"@servers/git-tracker-api": patch
---

Authenticate and authorize repository requests against a lock-free repository snapshot before beginning the transaction and locking the repository row, so a request that waits on Delcore or GitHub no longer holds the row lock or a pooled connection, and a request with an invalid credential never takes the lock. The locked row is re-checked for disconnection and lost public visibility. Add PostgreSQL concurrency tests with a gated Delcore mock and a unit test of the re-check.
