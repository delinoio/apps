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
  Local macOS packages use ad-hoc signing. The shared Delino tag release also
  publishes macOS Intel/Apple Silicon and Windows x64 desktop archives with the
  matching CLI synchronization helper. macOS archives are ad-hoc signed; Windows
  archives are unsigned and portable. Formal signing/notarization and installers
  remain separate. Follow [binary publication](repository-workflow-contract.md#lane-binary-publication).
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

Web output is static Cloudflare Pages content. The shared `release.yml`
workflow builds `apps/lane/dist` when selected by changesets and deploys the release commit to `lane.delino.io`. Lane keeps its independent
package version. The deployment uses the `delino-site-production` environment,
its shared Cloudflare credentials and `CLOUDFLARE_PAGES_PROJECT_LANE` variable,
with the `release-web-lane` concurrency group. Builds use the default public API
origin `https://lane.api.delino.io`. Follow the
[web publication procedure](repository-workflow-contract.md#web-publication)
for account setup and retrying an existing release commit. The `lane@vMAJOR.MINOR.PATCH` tag records successful component completion. Account provisioning, native
signing and store registration remain separate.

### Approved UI composition

Use desktop navigation/list/detail and mobile list/detail pages. Use English, a white/gray surface and blue accent. Keep progress separate from proposal disposition. Provide explicit Save, history, prompt restore, conflict comparison/resolution, and settings for devices, MCP approvals, webhook receivers and per-device notifications. Use shared vertical tabs to the left of the contextual sidebar and open Lane, Pace and RealQA from the New tab chooser; Pace shows its own sidebar and `Coming soon` page. RealQA follows the [capture and attachment contract](apps-lane-realqa-contract.md). Remove the former Perfine entry. Apply Toss UX guidance, keyboard/screen-reader access, visible focus, non-color state cues and text enlargement. Task Properties, Relationships, list Filters and sort, and repository Labels, Milestones and Cycles management follow the [task planning UI specification](#task-planning-ui-specification). These product constraints do not change the platform build/toolchain choices.

### Workspace tabs

Lane, Pace and RealQA share one vertical tab system on the left. Remove the
separate app icon rail and horizontal tab strip. At widths of at least 900 pixels,
show a 240-pixel sidebar with a Tabs heading, New tab action, scrollable tab rows
and bottom Account & sync and App settings actions. Each Lane tab retains its
contextual sidebar and content. Preserve the English UI, white/gray surfaces,
`#075aff` accent, outline controls and existing app artwork.

Open tasks, new-task drafts, pages, repository lists, Pace and RealQA in the same
tab list. Start with one All tasks tab. Sidebar, command-menu, relationship, event
and deep-link navigation opens the destination or selects its existing tab. A
task has one identity, including after a new draft is saved. Page, repository
and mini-app destinations have distinct identities. New task opens a separate
draft. New tab opens or selects a chooser tab with Lane, Pace and RealQA cards
in that order. Show Tasks and repositories, Coming soon and Capture and review
as their descriptions. Selecting a card replaces the chooser with that
mini-app's destination or selects its existing tab and removes the chooser.
Lane returns to its previous available Lane destination, defaulting to All tasks.
Pace retains its Coming soon placeholder. The command shortcut retains page,
repository and app destinations without using the chooser.

Below 900 pixels, a Tabs button opens the same vertical list in a dismissible
dialog and shows the count, complete titles, selection and unsaved state, with
selection and individual close actions. Keep New tab directly reachable beside
Tabs. Escape dismisses the dialog and restores focus. Stack chooser cards at
390 pixels without horizontal overflow. A repository tab keeps its repository
identity: on mobile, choosing another repository opens or selects that tab;
choosing All repositories selects the All tasks page.

Keep each pane mounted while inactive so its sidebar selection and scroll,
repository, search, filters, sort, list scroll, editor scroll and drafts survive
tab and mini-app switching. Inactive panels must be hidden from both keyboard
navigation and assistive technology. A selected task tab retains its associated
list beside the editor on desktop. `Back to list` selects the corresponding
list tab without discarding the task tab. The editor's `Cancel` action closes
its tab through the same unsaved-change guard and is disabled while saving.
Save is explicit; tab selection never
saves or discards input. Each tab owns its reads, mutations, conflict state and
retry identity: a late response can refresh its owning pane but cannot select
that pane or replace another tab's editor. Closing the pane invalidates its
outstanding read ownership; an already submitted mutation is not rolled back.
The repository pane owns pending planning-operation reports until their promises
settle, even after the planning manager closes. Overlapping old and reopened
managers must keep the pane busy until every operation finishes.
Leaving the Events tab ends a pending event-detail inspection, while retaining
its list filters and receiver draft; a late inspection must not open a dialog
when that tab returns. Subscribe to shared data invalidation once per window.

Tab labels show an icon, truncated title, a separate unsaved dot and close
button. Expose the complete title and `Unsaved changes` in accessible names,
hover/focus tooltips and the open-tabs menu. Use a pale-blue selected surface,
stronger title and blue left rule; distinguish focus from selection. Activation
and close targets are at least 44 pixels. Up/Down and Home/End move focus;
Enter/Space activates. Tab proceeds to the remaining controls and content.
Closing restores focus to the selected adjacent tab: choose the next row,
then the previous row. Closing the final tab opens a clean `All tasks` tab.

Closing an edited tab requires `Discard unsaved changes?`, with `Keep editing`
and `Discard changes`. Escape cancels. Focus the safe action and trap focus in
the dialog, restoring the originating control on cancellation. Disable closing
while its save or other mutation is running; switching tabs remains available.
Normal desktop window close and Quit require confirmation for drafts across all
tabs and must wait for pending mutations. Web refresh/close uses the browser's
available unload confirmation, without promising recovery. Account/data-space
changes, logout and explicit data reset check all tab drafts before initiating
the transition; after a confirmed transition remove old-space panes. An expired
or revoked web session clears its inaccessible panes immediately. Preserve
RealQA's existing capture/settings confirmation and memory-only draft boundary.

Use tab-local loading, empty, error/retry, unavailable/deleted and permission
states. Never erase an existing draft or report Save success on failure, external
change, deletion or lost permission. Preserve conflict comparison, history,
prompt restoration and existing request replay rules. Block app update restart
while any tab has an unsaved draft; leaving the Settings tab cancels an update
before installation as existing Settings navigation does.

At widths below 900 pixels, place existing contextual navigation inside the tab
panel above its page, retaining mobile list/detail behavior. Only the tab strip
scrolls horizontally; selected tabs stay reachable, and `New tab` and the
open-tabs menu stay fixed at its right. Preserve wrapping content and reachable
controls at 1440, 900, 899, 390 and 360 pixels and with enlarged text. Validate
keyboard, focus restoration, screen-reader roles, hidden panels and independent
scroll/draft retention. Fixtures and shared compilation do not establish actual
Android, iOS, macOS or Windows acceptance.

Tabs and unsaved drafts live only in the current window's memory, without
server or cross-device synchronization, automatic saves or restart recovery.
Pace and RealQA remain single mini-app views. Drag reordering, pinned/grouped
or split tabs, detached windows, reopening closed tabs and new global close
shortcuts are out of scope.

### Desktop app updates

Settings begins with an `App updates` card on macOS and Windows. Show the
installed Lane release tag separately from Lane's independent package version.
One `Update Lane` action checks public GitHub Releases, downloads the newest
stable Lane release for the current desktop architecture, verifies it, installs
it and restarts Lane. The action explains the restart before activation. No
account, synchronization connection, external browser or global CLI is required.
Checks run only on explicit activation. Ignore draft/prerelease/unrelated tags;
compare numeric release components and never downgrade. Search the latest 100
GitHub releases; absence of a supported release is an actionable error rather
than a claim that Lane is current.

Use the existing white/gray cards, English text and blue action. Show checking,
download percentage, verification and installation states in the same card.
Disable repeated activation during an operation, show `Lane is up to date.`
when appropriate, and offer `Retry update` after failure. Retain readable,
wrapping text, keyboard activation, visible focus and accessible status/progress
labels. Mobile and web do not show the desktop updater. Development and
native-testing packages show an unavailable explanation and cannot self-update.

Preserve task drafts in their tabs when opening Settings. Block update restart while any tab has an unsaved draft.
Require notification edits to be saved before updating; disable other Settings
mutations while updating. An open RealQA capture draft blocks installation.
Leaving Settings or closing its window cancels before installation, so later
edits cannot be lost to a delayed restart. Normal explicit Quit during download
also cancels installation. The native service must stop successfully before an
installer receives permission to replace files.

The desktop main process owns the update operation. The renderer can only read
status, start or cancel; it cannot choose a URL, filesystem path, version or
executable. Official builds embed the validated `LANE_RELEASE_TAG` and
`LANE_SOURCE_REVISION`. Download only this repository's matching desktop archive
and `SHA256SUMS` over HTTPS. Enforce byte limits, exact download size, checksum
and embedded release identity. Checksums provide transport/artifact integrity;
they do not add publisher signing or notarization to the existing distributions.
Reject unsafe archive paths, links outside the bundle, hard links, device files
and ambiguous sparse payloads. Preserve executable modes and internal macOS
framework symlinks. Discard validated AppleDouble metadata records.

Stage beside the installed app so replacement uses same-volume renames. Copy the
existing bundled standalone Node relay runtime, its license and the installer
outside the app before quitting; no system Node prerequisite is introduced.
The installer waits for the old host to exit and an explicit shutdown marker,
retains the previous app until a replacement window acknowledges startup, and
restores/reopens it after installation or startup failure. Update results survive
restart and appear in Settings. Retain a rollback bundle if restoration cannot
finish; do not delete it during staging cleanup. Never replace local databases,
credentials, preferences or external CLI installations. Store migration remains
an explicit, separate operation. A non-writable installation reports failure
without requesting elevation or changing its location.

Validate release selection, transport and archive rejection, cancellation,
shutdown permission, successful swap/startup and rollback with disposable
fixtures. Record real macOS and Windows upgrade/restart acceptance separately
from these fixtures and native packaging builds.

### Navigation command menu

Provide a navigation command menu with `Cmd+K` on macOS and `Ctrl+K` on
Windows/Linux web. The desktop application's Navigate menu exposes the same
command through `CmdOrCtrl+K`. Open it from any mini-app, including an editor,
without changing the current page or draft. Repeating the shortcut closes the
menu. Do not open it over another dialog or the web account gate.

Use an English single-line search field with `Go to a page or repository...`,
a visible `Esc` close button, and grouped destinations in this order:

- Pages: `All tasks`, `Proposals`, `Repositories`, `Events`, `Settings`.
- Apps: `Lane`, `Pace`, `RealQA`.
- Repositories: the repositories loaded for the active data space.

Filter locally by destination label and repository URL, ignoring case and
surrounding whitespace and matching every query word. Keep page and app
navigation available while repositories load or fail to refresh. Show
`No matching destinations` when no destination matches. Do not search task
bodies, create a separate repository cache, or issue requests on menu input.

Focus search when the menu opens. Arrow keys move the active result and wrap at
both ends; Enter opens it. Keep the active result visible, distinguish it with
a blue left marker and pale-blue surface, and expose its accessible name and
selection. In the browser, announce the active destination through a polite,
atomic status region while search retains focus. Announce when no destination
matches the query. Support direct click/touch activation and visible keyboard focus.
Escape, the close button and the shortcut dismiss the menu. Browser Tab focus
stays inside the dialog; closing restores the originating control when it still
exists, otherwise the workspace. Native hosts return focus to the workspace.

Page destinations select Lane and the requested page tab. A repository destination
selects its repository-list tab. Each existing destination keeps its own filters;
a new destination starts with its default filters. App destinations open or select their shared tabs and preserve the
existing return-to-Lane behavior. Lane destinations open or select tabs and preserve their drafts and filters.
Apply the existing RealQA unsaved-edit confirmation when leaving RealQA.
Cancellation retains its screen and draft. Opening or dismissing the menu alone
never discards edits.

Keep the existing white/gray surfaces, blue accent, outline icons, 44-pixel
minimum result targets, and keyboard hints. The panel is at most 560 pixels wide,
with 16-pixel outer padding on narrow screens, a constrained height and a
scrollable results area. Keep search and close controls reachable when text is
enlarged. Native/mobile keyboard and screen-reader acceptance must be recorded
separately from web fixture checks and shared bundle compilation.

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
Use the same artwork in the 24-pixel tab and sidebar marks and the scaled
coming-soon mark, with rounded corners. Keep accessible names, selection markers,
keyboard behavior and navigation unchanged. This does not change Lane platform
icons or implement RealQA QA functionality.

### Pace mini-app icon

`apps/lane/assets/pace-icon-source.png` owns the approved Pace artwork: a white
and pale-blue folded-ribbon speedometer on a cobalt-blue square background.
`apps/lane/assets/pace-icon.png` is its 256-pixel runtime derivative, embedded
as a data URI in the shared bundle for account-free native and web use. Use the
same artwork in tab rows, chooser cards, contextual heading and coming-soon page. Keep the
24-pixel navigation footprint, rounded corners, selection marker and accessible
app names. This artwork does not replace the Lane launcher icon.

### Pace unavailable view

Pace has one static unavailable view. Show only its embedded 72-by-72-pixel
decorative artwork, `Pace`, the `Coming soon` heading,
`Pace is not available yet.`, and one `Back to Lane` button. Use no card or
additional content. Keep the existing app selection callback, content-focus
transfer, Lane tabs, filters, pane scroll and drafts. Pace makes no request and
does not add a screen-specific Escape action.

At app widths of at least 900 pixels, center the group within the available
content pane when its height permits. Limit the group to 360 pixels including
24-pixel side and 48-pixel vertical padding. Give the artwork 18-pixel corners,
then use gaps of 24, 10, 12 and 28 pixels between the artwork, label, heading,
body and button. Use a 14-pixel medium label, 32-pixel semibold heading and
16-pixel body. The heading is `#172033`; supporting text is `#68758b`.
Make the button at most 200 pixels wide and at least 48 pixels tall, with a
white surface, one-pixel `#dce2ec` border, 10-pixel corners and `#075AFF`
text. Show a separated blue focus ring and pale-blue active feedback.

Below 900 pixels, preserve the existing rail and contextual navigation. Use
16-pixel side and 32-pixel vertical padding and a 28-pixel heading. Size the
group and button to the remaining content width, wrap enlarged text, and keep
the group scrollable from its top when the height is constrained. The Pace
content must not create horizontal scrolling at 360 pixels or 200% text.

### Product contract ownership

The following behavior is authoritative for all Lane entry points. The linked domain contracts specify implementation interfaces and conformance, without narrowing these requirements.

### Product boundaries and platforms

- Keep the existing shared ReactLynx application and platform hosts. Native apps target macOS, Windows, iOS, and Android. Linux has the CLI/local MCP and the online web app, not a native GUI.
- Native apps and the default CLI use local data without login. Optional synchronization and server webhook facilities require the existing Logto identity system. **The `lane-api` server must depend on `delcore-api`.** Integrate through Delcore's Connect API and follow the identity, tenant, and authorization ownership defined in `docs/project-delcore.md`, `docs/servers-delcore-api-contract.md`, and `docs/protos-delcore-v1-contract.md`.
- The web app requires login and server connectivity and reads/writes server data. Do not implement independent browser persistence, offline editing, offline relaunch guarantees, or an offline mutation queue for web.
- Isolate each user's server data. Identical repository URLs in different accounts do not share tasks. No team workspace or collaboration model is included.
- Provide the first version free of charge. No paid plans or billing. No feature flag is needed because this is a new product made available to all logged-in users on the server; local use remains account-free. Task planning metadata adds no feature flag, paid plan, team role or entitlement.
- Web domain: `lane.delino.io`. API domain: `lane.api.delino.io`.
- Register `lane-dev://` for development builds and `lane://` for distribution builds. Support login callbacks, opening the app, and links to a repository or task. Do not include decision-item links.
- Lane, Pace and RealQA use the shared vertical workspace tabs and the New tab mini-app chooser described above. Preserve their approved folded-lanes, folded-ribbon speedometer and checklist-and-cursor artwork in tab rows and chooser cards. App names and selection remain accessible; there is no separate app icon rail.
- Place Account & sync and App settings below the vertical tab list, with at least 44-pixel targets, visible keyboard focus and direct touch activation. These actions remain navigation controls without mini-app selected state.
- Both utility actions open Lane's existing Settings page from any mini-app, retain repository/task filters and open or select the Settings tab while preserving Lane drafts. When leaving RealQA, use its existing unsaved-edit confirmation. Cancellation preserves that view and draft. Confirmation navigates and discards its unsaved draft. `Account & sync` reveals and focuses `Local data and synchronization` on native or `Online account` on web. `App settings` reveals and focuses `Notifications on this device`. Keep the contextual Settings entry and its default destination. Opening a destination must not connect synchronization, sign in, request notification permission, change preferences or delete data. Existing account errors and safeguards remain authoritative; local native notification preferences remain usable if account lists fail. On constrained heights, keep the tab list scrollable and utility controls reachable without overlap; retain bottom anchoring when space permits.
- At widths of at least 900 pixels, place the 240-pixel app-specific sidebar between the shared tabs and content. Lane retains its navigation, repository list and connection status. Pace and RealQA each show their own name, icon and selected `Overview` item, without Lane navigation. Pace retains `Coming soon`; RealQA shows the capture/settings or read-only entry defined in its owning contract. Below 900 pixels, use the dismissible vertical tab list and show contextual navigation above the content, with scrollable content and wrapping text.
- Unimplemented mini-apps show their icon and name, `Coming soon`, `<App> is not available yet.`, and `Back to Lane`, without requiring login, connectivity or a permission prompt for that page. Both the Lane selector and return button restore the previous Lane page and filters. Keep Lane's loading, error, authentication and permission behavior unchanged. Do not add release dates or sign-up flows. The former Perfine navigation entry is removed.
- Lane app switching preserves its tabs and drafts. Apply the existing RealQA unsaved-edit confirmation before leaving its view. Cancel retains the current screen and draft; confirming discards that unsaved draft. App switching must support keyboard activation, visible focus and content-focus transfer. RealQA capture, composer, settings and saved-result behavior follow the [RealQA contract](apps-lane-realqa-contract.md).
- User-facing app and CLI text is English only. Validate keyboard operation, screen readers, visible focus, state distinctions that do not rely on color, and text enlargement on every released GUI platform. Follow repository form-focus and Escape-to-close rules where applicable.

### Repositories

- Show the repository directory with a heading, the organizational-URL explanation, an Add repository action and bordered rows. Each row shows the display name, normalized URL, and visible actions for tasks, planning, editing, deletion and conflict resolution when needed. Wrap actions and long URLs within the available width, including narrow screens and enlarged text.
- Open Add and Edit in a centered, scrollable modal no wider than 560px. Focus Repository URL first, show the HTTPS/SSH equivalence helper and URL-change warning, and save only through the explicit Save action. Close and Escape immediately end the editor lifetime, even with unsaved input or a submitted save. A submitted mutation continues; its late result may refresh lists but cannot change a newer editor.
- Distinguish initial loading, successful empty results and refresh failures. Keep existing rows and the current editor input after a refresh failure, with an explicit retry. A failed save retains the form and its retry identity. Lock both fields and repeat Save while a save is pending. Show unavailable or conflicting entities without claiming success. The delete confirmation focuses Close first and retains its exact destructive scope.
- Identify repositories by normalized URL host and repository path. Treat HTTPS/SSH URL forms and a terminal `.git` difference as equivalent. The URL is an organizational identifier: do not fetch repository contents or verify Git-host access.
- When task creation names an unregistered repository URL, register it and create the task.
- Default the displayed name to URL host/path; allow a separate user-defined display name.
- Allow URL changes while retaining tasks, history, and webhook connections. Reject a change to an already registered URL. Do not retain the old URL as an alias: a later creation request using it registers a new repository.
- Treat concurrent repository name/URL edits as a repository-level conflict. Preserve both versions for resolution through app, CLI, or MCP.
- Provide repository-specific task lists, progress/proposal filters, title/description search, and an all-repository task list.

### Tasks, proposals, prompts, and history

- A task has a required title, optional human-readable description, one optional prompt, progress state, and a separate proposal disposition. A title alone is sufficient for creation. Optional planning metadata and relationships are specified in the task planning sections below; omission always means unassigned.
- Progress states are To do, In progress, Blocked, Done, and Canceled. All transitions are allowed, including reopening Done/Canceled tasks. A creation request can set progress; omission means To do.
- Agents can create regular tasks directly, create proposed tasks, and modify existing tasks. Accepting a proposal makes it a regular task. Preserve rejected proposals and allow later acceptance. Accepting or rejecting does not reset progress.
- Apps, CLI, and MCP provide full management, including task creation/editing, progress changes, proposal acceptance/rejection, deletion, prompt editing/restoration, repository management, and conflict resolution.
- Keep prompt versions indefinitely until their owning task/repository is deleted. Users can inspect a past version and restore it; restoration creates a new version. A diff-view feature is not required.
- Preserve user-visible changes to title, description, progress, proposal acceptance/rejection, repository moves, planning metadata and relationships. Show before/after values, time, and change origin: app/CLI/MCP, device name, and account identity when logged in. Do not introduce agent-supplied display identities. Only prompts have a past-version restoration action.
- Task history and Prompt versions open separate named dialogs without changing the editor draft. Each dialog requests up to 100 versions per page with its own `promptsOnly` value, shows initial loading and empty states, and keeps loaded cards visible during continuation and retry. A pending cursor permits one request at a time. The fixed header contains Close; the continuation action stays below the scrollable cards. Close and Escape return focus to the entry control.
- Task history cards show the exact available absolute time and origin, followed by friendly field names and selectable Before/After values in the existing change-helper order. Creation uses absent before values. A missing declared parent or multiple parents show their respective explanation and no inferred field delta. Prompt versions cards distinguish an absent prompt, a present empty prompt and safe Markdown content; saved captures remain read-only in saved order. History has no restore action.
- Prompt restoration stays in Prompt versions and creates a new version with the selected source and current expected version. Disable restoration for a dirty, conflicted, deleted or busy editor. Show restoration progress and failures next to the selected version, retain the draft and loaded cards after failure, and allow an explicit unchanged retry only for a retryable failure, with the same request ID. Permission and stale-write failures remain visible without a retry action. A late completion follows the editor-lifetime rule above.
- Allow moving a task between repositories while preserving its ID, prompt, and history. A linked webhook event stays in its original repository and continues to reference the moved task ID.
- Store descriptions and prompts as Markdown source. Render Markdown in the app; allow source editing and copying. Do not execute embedded HTML. Automatically load images only from HTTPS URLs.
- Android and iOS load HTTPS Markdown images through their registered host image fetchers. Each redirect must stay on HTTPS, and normal TLS validation applies. Network, HTTP and image decode failures complete as image failures. Cancellation and view release prevent a late result from updating the view.
- Host-authorized inline PNG attachments and bundled artwork continue through the host fetchers without a shared URL or disk cache. Markdown cannot authorize an inline data URL or bypass attachment ownership checks.
- The task editor Prompt presence control keeps absent, present-empty and nonempty prompts distinct. Selecting Include prompt while a prompt is present, including after unsaved edits, keeps its exact Markdown and does not dirty the draft; selecting it from absence creates a present empty prompt; No prompt intentionally removes presence. Presence changes preserve captures, other fields and the draft base, and the control stays disabled while busy, conflicted or deleted.
- App edits require explicit Save; saves create versions. If external changes arrive during editing, retain unsaved input, indicate the external change, and check conflicts when saving.
- Opening a task is owned by its tab and the latest accepted editor action within that tab. A replacement read within the same editor, closing its tab, resetting data, and disposing of the adapter or data space supersede earlier pending reads, including a read of the same task started before. Selecting another tab, page or mini-app preserves each owning tab and does not transfer read ownership. A superseded read neither replaces a draft nor reopens a closed editor, changes the page, or reports an error. Cancelling a discard prompt supersedes nothing; only an accepted transition does.
- A task mutation (save, create, proposal change, prompt restoration, conflict resolution or deletion) belongs to the editor that submitted it. If that editor was closed, discarded or replaced before the reply arrived, even by a new editor for the same task, the already submitted server mutation is not cancelled or rolled back, but its result only refreshes read data. It never replaces, cleans, reopens or closes another editor, or changes the page, so the newer draft, its dirty state and selection stay intact without another discard prompt. A mutation from the editor still on screen adopts its result, clears dirty state and handles conflicts and failures as before. Starting to open another task does not replace the editor on screen: the editor changes only when that task's read succeeds, so a read that fails or returns nothing leaves the visible editor and its pending mutation intact, and the mutation's result is adopted when it arrives.
- Accepting a proposal, rejecting a proposal and restoring a prompt as a new version keep their request ID across a failed reply, including a reply lost after the action was stored, and across a refresh of a clean draft. An unchanged explicit retry (same task, expected version, proposal target or prompt source) reuses the ID and receives the stored result without a second version, head or conflict. A changed action, target, source, task or expected version gets a new ID. A completed action ends the attempt, so a later independent action, including another restoration of the same prompt, creates its own new version. The expected version is never advanced merely because a refresh arrived, and competing edits keep the existing stale-write and conflict behavior.
- If local storage is full or saving fails, retain unsaved input on the current screen and provide an error/retry action. Do not show successful save/synchronization. In web, connectivity loss blocks saving but retains current-screen input; reconnection requires an explicit retry, not an offline queue.
- The repository editor locks its URL and name fields while its own save is pending, so input cannot be accepted and then discarded by that save. A save completes only into the editor that submitted it: closing an editor, or opening another one (including the same repository again), starts a new lifetime, and a late response from an earlier lifetime refreshes lists but never closes, changes or reports an error into the newer editor. An unchanged retry of a failed repository save, including after a lost response, reuses the original request ID, entity ID and base so the server replays the original outcome. Changed URL or display name, a different operation or base, and a new editor session use a fresh request ID.
- The repository editor pre-fills the URL of an existing repository so that saving only the display name submits the same normalized identifier: it builds an `ssh://git@` URL for an identifier with an explicit `:443` port, which can only come from a nondefault SSH port, and an `https://` URL for every other identifier. This uses the existing raw-URL API and stores no transport. An explicit URL edit follows the normal transport-aware normalization, rename and uniqueness rules. The native store validates a stored normalized repository identifier (conflict resolution, accepted pull and push results, standalone merge) without assigning a transport, so no explicit port is stripped and malformed or noncanonical identifiers are still rejected.
- Web reconciles its API session with a credentialed `GET /auth/session` on start, explicit retry, focus and online resume, and before redeeming a deletion reauthentication, because an approval popup can replace the session cookie. Reconciliation precedes stream restarts, query invalidation and new account-bound requests. The newest session response wins and obsolete responses are discarded; a response that arrives after the session has ended (Disconnect, the sign-in gate or data deletion) is also discarded and cannot reinstall the removed session. An account change resets the account-bound view and its drafts. A changed device, session or CSRF token for the same account replaces credentials, notification preferences and the deletion target, retires in-flight results of the previous credentials and keeps the current-screen draft. An unchanged session keeps all state. A rejected session (401) stops account-bound work and shows the sign-in gate. A transient failure retains the view and input, blocks reads and saves, and recovers only through the explicit retry control, whose successful confirmation also invalidates retained queries so a view that kept a failed-loading state reloads; focus and online events do not retry it. Mutations are never queued or replayed, and a mutation that arrives during reconciliation is refused. This includes a notification preference save, which is also refused while the session is unconfirmed and is retired without registering a push device when the identity changes while it waits on the permission prompt or the service worker. Recovery does not use `POST /auth/refresh`, and cookie, Origin and CSRF enforcement is unchanged.
- Web links open a destination with `task=<id>` or `repository=<id>` (task wins when both are present) and an optional `account=<id>` guard. When the session check returns 401, the gate's sign-in link sends the complete, well-formed destination to `/auth/login` as separate typed fields while `return_to` stays the query-free registered callback; the API returns the validated fields on that callback after login. A malformed or incomplete link signs in plainly. After the session is installed and the app signals ready, the host delivers the destination once per page load; an `account` that differs from the authenticated account produces the explicit account-mismatch error and no navigation or account switch. Account switching (ConnectSync) uses the query-free sign-in link. Automated tests cover this with mocked Logto and Delcore and a disposable database; live Logto, Web Push and browser acceptance are separate and not implied.
- The standalone web account gate uses one main landmark and a centered white card no wider than 460 pixels. It shows the Lane wordmark, one state heading, supporting text, an atomic polite status, and full-width Sign in with Logto and Retry connection controls in that DOM order. A divider separates the controls from the online-connection and server-storage disclosure. Connecting shows `Connecting to Lane` and `Connecting…` with a non-color busy cue. A rejected session shows `Sign in to Lane` and `Sign in to use Lane on the web.` with sign-in emphasized. A transient failure shows `Unable to connect`, the complete safe error, and retry emphasized; it mentions retained current-screen input only when an authenticated view remains in memory. Offline guidance requires reconnection and an explicit retry. A pending retry cannot start a second attempt and releases its busy state on settlement. When the gate hides a focused app view, focus moves to its heading; successful recovery restores meaningful app focus. Initial reconciliation does not steal browser focus. The gate scrolls at short heights, wraps long status text at narrow widths and text enlargement, and cannot be dismissed with Escape.

### Task planning metadata

Tasks carry optional planning fields. A title remains sufficient for creation, and every field below is unassigned when omitted.

| Field                | Required behavior                                                                                                                                                      |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Priority             | Fixed values: None, Urgent, High, Normal, Low. None means unassigned.                                                                                                  |
| Type                 | Fixed values: None, Bug, Feature, Improvement, Documentation, Maintenance. None means unassigned.                                                                      |
| Labels               | Zero or more labels of the task's repository; at most 100 per task.                                                                                                    |
| Start date, due date | Real calendar dates (`YYYY-MM-DD`, leap years included) without time or time zone. When both exist, start date must not be later than due date; equal dates are valid. |
| Story points         | Optional integer from 0 through 2,147,483,647. Zero differs from unassigned. Reject negative, fractional and larger values.                                            |
| Milestone            | At most one milestone of the task's repository.                                                                                                                        |
| Cycle                | At most one cycle of the task's repository. Use Cycle as the UI term for the selected cycle/sprint capability.                                                         |

Task dates are independent of milestone target dates and cycle periods; a task can fall outside either plan's dates. Lane adds no deadline notifications, automatic progress changes, effort tracking or automatic rollover. Every field can be set, read, changed and cleared through the apps, CLI, local MCP and remote MCP. Keep the separate progress and proposal-disposition state machines unchanged.

### Repository planning items

Labels, milestones and cycles belong to exactly one repository, including in standalone local use. Apps, CLI and MCP provide create, list, read, edit, delete and history for each kind. Entities use UUID v7 identities, and tasks reference them by identity, never by display name.

- Every item has a required name and an optional description. Trim leading and trailing whitespace from names; a name has 1 to 100 Unicode scalar values after trimming and a description has at most 4,000.
- Within one repository and item kind, reject a duplicate name without regard to case; the item being edited and deleted items are excluded from the comparison. Different kinds and different repositories may use the same name.
- A label requires a color in `#RRGGBB` form on creation; Lane stores it upper-case and allows later color edits. A color is never the only indicator of a label; its name is always shown.
- A milestone has an optional target date and a completion boolean that is initially incomplete. Completion is an explicit edit; it does not change task progress and no task state changes it.
- A cycle requires a start date and an end date, with start date no later than end date. A cycle has no lifecycle state and no recurring-generation behavior.
- An item's kind and repository cannot change after creation. Edits use the expected-version and request-ID rules of the other entities, and concurrent edits of the same item are a whole-item conflict resolved through the same conflict operations.

### Task relationships

All relationship endpoints must be live tasks in the same repository and the same personal data space.

- Parent/subtask: a task has at most one parent. Subtasks are the inverse view. Reject self-links and hierarchy cycles.
- Depends on/blocks: two views of one directed dependency. Reject self-links and dependency cycles.
- Related: a symmetric relationship, stored on both endpoints. Reject self-links.
- Duplicate of: at most one original task, stored on the duplicate only. Reject self-links and duplicate-reference cycles.
- Relationships only describe work. They never change progress or proposal state, never reject a completion because related work is unfinished, and never automatically close duplicates or parents.
- Editing a task's Subtasks or Blocks changes the other tasks' stored fields in the same atomic operation. Each affected task receives its own new version with the caller's origin. If any affected task is in conflict the whole operation fails with the existing read-only conflict result, and a deleted affected task fails as not found or deleted. Failure leaves no partial change.
- Show linked task titles, not a new issue-number identity scheme.

### Moves, deletion and reference cleanup

- Moving a task to another repository clears its labels, milestone, cycle, parent, dependencies, related tasks and duplicate-of reference, and removes every other task's reference to it. Other linked tasks survive and receive a new version for each removed reference. The moved task keeps its priority, type, dates, story points, ID, prompt, capture ownership and applicable history.
- Deleting a task removes other tasks' relationship references to it. Deleting a label, milestone or cycle removes that assignment from every task that references it and then deletes the item. Linked tasks survive. Repository and account deletion keep their existing cascading rules, including deletion of that repository's planning items.
- Automatic cleanup writes use the deleting or moving actor's origin and run in the same transaction as the triggering operation. A failed operation leaves no partial cleanup, history, replay record or outbox row. Retried requests do not create duplicate relationships, entities or history.

### Planning history

- Record before/after values, time and the established app/CLI/MCP, device and authenticated account origin for every planning-field and relationship change, including automatic reference removal caused by moves and deletions. Record edits to labels, milestones and cycles in each item's own history. Clients derive each version's before/after values from that version and its declared parent, never from a chronologically neighboring version that may belong to another branch. A zero-parent version is a creation with absent before values. While a declared parent is not yet loaded, the history card keeps its time and origin and shows no field delta; it compares once the parent is loaded. A resolution with several parents keeps its metadata and shows no single-baseline delta.
- Do not add metadata-version restoration. Only the existing prompt restoration remains.
- Retention and deletion follow the existing rules. Deleting an owning entity removes its historical content. Never retain deleted content by copying it into another entity's history; tasks keep only identity references to labels, milestones, cycles and other tasks, so deleting an item erases its name and description everywhere. Surviving tasks keep their own history.

### Task filtering and sorting

Repository-specific and all-repository task lists, in apps, CLI and MCP, accept the planning filters and one sort.

- Filter by priority, type, labels, start-date range, due-date range, story-point range, milestone, cycle and task relationships, including finding unassigned values.
- Different fields combine with AND. Selected values within one field combine with OR; a label filter matches a task that has at least one selected label. A field's unassigned selection (no priority, no type, no labels, no start/due date, no points, no milestone, no cycle, no relationships) combines with that field's selected values with OR. A field with no selection and no unassigned choice does not restrict. A date or point range that has only bounds excludes unassigned tasks; bounds are inclusive. A relationship filter selects tasks that have at least one relationship of the chosen kinds (parent, subtasks, depends on, blocks, related, duplicate of), or none.
- Existing title/description search and progress/proposal filters remain and combine with AND with every planning field.
- In all-repository lists, identify labels, milestones and cycles with their repository name, allow selections from several repositories, and keep same-name items distinct by identity.
- Support one sort key at a time with ascending or descending direction. Keys are priority, type, start date, due date, story points, milestone name and cycle start date. Priority ascending is Urgent, High, Normal, Low. Type follows the order listed above. Dates compare as calendar dates. Milestone names compare case-insensitively with a deterministic code-point tie-break. Cycle sorting uses the cycle's start date. Unassigned values always sort last in both directions, and equal keys break ties by ascending task ID. Without a sort key, lists keep the existing task-ID order.
- Ordering and pagination are identical in every interface: a page boundary never repeats or skips a task, and concatenated pages equal the unpaged result. A cursor remains opaque and scoped to its query and account.
- Clear filters clears filter conditions while preserving the search text and sort selection.
- The task list's "Load more tasks" control serializes continuation loading per query generation: it stays disabled while a continuation is pending, repeated taps or Enter/Space presses start no second request, and each cursor page is appended once using the latest cursor. A failed continuation keeps the loaded rows, shows its error beside the control and releases it for an explicit retry without automatic retries. A change to the search, repository, progress, proposal, filters, sort or Tasks/Proposals page starts a new generation, and a completion from an older generation cannot change the list, cursor or control state.

### Planning synchronization and constraint rejection

Native and CLI saves commit locally without a server. Synchronization preserves the planning data, history and deletion cleanup of the accepted server state. Web remains online-only: an offline web save fails with retained input.

- The server validates every pushed task and planning item against the **accepted server state** with the rules above. Independently valid offline changes can combine into a violation, for example two devices creating a combined cycle, giving a task two parents, or creating the same name twice.
- A violating item is rejected with `CONSTRAINT_VIOLATION` and a named violation. The server keeps its accepted state and never rolls it back. The client keeps the rejected local version and its pending record in the blocked state, shows the reason in its synchronization status, never marks it synchronized and never discards it.
- The user corrects the local data with an ordinary edit and then retries the blocked item explicitly (`lane sync run --retry-blocked` or the equivalent app action). Ordinary whole-task version conflicts and their resolution are unchanged.

### Task planning UI specification

This text specification is authoritative without supporting images. Preview data is illustrative, not seeded product data. Existing navigation, task content editing, progress/proposal behavior and app selectors remain unchanged. Layout references were a 720-pixel desktop content width for the task editor and 760 pixels for the task list and repository management.

1. **Task creation and detail.** Add Properties and Relationships sections to the existing editor. Properties contains priority, type, labels, start date, due date, story points, milestone and cycle. Relationships contains Parent, Subtasks, Depends on, Blocks, Related and Duplicate of. Select existing tasks of the same repository by search and display their titles. Show Change parent when a parent exists. Field edits and relationship changes remain in the draft until the existing Save action succeeds. Opening a related task opens or selects its tab and preserves the current draft.
2. **Task lists.** Add a collapsible Filters region and separate sort-key and direction controls. Keep the existing search, progress and proposal controls. Label every filter, give each filter its own Unassigned choice, and identify repository-owned values with their repository in the all-repository list. Show task metadata compactly. Do not add grouping, bulk selection or saved views.
   On the Proposals page, use a compact heading and New task action, then the full-width title/description search, separate Proposal disposition and Progress menus, inline planning Filters and Clear filters, and a separate sort row. The page opens with Pending proposals, All progress, collapsed Filters and Default order. Pending proposals and explicit Pending both request PENDING; the other choices are Regular, Accepted and Rejected, with no All disposition choice. Progress is independent, so Pending includes Done and Canceled tasks. Keep the existing 100-item cursor, serialized continuation, query ownership, filter and sort semantics, and regular-task New task behavior. Rows show a title action, complete planning metadata, conflict text when applicable, and separate textual disposition and neutral-symbol progress. Show distinct initial loading, true-empty, no-match, error and continuation-retry states. The menu supports arrows, Enter/Space and Escape with focus returned to its trigger. At desktop widths, rows may align state text beside the title and metadata; below 900 pixels, controls and row content stack without clipping at 390 or 360 pixels. Keep list status announcements concise and preserve tab focus and scroll on return.
3. **Repository management.** Provide Labels, Milestones and Cycles tabs under the repository detail area, each with an item list and an edit area and with New item, Save, Delete and History actions. Labels expose name, color and description. Milestones expose name, description, optional target date and completion. Cycles expose name, description and required start and end dates. History shows before/after values, time and origin and has no Restore action. An empty list offers creation. Deletion confirmation explains that deleting an item detaches it from tasks while the tasks remain.
4. **Responsive layout.** At widths of at least 900 pixels keep the existing desktop list/detail navigation and use two-column fields where space permits. Below 900 pixels keep the mobile list/detail navigation and stack fields and filters in one column. At 360 pixels wrap long names and labels and keep every control usable without horizontal scrolling.
5. **Visual treatment.** English UI, white and gray surfaces, gray borders, dark text and the existing blue accent `#075AFF`. Keep the current design system and approved navigation. Color supplements text and is never the only state indicator.
6. **States and recovery.** Show loading and empty-result states and an explicit retry for list and option failures. Keep drafts after failed saves. Explain field validation and relationship-conflict causes in text. Disable duplicate submission while saving. Preserve the existing authentication, permission, deletion and whole-task-conflict states. Web offline saves fail with retained input; native, local CLI and local MCP saves remain local.
7. **Accessibility.** Keyboard operation, visible focus, accessible control names, screen-reader support, non-color state cues and text enlargement apply on every supported GUI platform. Dialogs close with Escape and return focus to their trigger. While any dialog is open, its backdrop covers the whole tab sidebar, including utility controls, so background pointer input cannot navigate or discard retained edits; the browser regression verifies actual hit targets for content-nested and root dialogs at desktop and narrow sizes. Apply the existing automatic-focus rule to forms with one critical input.

### Repository task-list presentation

A repository tab keeps its repository ID and shows its display name, or its normalized URL when no display name exists. The content header identifies the repository, retains the task-list subtitle, and offers New task with that repository preselected. The list has a full-width title-or-description search field, compact progress and proposal selectors, an in-flow Filters disclosure with its active planning-filter count and Clear filters, and separate compact sort-key and direction selectors. The mobile Repository selector opens or selects the chosen repository tab or All tasks without changing the current tab's identity. Search, filters, sort, scroll and results remain local to the tab.

At wide content sizes, results use aligned Task, Progress and Details columns with about 36%, 17% and 47% of the available row width. Rows have subtle separators and a minimum height near 64 pixels that grows with content. The task title remains the opening action. Progress is read-only text. Details keep every existing planning summary, relationship count and label. Show a separate textual Proposal badge only for pending proposals and a neutral outlined Conflict badge for conflicted tasks. An item without metadata has an empty Details cell. At narrow widths, stack each row as task title, progress and status, then details; show accessible field labels. Long names and metadata wrap, including at 360-pixel application width and with enlarged text. The application navigation layout and its 900-pixel breakpoint do not change.

The existing query and recovery rules remain authoritative: planning fields combine with AND, selected values and Unassigned within one field combine with OR, bounds are inclusive, and invalid values send no list query. Repository-owned options show only that repository's items. Clear filters affects planning conditions only. Sorting keeps canonical order, unassigned-last values and ascending task-ID ties. List requests use 100-item pages and opaque cursors with serialized continuation ownership. Keep distinct loading, empty, first-page error, continuation error and option error states with their existing explicit retry actions. Selectors and Filters expose expanded state, close with Escape and restore focus to the trigger. Ordinary list loads do not move focus.

### Pre-release data reset

Lane has not been deployed, and existing development data may be discarded. This pre-release exception applies to task planning only: there is no backward compatibility, no development-data preservation, no old-to-new data migration and no old-client support. The earlier local migration procedure no longer exists. Implementations must never reset silently when a store is opened.

- A local store or server schema that does not match the current format is refused with an update/reset instruction. The explicit procedures are `lane data reset` and the native Settings reset in the [local-store contract](apps-lane-local-store-contract.md#development-data-reset), and the disposable development database reset in the [server contract](servers-lane-api-contract.md#development-data-reset).
- Reset is destructive and explicit: it requires a confirmation control, coordinates with open handles and background workers, respects account boundaries and removes the associated content. Use it only with disposable development data, never production data.
- Record the reset, validation errors, rejected synchronization and the explicit correction and retry in the user and operator guides.

### Local storage, synchronization, and conflicts

- Native apps and CLI use host-side SQLite. ReactLynx accesses storage through platform Native Modules. Desktop app, CLI, and local stdio MCP share the same local data; CLI/MCP continue to work when the app window is closed.
- Use PostgreSQL for server synchronization data, retained events, and server state. Remote MCP can read/write synchronized data when every user device is off; devices receive these changes on reconnect.
- Synchronize all repositories, tasks, prompts, and history, not a user-selected subset. Notification enablement/display preferences are device-local exceptions.
- Initial synchronization preserves both datasets. Merge repositories with the same normalized URL; preserve tasks with different IDs separately. Conflicting versions with the same ID require resolution.
- Preserve both versions of concurrent task edits and require explicit resolution. The conflict unit is the **whole task**, including edits to different subfields. Until resolution, that task is read-only outside the resolution operation.
- App, CLI, and MCP can compare and resolve conflicts. Preserve both conflicting versions and the resolution as history; the selected/edited resolution is a new version.
- If multiple devices resolve the same conflict differently, keep the resolution first accepted by the server. Reject a later stale resolution and require reading current state again.
- The app binds a resolution session to the sorted, unique head IDs that the user reviewed when the session began and submits exactly that set as the expected heads; refreshed task heads never replace it. When a refresh changes the head set (a reordering is not a change), including a collapse to a single head, keep the edited resolution visible but block saving it and require an explicit `Review current versions` action that starts a new session on the current heads. The user may then choose a current version or explicitly reuse the retained edited resolution. The server's exact-head check remains authoritative for changes after the latest read: the client retains its input and shows the rejection.
- The whole-task conflict dialog shows every current head in source order as a complete, selectable comparison card. Show two cards side by side when space permits, wrap additional heads into later rows, and stack cards below 900 pixels. Each card shows the full version ID, origin, title, description, absent/present-empty/nonempty prompt source, saved captures, progress, proposal, repository, planning values, labels and named relationship targets. Mark the selected card with a check and text as well as a border. Keep the separate **Your resolution** section below the alternatives. Only the existing title, description, prompt presence/source, progress, proposal and repository controls edit its inherited whole-version payload. Typing into an absent Resolved prompt makes it present. One explicit Save resolution creates the new version. Disable resolution edits and duplicate saves while a save is pending; on failure keep the exact resolution and show the error beside the save action. Keep stale and collapsed-head review gates visible. The named modal focuses its heading first, contains focus, returns focus to Compare conflict on Close or Escape, and keeps all controls reachable at 900 and 360 pixels and with 200% text enlargement.
- Desktop supports background synchronization after the app window closes. Mobile synchronizes while running and at platform-permitted background opportunities. Web uses its active online connection; do not imply an offline data mode.
- Use server change notifications for live updates. Local process changes must reach the running app without requiring an app restart; verify this during implementation.
- A delivered change notification also rereads the mounted `Events` page: it reloads receivers and the first event page for the selected repository and state filters, supersedes older loads and continuations, restarts continuation from the refreshed cursor, and keeps unsaved receiver input. A failed reload stays visible and retryable with `Refresh events`.
- Each `Inspect event` action is a separate detail-read intent, even for an ID that is already being read. Only the newest inspection of the mounted `Events` page can open or replace the event details or publish its error. Closing the details with `Close` or `Escape`, opening a linked task, leaving the page and replacing the adapter end the ownership of older reads, so a late response or failure cannot replace the accepted event, reopen a dismissed dialog or show an obsolete error. A single current read, a missing-event response, a visible read failure and an explicit retry behave as before. Reading never completes or modifies an event; `Reopen for processing` and `Delete event` act on the accepted event, and an opened deletion confirmation keeps the event it captured. A delayed reopen response replaces the open details only when it names the event they currently show; a response for an event whose details were closed in the meantime is ignored.
- Native/CLI connectivity failure or expired Logto authorization does not block local work. Preserve pending changes; resume synchronization after reconnection/reauthentication.
- Unsupported synchronization format stops synchronization with an update instruction, while compatible local features and pending changes remain available. An older app must not open/write an unsupported local data format or initialize it as empty; require a supported version. Opening a local store whose format differs from the current one is refused with an instruction that names the explicit development reset described in [pre-release data reset](#pre-release-data-reset); it never resets implicitly.
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
- For individual task/repository deletion, the app asks for confirmation; a CLI delete command or MCP delete call is itself confirmation. Server-wide/account-data deletion requires explicit target-account confirmation and Logto reauthentication. When a task deletion request fails, the confirmation stays open and shows the failure inside itself; the editor draft is retained, nothing completes, no request is retried automatically, and the user can retry explicitly or close the confirmation.
- Task creation uses a request ID. Same ID/content returns the existing result; different content under the same ID is an error. Replaying creation after the task was deleted returns a deleted-result indication, with no body and no recreation.
- The server may process synchronized content: apply access controls and encryption in transit/at rest. End-to-end encryption is not required. Do not log task/prompt content, webhook payloads, or authentication secrets.

### Webhooks and agent-driven automation

- Support only the Lane common webhook format: required sender event ID, event type, occurrence time, and JSON object payload. No GitHub-specific adapter is required.
- Each receiving endpoint belongs to a repository. Changing its repository affects only future events. Retained events remain in their original repository.
- While a receiver save request is pending, the editor disables the receiver name, repository choices and Save so the submitted values cannot diverge from the visible draft. Success closes the editor and reloads. Failure retains the draft, shows the error and re-enables editing for an explicit retry.
- The server only receives, retains, and synchronizes events. A user's existing agent reads them through MCP and decides whether to create tasks. Lane does not interpret events with AI, execute agents, or add a user-rule automation engine.
- App, CLI, and MCP manage receiving settings and list/delete events. Retain events until explicitly deleted.
- The Events screen presents receiving settings and retained events on one vertically scrolling page. Its order is title and actions, guidance, Repository filter, Event state filter, receiving settings, then retained events and available continuation. Repository filters both lists; event state filters events only. Keep receiver rows expanded with name, repository and edit, key rotation and delete actions. Keep event rows in adapter order with full sender event IDs, Pending, Processing or Completed text badges and inspection actions. Do not add totals or internal tabs.
- Show loading before the first successful list response. A failed first page must show an error and an explicit retry without an empty-result claim. After a successful empty response, use distinct messages for no receivers and no matching events. A refresh of the same query keeps accepted rows and receiver drafts; filter changes clear old rows and continuation. A failed continuation keeps its rows and cursor for explicit retry. Only the current query or detail request may publish a result.
- Receiver editing keeps an explicit Save, autofocuses the name, locks both fields and Save during mutation, and keeps input and error after a failed save. Close and Escape dismiss the editor immediately. Creation and rotation show a signing key once in a dialog; editing never shows a new key. Event details show selectable inert JSON, processing history, zero-task outcomes and linked-task navigation. Reopen is available only for Completed events. Receiver and event deletion retain separate warnings and explicit confirmation. At desktop and narrow widths, controls, long identifiers and dialogs wrap; status and errors use text and accessible announcements without exposing payloads or keys.
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
- Web enablement creates a Web Push subscription only after the `push-worker.js` registration it created has an active worker; it never relies on another registration's readiness or a page controller. An already active worker (even with an update installing) and an existing subscription proceed immediately. A worker that becomes redundant before activation, or a missing worker, fails the enablement with an explicit error and saves no preference or push registration. Disabling never waits for activation. Every web preference save, including a disabled save in a browser with no push subscription, is acknowledged by the server registration for the authenticated device before the page reports success, installs the preference or unsubscribes. A disabled save sends no endpoint or keys and never requests permission, fetches push configuration or creates a subscription, whatever the permission state. A rejected save surfaces its error, keeps the edited Settings draft and previous preferences, and is not retried automatically.
- Notification preference choices and Save stay disabled while any Settings operation is pending, so input made during a save cannot be replaced by the acknowledged values. On success adopt only the acknowledged submitted values and keep later edits dirty until the next explicit Save; on failure keep the submitted values, dirty state and error for an explicit retry. A load never overwrites a dirty draft.
- Direct provider integration: APNs for Apple, FCM for Android, and Web Push for web. **Windows remote WNS delivery is deferred**; Windows supports running/background-client synchronization and local notifications. Do not add WNS package-registration requirements to this version.
- Unsupported remote-push environments, including Android without required Google Play services, can still use Lane. Explain the limitation and retain in-app indications and available local notifications.
- The macOS host keeps the latest APNs device token in memory only; it is never written to disk or logs. It registers the token through `RegisterPush` for each space that is synchronized and has notifications enabled, using that space's authenticated device and its current content preference. Reconciliation runs on the host's observation cadence, so a token received while a retained account was disconnected registers when that account reconnects, without another OS token callback or a preference change. A helper or RPC failure leaves the registration pending and retries it after 30 seconds; an authentication failure waits for the next verified reconnection instead of a timer. A forbidden result is not an authentication failure: the helper also answers it for a space that is not the active account selection, and a selection change is not a reconnection, so that space retries after the same delay and registers once it becomes the active selection. A verified reconnection also discards any registration attempt still outstanding, so a late authentication failure from before the reconnection cannot block the space again; the next observation registers afresh. Success is recorded only after the helper acknowledges it and is not repeated. A new token supersedes the older one, and a disabled, disconnected or removed space, or a stopped host, issues no registration. Disconnecting or disabling a space clears its registration state, so reconnecting or re-enabling registers it again. Registration never requests OS permission again, runs off the storage queue so local work continues, and does not change server delivery retries. Failure diagnostics are metadata-only events.
- Persist pending push deliveries. After an initial transient failure, retry at intervals of 30 seconds, 2 minutes, 10 minutes, 30 minutes, and 60 minutes, at most five retries. Expire delivery two hours after creation. Respect provider Retry-After; do not retry earlier or beyond expiry. Stop on permanent failure. Cancel unsent notifications if the proposal is accepted, rejected, or deleted.
- Provider connection code, configuration instructions, and mocked tests are in scope. Actual provider registration, account/device delivery acceptance, store publication, and production deployment are not required here.

### CLI, MCP, and repository integration

- CLI defaults to local operations; only an explicit remote option targets server data. Default output is human-readable; `--json` emits structured results/errors. Provide macOS/Windows/Linux executables and the npm installer package **`@delinoio/lane`**, with installation instructions.
- Local MCP uses stdio. Optional remote MCP uses browser-approved Logto authentication. Both expose the agreed management functions, with online-only server operations identified explicitly.
- Task planning fields, relationships, filters, sorts and repository labels/milestones/cycles use the typed protocol (extended task operations and the typed planning service) in native bridge, CLI, local MCP, remote MCP and web adapters. Do not add an untyped side-channel API. CLI keeps human and JSON output, explicit remote selection, retryable request IDs and expected-version checks; MCP names and schemas follow the typed-method mapping. Help text, the interface contracts and the common agent skill are updated together with these capabilities and limitations.
- Supply a common Lane `SKILL.md` and remote/local MCP connection instructions. Do not run or distribute an AI execution service.
- Preserve the Lane project ID/application ID and existing platform/toolchain contracts. Follow repository defaults for new server implementation (Rust), other unspecified new projects (Go), UUID v7 entity IDs, and Connect RPC business interfaces. MCP and webhook adapters must preserve the same ownership, version, deletion, and idempotency rules.
- Keep client, protocol, server, CLI, and distribution work in the same repository. Before runtime expansion, update `docs/project-lane.md`, the application foundation, and the documentation catalog; add owning server/protocol/CLI/distribution contracts and required scoped ownership instructions. Do not place issue-specific requirements or issue identifiers in repository instruction files.
- Follow repository license, environment, dependency, generated-output, formatting, and validation contracts. Retain existing PR/main/affected CI platform restrictions, including no new macOS PR runners.

### Operations and observability

- Produce deployable web/API/remote-MCP artifacts, configuration, and runbooks for the specified domains. Do not perform actual production deployment. Native completion requires build/install/behavior validation; store publication and formal distribution are follow-up work.
- Run database migrations explicitly, never on server startup. Roll back only to a binary compatible with the current schema; otherwise recover with a corrected forward version. Do not require down migrations. The pre-release reset for task planning is the documented exception described in [pre-release data reset](#pre-release-data-reset); it promises no rollback to an incompatible binary.
- No numeric latency, capacity, or availability SLO is promised. Measure response time/error rate and test functional recovery. Alert thresholds below are operational triggers, not SLOs.
- Include structured logs, request correlation IDs, health checks, basic metrics, dashboards, and automatic alerts. Record operational metadata, not user content or credentials. Planning operations log the operation, correlation ID, result category and duration only; they never log task content, label/milestone/cycle names or descriptions, relationship titles or other user content. Server logs expire after 30 days. Local native/CLI logs remain local, expire after 30 days, and have a combined 100 MiB limit; remove oldest entries when either limit is reached. The macOS host logs to UTC daily files and expires them by bucket date, not by modification time; follow the [local-store diagnostics contract](apps-lane-local-store-contract.md). No automatic diagnostic upload.
- Supply Prometheus rules and Alertmanager email routing/configuration. Operators configure email transport and recipients. No Lane-owned email provider integration is required.
- Service and DB health: probe every 30 seconds with a five-second timeout; alert after five minutes of continuous failure; resolve after five minutes continuously healthy.
- API: over a rolling five-minute window with at least 20 requests, alert if server errors/timeouts exceed 5% or p95 response time exceeds two seconds continuously for five minutes. Exclude long-lived connection lifetime from request-latency measurement.
- Push: alert when transient failure rate exceeds 10% over a 15-minute window with at least 20 delivery attempts, continuously for five minutes; also alert when the oldest pending delivery exceeds ten minutes.
- Resolve API/push alerts after the triggering condition clears continuously for ten minutes. Insufficient samples alone do not prove recovery; an empty push queue clears its age condition. Send email once on firing, once per hour while active, and once on recovery. Make these numbers operator-configurable with the agreed initial values.
- Document user troubleshooting and operator procedures for server/DB failures, synchronization, webhooks, push delivery, and alerts. For task planning, also document the development-data reset, validation errors, rejected synchronization items and their explicit correction and retry. No separate customer-support/admin diagnostic product is required. Task planning uses the existing deployment units, stores, sync workers, request limits, health checks, metrics and alert policies, adds no numeric SLO, and requires no external integration or new credential.

### Explicit exclusions

- Decision items in their entirety: no question/answer records, choices, answer history, deferral/reopening/acknowledgment, decision-specific UI/API/MCP functions, links, or notifications.
- Lane-driven AI calls, agent execution, Git content access, Git access verification, event interpretation by Lane AI, user-rule automation engines, and provider-specific webhook adapters.
- Team workspaces, billing/paid plans, feature flags, app password/biometric locks, end-to-end encryption, independent offline browser data, and Linux native GUI.
- File import/export, separate user backup/restore, trash/recovery, old repository URL aliases, mini-app implementations, and a separate customer-support tool.
- Task planning exclusions: user-defined fields or configurable priority/type vocabularies; assignees, reporters, subscribers and team collaboration; cross-repository relationships; grouping, bulk editing, saved views, nested Boolean filters and multi-key sorting; time tracking, estimated hours, date-time or time-zone inputs and fractional story points; external issue-tracker integration, new notifications, deadline reminders, automatic progress changes, recurring cycles and automatic rollover; metadata-history restoration; preserving or migrating existing development data and supporting old clients.
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

The Node component-test harness matches both Windows and POSIX source paths and normalizes stub lookup keys to forward slashes. Host-bound collaborators must be replaced on every test host so fixture runs do not activate production host effects or update polling.

Run `pnpm test` in `apps/lane` for TypeScript checks and both shared bundle builds.
Affected frontend CI runs on Ubuntu. Native compilation, launch, resizing,
lifecycle, signing, installation and store submission need separate platform
validation. No macOS runner is added to PR, main-push or affected manual CI.
Remove generated `dist` after validation.

Settings render tests bundle the real Settings screen with a Node ReactLynx stand-in and a scripted adapter. They cover notification Choices and Save staying disabled for the whole pending Settings operation, blocked tap and Enter/Space activation, acknowledged-value adoption, retained draft and error on rejected saves, and the dirty-draft guard on loads. They are bundle-level checks, not browser or native acceptance.

Required product tests cover all task/proposal transitions, optional prompts, explicit-save history, prompt restore as a new version, task moves, repository normalization and conflicts, safe Markdown, and input retention on errors. Task planning tests cover title-only creation; setting and clearing every field in every adapter; name, color, label-limit, date and story-point boundaries; same-name items across repositories and kinds; relationship self-link, cycle and multiple-parent rejection with unchanged progress; moves and each deletion kind with detached references, surviving tasks and history redaction; filter AND/OR/unassigned combinations, sort directions, unassigned-last, task-ID ties and page boundaries; restart persistence without a server; rejected synchronization with correction and explicit retry; response-loss replay and request-ID reuse; transaction rollback without partial cleanup; structured logs without user content; and the explicit development reset with active-handle coordination. Test keyboard operation, screen readers, visible focus, non-color states, text enlargement, the 900-pixel and 360-pixel layouts, loading/empty/error states and Escape handling on every released GUI platform.

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

Windows builds require Node 24, CMake, Visual Studio with the MSVC C++ desktop
toolchain, and a Windows 10/11 SDK containing `Windows.winmd`. Keep the Windows
`SystemRoot` environment intact. SDK NuGet and relay
Node ZIP extraction uses `SystemRoot/System32/tar.exe` explicitly, because Git
Bash can put GNU tar ahead of the Windows ZIP-capable bsdtar on `PATH`.
Install the frozen workspace from the root. On Windows, configure Git Bash as the pnpm script shell
for the root shell-based hook installer. Run `pnpm build:desktop` from
`apps/lane`; the launcher selects the architecture of the Node/Lynxtron runtime.
It resolves pinned official NuGet Foundation `1.8.260803002`, Runtime
`1.8.260921001` and C++/WinRT `2.0.250303.1` packages, verifies their archive
SHA512 values, and generates AppNotifications/AppLifecycle and OS projections.
The ignored `native/desktop/build/windows-app-sdk` cache records the package
pins, Windows SDK metadata and generated file hashes. Incomplete or changed
cache content is rebuilt; concurrent provisioning fails rather than sharing
partially generated headers. Preserve the original SDK and generator licenses
and notices in `resources/app/licenses/windows-app-sdk`.

An external SDK remains supported through the four absolute paths
`LANE_WINDOWS_APP_SDK_INCLUDE_DIR`, `LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR`,
`LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL` and `LANE_WINDOWS_APP_SDK_LICENSE`.
Set all four together or leave all four unset. The supplied bootstrap must
match the build architecture and the supplied headers/projections. External
SDK owners must retain any additional third-party notices with that SDK.

Set `LANE_NATIVE_TESTING=1` before `pnpm build:desktop` to include the Windows
policy, helper-transport and synthetic memory-renderer tests. Run
`ctest --test-dir native/desktop/build/addon -C Release --output-on-failure`
from `apps/lane`. The capture test checks all six tools, opacity, crop, invalid
input, queue recovery and nested-selector shutdown without reading screen
content. The platform test uses only synthetic helper traffic. These tests do
not install Lane, register Chrome, or establish interactive runtime acceptance.

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
