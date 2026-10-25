# Lane CLI and Local MCP Contract

## Scope

Planned `cmds/lane` owns the Go `lane` executable and local stdio MCP. `cmds/lane/internal` owns Go domain/store/sync adapters in the existing root module. Do not create a companion module, `go.work`, private checkout or Go server. The [distribution contract](packages-lane-distribution-contract.md) owns release artifacts and `@delinoio/lane`.

## Runtime and Language

Use Go selected by root `go.mod`, generated Lane Connect bindings, and a pinned CGO-free SQLite driver. Shared native/Go SQL paths and columns are fixed by the [local-store contract](apps-lane-local-store-contract.md). Go code uses `log/slog`. Keep stdout reserved for requested results or MCP protocol messages; diagnostics use stderr.

## Users and Operators

Individuals use local commands without login. Existing agents use local stdio MCP without a GUI/server. Users explicitly approve optional online connections. Operators install reviewed executables or the npm launcher.

## Interfaces and Contracts

### Target selection and output

`lane [--remote] [--json] [--space <id>] <command>` defaults to the active local space. Only explicit `--remote` targets server data; login alone does not change the default. `--space` selects an existing local space and is invalid with `--remote`. Server-only operations return ONLINE_REQUIRED with a direct instruction when their target is local/offline; never silently redirect a local task command to the server. Local `sync connect` explicitly authorizes contacting the server.

Human-readable output is default. `--json` emits one result object `{"ok":true,"result":{…}}` or `{"ok":false,"error":{"reason":"…","message":"…","correlationId":"…","retryable":false}}`. Preserve typed protocol enum names and decimal 64-bit values. Exit 0 means completed/read/replayed/deleted-result success; exit 1 means operation failure; exit 2 means invalid invocation. JSON errors also go to stdout; safe diagnostics go to stderr. Use ANSI color in interactive human output, with `NO_COLOR` and `--color=never` opt-out; never add ANSI to JSON/MCP.

Mutating invocations accept `--request-id <uuid>` for reliable retries and print the allocated request ID when omitted. Updates require `--expected-version <uuid>`; interactive clients can obtain it by a read but cannot issue an unconditional write. Conflict resolution requires the complete expected head set. Deletion commands are themselves confirmation; no extra prompt is required for individual task/repository/event deletion. Server account-data deletion still requires explicit account confirmation and browser reauthentication.

### Command surface and typed mapping

| Commands                                                     | Typed operation / behavior                                                                                               |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| `repository list/get/create/edit/delete`                     | LaneService repository methods; URL registration/rename rules apply                                                      |
| `task list/get/create/edit/delete`                           | LaneService task methods; create accepts a repository ID or URL, title and optional description/prompt/progress/proposal |
| `task progress`, `task accept`, `task reject`                | SetProgress / SetProposal with unchanged separate state                                                                  |
| `task move`                                                  | SaveTask whole snapshot with new repository ID and expected version                                                      |
| `task history`, `task prompt-history`, `task restore-prompt` | ListHistory / RestorePrompt; source version belongs to the same task                                                     |
| `conflict list/get/resolve`                                  | Read heads/alternatives and ResolveConflict; resolution creates a new version                                            |
| `sync connect/status/run/disconnect`                         | Browser approval, durable outbox status, background loop, KEEP_LOCAL/REMOVE_LOCAL choice                                 |
| `login`, `logout`                                            | Browser connection approval; logout requires retained-local-data choice                                                  |
| `device list/disconnect`                                     | AccountService; explicit remote target                                                                                   |
| `connection list/revoke`                                     | AccountService; explicit remote target                                                                                   |
| `notification get/set`                                       | Per-device preferences, not account-wide settings                                                                        |
| `receiver list/create/edit/rotate-key/delete`                | WebhookService receiving settings; online-only                                                                           |
| `event list/get/delete/claim/renew/release/complete/reopen`  | WebhookService; local list/get reads retained cached data; other operations online-only                                  |
| `data remove-local`                                          | Remove only selected local space through coordinated handle shutdown                                                     |
| `data delete-everywhere`, `data clear-server`                | AccountService deletion modes, explicit remote/account confirmation and reauthentication                                 |
| `mcp serve`                                                  | Local stdio MCP; default local store                                                                                     |

