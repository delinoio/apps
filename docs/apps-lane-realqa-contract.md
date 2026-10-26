# Lane RealQA Contract

## Scope and ownership

RealQA is a mini-app in the existing Lane application. This document extends the
[application foundation](apps-lane-foundation.md) with the approved capture,
editing, settings and saved-result behavior. Requirements below do not establish
implementation, platform acceptance or publication.

`apps/lane/src` owns shared presentation. `apps/lane/native/desktop` owns capture,
shortcuts, the floating composer host and Chrome connection lifecycle.
`apps/lane-chrome-extension` owns the companion extension source, build and store
submission materials. The extension has no standalone desktop installer.

The [local store](apps-lane-local-store-contract.md),
[protocol](protos-lane-v1-contract.md), [server](servers-lane-api-contract.md),
[CLI](cmds-lane-cli-contract.md) and [distribution](packages-lane-distribution-contract.md)
contracts retain their existing ownership. Their shared invariants apply to
attachments and URL connections. RealQA replaces the earlier mini-app exclusion
only for the behavior specified here; Pace remains outside this scope.

## Product requirements

### Product and platform boundaries

- Use existing Lane identity, native hosts, shared ReactLynx UI, storage, synchronization, and repository/task management. Preserve the Lane/Pace/RealQA rail order, approved icons, existing Lane state on app switching, and unsaved-edit protection.
- macOS and Windows: capture, image editing, floating task composition, RealQA settings, and Chrome integration. Windows runtime validation has the explicit release exception below.
- iOS, Android, and web: read saved results in existing Lane task/prompt views. Do not expose capture or RealQA configuration editing on these surfaces. The RealQA entry shows `Capture on desktop. View saved captures in Lane.` and `Open Lane`.
- Native use remains local and account-free. Synchronization is optional. Web requires an authenticated online connection and has no independent offline store or mutation queue.
- Individual users and their approved agents are the actors. Preserve personal-account isolation; do not add team workspaces, billing, feature flags, AI calls, or agent execution.

### Capture and shortcuts

- Support full-screen, individual-window, rectangular-region, and Chrome DOM-element capture. Full-screen capture asks the user to select a monitor and adds one image from that monitor.
- Provide four separately configurable shortcuts. All start unassigned. Reject conflicts with OS or other application shortcuts, explain the conflict, and preserve any previous valid binding. Support assigning, changing, canceling, and clearing a shortcut.
- Provide the same capture actions from the RealQA start screen and the composer's `Add capture` menu.
- Lane must already be running. Closing its main window does not disable capture shortcuts or the enabled background host. Fully quitting Lane stops this functionality; the extension explains that Lane must be started. Do not add automatic cold launch from an extension request.
- Each successful capture opens the floating composer or appends to the existing unsaved composer. Multiple captures belong to one new task. Preserve per-image browser metadata and edit state.
- Permission denial provides OS-settings guidance. Capture failure preserves the current draft and offers a manual retry. Selection cancellation adds no image and is not an error. Esc during capture selection cancels only that selection.

### Chrome extension and connection security

- Provide a RealQA Chrome extension using Chrome Native Messaging with the local Lane host. Do not open a loopback HTTP service for this integration.
- Include Native Messaging component registration and removal in the existing Lane installation lifecycle; do not create a standalone RealQA desktop installer.
- The first connection requires approval in Lane. Show extension/profile identification with `Approve` and `Reject`. Persist the approval on that device; settings can disconnect it and invalidate its connection credentials. Validate subsequent requests against the approved connection.
- Captures remain explicit user actions. Support normal Chrome profiles; exclude incognito and other browsers from DOM integration.
- With Chrome active and the extension available, the DOM shortcut opens the element selector. Selecting an element asks local Lane to capture it. Capture only the currently visible portion. Explain clipping and allow the user to scroll and capture again; do not auto-scroll or stitch.
- For unsupported browsers, missing extension, restricted pages, and unavailable connections, explain the reason and remedy, then stop DOM capture. Do not silently substitute region capture. General screen/window/region capture remains separately available.
- For each DOM capture, include page URL, page title, and CSS selector in the composer. Users can inspect, edit, or remove each value before saving. Removed values must not appear in the saved prompt.
- Treat browser metadata as data. Do not execute supplied HTML or CSS selectors. Do not record images, page content, URLs, or credentials in operational logs.

