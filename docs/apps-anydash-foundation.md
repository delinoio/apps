# AnyDash Application Foundation

## Scope

`apps/anydash` (`implemented`) owns the shared ReactLynx client and its hosts: the browser host for `anydash.delino.io`, the macOS and Windows Lynxtron desktop hosts, the iOS/iPadOS and Android Lynx SDK hosts, and the native widget hosts, including separate native widget hosts when the Lynxtron packaging or a Lynx SDK host cannot embed a widget extension. This contract specifies product behavior, platforms, UX and client tests. The [protocol contract](protos-anydash-v1-contract.md) owns the interfaces; the [CLI contract](cmds-anydash-cli-contract.md) owns the collector that the desktop app embeds. Native acceptance stays with the release gates below.

## Runtime and Language

Use Node.js 24, TypeScript, ReactLynx and Rspeedy, pinned to the versions used by the Lane application when implementation begins. Mobile hosts use the Lynx SDKs (Kotlin/Gradle on Android, Swift or Objective-C on iOS). Desktop hosts use pinned `@lynx-js/lynxtron` on macOS and Windows. The web host uses Lynx for Web, built with the Rstack toolchain, and deploys to Cloudflare Pages. Native widget hosts use the platform language the OS requires: Swift (WidgetKit) on macOS and iOS, C# in an MSIX package for the Windows Widgets Board, Kotlin on Android. TypeScript/ReactLynx and these platform languages are the UI exception to the repository language defaults. Clients use Connect RPC through `@connectrpc/connect-query` with React Query and the `@repo/protos` TypeScript bindings. Shared UI primitives come from `packages/lynx-ui` when they fit.

## Users and Operators

Solo business owners (primary), Delcore team members who view and edit shared team dashboards, and the maintainer, who approves the design and performs release validation.

## Interfaces and Contracts

### Platforms and support matrix

| Surface       | Scope                                                                                                                                                                                             |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Web           | Latest 2 major versions of Chrome, Edge, Firefox and Safari. Static deployment to Cloudflare Pages at `anydash.delino.io`.                                                                        |
| macOS app     | macOS 14+, arm64 and x64. ReactLynx through Lynxtron. Acts as a collector.                                                                                                                        |
| Windows app   | Windows 11 x64. Windows 11 arm64 ships only if Lynxtron supports a native arm64 build. Otherwise there is no Windows arm64 app, and those users get the CLI and the web app. Acts as a collector. |
| iOS / iPadOS  | iOS 16+. Phone and tablet, with a responsive wide layout on tablets. Viewer only.                                                                                                                 |
| Android       | Android 10 (API 29)+. Phone and tablet, with a responsive wide layout on tablets. Viewer only.                                                                                                    |
| Linux         | No native GUI and no Plasma widget in the first release. Linux users get the CLI collector and the web app.                                                                                       |
| CLI collector | macOS 14+ arm64/x64. Windows 11 x64/arm64. Ubuntu 24.04 and 26.04, Debian 12 and 13, each x64/arm64. Owned by the [CLI contract](cmds-anydash-cli-contract.md).                                   |

