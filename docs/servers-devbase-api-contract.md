# DevBase API server contract

## Scope

`servers/devbase-api` owns one independent Go server, its PostgreSQL migrations, maintenance jobs and OCI packaging. The schemas and generated bindings live in root `protos/devbase/v1` and the consumer examples in `examples/`. Status: the server package and its workspace registration exist; the examples are `planned` in the project index until they are added. The server provides the free, project-scoped end-user directory that developers manage for their own applications. It has no web console, REST endpoint, handwritten SDK or published client package. Production deployment, backup infrastructure and live-account acceptance are outside this contract.

## Runtime and Language

Use the root Go module and pinned toolchain, Connect RPC and PostgreSQL. Run one server process against one database. The process holds a PostgreSQL session lock so a second instance cannot start; request limits and the maintenance cycle therefore need no cross-instance coordination. Root `pnpm dev` includes the server through its workspace script.

## Users and Operators

- Developers: current Owners and Admins of a Delcore tenant, authenticated with their Delino login token.
- Applications of those developers, authenticated with project API keys.
- Operators who deploy the server, register its Delcore service identity, apply migrations and inspect audit records.

End users in a project directory are data. They never authenticate to this server, never gain Delino accounts or Delcore memberships, and never serve as credentials.

## Interfaces and Contracts

The wire contract is the [protocol contract](protos-devbase-v1-contract.md). This section owns the behavior behind it.

### Dependencies on Delcore

Access Delcore only through its Connect API; never read its database. The server uses these calls:

| Purpose                               | Delcore call                   | Credential                                                  |
| ------------------------------------- | ------------------------------ | ----------------------------------------------------------- |
| Identify the developer                | `GetCurrentUser`               | The caller's login token, forwarded unchanged               |
| Require current tenant Owner or Admin | `ListMembers` with page size 1 | The caller's login token, forwarded unchanged               |
| Read billing-independent tenant state | `GetTenantStates`              | A Logto client-credentials token with the `authorize` scope |

`ListMembers` succeeds only for a current Owner or Admin of the tenant, so Delcore stays the only owner of roles. For the calls that forward the caller's login token, a permission failure from Delcore is `permission_denied`, an authentication failure is `unauthenticated`, and any other failure, timeout or malformed answer is `unavailable`. The tenant-state call uses the server's own service credential, so every failure of that call, including an expired or rejected service token, is `unavailable` and never `unauthenticated`: the caller's token is not at fault and must not be asked to sign in again. The server never fails open and never reports an uncommitted mutation as successful. Cache neither roles nor tenant state; check current authorization and state on every request, including replays.

The three Delcore calls for one login request run concurrently. Every dependency call has its own timeout (default 3 seconds), and the whole request has a timeout (default 10 seconds). The server does not retry external calls inside a request. Clients retry mutations with the same idempotency key to resolve uncertain outcomes. External requests never run inside a database transaction.

The server registers as a Delcore consumer service with the `authorize` scope. Register the service and its Logto machine-to-machine application outside Git. The service token is requested with the configured resource and cached until shortly before it expires.

### Credentials and authorization

Send `Authorization: Bearer <token>`. A token that starts with `dbk_` is a project API key; any other token is a Delino login token.

| Operation group                           | Login token                   | `READ_ONLY` key | `CRUD` key |
| ----------------------------------------- | ----------------------------- | --------------- | ---------- |
| Project create, get, list, update, delete | Current tenant Owner or Admin | Denied          | Denied     |
| Key issue, get, list, revoke              | Current tenant Owner or Admin | Denied          | Denied     |
| End-user get, lookup, list                | Current tenant Owner or Admin | Allowed         | Allowed    |
| End-user create, update, delete           | Current tenant Owner or Admin | Denied          | Allowed    |

