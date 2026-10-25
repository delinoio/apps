# Use Lane

## Repositories and tasks

Register a repository URL or create a task with an unregistered URL to register it automatically. HTTPS and SSH forms and a terminal `.git` suffix identify the same repository. The default display name is the URL host/path; you can set a separate name. Lane does not fetch repository contents or check Git-host access.

Rename a repository or change its URL without losing tasks, history, or webhook connections. A URL already registered in the same account cannot be reused. An old URL is not retained as an alias: using it again registers a new repository. Concurrent repository edits require conflict resolution.

A task requires only a title. Description and one prompt are optional. Progress is **To do**, **In progress**, **Blocked**, **Done**, or **Canceled**; all transitions are allowed, including reopening. Omitted progress starts at To do. Proposal disposition is separate: agents can create regular tasks or proposals. Acceptance makes a proposal regular without resetting progress. Rejected proposals remain available and can later be accepted.

Use repository lists or the all-repository list, filter by progress or proposal disposition, and search titles/descriptions. Move a task to another repository without changing its ID, prompt, or history. Linked webhook events remain in their original repository and continue to reference the task.

The CLI provides `repository list/get/create/edit/delete`, `task list/get/create/edit/delete`, `task progress`, `task accept`, `task reject`, and `task move`. Run command help for the required arguments. Mutations accept `--request-id <uuid>`; keep the returned ID for retries. Edits require `--expected-version <uuid>` from a current read. Reusing a request ID with different content fails.

## Save, history, and prompts

App edits require **Save**. An explicit CLI edit saves as one command/version. Descriptions and prompts are Markdown source; embedded HTML does not execute and automatic image loading is limited to HTTPS. You can edit and copy the source.

History shows before/after values, time, and app/CLI/MCP origin with device and signed-in identity when available. Prompt versions remain until the owning task or repository is deleted. Inspect an old prompt and restore it to create a new version; restoration does not rewrite history. Only prompts have a past-version restore action. CLI commands include `task history`, `task prompt-history`, and `task restore-prompt`.

If another process or device changes an open editor, Lane retains your unsaved input and indicates the external change. Saving checks for conflicts. If storage is full or a save fails, keep the screen open, correct the problem, and retry. In web, a connection loss blocks Save while retaining current-screen input. Reconnection requires an explicit retry; web has no offline queue or offline relaunch guarantee.

## Synchronization and conflicts

Native apps and the local CLI work without login. Enable synchronization explicitly to connect an account. Initial synchronization preserves both datasets, merges repositories with the same normalized URL, and preserves tasks with different IDs. It synchronizes all repositories, tasks, prompts, and history. Notification preferences remain device-local.

Local work continues during network failure or expired login. Pending changes remain for reconnection or reauthentication. An oversized upload blocks that item with an explanation; other items continue. Lower server limits do not prevent reading or deleting previously accepted data. Unsupported synchronization formats stop synchronization with an update instruction while compatible local work remains available.

A task conflict includes the whole task, even if devices changed different fields. Both versions remain for comparison; normal edits are blocked until explicit resolution. `conflict list/get/resolve` supports this workflow in the CLI. Resolution creates a new version and preserves the alternatives and resolution history. A stale competing resolution is rejected; read the current state again before resolving. Repository conflicts follow the same explicit-resolution principle.

Desktop synchronization can continue after the window closes. Mobile synchronization runs while the app is active and at OS-permitted background opportunities. Web uses its active online connection.

Use `sync connect/status/run/disconnect`, `login`, and `logout` in the CLI. Logging in does not switch ordinary commands to remote data. Logout or disabling sync requires choosing whether to keep or remove local account data. Kept data remains separated from other accounts and is not automatically uploaded on an account switch.

## Devices, connections, and deletion

Settings show devices, their last synchronization times, and individually approved remote MCP connections. Disconnect a device or revoke a connection to block future server access. An offline device's existing local copy cannot be erased immediately by revocation.

Deletion has three distinct scopes:

1. **Remove only this device's data** removes the selected local copy. It does not delete server data or other devices' data. CLI: `data remove-local`.
2. **Delete everywhere** clears Lane server data/settings and propagates deletion to connected devices. Offline devices receive the deletion directive on their next authenticated reconnect and cannot upload old content. CLI: `--remote data delete-everywhere`.
3. **Clear server, keep local** clears synchronized server data and webhook receiving settings, retains local copies, and disconnects all devices and remote grants. Reconnection is explicit. CLI: `--remote data clear-server`.

The two server scopes require explicit account confirmation and fresh browser reauthentication. They preserve your Logto identity. Individual task/repository/event deletion commands confirm the named target and do not add another prompt. Deletion removes owned content/history; there is no trash or restore feature. Retried creation after deletion reports the deleted result without recreating content.

## Webhooks

Create, edit, rotate keys, and delete receivers in settings or with `--remote receiver` commands. Each receiver belongs to a repository; reassignment affects only future events. Secrets appear once on creation/rotation. If lost, rotate the key; the old key stops working immediately.

Send the Lane common format: sender event ID, event type, occurrence time, and JSON object payload. Sign the exact body bytes with the receiver's decoded secret and a fresh timestamp. The sender uses `Lane-Timestamp` and `Lane-Signature`; only timestamps within five minutes in either direction are accepted. Repeated event identity/content returns its previous result; changed content under that identity fails. Deleted events are not recreated by retries.

The UTF-8 JSON body has this shape:

```json
{
  "id": "sender-event-id",
  "type": "event-type",
  "occurredAt": "2026-10-25T00:00:00Z",
  "payload": {}
}
```

Require nonempty `id` and `type`, a valid UTC timestamp, and an object payload with no duplicate JSON keys. `Lane-Timestamp` is decimal Unix seconds. Set `Lane-Signature` to `v1=` followed by lowercase HMAC-SHA256 hex over the ASCII timestamp, one period, and the exact raw body bytes. Decode the one-time unpadded base64url secret to bytes before signing. Do not reserialize the body after signing.

Events stay until explicitly deleted. Use `event list/get/delete/claim/renew/release/complete/reopen` for management, with `--remote` for mutations. Reading is not processing. See [MCP event processing](./mcp#process-webhook-events) for leases, atomic completion, and retry rules.

## Notifications

Enable notifications in settings to request OS permission. Configure each device independently. Default notices show repository name and task title; choose generic content for more privacy. Only a newly proposed task created through MCP triggers an OS notice. CLI/app changes and progress updates do not. Pending proposals remain visible in-app.

Local MCP proposals can notify locally without synchronization. With sync, other eligible devices receive push; proposal/device deduplication avoids duplicates on the origin device. Windows remote WNS is not supported in this version; running/background synchronization and local notifications remain available. Android without required Google Play services and other unsupported push environments can still use Lane and its in-app indications.
