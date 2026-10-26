# Connect an agent

Your agent reads and manages Lane through MCP. Lane does not run the agent. Install the common Lane `SKILL.md` supplied with the CLI package using your agent's skill installation procedure.

## Local stdio MCP

Configure an MCP server with executable `lane` and arguments `["mcp", "serve"]`. For clients that use an `mcpServers` configuration object:

```json
{
  "mcpServers": {
    "lane": {
      "command": "lane",
      "args": ["mcp", "serve"]
    }
  }
}
```

Client configuration formats vary; use these command and argument values in your client's configuration. The executable must be available on that client's PATH. Standard output is reserved for MCP messages.

Local MCP needs no login or server. It uses the same local data as the desktop app and CLI, including while the app window is closed. Access follows OS-user permissions: approving a local MCP process lets that process manage the selected local data. It is not a remote connection approval.

## Remote MCP

For a client that supports Streamable HTTP MCP, enter:

```text
https://lane.api.delino.io/mcp
```

Complete the browser Logto approval flow. Each approved connection has access to all repositories in your personal account, not a selected subset. Do not paste tokens into prompts or invent authorization proofs. Remote MCP manages synchronized server data even when all your devices are off; devices receive its changes on reconnect.

List approved connections in Lane settings or with `lane --remote connection list`. Revoke one connection in settings or through `lane --remote connection revoke`, using command help for its arguments. Revocation blocks subsequent access; it does not erase data already held by an offline device.

## Management and safe retries

MCP exposes repository and task creation, reading, editing and deletion; task planning fields, relationships, filters and sorting; repository label, milestone and cycle management; progress changes; proposal acceptance/rejection; task moves; history and prompt restoration; conflict comparison/resolution; and applicable connection, device, notification, receiver and event operations. Tools use names such as `lane_create_task`, `lane_resolve_conflict`, and `lane_complete_event`. Read the tool's input schema and online-only description before calling it.

Use stable request IDs for retries. Retry the same request ID with the same content. Read current versions before edits; conflicts require the full expected head set and explicit resolution. A replay after deletion reports a deleted result and must not recreate the object. The adapter records the actual MCP origin; agents must not invent a display identity.

Task, repository, and event deletion calls confirm their target. Account-wide deletion also requires the user's explicit account confirmation and fresh browser reauthentication. An agent cannot fabricate that proof.

## Task planning through MCP

Task tools accept and return priority, type, labels, start and due dates, story points, milestone, cycle and relationships, and the task list tool accepts the planning filters and one sort key with a direction. Edit with care: `lane_save_task` replaces the entire task snapshot and does not merge omitted fields from the stored task, so a call that omits existing labels, dates, story points, milestone, cycle or relationships clears them. Read the task with `lane_get_task`, copy it, change only the fields you intend to change and submit every other field unchanged with the expected version. Only `lane task edit` in the CLI preserves omitted fields. Repository labels, milestones and cycles use five typed tools in both local and remote MCP:

- `lane_list_planning_items` and `lane_get_planning_item` read items, optionally for one repository and one kind (label, milestone or cycle).
- `lane_create_planning_item` and `lane_save_planning_item` create and update an item. An update needs the current expected version, and an item's kind and repository cannot change.
- `lane_delete_planning_item` deletes an item. The call confirms its target, detaches the item from tasks and keeps those tasks.

Read item and task history with the existing history tool; there is no restore tool for planning metadata. Conflicts on planning items use the existing conflict tools with the label, milestone or cycle kind. Planning rules, such as same-repository references, unique names, date order and cycle-free relationships, apply to MCP exactly as to the app and CLI. A rejected change returns `CONSTRAINT_VIOLATION` with a named violation; correct the input before retrying with a new request ID.

When synchronization has blocked an item after a server-side constraint violation, `lane_get_local_status` lists the item and its `CONSTRAINT_VIOLATION:<VIOLATION>` reason. Correct the local data with a normal edit, then ask the user to run `lane sync run --retry-blocked`, or run it when the user has asked you to manage synchronization.

No MCP tool resets development data. If a tool reports `UNSUPPORTED_FORMAT`, tell the user and let them decide whether to use the explicit [development data reset](user-guide.md#development-data-reset) command. Never run it on your own initiative.

## Process webhook events

Receiver settings and event processing are online-only. Reading an event does not complete it. Claim an event to receive a five-minute lease. Renew before expiry for five minutes from renewal. Complete through the event completion tool, supplying the tasks to create atomically; zero tasks is valid. Do not create the same tasks separately before completion.

Use the returned lease token only for its event. Release it with an error reason if processing fails. An expired or released token cannot complete work. A lost response is retried with the same request identity and content. Completion records and task links remain when an event is reopened. Deleting an event or its repository invalidates its lease; deleting its receiver stops new intake but retains existing events and valid processing leases.

Offline devices can read retained event copies but cannot claim or complete events. Lane stores events; your agent decides what they mean.