- The clients are one shared ReactLynx application. The UI is English only; localization beyond English is out of scope. The user selects a time zone (`UpdateAccountSettings`), which every dashboard view uses.
- Lynxtron targets only macOS and Windows x64, and Linux UI is not planned upstream: [Lynxtron announcement](https://lynxjs.org/next/blog/lynxtron.html), [Lynx 2026 roadmap](https://lynxjs.org/next/blog/lynx-open-source-roadmap-2026), [lynx-family/lynx discussion 852](https://github.com/lynx-family/lynx/discussions/852). Windows arm64 support and native widget extension packaging are unverified; the decided fallbacks are no Windows arm64 app and separate native widget hosts. The release is not reduced in scope.

### Development and routes

- `pnpm dev` in `apps/anydash` injects Infisical development settings (default path `/`) and registers the Portless route `anydash` at `https://anydash.localhost` with fixed loopback port `127.0.0.1:46330`, failing on conflict. `pnpm dev:desktop` builds and launches the desktop host. Register the route in the [workflow contract](repository-workflow-contract.md#named-local-development-urls) when runtime begins.
- Web routes: `/` (space and dashboard home), `/connect/<enrollment_id>` (device approval), `/spaces/<space_id>/dashboards/<dashboard_id>` (dashboard, optional `widget=<widget_id>` query to focus a widget), `/settings` (time zone, devices, connections, activity, data deletion).
- Native deep links: `anydash://spaces/<space_id>/dashboards/<dashboard_id>?widget=<widget_id>` and `anydash://connect/<enrollment_id>`. Native widgets open the first form.

### Authentication, spaces and onboarding

- Logto sign-in is required on every platform. Clients use Logto authorization code flow with PKCE and request two access tokens: one for the AnyDash API resource and one for the Delcore API resource. After sign-in the client calls `ConnectAccount` with the Delcore token, then `ListSpaces` with a fresh Delcore token whenever it needs current team memberships. The platform host, not the shared ReactLynx bundle, obtains the Delcore token and sets `delcore_access_token` in these two requests, so neither token crosses the host bridge.
- The personal space is the user's Delcore personal tenant. Team spaces are Delcore organization tenants in which the user has the Owner, Admin or Member role. Billing-only users do not see the team space.
- Personal space onboarding starts with connecting a collector device. The personal dashboard is usable only after a collector device is connected (`OnboardingState` `READY`); until then the personal space shows the connection steps: install the desktop app (macOS, Windows) or the CLI, then approve the device. Team space dashboards are usable without a personal collector device.
- Device enrollment: the CLI shows a connection code and a URL; the desktop app starts the same enrollment through its collector and opens the same URL in the browser. The approval page (`/connect/<enrollment_id>`) shows the device name, platform and collector version and asks the user to type the code shown on the device; the code input receives focus. It shows the remaining attempts, expiry, invalidation after 5 failures and a "request a new code" instruction. A code expires after 10 minutes and can be used once.
- Logout on a collector device deletes the local device credentials, stops collection and upload and removes the native widget snapshots stored in the collector. The server keeps the device registration and its data. Signing in again reattaches the same device.

### Dashboards, widgets and team permissions

- Each space can have many dashboards. Users create, rename and delete them within the quotas (50 dashboards per space, 100 widgets per dashboard). Quota errors show guidance.
- Users add and remove widgets from the provided widget types (`WidgetType`). Custom widget types are out of scope. Users edit widget position, size, title and presentation on a 12-column grid; the editor supports keyboard and screen-reader alternatives to drag and resize.
- Team permissions: Owner and Admin can create, delete, edit and view team dashboards. Member can edit and view. Users who have only the Billing role, or no role, have no access. The client hides controls that `Space.permissions` does not grant, and the server enforces them.
- Personal connections and local usage are shared into a team dashboard only as the display data of widgets the owner added. Team members cannot access the source connection or any other data. Adding a widget to a team dashboard shows a confirmation that names the shared source scope.
- On another user's shared widget, team members can co-edit only position, size, title and presentation. Only the data owner can change the query period, filters and sharing scope (`caller_is_data_owner`). Team-role edit permission does not bypass this rule.
- Saving a dashboard checks its version. On a concurrent-edit conflict, the save is rejected; the client loads the latest version and asks the user to edit again. It never merges silently.
- When the owner stops sharing or leaves the team, server access is revoked immediately. Apps and native widgets delete the cached data of that widget at the next launch or refresh and show a "sharing ended" state. Offline clients can keep showing old data until the next launch or refresh. Data already displayed, or copies the user saved separately, cannot be revoked.

### Metrics and states

- Each metric shows its value, "Not provided", or a failure state with the last successful value and its time. "Not provided" is visually distinct from a temporary collection failure.
- Subscription remaining is shown only as `100% − provider-reported used percentage` (`SUBSCRIPTION_REMAINING_PERCENT`). Absolute remaining amounts show "Not provided".
- External costs show the provider's original currency; local estimated costs show USD, labeled "estimated" and separate from subscription fees and actual bills. "Cannot be calculated" and partial costs are shown as such.
- Connection states show "authentication expired" and the reconnect action; local CLI connections explain that polling resumes automatically after signing in to that CLI again.
- Local usage shows "partial aggregation" with the skipped record count and an update-the-collector instruction, and the duplicate-device notice whenever data from two or more devices is combined.
- The Vercel connection screen warns that the token can write, because Vercel has no read-only personal token, and that AnyDash calls only read endpoints.
- Data refreshes while visible: dashboard data queries every minute, with manual refresh limited to once per minute per connection or device and the remaining wait shown.

### Native widgets

- Native widgets: the macOS desktop and Notification Center, the Windows Widgets Board, and the iOS and Android home screens.
- A native widget shows a summary (`WidgetSummary`) of one in-app widget that the user selects, and its last refresh time. Tapping it opens the matching dashboard in the app. Editing happens only in the app; editing from native widgets is out of scope. A native widget refreshes on the cadence the OS allows.
- Desktop: the desktop app's main process stays resident after the window closes and writes snapshots of the selected widgets to the collector with `PutNativeWidgetSnapshots` after each data refresh; widget hosts read them with `ListNativeWidgetSnapshots`. The Lynxtron packaging cannot embed a macOS WidgetKit extension or a Windows Widgets Board provider, so both are separate native hosts that read the local collector process: `native/widgets/macos` is a Swift WidgetKit extension in an interface-less containing app (macOS 14+), and `native/widgets/windows` is a C# COM server in an MSIX package (Windows 11 x64). The macOS extension is sandboxed, as WidgetKit requires, and reads the collector socket through a read-write exception for the AnyDash data directory; the collector still checks the peer user ID. Without a running collector, both keep the last snapshot they read with its time.
- Choosing the widget: iOS 17+ and macOS use a WidgetKit configuration intent, Android uses the widget configuration activity, and Windows uses Widgets Board customization. iOS 16 shows the most recently refreshed snapshot.
- Mobile: the app writes snapshots to the app group (iOS) or app-private storage shared with the widget provider (Android). When the OS lets the widget refresh in the background, the widget extension calls `QueryWidgetData` with the app's stored session (`CLIENT_KIND_NATIVE_WIDGET`) and writes the result back to the stored snapshot, so an `ENDED` state stays without data at later refreshes. The write happens only while the stored snapshot is still the one that started the request: a sign-out, a full replacement or a newer app-written snapshot (such as `ENDED`) during the request wins, so a late response never restores a signed-out user's data or older data. Without a background refresh, the widget shows the latest snapshot and its time.
- `ENDED` snapshots delete displayed data and show "sharing ended".

### Desktop collector integration

- The desktop app embeds the matching `anydash` executable and connects to the shared collector through local IPC; it starts the embedded collector when none runs. The app and the CLI never collect twice.
- Closing the app window does not stop collection; the app stays in the menu bar (macOS) or notification area (Windows). An explicit Quit stops the app and the collector it started. Starting at login is opt-in in settings.
- Settings expose devices (list, delete, set preferred device), connections (add with credential, register this device, remove, delete), session paths, key rotation, the collector status fields and an "about" view with versions.
- Desktop updates: installers and a signed auto-update feed are published on GitHub Releases. The app installs an update only after its signature verifies and the user confirms.

### Account data, errors and support

- The app has an in-app "delete AnyDash data" action, which deletes all of the user's AnyDash data after confirmation. It links to the Delcore/Delino account withdrawal flow for deleting the account itself.
- Error screens show the correlation ID, device ID, connection ID and collector version where applicable and let the user copy them.
- Support links point to public GitHub Issues with a warning not to post personal data, and to GitHub private vulnerability reporting for security reports.
- "Update required" blocks server features with an update instruction; desktop collection continues locally.

### UX requirements and design approval

- Follow Toss Design Guidelines. Dialogs close with `Esc`. A form with a single critical input focuses that input.
- Accessibility: WCAG 2.2 AA on the web. VoiceOver, TalkBack and Narrator support, and keyboard operation in the apps.
- Before UI implementation, the user approves a design preview of the empty, loading, access-denied, error, recovery and confirmation states and the final English copy. This copy includes "Not provided", "sharing ended", "authentication expired", "partial aggregation", "cannot be calculated", and the duplicate-device notice. Implementation does not start before this approval.

## Storage

Clients keep a React Query cache and, on native platforms, a persisted cache of the latest dashboard data and native widget snapshots, keyed by user and widget. Logout and `ENDED` sharing remove the affected cached data. Clients never store external provider credentials except through the collector. Session tokens use the platform secure store (Keychain, Android Keystore, Windows Credential Manager) and browser storage defined by the Logto SDK.

## Security

Authorization is enforced by the server; client permission checks only hide controls. Clients never send ownership fields as proof. Sign-out removes tokens and cached data. The approval page shows the requesting device's details so users can reject unexpected enrollments.

## Logging

Clients log errors with the correlation ID and safe metadata only, never tokens, provider data or usage values. No product analytics or success metrics are collected.

## Build and Test

Run `pnpm test` from `apps/anydash`. Required client tests:

- The dashboard and widget editor, with version-conflict recovery.
- The "sharing ended" cache purge, and the Not-provided, failure, partial and cannot-be-calculated states.
- WCAG 2.2 AA checks on the web and screen-reader checks in the apps.
- Browser matrix tests.
- Tablet layouts.
- Onboarding: personal space blocked until a device is connected; team space without a device.

Native hosts and their checks:

- Desktop: `pnpm build:desktop` builds the Lynxtron app (ad-hoc signed locally) and `pnpm dev:desktop` launches it; `pnpm test` runs the desktop main-process tests (`native/desktop/desktop.test.mjs`).
- iOS: `pnpm build:ios` generates the project with XcodeGen, installs the pinned pods and builds the app and widget extension for the simulator. Public build settings come from `PUBLIC_ANYDASH_API_URL`, `PUBLIC_ANYDASH_RESOURCE`, `PUBLIC_DELCORE_RESOURCE`, `PUBLIC_LOGTO_ENDPOINT` and `PUBLIC_ANYDASH_NATIVE_APP_ID`.
- Android: `pnpm build:android` builds the app and App Widget with Gradle and JDK 17; run `./gradlew testDebugUnitTest` in `native/android` for the decoder and PKCE tests. The same public variables, or Gradle properties, configure the build.
- macOS and Windows widget hosts: `pnpm build:macos-widgets` (macOS with Xcode) and `pnpm build:windows-widgets` (Windows x64 with the .NET 8 SDK).
- `pnpm test:widgets` runs the Swift decoder and macOS collector client tests on macOS and the C# decoder and card tests where a .NET 8 SDK exists. `pnpm fixtures:widgets` regenerates the decoder fixtures from the TypeScript bindings; `pnpm test` checks that they are current.

Release gates performed by the maintainer: app and native widget runs on each supported OS; aggregation from 2 or more devices and automatic collection-device failover; team permissions and share revocation. Bundle builds and fixtures do not establish native acceptance.

## Dependencies and Integrations

`@repo/protos`, `@connectrpc/connect-query`, React Query, Logto SDKs, `packages/lynx-ui`, Lynxtron and the Lynx SDKs, the embedded `anydash` collector, and the AnyDash API. Public documentation pages live in `apps/delino-docs`; see the [distribution contract](packages-anydash-distribution-contract.md#public-documentation).

## Change Triggers

Change the protocol, server, CLI and distribution contracts together for behavior or interface changes. Update public guides with user-visible changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Project](project-anydash.md)
- [Repository defaults](repository-defaults.md)
- [Protocol](protos-anydash-v1-contract.md)
- [Server](servers-anydash-api-contract.md)
- [CLI and collector](cmds-anydash-cli-contract.md)
- [Distribution](packages-anydash-distribution-contract.md)
- [Lane application](apps-lane-foundation.md)
- [Lynx UI](project-lynx-ui.md)