### URL-to-repository rules

- Settings allow adding, editing, and deleting URL-prefix-to-Lane-repository mappings. Persist them locally and synchronize them within the user's Lane account when synchronization is enabled. Shortcuts and Chrome approvals remain device-local.
- Accept HTTP/HTTPS URLs. Normalize hostname case and default ports. Match paths case-sensitively; exclude query strings and fragments from matching.
- Require a path boundary: `/app` matches `/app` and `/app/...`, not `/apple`. Choose the longest matching prefix. Reject duplicate normalized prefixes.
- The first capture's matching rule preselects the repository. Without a match, including non-Chrome captures, leave it unselected for the user to choose. Do not infer the last-used or current Lane repository.
- When another capture matches a different repository, retain the composer's current repository and show a warning. The user can change the repository explicitly.
- Concurrent changes to a rule preserve both alternatives and require explicit resolution. Exclude unresolved rules from automatic selection and show the conflict.
- Deleting the target repository deletes its rules. An open composer referencing that repository must require repository reselection before saving.

### Approved design A: floating composer and image editor

- Desktop reference viewport: 1200 × 840 logical pixels. Use an image editor on the left and task form on the right. In narrower windows, put the form below the editor and allow vertical scrolling.
- Use Lane's white/light-gray surfaces, dark text, cobalt-blue primary actions, restrained borders, and rounded controls. All user-facing text is English. Preserve existing app navigation outside this window.
- Header: `RealQA` and a close action. Editor toolbar: crop, freehand drawing, arrow, rectangle, text, blur, opaque cover, undo, and redo. Provide layer ordering controls for annotations, including keyboard-operable move-up/down actions.
- Keep each image's edit state in memory before save. Composite the final result into PNG, including blur and opaque covers. Do not persist original images or editable layers.
- A thumbnail strip selects images and supports deletion and reordering without requiring drag-and-drop. `Add capture` offers the four capture modes and appends to this draft.
- Form: `Repository`, required `Title`, and optional `Description`. Saving requires a selected repository, a nonblank title, and at least one image. Show validation errors beside the affected control. Focus the title when the composer first opens.
- `Capture details` shows editable/removable URL, page title, and CSS selector for the selected image. Show a nonblocking different-repository notice when applicable.
- `Save to Lane` creates a new regular Lane task with the existing default To do progress. Store the user's problem description, ordered attachment references, and retained metadata in the task prompt. Do not create a separate RealQA issue entity or append to an existing task.
- Show `Saving…` and prevent duplicate submission. A double click, lost response, or retry must not create another task. Preserve stable request identities and Lane's existing idempotency rules.
- On failure, preserve the unsaved draft and provide `Retry`; do not report success. On success, close the composer and show confirmation with `Open in Lane`.
- With no images, show an empty state and `Add capture`; saving remains unavailable. Closing or pressing Esc asks for confirmation before discarding a draft. Cancel keeps it. App exit/crash does not recover unsaved drafts.
- Store unsaved captures, originals, layers, and text in memory only: no temporary-file persistence and no automatic upload before save. Release originals/layers on successful save or explicit discard.
- Saved images cannot be reopened for image editing in this version. Existing Lane title, prompt, and other task-field editing remains available.

### Approved design B: RealQA start and settings

