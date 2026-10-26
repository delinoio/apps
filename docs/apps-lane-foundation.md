# Lane Application Foundation

## Scope

`apps/lane` owns the shared ReactLynx client and hosts for Android, iOS, macOS, Windows and web. This contract specifies the complete product behavior and preserves the existing platform build interfaces. New product capabilities and paths in the project index are planned requirements, not claims of runtime completion. Formal/store publication remains separate.

## Runtime and Language

Use Node.js 24, TypeScript, ReactLynx 0.129 and Rspeedy 0.18.3. Mobile hosts use
Lynx 4.1.0; Android PrimJS uses 4.1.1. Web uses web-core 0.26.5. Kotlin/Gradle,
Objective-C/CocoaPods and C++17/CMake own native integration. Desktop uses pinned `@lynx-js/lynxtron` 0.0.30 on macOS and Windows. Its npm runtime supplies Lynx; developers do not install a separate desktop Lynx SDK. C++17/CMake builds the Node-API storage and OS adapter using pinned `node-api-headers` 1.9.0. Linux can use the browser build.

## Users and Operators

Individuals manage tasks and prompts; existing agents use MCP. Native and default CLI use are account-free. Operators provide optional synchronization and online web facilities. Maintainers configure signing and platform distribution separately.

## Interfaces and Contracts

The name is `Lane`; the identifier is `io.delino.lane`. `src/index.tsx` registers
one shared root. `pnpm build` emits `dist/main.lynx.bundle`, the browser bundle,
a local browser runtime and license notices. `pnpm build:frontend` is equivalent.

`pnpm dev` injects Infisical development settings and registers the `lane`
Portless route at `https://lane.localhost`, with fixed loopback port 46325 and
conflict rejection. It serves the browser host and shared bundles through Rspeedy.
Native builds embed the local production bundle and do not fetch remote templates.

- Desktop: run `pnpm dev:desktop` from `apps/lane`. After Infisical injection,
  this builds the shared bundle, compiles the Node-API host, packages the pinned
  Lynxtron runtime and starts Lane. `pnpm build:desktop` prepares the application
  without starting it. macOS requires Xcode command-line tools and CMake 3.24 or
  later; Windows requires Visual Studio C++ tools, CMake and the existing Windows
  App SDK headers/projections/bootstrap/license configuration for OS notifications.
  `LANE_LYNX_SDK` is not used. Root `pnpm install` installs the matching runtime
  through the lockfile; optional dependencies and the approved Lynxtron install
  script must remain enabled. The helper does not download a separate Lynx SDK.
  Generated packages are `native/desktop/build/Lane.app` on macOS and
  `native/desktop/build/application` on Windows. Both embed the shared production
  bundle and the bundled native module. The main process owns windows, menus,
  protocol activation and the background native service. Closing a window releases
  rendering and invalidates pending reauthentication without stopping enabled
  synchronization; explicit Quit awaits native shutdown. Each build refreshes
  application resources even when the native module does not relink.
  Local macOS packages use ad-hoc signing; release signing remains separate.
  `LANE_URL_SCHEME=lane` selects distribution links; the default is `lane-dev`.
  The optional `LANE_SYNC_HELPER` is a matching packaged CLI executable with
  adjacent `LICENSE`, `NOTICE` and `THIRD_PARTY_NOTICES`. Otherwise the build uses
  an existing matching `packages/lane/dist/artifacts` executable when available.
  Missing helpers leave standalone storage available and report synchronization
  dependency failures. No globally installed CLI is assumed.
- Android: install JDK 17 or 21 and Android SDK 35. The committed Gradle
  wrapper selects 8.11.1. Run
  `pnpm build:android` for an unsigned release APK. Import `native/android` in
  Android Studio for installation and debugging.
- iOS: install Xcode, XcodeGen and CocoaPods. Run `pnpm mobile:ios:init` to
  generate the Xcode project and install pods after building the shared bundle.
  Run `pnpm build:ios` for an unsigned simulator build. Open the generated
  workspace for device development and signing.

