# AnyDash CLI reference

The `anydash` executable is the AnyDash command-line interface and the collector. Install it as described in [Getting started](/services/anydash/getting-started#cli).

## Synopsis

```sh
anydash [--json] [--color auto|always|never] <command>
anydash --version
anydash --help
```

| Option                        | Description                                                                                                            |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `--json`                      | Print one JSON object instead of human-readable output.                                                                |
| `--color auto\|always\|never` | Control ANSI color in human output and logs. `auto` (the default) colors human output only when it goes to a terminal. |
| `--version`                   | Print the executable version.                                                                                          |
| `--help`                      | Print the command summary.                                                                                             |

Results go to standard output and diagnostics go to standard error. Human output prints IDs in full so that you can copy them.

Exit status:

- `0`: the command succeeded.
- `1`: the operation failed.
- `2`: the invocation was invalid, for example an unknown command or a missing option value.

With `--json`, a success prints `{"ok":true,"result":{...}}`. A failure prints:

```json
{
  "ok": false,
  "error": {
    "reason": "ERROR_REASON_RATE_LIMITED",
    "message": "...",
    "correlationId": "...",
    "retryAfterSeconds": 30
  }
}
```

`retryAfterSeconds` is present only when you must wait before you try again. JSON field names use camelCase, enumeration values use their full names, and 64-bit numbers are decimal strings. JSON output never contains ANSI color codes.

## Commands

| Command                                                    | Description                                                                       |
| ---------------------------------------------------------- | --------------------------------------------------------------------------------- |
| `login [--device-name NAME]`                               | Connect this device with a connection code and browser approval.                  |
| `logout`                                                   | Delete the device credentials from this computer and stop collection and upload.  |
| `run [--service \| --owner foreground\|service\|desktop]`  | Run the collector.                                                                |
| `service install`, `service uninstall`                     | Install or remove the per-user service that starts the collector when you log in. |
| `status`                                                   | Show the collector, device, connection and parser status.                         |
| `connect <provider> [options]`                             | Add an external connection, or register this device for an existing connection.   |
| `connection list`                                          | List the connections registered for this device.                                  |
| `disconnect <connection-id>`                               | Remove this device's credential for a connection.                                 |
| `refresh [<connection-id>]`                                | Request a manual refresh of a connection, or a local usage sync.                  |
| `key rotate [--new-key-env VAR]`                           | Rotate the local encryption key.                                                  |
| `paths list`, `paths add`, `paths disable`, `paths enable` | Manage the folders that the collector scans for session files.                    |

When a collector is running, the other commands talk to it. When no collector is running, a command does its work directly and then exits; it does not start collection.

### login

```sh
anydash login [--device-name NAME]
```

Starts the device connection. The command prints the approval URL and the connection code, opens the browser when the computer has a graphical session, and waits until you approve the device, reject it, or the code expires or stops working. `--device-name` sets the name shown on the approval page; the default is the host name. After a previous `logout`, `login` reattaches the same device. See [Connect a device](/services/anydash/getting-started#connect-a-device).

### logout

```sh
anydash logout
```

Deletes the device credentials from this computer, releases the device's collection assignments for external connections, and stops collection and upload. AnyDash keeps the device registration and its data. Run `anydash login` to reattach the device.

### run

```sh
anydash run
```

Runs the collector in the foreground and also prints its logs to the terminal. Stop it with Ctrl+C. Only one collector can run for each OS user on a computer; if one is already running, `run` fails.

The installed user service uses `anydash run --service`, and the desktop app uses `anydash run --owner desktop`. These forms run without terminal logs. You normally do not run them yourself.

### service install and service uninstall

```sh
anydash service install
anydash service uninstall
```

`service install` registers a per-user service that starts the collector when you log in, and starts it. It needs no administrator rights. If a foreground or desktop-app collector is running, stop it first.

| Platform | Service                                                                                                                                                                                               |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS    | LaunchAgent `~/Library/LaunchAgents/io.delino.anydash.collector.plist`, which starts at login and restarts the collector if it stops.                                                                 |
| Linux    | systemd user unit `~/.config/systemd/user/anydash-collector.service`. On a server without a login session, enable lingering for your user (the command prints how) so that the service keeps running. |
| Windows  | Task Scheduler task `AnyDash Collector`, which runs at logon with limited rights and without a console window.                                                                                        |

If `ANYDASH_HOME` or `ANYDASH_KEY_SOURCE` is set when you install the service, the service uses the same value.

`service uninstall` stops the collector and removes the service. On Windows, the stopped collector cannot release its collection assignments, so another device takes over external connections after 5 minutes.

When the desktop app finds a collector that the service started, **Quit** does not stop it. Run `anydash service uninstall` to stop it.

### status

```sh
anydash status
```

Shows these fields. You can copy every ID.

| Field             | Meaning                                                                                                                                                                                                                                                                                                |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Collector         | Whether a collector is running, and what started it: the CLI (foreground or service) or the desktop app. Without a running collector, the output ends with "The collector is not running."                                                                                                             |
| Run state         | Running, not logged in, offline, update required, or device credentials inactive.                                                                                                                                                                                                                      |
| Device ID         | The ID of this device.                                                                                                                                                                                                                                                                                 |
| Collector version | The version of the running collector.                                                                                                                                                                                                                                                                  |
| Key source        | Where the local encryption key comes from: the OS secure store, the key file, or an environment variable. It is unspecified until the first credential is stored.                                                                                                                                      |
| Autostart         | Whether the user service is installed.                                                                                                                                                                                                                                                                 |
| Last local sync   | The time of the last scan of local session files.                                                                                                                                                                                                                                                      |
| Last upload       | The time of the last accepted upload.                                                                                                                                                                                                                                                                  |
| Pending buckets   | The number of usage records that wait for upload, and the start time of the oldest.                                                                                                                                                                                                                    |
| Last error        | The reason, correlation ID and time of the last error.                                                                                                                                                                                                                                                 |
| Connections       | For each connection: connection ID, provider, display name, whether this device has the credential, whether this device is the current collection device (`lease`) and its sequence number (`epoch`), last status, failure class, last success, next attempt and the correlation ID of its last error. |
| Parsers           | For Codex and Claude Code: the parser state (OK, partial, no sources or disabled), the number of skipped records, the number of session paths and the parser version.                                                                                                                                  |
| Login             | While `login` waits, the connection code and the approval URL.                                                                                                                                                                                                                                         |
| Notice            | A new version is available, an update is required, or some session records could not be read and you should update the collector.                                                                                                                                                                      |

`anydash --json status` prints the same data as JSON, plus `collectorRunning`.

### connect

```sh
anydash connect <codex|claude-code|grok|vercel|supabase|cloudflare> \
  [--connection-id ID] [--name NAME] [--token-stdin | --token-env VAR] [--team-id ID]
```

Adds an external connection and stores its credential on this device. The command prints the new connection ID.

| Option               | Description                                                                                                                                                                                     |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--connection-id ID` | Register this device for an existing connection instead of creating a new one. Each device stores its own credential. See [collection devices](/services/anydash/providers#collection-devices). |
| `--name NAME`        | The display name of the connection.                                                                                                                                                             |
| `--token-stdin`      | Read the API token from the first line of standard input.                                                                                                                                       |
| `--token-env VAR`    | Read the API token from the environment variable `VAR`.                                                                                                                                         |
| `--team-id ID`       | The Vercel team ID. Required for Vercel.                                                                                                                                                        |

Vercel, Supabase and Cloudflare need an API token. For security, the CLI accepts a token only from standard input or an environment variable, never as a command-line argument. Codex, Claude Code and Grok use the sign-in of their own CLI, so do not supply a token for them.

```sh
printf '%s\n' "$SUPABASE_TOKEN" | anydash connect supabase --name "Supabase" --token-stdin
anydash connect vercel --team-id team_123 --token-env VERCEL_TOKEN
anydash connect codex
```

For Vercel, the command reminds you that Vercel tokens can write; AnyDash calls only read endpoints. See [Provider connections](/services/anydash/providers) for the required tokens and permissions.

### connection list

```sh
anydash connection list
```

Lists the connections registered for this device with their ID, provider, name, collection assignment (`lease`), last status and last success.

### disconnect

```sh
anydash disconnect <connection-id>
```

Deletes this device's credential for the connection, releases its collection assignment and unregisters this device from the connection. The connection itself stays. To delete a connection and its data, use **Settings**, then **Connections** in the app or on the web.

### refresh

```sh
anydash refresh
anydash refresh <connection-id>
```

Without an ID, requests a local usage sync. With an ID, requests a refresh of that external connection. You can refresh each connection or device once per minute; an earlier request shows the remaining wait. If no collector is running, `anydash refresh` syncs local usage once, uploads it and exits.

### key rotate

```sh
anydash key rotate [--new-key-env VAR]
```

Rotates the local encryption key that protects the credentials on this computer. The command creates a new key and re-encrypts all stored credentials in one step. If any step fails, the old key and the stored credentials stay unchanged. AnyDash never rotates the key automatically.

If the key comes from the `ANYDASH_ENCRYPTION_KEY` environment variable, stop the collector first, put the new key in another environment variable, and run `anydash key rotate --new-key-env VAR`. After the rotation, set `ANYDASH_ENCRYPTION_KEY` to the new key before the collector starts again.

### paths

```sh
anydash paths list
anydash paths add <codex|claude-code> <path>
anydash paths disable <codex|claude-code> <path>
anydash paths enable <codex|claude-code> <path>
```

`paths list` shows each session folder with its tool, origin (detected or added), whether it is enabled and whether it exists. `paths add` adds an absolute folder that the collector scans with the same rules as the detected folders. `paths disable` stops scanning a detected or added folder; usage that was already collected from it stays. `paths enable` turns a folder on again. The collector does not follow folder symbolic links.

The collector detects these folders:

- **Codex:** `sessions` and `archived_sessions` in `$CODEX_HOME` (default `~/.codex`), including compressed `.jsonl.zst` files.
- **Claude Code:** `projects` in each folder listed in `CLAUDE_CONFIG_DIR` (comma-separated). When `CLAUDE_CONFIG_DIR` is not set, `projects` in `~/.claude` and in `$XDG_CONFIG_HOME/claude` (default `~/.config/claude`).

AnyDash only reads session files. It never changes or deletes them.

## Credential storage and encryption keys

The collector encrypts the stored API tokens and its own device credentials with a local key. It reads Codex, Claude Code and Grok sign-ins in place on every cycle and never copies them.

The collector gets the key from the first available source:

1. The OS secure store: the macOS Keychain, Windows Credential Manager, or the Linux Secret Service when a session bus and an unlocked keyring are available.
2. The environment variable `ANYDASH_ENCRYPTION_KEY`, which contains 32 bytes encoded as base64url.
3. The key file `encryption.key` in the configuration directory. The collector creates it with mode `0600` and refuses to use it if other users can read it.

The key file is stored apart from the encrypted data. Use the environment variable or the key file for unattended runs on servers without a keychain. Set `ANYDASH_KEY_SOURCE=os`, `env` or `file` to force one source.

## Files and directories

| Platform | Data                                                | Configuration                                    | Logs                                                           |
| -------- | --------------------------------------------------- | ------------------------------------------------ | -------------------------------------------------------------- |
| macOS    | `~/Library/Application Support/io.delino.anydash`   | `~/Library/Preferences/io.delino.anydash`        | `~/Library/Logs/io.delino.anydash`                             |
| Windows  | `%LOCALAPPDATA%\Delino\AnyDash\Data`                | `%APPDATA%\Delino\AnyDash`                       | `%LOCALAPPDATA%\Delino\AnyDash\Logs`                           |
| Linux    | `$XDG_DATA_HOME/anydash` (`~/.local/share/anydash`) | `$XDG_CONFIG_HOME/anydash` (`~/.config/anydash`) | `$XDG_STATE_HOME/anydash/logs` (`~/.local/state/anydash/logs`) |

The data directory holds the local usage store, the encrypted credentials and the source path settings. The local store keeps only token counts, model names, times, hashed session identifiers and paths. It never stores prompts or responses. When the device is offline, or when uploads are rejected, the collector keeps up to 90 days of usage and uploads it after it reconnects.

## Environment variables

| Variable                  | Effect                                                                                                               |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `ANYDASH_HOME`            | Put the data, configuration and log directories in the `data`, `config` and `logs` subdirectories of this directory. |
| `ANYDASH_ENCRYPTION_KEY`  | Supply the encryption key (base64url of 32 bytes).                                                                   |
| `ANYDASH_KEY_SOURCE`      | Force the key source: `os`, `env` or `file`.                                                                         |
| `NO_COLOR`                | Turn off ANSI color in human output and logs.                                                                        |
| `ANYDASH_LOG_COLOR=never` | Turn off ANSI color in human output and logs.                                                                        |
| `CODEX_HOME`              | The Codex folder that the collector reads (default `~/.codex`).                                                      |
| `CLAUDE_CONFIG_DIR`       | The Claude Code folders that the collector reads (comma-separated).                                                  |

## Logs

Every command writes structured logs to the log directory above, in files named `anydash.YYYY-MM-DD.log` (UTC date) that only your user can read. Foreground runs also print the logs to the terminal. A new file starts each day. The collector keeps logs for 7 days or 50 MiB in total, whichever limit comes first, and deletes the oldest files first.

Logs contain times, events, correlation IDs, device and connection IDs, providers and result categories. They never contain tokens, credentials, prompts, responses or session contents. AnyDash does not upload your logs.

Logs and human output use ANSI color by default. To turn color off, use `--color never`, or set `NO_COLOR` or `ANYDASH_LOG_COLOR=never`.

## Updates

Update the CLI through the channel you installed it with: run the install script again, run `npm update -g @delinoio/anydash`, or install the desktop app update. There is no self-update command. When AnyDash no longer supports your version, `status` shows "update required"; collection continues locally for up to 90 days, and upload resumes after you update.