- Replace the RealQA placeholder with capture and settings content, not a separate results list. At a desktop reference width of 1100 pixels, show four capture actions in a row; reduce to two/one columns as space narrows.
- Order the page as capture actions, `Keyboard shortcuts`, `URL connections`, then `Chrome connection`. Explain that saved captures appear in Lane tasks and provide `Open Lane`.
- Shortcut rows show the current assignment or `Not set`, plus `Assign` and applicable change/clear controls.
- URL rules show prefix, target repository, edit/delete actions, and `Add connection`. The editor has URL prefix and repository fields with explicit Save. Focus the first URL input. Display validation errors inline and confirm deletion. Empty state: `No URL connections`.
- Show local-only, synchronization-pending, synchronized, and failed states accurately. Do not label a local-only configuration as synchronized.
- Show both alternatives for rule conflict resolution. Require explicit resolution. Missing repositories require reselection.
- Chrome connection states are disconnected, approval pending, connected, and error, expressed in text. Confirm `Disconnect`. `Install extension` opens the published Chrome Web Store listing.
- Capture selection surfaces provide monitor/window choices or a region selector plus cancellation. Selection must also be keyboard-operable.

### Approved design C: saved attachment rendering

- Extend the existing task prompt rendering rather than introducing a RealQA task browser. Display the problem description, then final images in saved order with retained metadata. Include textual image-position labels such as `Capture 1 of 2`.
- Fit images to the container and wrap long URL/selector text, including at a 390-pixel mobile reference width. Preserve readable content under text enlargement and narrow layouts.
- Distinguish loading, waiting for synchronization, retrieval failure, permission denial, and deletion. Offer `Retry` for retryable retrieval failures and synchronization wait states. Permission recovery uses existing Lane authentication/authorization flows, never another account.
- Metadata is inert text or an appropriate URL link, not executable markup. Give attachments accessible names. Keep descriptions and status readable when image bytes are unavailable.
- Do not add image editing/deletion controls to this read-only attachment section. Preserve existing task editing, history, prompt restoration, moves, and deletion.

### Shared interaction and accessibility requirements

- Provide keyboard operation, accessible control names, screen-reader support, visible focus, non-color-only state cues, and text enlargement. Image/annotation reordering cannot depend solely on dragging.
- Dialogs close with Esc; when they contain unsaved changes, use discard confirmation. Preserve drafts when the user cancels that confirmation.
- Keep existing Lane loading, auth, permission, task, navigation, and app-switch behavior outside the specified changes.

### Attachments, storage, synchronization, and interfaces

- Add durable attachment references and authorized retrieval to Lane's common typed contracts, including GUI, local CLI/MCP, and remote CLI/MCP consumers. Use UUID v7 identities and the existing Connect business-interface conventions. Native Messaging is an OS/browser adapter, not a replacement domain API.
- Persist final PNG images and prompt metadata only. Native storage is the local source of truth for standalone work and pending synchronization; PostgreSQL owns server metadata, and private R2 objects hold synchronized image bytes.
- Use authorized signed upload/download access. Prompts must contain durable attachment references, not permanent public URLs or expiring signed URLs. Check ownership and account authorization when resolving attachments.
- Save the task and its local attachments before optional upload. Show pending synchronization separately from successful local saving. Preserve locally saved results on upload/auth/network failure and use existing Lane synchronization retry and limit semantics.
- Apply existing configurable server byte/rate limits. Do not invent standalone local quotas or claim a default numeric limit. A rejected oversized item must not block unrelated synchronization or access to previously accepted data.
- Preserve Lane's whole-task version/conflict behavior, request deduplication, deletion precedence, account routing, and invalidation rules. An attachment reference must not bypass those boundaries.
- Removing an image reference from the current prompt does not delete an image still needed by retained history. Retain those images until owning task/repository deletion. Delete associated image content with its owner and preserve only the minimal identities needed by existing replay/deletion rules.
- Preserve existing task-move semantics: moving a task preserves its identity, prompt, history, and required attachments. Deleting its former repository must not delete the moved task's content.
- Keep account deletion, local removal, and server-clear/keep-local behavior consistent for attachments and synchronized URL rules. Do not relabel retained data into another account.
- Extend the shared schema and typed protocol in the owning contracts before runtime changes. Preserve existing field numbers and interfaces. Apply existing format negotiation and unsupported-schema refusal; old clients must not silently drop attachment/rule data or overwrite an unsupported store.
- Lane has not been deployed. Store schema changes before release use the explicit development reset in the [local-store contract](apps-lane-local-store-contract.md) instead of migration. No new import/export or backup product is included.