Names are public command groups; help must spell out required arguments and local/online distinctions. `task list` supports all repositories or one repository, progress/proposal filters and title/description substring search. Prompt input may use a literal or stdin; reading source text is not a database import/export feature. Explicit task edits save immediately as one command/version. Shared CLI execution continues when the app window is closed.

`sync run` owns a bounded-retry loop and the per-space ownership lease from the local-store contract. It releases ownership on clean exit; crashes leave durable outbox data and an expiring owner record. Persist acknowledgments/cursors before removing pending work. Desktop host supervision uses the same executable, not a second queue implementation. Mobile hosts implement the protocol within their own lifecycle. Do not promise background execution while the OS suspends an app.

### MCP surface

`lane mcp serve` implements stdio MCP. Do not print banners or logs on stdout. Use explicit JSON schemas derived from typed messages. Tool names are `lane_<snake_case_method>` for LaneService, AccountService and WebhookService methods, for example `lane_create_task`, `lane_resolve_conflict`, `lane_claim_event` and `lane_complete_event`. Also expose `lane_get_local_status`, `lane_connect_sync`, `lane_disconnect_sync`, `lane_remove_device_data`, `lane_get_notification_preferences` and `lane_set_notification_preferences` for local device management. Local conflict listing can use the typed list response's alternatives; a separate untyped mutation is not allowed.

MCP arguments omit Origin; the adapter sets MCP and its real device/account connection identity. A new proposed task through MCP can notify; the same operation through CLI uses CLI origin and cannot notify. Tool descriptions identify online-only functions and explicit irreversible deletion semantics. Task/repository deletion invocation is confirmation. Remote account deletion still requires a human browser reauthentication flow and account confirmation; an agent cannot supply a fabricated proof.

Local MCP uses the same SQLite transactions, durable request IDs and conflict rules as CLI. Server-only operations require explicit remote selection/approval; no offline event processing lease exists. Remote MCP is the separate server endpoint, authenticated with a browser-approved full-account grant, and remains available with all devices off. Revoke grants through app/CLI; do not interpret a local stdio launch as a remote approval.

## Storage

Use the exact registry and SQLite layout in the local-store contract. Never use `os.UserConfigDir` as a substitute for the Windows LocalAppData path. CLI/local MCP must interoperate with an already running app and with a closed app. Credentials live in per-OS credential storage; account switch selects a separate local space. Keep local data/outbox on expired authorization or network failure. Unsupported schema is an error, never a fresh empty database.

## Security

Use browser login approval with PKCE/state and correct OAuth audiences. Do not accept user-supplied account/tenant attribution as authorization. Local process access is bounded by OS-user permissions. Never upload a prior account's retained data on login to a different account. One-time receiver secrets are shown only in create/rotate responses; loss requires rotation, not retrieval. Do not run agents, shell commands from prompts, Git access verification or AI services.

## Logging

Use structured stderr/file metadata with correlation IDs. Exclude task/prompt/webhook content, secrets and token values. Apply the combined native/CLI 100 MiB and 30-day log limits and no-upload rule. Failure output must distinguish local save failure, pending sync rejection, auth failure and unavailable dependency.

## Build and Test

At implementation run root Go tests/vet for affected packages and CLI builds for macOS, Windows and Linux. Run shared protocol vectors, human/JSON output tests, stdio MCP framing/schema tests, SQLite concurrency and native cross-process compatibility tests. Exercise local CLI/MCP with no account, no server and no app window. Test explicit remote targeting, request retries, whole-task conflicts, event completion atomicity, account deletion proof and revocation with mocked services.

Cross-compilation alone does not establish executable installation or native app behavior on that OS. Record exact runtime/platform checks separately. Preserve root Rust/frontend checks when those sources also change.

## Dependencies and Integrations

Use repository protocol generation, the root Go module and platform credential adapters. Coordinate the helper's packaging with native hosts. No external agent runtime is a Lane dependency.

## Change Triggers

Change protocol/local-store/app/distribution contracts together for command, schema or installation changes. Update scoped instructions only for ownership or development procedures.

## References

- [Project](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [Protocol](protos-lane-v1-contract.md)
- [Server](servers-lane-api-contract.md)