Other tenant roles and users outside the tenant receive `permission_denied`. A key belongs to exactly one project; a request for another project's resource with that key is `not_found`. Invalid, unknown, revoked or expired credentials are `unauthenticated`. A key used for an operation outside its permission is `permission_denied`.

Project API keys use the format `dbk_<32 lowercase hexadecimal characters of the key UUID>_<43 base64url characters of 32 random bytes>`. Generate the secret with a cryptographically secure source. Store only the key ID and the SHA-256 hash of the secret; compare in constant time. Never store, log or return the raw key except in the initial issuance response. Revocation takes effect immediately. Mutations authenticated by a key re-validate the key row inside their transaction with a shared row lock, so a revocation that commits first rejects later work and a revocation waits for work that already holds the lock.

### Tenant state

The server maps the Delcore `GetTenantStates` result of the project's tenant as follows. Billing, plan and balance conditions never restrict this free feature.

| State                 | Project key calls | Login Owner/Admin calls                                                                                                  |
| --------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `ACTIVE`              | Allowed           | Allowed                                                                                                                  |
| `BLOCKED`, `DELETING` | Denied            | Allowed only for project get/list, key get/list, key revocation, end-user deletion and project deletion; all else denied |
| `UNKNOWN`, `DELETED`  | Denied            | Denied                                                                                                                   |
| Check failed          | `unavailable`     | `unavailable`                                                                                                            |

Denied calls return `permission_denied`. Creation, modification and key issuance are denied in `BLOCKED` and `DELETING`, and general end-user reads are denied with keys and login tokens alike. Operator blocking is independent of billing.

### Projects

- A Delcore tenant owns projects. Each project owns a separate end-user directory; no user is shared across projects, including projects in the same tenant.
- A project stores an internal UUID v7, the owning tenant, a required name, creation and update timestamps and a version that starts at 1 and increases on each update.
- Project names are unique within a tenant ignoring case, non-empty, without leading or trailing whitespace, valid UTF-8 and at most 256 bytes. Updates change only the name and require the current version. Projects never move between tenants.
- Deleting a project requires the current version and deletes the project, its users and its keys in one transaction. A default of at most 100 projects per tenant applies; concurrent creation cannot exceed it.

### End users

- An end user stores an internal UUID v7, the project, a required external user ID, an optional name and email, a JSON-object metadata value, timestamps and a version.
- External IDs are case-sensitive, unique within the project, non-empty, without leading or trailing whitespace and at most 256 UTF-8 bytes. They may change without changing the internal UUID. Names are at most 256 UTF-8 bytes. Email is optional, at most 254 bytes and must be a plain address; duplicate emails are allowed.
- Metadata is a JSON object of at most 16 KiB when serialized. An entire request is at most 64 KiB. Violations are `invalid_argument`. The transport rejects messages larger than 256 KiB before any handler runs, with a plain Connect `resource_exhausted` error that has no DevBase `ErrorDetail` and no trace header; this is the only error without them.
- Creation is explicit: there is no upsert. Reads are by internal UUID, by exact external ID and by paginated listing; there is no search by email, name or metadata.
- Updates use `update_mask` with `external_id`, `name`, `email` and `metadata` and require the current version. A masked name or email with an empty value clears it. Masked metadata replaces the whole object; an empty object clears it. Required fields cannot be cleared. An empty mask is invalid.
- Deleting requires the current version, removes the row permanently and immediately, and makes the external ID reusable. A user created later with a reused external ID, a new idempotency key and a new UUID has no association with the deleted one. A default of at most 1,000,000 users per project applies; concurrent creation cannot exceed it.
- This service does not look up users for other services, act on users or notify other services about deletions.

### Project API keys

- A project may have multiple keys, at most 20 active ones by default. A key is active while it is neither revoked nor expired.
- A key has a typed `READ_ONLY` or `CRUD` permission. Expiration is optional; omitting it means no automatic expiry. An expiration in the past is invalid.
- Issuance returns the secret once. Replaying issuance returns the complete stored key representation, including its project, permission, creation time and expiry, with an empty secret. If the response was lost, revoke that key and issue another. Rotate by issuing a new key and revoking the old one.
- Revocation is idempotent in effect and immediate. Keys appear in management reads with their revocation and expiry times but never with secrets or hashes.