### Operations, privacy, and documentation

- Apply the existing Lane operational contract: structured metadata-only logs, correlation IDs, health checks, basic metrics, dashboards/alerts, and existing troubleshooting procedures. Include operation/result/timing evidence for capture, connection, save, and attachment synchronization without user content, screenshots, URLs, raw selectors, credentials, or signed URLs.
- Existing retention remains: server logs expire after 30 days; native/CLI logs remain local with a combined 100 MiB limit and 30-day retention. No automatic diagnostic upload.
- No additional numeric RealQA SLO, capacity promise, separate support/admin product, billing, or feature-flag rollout is introduced. Authorization, account isolation, resource state, and limits remain independently enforced.
- Keep credentials and publication/signing material out of Git and product logs. Use existing environment ownership and platform credential storage.
- Update the Lane project index and affected application, local-store, protocol, server, CLI/MCP, and distribution contracts, plus `docs/README.md`. Establish the Chrome extension's canonical source/build ownership in those contracts. Update scoped instructions only for actual ownership/procedure changes.
- Extend user-facing documentation with capture/edit/save, attachment retrieval, extension installation, permissions, approval/revocation, URL matching, privacy handling, and troubleshooting. Update common agent guidance for authorized image retrieval. Prepare store listing/privacy/submission materials. Keep repository internals in `docs/`, not public product guides.

### Release and acceptance boundaries

- macOS: verify actual installation and Native Messaging registration/removal in Lane's existing packaging, then verify capture/edit/save/browser behavior.
- Windows: implement and build the RealQA additions and Lane's Native Messaging packaging integration. **A Windows signing key is unavailable. Installation, Native Messaging runtime, and actual feature validation are explicitly deferred to follow-up work after the signing key is available.** Do not describe a build as Windows runtime acceptance or claim the Windows feature is verified for release.
- Verify actual saved-result rendering on mobile and web. Keep native/platform execution evidence separate from shared bundle, fixture, and packaging checks.
- Publish the Chrome extension to Chrome Web Store and record actual listing/publication evidence. Store account access and review are release dependencies; a ZIP/build or submission alone does not establish publication.
- Lane's own formal app-store launch and server production deployment remain excluded. RealQA is part of Lane's existing hosts, not a separately shipped Windows/macOS application.
- Preserve repository CI restrictions, including no new macOS PR/main/affected runners. Record source revisions, commands, results, and omitted/deferred checks in PR/CI records without user data or secrets.

## Acceptance and test scenarios

