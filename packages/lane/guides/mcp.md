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

MCP exposes repository and task creation, reading, editing and deletion; progress changes; proposal acceptance/rejection; task moves; history and prompt restoration; conflict comparison/resolution; and applicable connection, device, notification, receiver and event operations. Tools use names such as `lane_create_task`, `lane_resolve_conflict`, and `lane_complete_event`. Read the tool's input schema and online-only description before calling it.

Use stable request IDs for retries. Retry the same request ID with the same content. Read current versions before edits; conflicts require the full expected head set and explicit resolution. A replay after deletion reports a deleted result and must not recreate the object. The adapter records the actual MCP origin; agents must not invent a display identity.

Task, repository, and event deletion calls confirm their target. Account-wide deletion also requires the user's explicit account confirmation and fresh browser reauthentication. An agent cannot fabricate that proof.

## Process webhook events

Receiver settings and event processing are online-only. Reading an event does not complete it. Claim an event to receive a five-minute lease. Renew before expiry for five minutes from renewal. Complete through the event completion tool, supplying the tasks to create atomically; zero tasks is valid. Do not create the same tasks separately before completion.

Use the returned lease token only for its event. Release it with an error reason if processing fails. An expired or released token cannot complete work. A lost response is retried with the same request identity and content. Completion records and task links remain when an event is reopened. Deleting an event or its repository invalidates its lease; deleting its receiver stops new intake but retains existing events and valid processing leases.

Offline devices can read retained event copies but cannot claim or complete events. Lane stores events; your agent decides what they mean.