### Pagination

Project, key and end-user lists use cursor pagination ordered by creation time and internal ID. The default page size is 50 and the maximum is 100; a larger value is `invalid_argument`. Page tokens are opaque; a malformed token is `invalid_argument`. Responses carry no total count and pages are not snapshot-consistent: rows created or removed during traversal may be missed or skipped, but a row that exists during the whole traversal appears exactly once.

### Idempotency and concurrency

- Every mutation requires an idempotency key of 8 to 128 characters from `A-Za-z0-9._:-`. Replay records are kept for 24 hours; after expiry the key is a new request.
- A replay record is unique per authenticated caller (login user or key) and idempotency key. It stores the operation, the tenant and project of the original request and a fingerprint of the input, and later requests compare all of them. Because the operation is compared data and not part of the uniqueness scope, one caller cannot use the same key for two different operations: the second is `already_exists`. Another caller can never read or collide with a caller's records. The same key with the same request returns the prior result. The same key with different input, a different operation or a different tenant or project is `already_exists`. A separate create with a duplicate external ID or project name is `already_exists`.
- Authentication, current authorization and tenant-state checks run before the replay lookup, so a replay record is never authorization. When a replayed delete targets a project that no longer exists, either `DeleteProject` or a `DeleteEndUser` whose project was deleted afterwards while its tenant still exists, the server first authenticates the caller, which confirms the identity and needs no tenant. It then makes one routing-only lookup of the caller's own replay record by caller and idempotency key alone, without filtering by operation or project, so the uniqueness rule above still decides conflicts, and reads only the tenant ID. A missing record is `not_found`. Only after that lookup does it run the Owner or Admin and tenant-state checks against the recorded tenant, and a failed check returns its error. It then compares the request with the record: a different operation or project is `already_exists` and a matching delete record returns the stored result. Any other replayed mutation for a missing project takes the same path, but its record is scrubbed, so it ends in `not_found` or `already_exists`. Tenant cleanup runs only for the `DELETED` state, which denies every login call, and cleanup removes the project's keys, so no authorization can pass afterwards: a repeated delete after tenant cleanup is `permission_denied` or `unauthenticated`, and the guarantee covers only projects removed while the tenant still exists.
- Concurrent requests with the same key are serialized by a transaction-scoped advisory lock on the caller and key, so exactly one executes and the others replay.
- Stale versions return `aborted`. Uniqueness, quotas, key validity, deletion and replay safety use database constraints, row locks and atomic counters; preflight checks alone never decide them. End-user creation and deletion update an atomic per-project counter. Project creation takes a tenant-scoped advisory lock before counting. Key issuance locks the project row before counting active keys.
- Work committed before a deletion may succeed. After a deletion commits, an in-flight request for the deleted project or user fails with `not_found` and cannot recreate data.

### Replay privacy and non-resurrection

When a user is deleted, the same transaction clears the stored response and fingerprint of every replay record that refers to the user and marks the record as scrubbed. Replaying a scrubbed create or update returns `not_found`; the deleted data is never restored or returned. Project deletion and tenant cleanup scrub every replay record of the project's users, keys and project operations the same way. A scrubbed record keeps only non-personal routing data until the 24-hour policy deletes it: the caller, key, operation, tenant ID, project ID, the scrubbed marker and the expiry. Delete operations are never scrubbed. A project or end-user delete record keeps its fingerprint, tenant ID and project ID, so the authenticated caller can repeat a delete after the project is gone while the tenant still exists; the server takes the tenant ID from that record, rechecks current authority against it, and returns the original empty result. A scrubbed create or update record has no fingerprint, so its replay returns `not_found` whatever the input, while a different operation, tenant or project still conflicts. Key issuance records never contain the secret.