- [ ] RealQA replaces the Lane placeholder with approved start/settings behavior while preserving existing navigation and artwork.
- [ ] A macOS user can capture a chosen monitor, window, region, or visible Chrome DOM element and collect multiple images in one floating composer.
- [ ] All four shortcuts are configurable, start unassigned, detect conflicts, and remain available with the main Lane window closed while Lane is running.
- [ ] Every specified editor tool, undo/redo, layer ordering, image deletion, and image ordering works before save. Final images contain the flattened edits; originals and layers are not persisted.
- [ ] The composer validates repository/title/image requirements, preserves draft input after errors, and creates exactly one new Lane task per successful logical save.
- [ ] Chrome metadata can be inspected, edited, and removed per image. Saved output contains only retained metadata and final PNG attachments.
- [ ] Native Messaging requires explicit first approval, validates the approved connection, and stops accepting it after disconnect. Unsupported/restricted contexts produce actionable errors without fallback capture.
- [ ] URL mapping uses normalized, boundary-aware longest-prefix selection; no match leaves the repository unselected. Subsequent differing matches do not change the selected repository.
- [ ] URL rules synchronize per account, preserve conflicting alternatives, and are removed with deleted target repositories. Shortcuts and Chrome approvals stay device-local.
- [ ] Local capture/save works without login or server access. Optional image synchronization preserves local work on failure and accurately reports pending/error states.
- [ ] Authorized GUI, CLI, and MCP consumers can retrieve attachments through stable references. Another account or revoked connection cannot obtain them through an identifier alone.
- [ ] Saved images render in existing desktop/mobile/web task prompts, with accessible metadata and distinct loading/pending/error/permission/deleted states. There is no saved-image re-editing workflow.
- [ ] History retains required images; task/repository/account deletion follows Lane ownership and replay rules without restoring deleted content. Moves preserve attachments.
- [ ] Approved responsive, keyboard, focus, screen-reader, and enlargement behavior passes the agreed platform checks.
- [ ] Logs/metrics and documentation satisfy the existing Lane contract and the RealQA-specific content exclusions and support scenarios.
- [ ] macOS installation/runtime and mobile/web saved-result acceptance are recorded. Windows implementation/build evidence is recorded with installation/runtime checks explicitly deferred, not passed.
- [ ] The Chrome extension is published in Chrome Web Store, with publication evidence and installation/privacy/troubleshooting documentation.
- [ ] Cross-domain contracts, conformance tests, applicable build/test checks, and regression evidence are updated together.

## Test Scenarios

1. Capture each mode on macOS, select among multiple monitors, select a window/region, cancel with Esc, deny OS permission, and retry failures without losing an existing draft. Check display scaling and captured bounds.
2. Assign/change/clear each shortcut; reject OS/application collisions without losing the previous valid binding. Exercise main-window closure, explicit Quit, and the extension's Lane-not-running guidance.
3. Approve/reject initial Chrome connection, reconnect an approved profile, revoke it, and reject subsequent unauthorized requests. Cover missing extension, non-Chrome window, restricted page, and incognito exclusion.
4. Select a partially visible DOM element; capture only visible pixels and show clipping guidance. Ensure selection/annotation UI is not mistaken for saved page content.
5. Exercise crop, drawing, arrows, rectangles, text, blur, opaque cover, undo/redo, and layer reorder. Inspect stored final PNGs and verify that original/layer files and unsaved temporary captures were not persisted or uploaded.
6. Append multiple captures, reorder/remove them, retain per-image metadata, and remove/edit metadata before save. Verify different-repository notices without automatic reassignment.
7. Validate empty/whitespace title, missing repository, and zero images. Save with no problem description. Exercise duplicate clicks, response loss, storage-full, and retry with the same request identity.
8. Verify discard confirmation and cancel behavior, memory-only draft lifetime, successful-save cleanup, and no unsaved-draft recovery after process termination.
9. Test URL hostname/default-port normalization, path case, ignored query/fragment, `/app` versus `/apple`, longest match, duplicate normalized rules, invalid schemes, no match, and rule update/deletion.
10. Synchronize URL rules across devices; create concurrent edits and resolve explicitly. Delete a repository while a composer references it and require reselection.
11. Save offline, close/reopen after successful local save, reconnect, and verify attachment synchronization. Inject auth, network, R2, limit, and partial-upload failures with disposable PostgreSQL and mocked R2/identity dependencies. Preserve unrelated synchronization and distinguish local save success from remote completion.
12. Retrieve attachments from GUI, local CLI/MCP, and remote CLI/MCP. Test cross-account IDs, revoked credentials, expired signed access, unavailable objects, and malformed references without exposing data or interpreting metadata as executable input.
13. Remove a current reference, restore an earlier prompt, move a task, delete its former repository, delete its current owner, and exercise all existing account/local deletion modes. Ensure history and deletion precedence work for image content and delayed synchronization.
14. Test unsupported old-schema refusal with the explicit reset instruction; validate shared typed bindings, schema checks, and cross-language fixtures.
15. Exercise approved layouts, keyboard-only capture/composer/settings flows, screen readers, focus transfer, non-color states, narrow viewports, and text enlargement. Verify read-only result rendering on actual mobile and web targets.
16. Assert structured failure categories/correlation IDs, retention, and exclusion of images, problem text, URL/selector values, tokens, and signed URLs from logs. Cover relevant existing alert/recovery and support procedures without adding a new numeric SLO.
17. Run applicable frontend (`pnpm test` in `apps/lane`), root Go/protocol, and Rust checks. If Rust changes, include root `cargo test` and owning server-workspace checks. Preserve existing Lane task/proposal/history/conflict/navigation/auth regression coverage and CI allocation rules. Record exact commands and results at implementation time.
18. Verify macOS installation/removal and Native Messaging integration. Build Windows and record the signing-key-dependent installation/runtime follow-up as unperformed. Verify Chrome Web Store publication separately from build/submission artifacts.

