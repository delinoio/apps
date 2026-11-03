---
"@servers/git-tracker-api": patch
---

Make `RevokeToken` fail when it revokes no token. The handler now parses `tokenId` as a UUID (malformed text is invalid-argument), matches `id` and `repository_id` without a text cast, and returns not-found before the audit insert when no row matches. An unknown token, or a token of another repository, no longer writes a `token_revoked` audit row. Revoking an already revoked token stays idempotent and is audited on every call. Add PostgreSQL integration coverage and state the behavior in the server contract.

<!-- public-changelog:start -->

Revoking a token that does not exist, or that belongs to another repository, now returns a not-found error instead of reporting success.

<!-- public-changelog:end -->
