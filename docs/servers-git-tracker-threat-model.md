# Git Tracker threat model

## Boundaries

Untrusted browsers, CI jobs, forks, repository configuration, JSON rows and GitHub events cross into the Connect service. PostgreSQL is authoritative. Logto authenticates users; Delcore authorizes current space permissions and confirms finalized deletion. GitHub confirms repository administration, App installation, commit context and visibility. No client-supplied permission or visibility flag is authoritative.

## Threats and controls

- Cross-space reads and mutations: verify current Delcore permissions for the connected space on protected operations. Repository IDs are globally unique. An initial connection requires both space management and GitHub administrator authority.
- Credential theft: require OIDC issuer, audience, signature and expiry; bind Actions run identity, repository, event, PR and commit. Fork credentials cannot write upstream push data or supply authoritative baseline overrides. API tokens are repository-scoped, expire and can be revoked; store only digests.
- Forged or reordered GitHub events: verify HMAC before parsing effects. Repository events re-fetch current metadata. App removal/disconnection makes all retained history private. A failed visibility lookup retains the last confirmed visibility by contract.
- Concurrent or replayed uploads: lock the repository row, validate the whole request and commit atomically. Same numeric row values are retransmissions; changed values conflict. Registration order and frozen comparison references cannot be changed by completion or retransmission.
- Malicious numbers: accept only finite JSON decimal syntax, bound parser text/exponent expansion and compare exact rationals. Reject unsupported units and negative thresholds. Never use floating point for regression decisions.
- Data resurrection after deletion: references contain IDs, not cached baseline values. Permanent deletion removes measurements and configuration; deleted baselines become unavailable. Delcore finalization removes connections, tokens and history.
- Stale PR publication: publish only after persistence and re-read current state under the repository lock. Check current PR head and latest registration. Publication errors warn without rolling back measurements or changing regression status.
- Secret or private-data exposure: disable public response caching, return generic internal errors and safe correlation IDs, use bounded metric labels and exclude raw measurements/credentials from logs and audits. Public-to-private changes stop subsequent anonymous reads.

## Intentional limits

A visibility transition that GitHub synchronization has not observed remains public until successful synchronization. An in-flight response can have been authorized before a visibility transition. Measurements have no expiry or product quota. Payload and numeric parser bounds protect runtime resources. Automated security tests use fixtures and mocks; external certification and live-provider acceptance are separate.

## References

- [Project](project-git-tracker.md)
- [Server contract](servers-git-tracker-api-contract.md)