Because project deletion removes the project's keys, a key-authenticated replay after project deletion is `unauthenticated`.

### Limits and failures

All numeric limits are configurable. Defaults:

| Limit                          | Value                         |
| ------------------------------ | ----------------------------- |
| Projects per tenant            | 100                           |
| Users per project              | 1,000,000                     |
| Active keys per project        | 20                            |
| User API rate per project      | 1,000 requests/s, burst 2,000 |
| Management API rate per tenant | 50 requests/s, burst 100      |

User API calls (end-user operations) are limited per project across all keys and login tokens. Management calls (project and key operations) are limited per tenant. The limiters are process-local token buckets keyed by verified project or tenant IDs, evicted when idle. Extra credentials never add capacity. A rate rejection returns `resource_exhausted` with `ErrorDetail.retry_after` and a `Retry-After` header; a resource quota rejection returns `resource_exhausted` with the quota reason. A limit is a protection, not a throughput guarantee.

Error codes: `unauthenticated` for invalid authentication, `permission_denied` for insufficient permission or tenant state, `not_found` for a missing resource or another project's resource, `invalid_argument` for invalid input, `already_exists` for duplicates and idempotency conflicts, `aborted` for stale versions, `resource_exhausted` for limits and `unavailable` for dependency or database failure. Every error carries `ErrorDetail` with a safe trace ID and the `X-Devbase-Trace-Id` header. Error messages never include credentials or personal data.

### Retention and tenant lifecycle

- Every five minutes (configurable) the maintenance cycle reads the distinct tenant IDs of known projects in batches of at most 100 and calls `GetTenantStates`. Only an explicit `DELETED` state deletes that tenant's projects, users and keys, scrubbing replay records in the same transaction. `UNKNOWN`, `DELETING`, `BLOCKED`, `ACTIVE` and any failure never delete data. A failed batch is logged, counted and retried on the next cycle.
- The same cycle deletes replay records older than 24 hours and audit records older than 90 days.
- Successful project, key and end-user changes, and automatic tenant cleanup, write one audit record in the same transaction: timestamp, action, actor, target internal ID and trace ID. Audit records contain no names, email addresses, metadata, raw keys, external user IDs or tenant IDs. Minimal audit records remain after user deletion until retention expires.
- There is no public audit API. Operators query the `audit_events` table with `devbase-api audit` or the documented SQL.
- Operational deletion does not define backup erasure. Decide the production backup retention and deletion policy when production deployment is planned.

## Storage

PostgreSQL is authoritative. Tables: `projects`, `api_keys`, `end_users`, `replays`, `audit_events` and `devbase_schema`. Entity IDs are UUID v7. Foreign keys cascade project deletion to keys and users. The database enforces unique `(tenant_id, lower(name))` for projects and unique `(project_id, external_id)` for users.

Migrations are explicit, ordered and immutable SQL files embedded in the binary. `devbase-api migrate` applies them under an advisory lock and records a version and SHA-256 digest per file. The server refuses to start and reports unready when the recorded schema differs from the binary's expected version or digests. `devbase-api check-schema` performs the same check without serving. Recovery is fix-forward: correct a defect with a new migration and release. Do not roll back to a prior image or write reverse migrations.

## Security