## Out of Scope

- A standalone RealQA desktop application or separate RealQA desktop installer.
- Mobile/web capture, image import/editing, or RealQA settings mutation; Linux native GUI.
- Saving to an existing task, a separate RealQA issue entity, or a RealQA-specific saved-results browser.
- Persistent originals/layers, autosaved drafts, crash recovery for unsaved work, and post-save image re-editing.
- Automatic scrolling/stitching of entire DOM elements, other-browser DOM integration, incognito support, automatic region fallback, or launching a fully quit Lane from the extension.
- AI execution, agent orchestration, Git access, team collaboration, billing, feature flags, or a separate admin/support product.
- New file import/export, backup/restore, numeric RealQA SLO, or unrelated Lane redesign.
- Windows installation/runtime acceptance and formal signed distribution before signing-key availability; these are follow-up work, not evidence supplied by these builds.
- Lane's formal app-store launch and server production deployment. Chrome Web Store publication is explicitly in scope.

## Chrome companion transport and packaging

`apps/lane-chrome-extension/src` owns a Manifest V3 extension with `activeTab`,
`scripting`, `nativeMessaging`, and `storage` permissions. Incognito is disabled.
The selector runs in Chrome's isolated world. It accepts only genuine user input
(`isTrusted === true`) for pointer selection, Tab traversal, Enter capture, click
capture and Escape cancellation; page-dispatched events share the DOM but are
ignored before any side effect so the session stays intact. It sends a selected
visible rectangle and literal metadata to the extension worker. The worker captures the active tab,
crops the selected rectangle in memory using the screenshot's pixel dimensions,
and sends only that PNG to Lane. Lane validates and appends it to the existing
composer; the extension cannot save a task. A DOM shortcut without an active tab
grant explains that the user must first click the extension on that page.
In-app DOM actions focus the last connected approved Chrome profile and select
its last-focused active tab. Only that profile receives an activation request.
DOM shortcuts retain the foreground Chrome check and do not activate a browser.
Chrome hides a tab's URL until the extension has access to it, and the extension
requests neither the `tabs` nor host permissions. A native selection request whose
active normal tab has an omitted or empty URL therefore returns `PERMISSION_REQUIRED`
and the click-extension guidance, without status requests, activation, injection or
capture. Only a known restricted URL, a file URL, a Web Store URL, an incognito tab
or a missing tab is `UNSUPPORTED_PAGE`. The Web Store exclusion compares the parsed
HTTPS hostname and path: `chromewebstore.google.com`, and `chrome.google.com` with
the path `/webstore` or `/webstore/...`, are excluded regardless of query or
fragment. Hostname lookalikes and sibling paths such as `/webstorefront` are
ordinary pages.

The extension service worker is a module worker and must not use top-level `await`:
Chromium rejects asynchronous module graphs for service workers. It restricts
`chrome.storage.local` to trusted extension contexts by starting that setting at
module evaluation without awaiting it, registers every message, command and
tab-removal listener synchronously, and makes every credential read or write,
native message, screenshot and selector injection wait for the setting to succeed.
If the setting fails, the worker logs the failure once, replies or shows a status
with a reload remedy, and performs no protected work until Chrome restarts the
worker. Node fixtures and a static top-level-await check cover this ordering; they
do not replace loading the unpacked build in Chrome and restarting its worker.