Web output is static Cloudflare Pages content. The shared `release-delino.yml`
workflow builds `apps/lane/dist` and deploys it from each immutable
`delino@vMAJOR.MINOR.PATCH` tag to `lane.delino.io`. Lane keeps its independent
package version. The deployment uses the `delino-site-production` environment,
its shared Cloudflare credentials and `CLOUDFLARE_PAGES_PROJECT_LANE` variable,
with the `release-lane` concurrency group. Builds use the default public API
origin `https://lane.api.delino.io`. Follow the
[web publication procedure](repository-workflow-contract.md#web-publication)
for account setup and retrying an existing tag. Account provisioning, native
signing and store registration remain separate.

### Approved UI composition

Use desktop navigation/list/detail and mobile list/detail pages. Use English, a white/gray surface and blue accent. Keep progress separate from proposal disposition. Provide explicit Save, history, prompt restore, conflict comparison/resolution, and settings for devices, MCP approvals, webhook receivers and per-device notifications. Use an icon-only app rail to the left of the contextual sidebar. Lane, Pace and RealQA are equal app selectors in that order; Pace shows its own sidebar and `Coming soon` page. RealQA follows the [capture and attachment contract](apps-lane-realqa-contract.md). Remove the former Perfine entry. Apply Toss UX guidance, keyboard/screen-reader access, visible focus, non-color state cues and text enlargement. These product constraints do not change the platform build/toolchain choices.

### Application icon

Use the folded-lanes icon for macOS, Windows, iOS, Android, the web favicon,
the in-app Lane selector and the Lane sidebar heading.
The mark consists of a white L-shaped ribbon and a pale-blue parallel ribbon,
with restrained fold shading over a cobalt-blue background centered on `#075AFF`.
Keep the application name `Lane` and existing navigation behavior.

`apps/lane/assets/icon-source.png` owns the approved square artwork. iOS uses an
opaque 1024-pixel asset catalog image. macOS uses an ICNS with a rounded tile and
transparent outer margins; Windows embeds a multi-resolution ICO for the
executable, window and tray. Android supplies density-specific legacy icons,
adaptive icons and a flat monochrome silhouette for themed icons. Keep the
adaptive mark inside the central safe zone. The web host ships an SVG favicon
with flat ribbons for small sizes and a 180-pixel Apple touch icon.

Preserve the unmasked square artwork when deriving platform assets. Apply masks
only to platform variants, and keep both lanes distinct at small sizes. Check
16-, 32- and 64-pixel rendering, platform masking and packaged icon references.
Asset compilation does not establish launcher, Dock, taskbar or device acceptance.

### RealQA mini-app icon

RealQA uses a white checklist with two blue checkmarks and an overlapping white
mouse cursor on a cobalt-blue background. Pale-blue folded edges match the Lane
application icon. This represents assistance for human-led QA.

`apps/lane/assets/realqa-icon-source.png` owns the approved square artwork.
`apps/lane/assets/realqa-icon.png` is its 128-pixel runtime derivative, embedded
as a data URI so packaged native hosts need no remote image request. Regenerate
it with `sips -z 128 128 apps/lane/assets/realqa-icon-source.png --out apps/lane/assets/realqa-icon.png`.
Use the same artwork in the 24-pixel app rail and sidebar marks and the scaled
coming-soon mark, with rounded corners. Keep accessible names, selection markers,
keyboard behavior and navigation unchanged. This does not change Lane platform
icons or implement RealQA QA functionality.

### Pace mini-app icon

`apps/lane/assets/pace-icon-source.png` owns the approved Pace artwork: a white
and pale-blue folded-ribbon speedometer on a cobalt-blue square background.
`apps/lane/assets/pace-icon.png` is its 256-pixel runtime derivative, embedded
as a data URI in the shared bundle for account-free native and web use. Use the
same artwork in the app rail, contextual heading and coming-soon page. Keep the
24-pixel navigation footprint, rounded corners, selection marker and accessible
app names. This artwork does not replace the Lane launcher icon.

### Product contract ownership

The following behavior is authoritative for all Lane entry points. The linked domain contracts specify implementation interfaces and conformance, without narrowing these requirements.

### Product boundaries and platforms

- Keep the existing shared ReactLynx application and platform hosts. Native apps target macOS, Windows, iOS, and Android. Linux has the CLI/local MCP and the online web app, not a native GUI.
- Native apps and the default CLI use local data without login. Optional synchronization and server webhook facilities require the existing Logto identity system. **The `lane-api` server must depend on `delcore-api`.** Integrate through Delcore's Connect API and follow the identity, tenant, and authorization ownership defined in `docs/project-delcore.md`, `docs/servers-delcore-api-contract.md`, and `docs/protos-delcore-v1-contract.md`.
- The web app requires login and server connectivity and reads/writes server data. Do not implement independent browser persistence, offline editing, offline relaunch guarantees, or an offline mutation queue for web.
- Isolate each user's server data. Identical repository URLs in different accounts do not share tasks. No team workspace or collaboration model is included.
- Provide the first version free of charge. No paid plans or billing. No feature flag is needed because this is a new product made available to all logged-in users on the server; local use remains account-free.
- Web domain: `lane.delino.io`. API domain: `lane.api.delino.io`.
- Register `lane-dev://` for development builds and `lane://` for distribution builds. Support login callbacks, opening the app, and links to a repository or task. Do not include decision-item links.
- Keep Lane, Pace and RealQA as equal, enabled app selectors in a 64-pixel icon rail, ordered from the top. Use the approved folded-lanes artwork for Lane, the folded-ribbon speedometer artwork for Pace, and the approved checklist-and-cursor artwork for RealQA. Each target is 44 by 44 pixels with a 24-pixel icon and 12-pixel spacing. Show app names on hover/focus and expose accessible names and selected state. Use a pale-blue background, blue icon and left marker for selection.
  Tooltips must fit the complete app name outside the trigger's bounds.
- At widths of at least 900 pixels, place the 240-pixel app-specific sidebar between the rail and content. Lane retains its navigation, repository list and connection status. Pace and RealQA each show their own name, icon and selected `Overview` item, without Lane navigation. Pace retains `Coming soon`; RealQA shows the capture/settings or read-only entry defined in its owning contract. Below 900 pixels, retain the left rail and show contextual navigation above the content, with scrollable content and wrapping text.
- Unimplemented mini-apps show their icon and name, `Coming soon`, `<App> is not available yet.`, and `Back to Lane`, without requiring login, connectivity or a permission prompt for that page. Both the Lane selector and return button restore the previous Lane page and filters. Keep Lane's loading, error, authentication and permission behavior unchanged. Do not add release dates or sign-up flows. The former Perfine navigation entry is removed.
- Apply the existing unsaved-edit confirmation before switching apps. Cancel retains the current screen and draft; confirming discards the unsaved draft. App switching must support keyboard activation, visible focus and content-focus transfer. RealQA capture, composer, settings and saved-result behavior follow the [RealQA contract](apps-lane-realqa-contract.md).
- User-facing app and CLI text is English only. Validate keyboard operation, screen readers, visible focus, state distinctions that do not rely on color, and text enlargement on every released GUI platform. Follow repository form-focus and Escape-to-close rules where applicable.

### Repositories

- Identify repositories by normalized URL host and repository path. Treat HTTPS/SSH URL forms and a terminal `.git` difference as equivalent. The URL is an organizational identifier: do not fetch repository contents or verify Git-host access.
- When task creation names an unregistered repository URL, register it and create the task.
- Default the displayed name to URL host/path; allow a separate user-defined display name.
- Allow URL changes while retaining tasks, history, and webhook connections. Reject a change to an already registered URL. Do not retain the old URL as an alias: a later creation request using it registers a new repository.
- Treat concurrent repository name/URL edits as a repository-level conflict. Preserve both versions for resolution through app, CLI, or MCP.
- Provide repository-specific task lists, progress/proposal filters, title/description search, and an all-repository task list.

### Tasks, proposals, prompts, and history

- A task has a required title, optional human-readable description, one optional prompt, progress state, and a separate proposal disposition. A title alone is sufficient for creation. Do not add priority, labels, or due dates.
- Progress states are To do, In progress, Blocked, Done, and Canceled. All transitions are allowed, including reopening Done/Canceled tasks. A creation request can set progress; omission means To do.
- Agents can create regular tasks directly, create proposed tasks, and modify existing tasks. Accepting a proposal makes it a regular task. Preserve rejected proposals and allow later acceptance. Accepting or rejecting does not reset progress.
- Apps, CLI, and MCP provide full management, including task creation/editing, progress changes, proposal acceptance/rejection, deletion, prompt editing/restoration, repository management, and conflict resolution.
- Keep prompt versions indefinitely until their owning task/repository is deleted. Users can inspect a past version and restore it; restoration creates a new version. A diff-view feature is not required.
- Preserve user-visible changes to title, description, progress, proposal acceptance/rejection, and repository moves. Show before/after values, time, and change origin: app/CLI/MCP, device name, and account identity when logged in. Do not introduce agent-supplied display identities. Only prompts have a past-version restoration action.
- Allow moving a task between repositories while preserving its ID, prompt, and history. A linked webhook event stays in its original repository and continues to reference the moved task ID.
- Store descriptions and prompts as Markdown source. Render Markdown in the app; allow source editing and copying. Do not execute embedded HTML. Automatically load images only from HTTPS URLs.
- App edits require explicit Save; saves create versions. If external changes arrive during editing, retain unsaved input, indicate the external change, and check conflicts when saving.
- If local storage is full or saving fails, retain unsaved input on the current screen and provide an error/retry action. Do not show successful save/synchronization. In web, connectivity loss blocks saving but retains current-screen input; reconnection requires an explicit retry, not an offline queue.

### Local storage, synchronization, and conflicts

- Native apps and CLI use host-side SQLite. ReactLynx accesses storage through platform Native Modules. Desktop app, CLI, and local stdio MCP share the same local data; CLI/MCP continue to work when the app window is closed.
- Use PostgreSQL for server synchronization data, retained events, and server state. Remote MCP can read/write synchronized data when every user device is off; devices receive these changes on reconnect.
- Synchronize all repositories, tasks, prompts, and history, not a user-selected subset. Notification enablement/display preferences are device-local exceptions.
- Initial synchronization preserves both datasets. Merge repositories with the same normalized URL; preserve tasks with different IDs separately. Conflicting versions with the same ID require resolution.
- Preserve both versions of concurrent task edits and require explicit resolution. The conflict unit is the **whole task**, including edits to different subfields. Until resolution, that task is read-only outside the resolution operation.
- App, CLI, and MCP can compare and resolve conflicts. Preserve both conflicting versions and the resolution as history; the selected/edited resolution is a new version.
- If multiple devices resolve the same conflict differently, keep the resolution first accepted by the server. Reject a later stale resolution and require reading current state again.
- Desktop supports background synchronization after the app window closes. Mobile synchronizes while running and at platform-permitted background opportunities. Web uses its active online connection; do not imply an offline data mode.
- Use server change notifications for live updates. Local process changes must reach the running app without requiring an app restart; verify this during implementation.
- Native/CLI connectivity failure or expired Logto authorization does not block local work. Preserve pending changes; resume synchronization after reconnection/reauthentication.
- Unsupported synchronization format stops synchronization with an update instruction, while compatible local features and pending changes remain available. An older app must not open/write an unsupported local data format or initialize it as empty; require a supported version.
- Operators must configure server input-size and request-rate limits before startup. Provide no default limits; missing configuration fails startup. Do not apply these server limits to standalone local input. An oversized upload blocks only that item with an explanation; other items continue synchronizing. Lowering limits must not block reading/downloading/deleting previously accepted data; reject only oversized new or modified uploads.

### Authentication, connection management, and deletion

- Use existing Logto for online identity. `lane-api` depends on `delcore-api` for shared tenant authorization under the existing Delcore contracts; Logto remains the identity provider. CLI/remote MCP connect through browser login approval. Local stdio MCP does not require a server login.
- Every approved remote MCP connection has full management access to all repositories in its account. Provide an approved-connection list and individual access revocation in app/CLI. Enforce revocation on server access.
- Provide device list, last synchronization time, and device disconnection through app/CLI/MCP. Disconnection prevents later server access; it cannot erase an offline device's existing local data.
- Isolate local data by OS user/browser profile as applicable. Do not add a Lane password/biometric app lock. Account switching must keep retained data from an earlier account in a separate local space and must not upload it to the new account.
- On logout or disabling synchronization, let the user keep local data or remove it from that device.
- Delete task/repository content and history permanently; no trash or recovery. Deletion wins over concurrent offline edits. Remove content/history immediately from connected stores, retaining only identity/deletion-version records needed to stop resurrection and propagate deletion when a device reconnects.
- Separate these actions: remove this device's data; delete server and connected-device data; delete server synchronization data and webhook receiving settings while retaining local data. Keep the shared Logto identity in all cases.
- When deleting server data while retaining local data, disconnect **all devices** to prevent automatic reupload. Restarting synchronization requires explicit reconnection.
- For individual task/repository deletion, the app asks for confirmation; a CLI delete command or MCP delete call is itself confirmation. Server-wide/account-data deletion requires explicit target-account confirmation and Logto reauthentication.
- Task creation uses a request ID. Same ID/content returns the existing result; different content under the same ID is an error. Replaying creation after the task was deleted returns a deleted-result indication, with no body and no recreation.
- The server may process synchronized content: apply access controls and encryption in transit/at rest. End-to-end encryption is not required. Do not log task/prompt content, webhook payloads, or authentication secrets.

### Webhooks and agent-driven automation

- Support only the Lane common webhook format: required sender event ID, event type, occurrence time, and JSON object payload. No GitHub-specific adapter is required.
- Each receiving endpoint belongs to a repository. Changing its repository affects only future events. Retained events remain in their original repository.
- The server only receives, retains, and synchronizes events. A user's existing agent reads them through MCP and decides whether to create tasks. Lane does not interpret events with AI, execute agents, or add a user-rule automation engine.
- App, CLI, and MCP manage receiving settings and list/delete events. Retain events until explicitly deleted.
- Require a secret-key signature over request content and timestamp. Accept timestamps within five minutes in either direction of server time. Retries use the same event ID with a fresh timestamp/signature. A rotated key invalidates the prior key immediately. Show secrets once on creation/rotation; loss requires rotation, not retrieval.
- Deduplicate by receiving configuration and sender event ID. Same ID/content returns the prior result; changed content is an error. Retain deletion identity records so replay of a deleted event reports already received/deleted and does not store it again.
- Reading does not mark processing complete. Agents explicitly complete events and link zero or more generated task IDs. Zero tasks is a valid outcome.
- Processing requires an online server lease. A lease lasts five minutes and can be renewed before expiry for five minutes from renewal. Require a valid lease token for completion; reject expired/released tokens even if no new worker has claimed the event.
- Atomically validate the lease, create resulting tasks, and mark event completion. Failed completion must not leave partial generated tasks. Preserve request deduplication for response-loss retries.
- Agents can release a lease with an error reason for immediate reclaim. A disconnected agent's lease becomes reclaimable on expiry. Offline devices can read retained events but cannot acquire leases or complete processing offline.
- Allow reopening completed events while preserving prior processing records and task links; the event can then be claimed again.
- Deleting an event or its owning repository invalidates its lease and rejects later completion/task creation from that lease.
- Deleting a receiving setting stops new intake but retains existing events and permits existing leases, completion, and reprocessing.

### Notifications

- Provide in-app pending-proposal indications and counts. Only a **new proposal created through MCP** triggers an OS notification. App/CLI changes do not trigger notifications. No task-progress or decision-item notifications.
- Without synchronization, use local notifications for local MCP proposals. With synchronization, notify other eligible devices through server push. Deduplicate by proposal ID/device ID so the origin device does not receive the same proposal notification twice.
- Request OS notification permission only when the user enables notifications in settings. Configure enablement and content per device. Default content shows repository name and task title; users can select a generic notice instead.
- Direct provider integration: APNs for Apple, FCM for Android, and Web Push for web. **Windows remote WNS delivery is deferred**; Windows supports running/background-client synchronization and local notifications. Do not add WNS package-registration requirements to this version.
- Unsupported remote-push environments, including Android without required Google Play services, can still use Lane. Explain the limitation and retain in-app indications and available local notifications.
- Persist pending push deliveries. After an initial transient failure, retry at intervals of 30 seconds, 2 minutes, 10 minutes, 30 minutes, and 60 minutes, at most five retries. Expire delivery two hours after creation. Respect provider Retry-After; do not retry earlier or beyond expiry. Stop on permanent failure. Cancel unsent notifications if the proposal is accepted, rejected, or deleted.
- Provider connection code, configuration instructions, and mocked tests are in scope. Actual provider registration, account/device delivery acceptance, store publication, and production deployment are not required here.

### CLI, MCP, and repository integration

- CLI defaults to local operations; only an explicit remote option targets server data. Default output is human-readable; `--json` emits structured results/errors. Provide macOS/Windows/Linux executables and the npm installer package **`@delinoio/lane`**, with installation instructions.
- Local MCP uses stdio. Optional remote MCP uses browser-approved Logto authentication. Both expose the agreed management functions, with online-only server operations identified explicitly.
- Supply a common Lane `SKILL.md` and remote/local MCP connection instructions. Do not run or distribute an AI execution service.
- Preserve the Lane project ID/application ID and existing platform/toolchain contracts. Follow repository defaults for new server implementation (Rust), other unspecified new projects (Go), UUID v7 entity IDs, and Connect RPC business interfaces. MCP and webhook adapters must preserve the same ownership, version, deletion, and idempotency rules.
- Keep client, protocol, server, CLI, and distribution work in the same repository. Before runtime expansion, update `docs/project-lane.md`, the application foundation, and the documentation catalog; add owning server/protocol/CLI/distribution contracts and required scoped ownership instructions. Do not place issue-specific requirements or issue identifiers in repository instruction files.
- Follow repository license, environment, dependency, generated-output, formatting, and validation contracts. Retain existing PR/main/affected CI platform restrictions, including no new macOS PR runners.

### Operations and observability

- Produce deployable web/API/remote-MCP artifacts, configuration, and runbooks for the specified domains. Do not perform actual production deployment. Native completion requires build/install/behavior validation; store publication and formal distribution are follow-up work.
- Run database migrations explicitly, never on server startup. Roll back only to a binary compatible with the current schema; otherwise recover with a corrected forward version. Do not require down migrations.
- No numeric latency, capacity, or availability SLO is promised. Measure response time/error rate and test functional recovery. Alert thresholds below are operational triggers, not SLOs.
- Include structured logs, request correlation IDs, health checks, basic metrics, dashboards, and automatic alerts. Record operational metadata, not user content or credentials. Server logs expire after 30 days. Local native/CLI logs remain local, expire after 30 days, and have a combined 100 MiB limit; remove oldest entries when either limit is reached. No automatic diagnostic upload.
- Supply Prometheus rules and Alertmanager email routing/configuration. Operators configure email transport and recipients. No Lane-owned email provider integration is required.
- Service and DB health: probe every 30 seconds with a five-second timeout; alert after five minutes of continuous failure; resolve after five minutes continuously healthy.
- API: over a rolling five-minute window with at least 20 requests, alert if server errors/timeouts exceed 5% or p95 response time exceeds two seconds continuously for five minutes. Exclude long-lived connection lifetime from request-latency measurement.
- Push: alert when transient failure rate exceeds 10% over a 15-minute window with at least 20 delivery attempts, continuously for five minutes; also alert when the oldest pending delivery exceeds ten minutes.
- Resolve API/push alerts after the triggering condition clears continuously for ten minutes. Insufficient samples alone do not prove recovery; an empty push queue clears its age condition. Send email once on firing, once per hour while active, and once on recovery. Make these numbers operator-configurable with the agreed initial values.
- Document user troubleshooting and operator procedures for server/DB failures, synchronization, webhooks, push delivery, and alerts. No separate customer-support/admin diagnostic product is required.

### Explicit exclusions

- Decision items in their entirety: no question/answer records, choices, answer history, deferral/reopening/acknowledgment, decision-specific UI/API/MCP functions, links, or notifications.
- Lane-driven AI calls, agent execution, Git content access, Git access verification, event interpretation by Lane AI, user-rule automation engines, and provider-specific webhook adapters.
- Team workspaces, billing/paid plans, feature flags, app password/biometric locks, end-to-end encryption, independent offline browser data, and Linux native GUI.
- File import/export, separate user backup/restore, trash/recovery, priority/labels/due dates, old repository URL aliases, mini-app implementations, and a separate customer-support tool.
- Windows remote WNS push and its package-registration work, actual production deployment, store publication/formal release, and actual Logto/push/email account/device delivery validation.

## Storage

Use the [shared local-store contract](apps-lane-local-store-contract.md) for native persistence and the [server contract](servers-lane-api-contract.md) for online web and synchronization. Web has no independent persistent task store. `dist`, native build output, generated Xcode
projects, CocoaPods and local SDK paths remain ignored. Native packages embed
only the compiled template, required runtime resources and license notices.

## Security

Add only the network, callback and notification capabilities required by the product behavior above. Keep SDK downloads, credentials and signing material outside tracked source. Native callbacks use `lane-dev://` in development and `lane://` in distribution; accept only validated callback state and repository/task identifiers.
Development commands follow the root Infisical contract. SDK libraries and
notices must remain together when packaging native applications.

## Logging

Build helpers emit structured start, completion and failure events. Native hosts
report startup and template-load failures without user content. Platform runtime
logs remain available through the platform's development tools. Apply the content exclusion, retention and shared size limit specified above.

## Build and Test

Run `pnpm test` in `apps/lane` for TypeScript checks and both shared bundle builds.
Affected frontend CI runs on Ubuntu. Native compilation, launch, resizing,
lifecycle, signing, installation and store submission need separate platform
validation. No macOS runner is added to PR, main-push or affected manual CI.
Remove generated `dist` after validation.

Required product tests cover all task/proposal transitions, optional prompts, explicit-save history, prompt restore as a new version, task moves, repository normalization and conflicts, safe Markdown, and input retention on errors. Test keyboard operation, screen readers, visible focus, non-color states and text enlargement on every released GUI platform.

Native acceptance must exercise SQLite reads/writes, close/reopen, failed writes, concurrent CLI/MCP access while the window is closed, and automatic observation in a running app on each relevant host. Android/Windows execution and automatic change observation remain acceptance gates. Earlier isolated macOS/iOS probes do not establish product completion. Do not infer web offline support from an IndexedDB experiment.

Use the shared protocol vectors for sync, deletion, replay and conflict behavior. Use disposable PostgreSQL and mocked Logto, Delcore, push and email dependencies for integrations. Actual external accounts/providers, production deployment and store/formal release acceptance are excluded from automated acceptance claims.

## Dependencies and Integrations

The ReactLynx toolchain versions match the existing Lynx UI workspace. Use shared UI primitives where they meet the approved design and platform accessibility requirements. Native SDKs are platform dependencies, not
vendored source. Desktop resolves its pinned Lynxtron runtime through npm and retains the runtime license files in the packaged application. Android resolves Maven artifacts; iOS resolves CocoaPods. Preserve original third-party licenses. `native/licenses` retains the Lynx 4.1.0
license and notice for the earlier Lynx SDK; Lynxtron desktop packages retain their own upstream notices without replacing them with those earlier files. Mobile dependency managers retain
their dependency notices; include them in signed release packaging.

## Change Triggers

Keep this contract, the project index, documentation catalog and affected CI
selection aligned. Update instructions only when ownership or development rules
change. Record validation limits in PR and CI evidence.

## References

- [Project index](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [Environment ownership](repository-environment-contract.md)
- [Official Lynx integration](https://lynxjs.org/guide/start/integrate-with-existing-apps.html)
- [Lynxtron](https://lynxjs.org/lynxtron/)
- [Lynxtron communication](https://lynxjs.org/lynxtron/learn/communication-between-node-and-lynx)

## RealQA extension to the foundation

The [RealQA contract](apps-lane-realqa-contract.md) replaces the RealQA placeholder
and earlier exclusion of its mini-app implementation. It owns the approved
start/settings, floating composer/editor and read-only attachment views. Keep
Pace's placeholder, approved artwork and existing Lane navigation behavior.
The RealQA contract also defines the Chrome Web Store publication requirement
and the explicit Windows installation/runtime deferral.

### Desktop capture packaging and local validation

The desktop build also compiles `lane-chrome-host`, a standalone relay launcher.
It bundles the official Node 24.22.1 security release and its original license
next to the relay module. macOS packages also include the patched Node 22.23.4
compatibility runtime for macOS 12 through 13.4, because the official Node 24
binary requires macOS 13.5. The launcher selects the compatible runtime. Both
versions address the [October security release](https://nodejs.org/en/blog/vulnerability/october-2026-security-releases);
artifact checksums establish download integrity, not absence of vulnerabilities.
This private runtime does not update Lynxtron's separately embedded runtime. The launcher starts only this private runtime, never
Lynxtron or the Lane application. `LANE_CHROME_EXTENSION_IDS` supplies exact,
comma-separated development or published extension IDs at build time. An empty
value leaves Chrome registration and the connection service disabled; it does
not produce a placeholder store listing.

From `apps/lane`, `node scripts/desktop.mjs install` installs the generated Lane
application in the current user's application directory and registers the
packaged relay for its configured extension IDs. It refuses to replace an
existing installation. After quitting Lane, `node scripts/desktop.mjs uninstall`
removes the matching registration and application; it preserves Lane user data.
Registration removal verifies its owner so an obsolete installer cannot remove
a newer installation's registration. The existing Windows signing/runtime
acceptance boundary still applies.

Use a `LANE_NATIVE_TESTING=1` build with an absolute `LANE_NATIVE_TEST_ROOT` for
native tests. The opt-in `LANE_NATIVE_TEST_CAPTURE=1` probe requires that scratch
root. It checks existing capture permission without granting it, captures actual
OS sources in memory, edits the result and saves/retrieves a fully covered small
fixture through the native store. It does not establish human selector,
permission-dialog, accessibility, Chrome or publication acceptance. CTest also
provides the synthetic memory renderer and reauthentication tests; desktop
controller tests run with `node --test native/desktop/realqa.test.mjs`.

For isolated macOS install/removal checks, set `LANE_NATIVE_TESTING=1` and an absolute
`LANE_NATIVE_INSTALL_TEST_ROOT`. Only a native-testing package can enter this
scratch installation path. Windows rejects this test-root option for both install
and uninstall before filesystem or registry changes, because relocating a
manifest does not isolate HKCU. Its macOS application and Native Messaging registration
remain under the scratch root. Use an explicitly identified test extension ID
for these fixtures and record them separately from actual Chrome installation
and published-extension acceptance. Production builds disable native test-root
injection. Keep Windows compilation evidence separate from its deferred runtime
checks, and retain the repository CI platform restrictions.
