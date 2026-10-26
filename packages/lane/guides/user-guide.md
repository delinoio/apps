# Use Lane

## Repositories and tasks

Register a repository URL or create a task with an unregistered URL to register it automatically. HTTPS and SSH forms and a terminal `.git` suffix identify the same repository. The default display name is the URL host/path; you can set a separate name. Lane does not fetch repository contents or check Git-host access.

Rename a repository or change its URL without losing tasks, history, or webhook connections. A URL already registered in the same account cannot be reused. An old URL is not retained as an alias: using it again registers a new repository. Concurrent repository edits require conflict resolution.

A task requires only a title. Description and one prompt are optional. Progress is **To do**, **In progress**, **Blocked**, **Done**, or **Canceled**; all transitions are allowed, including reopening. Omitted progress starts at To do. Proposal disposition is separate: agents can create regular tasks or proposals. Acceptance makes a proposal regular without resetting progress. Rejected proposals remain available and can later be accepted.

Use repository lists or the all-repository list, filter by progress or proposal disposition, filter and sort by [planning fields](#plan-tasks), and search titles/descriptions. Move a task to another repository without changing its ID, prompt, or history. Linked webhook events remain in their original repository and continue to reference the task.

The CLI provides `repository list/get/create/edit/delete`, `task list/get/create/edit/delete`, `task progress`, `task accept`, `task reject`, and `task move`, plus `label`, `milestone`, and `cycle` for repository planning items. Run command help for the required arguments. Mutations accept `--request-id <uuid>`; keep the returned ID for retries. Edits require `--expected-version <uuid>` from a current read. Reusing a request ID with different content fails.

## Save, history, and prompts

App edits require **Save**. An explicit CLI edit saves as one command/version. Descriptions and prompts are Markdown source; embedded HTML does not execute and automatic image loading is limited to HTTPS. You can edit and copy the source.

History shows before/after values, including planning fields and relationship changes, time, and app/CLI/MCP origin with device and signed-in identity when available. Prompt versions remain until the owning task or repository is deleted. Inspect an old prompt and restore it to create a new version; restoration does not rewrite history. Only prompts have a past-version restore action. CLI commands include `task history`, `task prompt-history`, and `task restore-prompt`.

If another process or device changes an open editor, Lane retains your unsaved input and indicates the external change. Saving checks for conflicts. If storage is full or a save fails, keep the screen open, correct the problem, and retry. In web, a connection loss blocks Save while retaining current-screen input. Reconnection requires an explicit retry; web has no offline queue or offline relaunch guarantee.

## Plan tasks

Every planning field is optional. A title alone still creates a task, and an unset field is shown as unassigned. In the app, open a task and use **Properties** and **Relationships**. Changes stay in the draft until **Save** succeeds. In the CLI, `task create` and `task edit` accept the options below; omitting an option leaves it unchanged on edit, and `none` or a `--clear-*` option clears it.

| Field        | Values and CLI option                                                                                                                                           |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Priority     | None, Urgent, High, Normal, Low. `--priority none\|urgent\|high\|normal\|low`.                                                                                  |
| Type         | None, Bug, Feature, Improvement, Documentation, Maintenance. `--type none\|bug\|feature\|improvement\|documentation\|maintenance`.                              |
| Labels       | Up to 100 labels of the task's repository. `--label-id UUID` (repeat to replace the whole set) and `--clear-labels`.                                            |
| Start / due  | Real calendar dates as `YYYY-MM-DD`, with no time or time zone. The start date cannot be later than the due date. `--start-date D\|none`, `--due-date D\|none`. |
| Story points | A whole number from 0 through 2147483647. 0 is different from unassigned. Fractions and negative numbers are rejected. `--story-points N\|none`.                |
| Milestone    | One milestone of the task's repository. `--milestone-id UUID\|none`.                                                                                            |
| Cycle        | One cycle of the task's repository. `--cycle-id UUID\|none`.                                                                                                    |

Task dates do not have to fall inside a milestone's target date or a cycle's period. Lane sends no deadline reminders, changes no progress automatically, and does not roll tasks over to a later cycle. Invalid values are rejected with a message that names the field.

## Labels, milestones, and cycles

Labels, milestones, and cycles belong to one repository. In the app, open the repository and use the **Labels**, **Milestones**, and **Cycles** tabs to create, edit, delete, and view the history of each item. The CLI provides `label`, `milestone`, and `cycle` with `list`, `get`, `create`, `edit`, `delete`, and `history`.

- Names are trimmed and have 1 to 100 characters. Descriptions have at most 4000 characters. A name must be unique within its repository and item kind, ignoring letter case. Different kinds and different repositories can use the same name.
- A label needs a color in `#RRGGBB` form, which Lane stores in upper case. You can change it later. The name is always shown, so color is never the only cue.
- A milestone has an optional target date and a completed setting that starts as not completed. You change it explicitly; no task state changes it.
- A cycle needs a start date and an end date that is not earlier than the start date. A cycle has no lifecycle state and does not repeat.
- An item's repository and kind cannot change. Edits need the current version, like other edits (`edit ID --expected-version UUID`).
- **Delete** detaches the item from every task that uses it and keeps those tasks. The item's name and description are erased everywhere, including from the tasks' history, which keeps only the reference.
- History shows the before and after values, time, and origin. There is no restore action for planning metadata; only prompts can be restored.

Examples:

```sh
lane label create --repository-id <repository-uuid> --name Backend --color '#075AFF'
lane cycle create --repository-id <repository-uuid> --name "Sprint 12" --start-date 2026-11-02 --end-date 2026-11-15
lane task edit <task-uuid> --expected-version <version-uuid> --priority high --type bug --label-id <label-uuid> --story-points 3
```

## Relationships

Link tasks of the same repository from the task's **Relationships** section or with the CLI:

- **Parent** and **Subtasks**: a task has at most one parent. Subtasks are the inverse view. CLI: `--parent-id UUID|none`, `--add-subtask UUID`, `--remove-subtask UUID`.
- **Depends on** and **Blocks**: two views of one directed dependency. CLI: `--depends-on UUID` (repeat to replace the set), `--clear-depends-on`, `--add-blocks UUID`, `--remove-blocks UUID`.
- **Related**: a symmetric link stored on both tasks. CLI: `--related UUID` (repeat to replace the set), `--clear-related`.
- **Duplicate of**: at most one original task. CLI: `--duplicate-of UUID|none`.

Lane rejects a link to the task itself, to a task of another repository, and to a missing or deleted task. It also rejects a cycle in the parent, dependency, or duplicate links. Editing Subtasks or Blocks changes the other tasks in the same operation, and each changed task gets its own history entry. If one of those tasks is in a conflict, the whole edit fails and nothing changes.

Relationships only describe work. They never change progress or proposal state, never stop you from completing a task, and never close a duplicate or parent automatically.

**Moves and deletion.** Moving a task to another repository clears its labels, milestone, cycle, and relationships, and removes other tasks' links to it. It keeps the priority, type, dates, story points, prompt, and ID. Deleting a task, label, milestone, or cycle detaches the references from other tasks. In every case the other tasks remain, with a new history entry for each removed reference. If the operation fails, nothing is changed.

## Filter and sort tasks

Task lists, for one repository or all repositories, have a **Filters** region and separate sort key and direction controls. Search, progress, and proposal filters keep working and combine with the planning filters. In the all-repository list, labels, milestones, and cycles show their repository name, and items with the same name stay distinct.

- Different filters combine with **AND**. Several values chosen in one filter combine with **OR**. A task matches a label filter when it has at least one selected label.
- Each filter has its own **Unassigned** choice (no priority, no type, no labels, no start date, no due date, no points, no milestone, no cycle, or no relationships). It is added to the selected values with OR. A filter with no selection does not restrict the list.
- Date and story point ranges include their bounds. A range with only bounds excludes tasks without a value; add the unassigned choice to include them.
- A relationship filter matches tasks that have at least one of the chosen kinds (parent, subtasks, depends on, blocks, related, duplicate of), or none at all.
- Sort by one key: priority, type, start date, due date, story points, milestone name, or cycle start date, ascending or descending. Tasks without a value always come last in both directions. Tasks with equal values are ordered by task ID. Without a sort key, lists keep their task ID order. Paging continues in the same order.

CLI options for `task list`: `--priority V` (repeatable) with `--priority-unassigned`, `--type V` with `--type-unassigned`, `--label-id UUID` with `--unlabeled`, `--start-from D`, `--start-to D`, `--start-unassigned`, `--due-from D`, `--due-to D`, `--due-unassigned`, `--points-min N`, `--points-max N`, `--points-unassigned`, `--milestone-id UUID` with `--no-milestone`, `--cycle-id UUID` with `--no-cycle`, `--relationship KIND` (repeatable; `parent`, `subtasks`, `depends-on`, `blocks`, `related`, `duplicate-of`) with `--no-relationships`, `--sort KEY` (`priority`, `type`, `start-date`, `due-date`, `story-points`, `milestone-name`, `cycle-start-date`) and `--direction asc|desc`. Add `--cursor` to continue a page.

```sh
lane task list --priority urgent --priority high --priority-unassigned --due-to 2026-12-31 --sort due-date --direction asc
```

## Planning limits

Lane has no assignees, bulk editing, grouping, saved views, nested Boolean filters, multi-key sorting, user-defined fields or vocabularies, cross-repository relationships, time tracking, estimated hours, fractional story points, date-time or time-zone inputs, external issue-tracker integration, deadline reminders, recurring cycles, or restore for planning metadata. Planning data synchronizes with the rest of your account. The web app needs an online connection for every planning change.

## Synchronization and conflicts

Native apps and the local CLI work without login. Enable synchronization explicitly to connect an account. Initial synchronization preserves both datasets, merges repositories with the same normalized URL, and preserves tasks with different IDs. It synchronizes all repositories, tasks, prompts, and history. Notification preferences remain device-local.

Local work continues during network failure or expired login. Pending changes remain for reconnection or reauthentication. An oversized upload blocks that item with an explanation; other items continue. Lower server limits do not prevent reading or deleting previously accepted data. Unsupported synchronization formats stop synchronization with an update instruction while compatible local work remains available.

A task conflict includes the whole task, even if devices changed different fields. Both versions remain for comparison; normal edits are blocked until explicit resolution. `conflict list/get/resolve` supports this workflow in the CLI. Resolution creates a new version and preserves the alternatives and resolution history. A stale competing resolution is rejected; read the current state again before resolving. Repository conflicts follow the same explicit-resolution principle. Label, milestone, and cycle conflicts cover the whole item and use `conflict list/get/resolve --kind label|milestone|cycle`.

Desktop synchronization can continue after the window closes. Mobile synchronization runs while the app is active and at OS-permitted background opportunities. Web uses its active online connection and needs it for every change, including planning changes.

Use `sync connect/status/run/disconnect`, `login`, and `logout` in the CLI. Logging in does not switch ordinary commands to remote data. Logout or disabling sync requires choosing whether to keep or remove local account data. Kept data remains separated from other accounts and is not automatically uploaded on an account switch.

### Rejected planning changes

Changes made on different devices can be valid alone but invalid together, for example a dependency cycle, a second parent, or the same label name created twice. The server then rejects the changed item with `CONSTRAINT_VIOLATION` and the reason, such as `PARENT_CYCLE`, `DEPENDENCY_CYCLE`, `DUPLICATE_CYCLE`, `CROSS_REPOSITORY`, `REFERENCE_NOT_FOUND`, `DUPLICATE_NAME`, `LABEL_LIMIT`, `DATE_ORDER`, or `FIELD_RANGE`. The server keeps the data it already accepted. Your local version stays on the device in the blocked state with the reason `CONSTRAINT_VIOLATION:<VIOLATION>`. Lane never marks it as synchronized and never discards it, and `sync status` lists it.

Correct the local data with an ordinary edit, then retry the blocked items:

```sh
lane sync status
lane task edit <task-uuid> --expected-version <version-uuid> --parent-id none
lane sync run --retry-blocked
```

The retry does not confirm an account epoch change and does not clear account-deletion state. Whole-task version conflicts are a different state and use `conflict list/get/resolve`.

## Devices, connections, and deletion

Settings show devices, their last synchronization times, and individually approved remote MCP connections. Disconnect a device or revoke a connection to block future server access. An offline device's existing local copy cannot be erased immediately by revocation.

Deletion has three distinct scopes:

1. **Remove only this device's data** removes the selected local copy. It does not delete server data or other devices' data. CLI: `data remove-local`.
2. **Delete everywhere** clears Lane server data/settings and propagates deletion to connected devices. Offline devices receive the deletion directive on their next authenticated reconnect and cannot upload old content. CLI: `--remote data delete-everywhere`.
3. **Clear server, keep local** clears synchronized server data and webhook receiving settings, retains local copies, and disconnects all devices and remote grants. Reconnection is explicit. CLI: `--remote data clear-server`.

The two server scopes require explicit account confirmation and fresh browser reauthentication. They preserve your Logto identity. Individual task/repository/event deletion commands confirm the named target and do not add another prompt. Deletion removes owned content/history; there is no trash or restore feature. Retried creation after deletion reports the deleted result without recreating content.

## Development data reset

Lane has not been released. The planning feature changed the local data format, and Lane does not migrate earlier development data. Opening a local space in an older or incompatible format fails with `UNSUPPORTED_FORMAT` and names the reset command. Lane never resets or recreates a store silently.

The reset is an explicit, destructive procedure for disposable development data. It discards all data of the one local space you name and creates an empty store with the same space ID. Do not use it for data you want to keep, and do not use it to clear a synchronization rejection or conflict.

```sh
lane --space <space-uuid> data reset --confirm-development-reset
```

- It works only on an existing local space. It rejects `--remote`, a missing `--space`, a missing confirmation flag, and request payloads.
- Quit the app and stop other CLI commands, MCP clients, and `sync run` before you start. If any of them still uses the space, the command fails with `STORAGE_BUSY` and changes nothing; stop them and run it again. An interrupted reset is finished by running the same command again.
- A space connected to an account keeps its account connection but loses its local data, and synchronization stays stopped until you confirm reconnection with `sync connect --confirm-epoch-change`. A reset never deletes server data and never signs you out.
- Native desktop and mobile apps offer the same operation as **Reset development data** in Settings; you type the shown confirmation text first. Cancelling or mistyping changes nothing.
- No MCP tool performs a reset. Agents must not run the command unless you explicitly ask them to.

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

Events stay until explicitly deleted. Use `event list/get/delete/claim/renew/release/complete/reopen` for management, with `--remote` for mutations. Reading is not processing. See [MCP event processing](mcp.md#process-webhook-events) for leases, atomic completion, and retry rules.

## Notifications

Enable notifications in settings to request OS permission. Configure each device independently. Default notices show repository name and task title; choose generic content for more privacy. Only a newly proposed task created through MCP triggers an OS notice. CLI/app changes and progress updates do not. Pending proposals remain visible in-app.

Local MCP proposals can notify locally without synchronization. With sync, other eligible devices receive push; proposal/device deduplication avoids duplicates on the origin device. Windows remote WNS is not supported in this version; running/background synchronization and local notifications remain available. Android without required Google Play services and other unsupported push environments can still use Lane and its in-app indications.
