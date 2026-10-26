# Troubleshoot Lane

## Installation fails

Check your OS, CPU, and package version. Keep npm optional dependencies enabled so the matching native package can be installed. If optional dependencies were omitted, reinstall the exact version with `npm install -g --include=optional @delinoio/lane@0.1.0`. A missing artifact or checksum/version mismatch stops the launcher before execution. Reinstall the exact reviewed package from a trusted source; do not bypass verification. If the CLI's version differs from the archive manifest, stop and obtain a matching artifact.

## Save or synchronization fails

For a native local save failure, keep the editor open, check available disk space and OS-user access, then retry Save. A pending synchronization failure is different from a failed local save: use sync status to identify rejected items. Correct an oversized item without discarding unrelated pending changes.

For a network or login failure, native/CLI local work continues. Reconnect or complete browser login to resume pending synchronization. Web requires connectivity; keep the current screen open and retry Save explicitly after reconnecting. Do not rely on web offline editing or relaunch to preserve input.

If the server or its authorization dependency is unavailable, retry later with the same request ID and content. Keep correlation IDs when reporting a failure, but do not share task/prompt content, webhook payloads, credentials, or tokens in diagnostic reports. Local logs stay local and are limited to 30 days and 100 MiB combined; there is no automatic upload.

For an unsupported local format, install a compatible application version. Do not erase or initialize the store as empty. An unsupported sync format requires an update but does not justify discarding pending changes.

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