- Verify Delino identity through Delcore. Keys are random 256-bit secrets stored as hashes.
- Enforce tenant and project boundaries in every query; a project key never crosses its project.
- Reject unsafe input and bound request size before processing. Use parameterized SQL only.
- Never log credentials, secrets, replay payloads, names, email addresses, metadata or external user IDs. Log trace IDs, procedure names, bounded result categories and counts.
- Service credentials and configuration stay outside Git.
- Browser access is limited to the DevBase console. `WEB_ORIGIN` holds one exact origin (scheme, host and optional port, with no path, trailing slash or wildcard); when it is empty, no browser origin is allowed. A request to a Connect procedure whose `Origin` header differs from it is rejected with HTTP 403 before authentication, and a request without `Origin` is unaffected. For the configured origin, a preflight `OPTIONS` request is answered with 204 without credentials and allows `POST` and the `Authorization`, `Content-Type`, `Connect-Protocol-Version` and `Connect-Timeout-Ms` request headers. Responses to that origin carry its exact `Access-Control-Allow-Origin`, `Vary: Origin` and an exposed `X-Devbase-Trace-Id` and `Retry-After`. The server never sends `Access-Control-Allow-Credentials`, because authentication uses bearer tokens only. Operational endpoints carry no CORS headers. Applications must keep project API keys in server-side code; browsers of other origins are not supported.

## Logging

Use `log/slog` structured logs with trace IDs and bounded error categories. Operator logs are colored key/value text by default; `NO_COLOR` disables the color and `LOG_FORMAT=json` emits one JSON object per record for log ingestion. Any other `LOG_FORMAT` value stops startup. Expose Prometheus metrics for request count, error rate by Connect code, latency, limit rejection and maintenance cleanup failure, without tenant, project, user or key IDs in labels. Support OpenTelemetry through the standard `OTEL_*` variables. The server exposes `/healthz` and `/readyz`; readiness checks the database schema.

Operational inspection starts from the trace ID in a failing response: search structured logs for it. Check `/metrics` for the rejection and cleanup counters, run `devbase-api check-schema` for schema questions, and query `audit_events` for change history. Numeric SLOs, dashboards and alert infrastructure are not part of this contract.

## Build and Test

- Go unit tests for validation, pagination cursors, key parsing, rate limiting and configuration.
- PostgreSQL integration tests (`TEST_DATABASE_URL`, build tag `integration`) with a mocked Delcore: complete Owner and Admin flows, rejected Member and Billing roles, invalid authentication, cross-tenant and cross-project access, unknown, expired and revoked keys, read-only writes, all tenant states and exceptions, dependency failure, validation boundaries, both lookups, cursors, masks, stale versions, concurrent duplicate creates, quota boundaries, same-key retries, differing-input retries, concurrent update and delete, concurrent revocation, project deletion races, lost responses, replay expiry, deleted-resource replay without personal data, one-time-secret replay, replay isolation, aggregated rate limits, timeouts, cleanup cycles including mixed states and retry, audit retention and the absence of secrets and personal fields from audit, replay markers and logs.
- Regression tests that exercise a real Delcore API server are separate acceptance and must be reported when not run.
- Protocol lint, generation, freshness and FILE breaking checks from the repository root.
- `go vet ./servers/...`, `go build` of every server command, and `go test -race ./servers/...`.
- OCI build, `migrate`, `check-schema` and startup checks against disposable PostgreSQL.
- Execute the Go and TypeScript examples against a local server with mocked Delcore.

Fixtures and builds do not establish live-account or production acceptance.

## Dependencies and Integrations

Connect-Go, pgx, Prometheus client, OpenTelemetry and Delcore through its generated Connect client in `protos/delcore/v1`. Logto issues Delino login tokens and the service token.

## Change Triggers

Update this contract, the [protocol contract](protos-devbase-v1-contract.md) and the [project index](project-devbase.md) together when behavior, limits, paths or deployment boundaries change. Record validation and unresolved limits in pull requests and CI records.

## Environment configuration