The desktop `chrome-bridge.mjs` owns profile approval and token-hash persistence.
`chrome-native-host.mjs` is a Chrome-launched stdio relay to an already running
Lane Unix socket or Windows named pipe. It never starts Lane or serves HTTP.
The endpoint uses a per-run secret; runtime files and Unix sockets use owner-only
permissions. Stored profile approvals are device-local and are not synchronized.
Revocation invalidates credentials and closes matching connections. The OS-user
boundary applies to the desktop bridge and its state.

The existing Lane installer bundles the relay with a private runtime or standalone
executable and registers exact extension origins through the extension's
`scripts/native-registration.mjs`. Removal unregisters only that installation's
manifest. A real development or published extension ID is required; packaging
must not substitute a fabricated Chrome Web Store ID. `pnpm test` in the extension
workspace validates fixtures and creates its unpacked build; `pnpm package`
creates its review ZIP. Store copy and privacy materials live in `store/`.
Publisher account access, a real assigned extension ID, hosted privacy/support
information, runtime screenshots and Google review remain publication inputs.
Builds and fixture tests do not establish browser/native or store acceptance.

## Desktop capture host interface

The desktop `LaneHost.realqa(requestJson, callback)` method accepts an `op`
discriminant. It returns a JSON-string `{ok:true,value}` envelope or a safe typed
error. It exposes capture sources, capture actions, device shortcuts, permission
settings, the current draft, image edits, undo/redo, image ordering, export,
explicit discard and successful-save completion. It exposes no paths, arbitrary
OS commands or image import API. UI callers use this method only from the Lynx
background runtime.

The host owns one memory-only draft across its floating composer and main window.
Images have UUID v7 identities, pixel dimensions, PNG previews, ordered layers,
optional crop and literal browser metadata. Draft task and request identities are
stable across unchanged retries. Rendering always starts from the in-memory
original and flattens the ordered layers before applying the crop. Only exported
final PNGs, their SHA-256 digests and retained metadata enter `CreateCaptureTask`.
Export freezes mutations until `complete` or `saveFailed`; this prevents capture
arrivals during saving from being discarded by a successful completion. Store
failure retains the draft. A successful completion carries both task and space
identities in `lane.realqa.saved`; consumers must use the existing account and
unsaved-edit navigation guard.

`lane.realqa.changed` invalidates draft/settings state. `lane.realqa.select`
requests a monitor/window chooser. `lane.realqa.closeRequested` requests explicit
discard confirmation; the native close button cannot discard a draft directly.
The `state` response identifies its requesting window as `main` or `composer` so
initial view loading does not depend on receiving a one-time event. Selection
cancellation does not mutate the draft. Quit cancels active native selection,
stops Chrome connections and releases capture memory.

macOS 14 and later use ScreenCaptureKit screenshot APIs. To preserve Lynxtron's
macOS 12 minimum, earlier systems use the OS memory capture API through a scoped
dynamic compatibility path; the ScreenCaptureKit framework is weak-linked.
Windows uses monitor/window enumeration, GDI memory capture and GDI+ PNG
encoding/rendering. Platform renderers implement crop, paths, arrows, rectangles,
text, blur and opaque cover. Neither renderer writes capture files. Region
selection supports arrows, Shift plus arrows, Return and Escape, as well as the
pointer. Shortcuts use exclusive Carbon registration and system reservation
checks on macOS, and `RegisterHotKey` on Windows. Changing a shortcut registers
its replacement before releasing its previous valid binding.

Chrome status includes identified pending approvals and device connections.
`chrome.approve` and `chrome.reject` name the pending approval; `chrome.disconnect`
names the profile to revoke. The host decodes accepted PNG bytes with the OS
renderer before adding them. DOM selection failures retain the draft and provide
context-specific Chrome guidance without substituting region capture.
