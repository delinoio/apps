# Troubleshoot Lane

## Installation fails

Check your OS, CPU, and package version. Keep npm optional dependencies enabled so the matching native package can be installed. If optional dependencies were omitted, reinstall the exact version with `npm install -g --include=optional @delinoio/lane@0.1.0`. A missing artifact or checksum/version mismatch stops the launcher before execution. Reinstall the exact reviewed package from a trusted source; do not bypass verification. If the CLI's version differs from the archive manifest, stop and obtain a matching artifact.

## Save or synchronization fails

For a native local save failure, keep the editor open, check available disk space and OS-user access, then retry Save. A pending synchronization failure is different from a failed local save: use sync status to identify rejected items. Correct an oversized item without discarding unrelated pending changes.

For a network or login failure, native/CLI local work continues. Reconnect or complete browser login to resume pending synchronization. Web requires connectivity; keep the current screen open and retry Save explicitly after reconnecting. Do not rely on web offline editing or relaunch to preserve input.

If the server or its authorization dependency is unavailable, retry later with the same request ID and content. Keep correlation IDs when reporting a failure, but do not share task/prompt content, webhook payloads, credentials, or tokens in diagnostic reports. Local logs stay local and are limited to 30 days and 100 MiB combined; there is no automatic upload.

For an unsupported local format, install a compatible application version. Do not erase or initialize the store as empty. An unsupported sync format requires an update but does not justify discarding pending changes.

## Planning changes are rejected

A validation error names the field: a name that is empty or longer than 100 characters, a duplicate name in the same repository and item kind, a color that is not `#RRGGBB`, more than 100 labels on a task, a start date after the due date or a cycle ending before it starts, a story point value outside 0 to 2147483647, a label, milestone, cycle or related task from another repository, a missing or deleted reference, a link to the task itself, or a parent, dependency or duplicate cycle. Correct the input and save again. Nothing was changed by the rejected save, and your input stays on screen.

If an item is blocked after synchronization with `CONSTRAINT_VIOLATION:<VIOLATION>`, the server found that changes from several devices conflict, for example two parents or a combined dependency cycle. The server keeps what it had already accepted, and your local version stays on the device. Run `lane sync status` to list the blocked items, correct the local data with an ordinary edit (read the current version first and use a new request ID), then run `lane sync run --retry-blocked`. Do not delete the data to hide the rejection.

A move or deletion removes references to the moved or deleted item from other tasks and keeps those tasks. If a task seems to have lost a label, milestone, cycle, or link, check the item's history; the removal appears as a new version of the task. A planning list that looks incomplete may have an active filter. Open **Filters** and clear it, or check whether a filter needs its **Unassigned** choice.

## Unsupported local format and development reset

`UNSUPPORTED_FORMAT` means the local space was written in a format that this Lane version does not read, for example development data from before task planning. Lane does not change it. If a newer compatible Lane version exists, install that version. For disposable development data only, you can run the explicit [development data reset](./user-guide#development-data-reset), which discards that space's local data. It is not a repair for synchronization, conflicts, or other errors, and agents must not run it unless you ask. If it fails with `STORAGE_BUSY`, quit the app and stop other Lane commands and `sync run`, then run it again.

## Conflicting versions

Compare both versions before resolving. A conflict covers the entire task, including separate-field edits. Normal edits stay blocked until resolution. If another device resolved first, reread the current version and heads; do not retry an obsolete resolution as an unconditional overwrite. Unsaved editor input remains available while you decide.

## Revoked devices or connections

A revoked MCP connection must receive a new browser approval before accessing the account. A disconnected device must reconnect explicitly. Existing offline copies remain local unless the user chooses a deletion scope that removes them. Do not connect a different account to recover another account's retained data.

## Webhooks

For a rejected signature, check sender clock accuracy, the current key, exact raw body bytes, and timestamp/signature headers. Key rotation invalidates the old key immediately. Retry the same sender event ID/content with a fresh timestamp/signature. A changed body under the same identity is a conflict.

An expired or released lease cannot complete processing. Claim again if the event remains available. A failed completion creates no partial tasks. After a response loss, retry the original completion request unchanged to retrieve its outcome. Reading an event never marks it complete, and offline processing cannot acquire a lease.

## Notifications

Check this device's notification preference, OS permission, proposal origin, and whether the proposal is still pending. Accepted, rejected, and deleted proposals cancel unsent notifications. Delivery can be delayed by transient provider failure and expires two hours after creation. Permanent provider failures stop retries.

Windows has no remote WNS delivery in this version. Android without required Google Play services may lack remote push. Use in-app proposal counts, available local notifications, and synchronization in these environments. A missing OS notification does not mean the proposal was lost.

On iOS, background notification wakes are controlled by the OS and are not guaranteed. A proposal notification may wait for the next permitted synchronization opportunity or app opening. Native push messages prompt synchronization before local display; receiving a push is not itself proof that a notice was shown.

## RealQA and Chrome

- **Lane is not running:** start Lane and retry the explicit capture. Closing its main window is different from quitting the app.
- **Connection unavailable:** check that the installed Lane build includes the matching extension registration. Reinstall the matching trusted build if registration is missing. Another browser is not supported for DOM capture.
- **Approval required or disconnected:** connect from the extension and approve its profile in Lane. A revoked token cannot be reused.
- **Page cannot be selected:** use a normal HTTP or HTTPS page outside incognito and the Chrome Web Store. Click the extension on that page to grant access, then retry.
- **Element is clipped:** only visible pixels are included. Scroll and make another capture yourself. RealQA never stitches hidden portions.
- **Screen recording denied:** follow Lane's OS settings guidance, enable capture access, and retry. Your current draft remains open.
- **Attachment waiting for synchronization:** keep the source device connected until its saved image uploads. The local task is already saved. Retry failed synchronization without creating a duplicate task.
- **Save failed:** keep the composer open, correct the reported cause, and retry Save. Discard closes the unsaved draft and releases its images.

## Desktop update failed

Keep your existing Lane installation and local data. Check your internet
connection, available disk space and permission to write to the installation
folder, then select **Retry update** in **Settings → App updates**. GitHub may
temporarily limit update checks; wait before retrying a rate-limit error.

Finish or close open RealQA captures and save notification changes before
retrying. Close other Lane windows and Chrome capture sessions if installation
cannot replace files. If the new app cannot start, Lane attempts to restore the
previous app and shows the error when it reopens. A checksum or package-identity
error requires a fresh verified download; do not bypass verification.