Server environment variables have no service-name prefix. Required: `DATABASE_URL`, `DELCORE_URL`, `DELCORE_RESOURCE`, `LOGTO_TOKEN_URL`, `CLIENT_ID` and `CLIENT_SECRET`. Optional with defaults: `LISTEN` (`127.0.0.1:46317`), `MAX_PROJECTS_PER_TENANT`, `MAX_USERS_PER_PROJECT`, `MAX_ACTIVE_KEYS_PER_PROJECT`, `USER_RATE_PER_SECOND`, `USER_RATE_BURST`, `MANAGEMENT_RATE_PER_SECOND`, `MANAGEMENT_RATE_BURST`, `DEPENDENCY_TIMEOUT`, `REQUEST_TIMEOUT` and `CLEANUP_INTERVAL` (durations such as `3s`, `10s` and `5m`). `LOG_FORMAT` selects `text` (default) or `json`. `WEB_ORIGIN` is optional with no default: use `http://127.0.0.1:46313` for the local console, `https://devbase.delino.io` for production, or the exact origin that Portless prints. Standard `OTEL_*` and `NO_COLOR` retain their names. PostgreSQL integration tests read `TEST_DATABASE_URL`. The examples read `DEVBASE_URL` and `DEVBASE_TOKEN`. `DELCORE_URL`, `DELCORE_RESOURCE`, `LOGTO_TOKEN_URL`, `CLIENT_ID` and `CLIENT_SECRET` use the same names as Git Tracker but identify this service's own Delcore service identity; provide them per service process. Reject missing, non-positive or contradictory values at startup. Commit only safe placeholders in `servers/devbase-api/.env.example`.

## Local development

The development command first injects the Infisical `dev` variables of the `/devbase-api` folder using the local root project configuration. CLI installation, authentication and project access are required; follow the [environment contract](repository-environment-contract.md#local-development-injection).

Run `pnpm dev` at the repository root for all development servers, or from `servers/devbase-api` for this API only. The script runs [Air](https://github.com/air-verse/air) with `root = "../.."` and loads the optional ignored `servers/devbase-api/.env`; use `.env.example` as a guide. Root `pnpm dev` starts the local `devbase-postgres` Compose database and applies its URL as `DATABASE_URL` through the shared API launcher after Infisical injection; follow the [development database procedure](repository-workflow-contract.md#local-development-databases) and the [environment contract](repository-environment-contract.md#local-development-injection). Direct service-local development sets `DATABASE_URL` itself. Run `devbase-api migrate` before starting; Air does not provision dependencies, apply migrations or bypass configuration and schema checks. Keep the local API listener at `127.0.0.1:46317`. A port conflict fails startup; no alternate port is selected. See the [workflow contract](repository-workflow-contract.md) for watcher scope.

## Operator procedures

- **Migrate:** run `devbase-api migrate` against the target database before starting a new version. It is safe to repeat.
- **Schema compatibility:** `devbase-api check-schema` exits non-zero when the schema is missing, newer or modified.
- **Audit query:** `devbase-api audit --since <RFC3339> [--target <uuid>] [--actor <actor>] [--limit <n>]` prints matching records as JSON lines, newest first. Equivalent SQL: `SELECT occurred_at, action, actor, target_id, trace_id FROM audit_events WHERE occurred_at >= $1 ORDER BY occurred_at DESC`.
- **Cleanup failures:** watch the cleanup failure counter and the `tenant cleanup failed` log events. Failures retry on the next cycle. A persistent failure usually means Delcore or database unavailability; check Delcore health and the service credentials first.
- **Fix-forward recovery:** deploy a corrected image and, if data needs correction, a new forward migration. Take a database backup before applying a migration. Do not roll back images or reverse migrations.
- **Key loss:** a lost key secret is unrecoverable. Revoke the key and issue another.

## Known limits

Lists are not snapshot-consistent. The key secret cannot be recovered. An external ID can refer to a new internal identity after deletion and reuse. Downstream data in other services is not deleted by this service. Dependency outages deny access instead of degrading. Request limits do not certify sustainable throughput. Backup erasure policy is deferred to production planning.

## References

- [Repository defaults](repository-defaults.md)
- [Delcore server](servers-delcore-api-contract.md)
- [Protocol contract](protos-devbase-v1-contract.md)
- [Project index](project-devbase.md)
