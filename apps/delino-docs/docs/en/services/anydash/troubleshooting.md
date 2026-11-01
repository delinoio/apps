# Troubleshoot AnyDash

## Find the correlation ID

Every AnyDash error has a correlation ID. Use it when you report a problem.

- **App and web:** the "Something went wrong" panel shows the correlation ID, device ID, connection ID and collector version. Select **Copy details** to copy them.
- **CLI:** `anydash status` shows the correlation ID of the last error and of the last error of each connection. With `--json`, every error contains `correlationId`.
- **Logs:** the collector logs contain the correlation ID of each error. See [logs](/services/anydash/cli#logs) for their location.

## Common errors

### Update required

"Update required. This version of AnyDash is no longer supported. Update to keep syncing. Collection continues on this device."

AnyDash no longer accepts your app or CLI version. The collector stops uploading but keeps collecting local usage for up to 90 days. Update the desktop app, run the install script again, or run `npm update -g @delinoio/anydash`. Upload resumes automatically after the update. Usage older than 90 days is not recovered.

### Rate limited

You sent too many requests. AnyDash allows 600 requests per minute for your sign-in and 60 requests per minute for each device. The error tells you when you can try again; with `--json`, see `retryAfterSeconds`. The collector waits automatically.

Manual refresh is limited to once per minute for each connection or device. Before then, the Refresh control shows "Refresh available in {n}s", and `anydash refresh` prints the remaining wait.

When a provider limits AnyDash, the widget shows "{Provider} asked us to wait. Next try at {time}." AnyDash follows the provider's guidance and shows the last successful value until then.

### Limit reached (quota exceeded)

"You can have up to {limit} {things}. Delete one to add another."

You reached a quota: 20 collector devices or 50 external connections per user, 50 dashboards per space, or 100 widgets per dashboard. Delete a device, connection, dashboard or widget that you no longer need, and try again.

### Authentication expired

The provider no longer accepts the credential, and the connection has stopped polling.

- **Vercel, Supabase or Cloudflare:** "Reconnect {provider} to resume." Create a new token and add the connection again. See [Provider connections](/services/anydash/providers).
- **Codex or Claude Code:** "Sign in to the {CLI} CLI again on {device}. AnyDash resumes automatically." Sign in to that CLI again, or run it so that it renews its sign-in. AnyDash reads the sign-in again every 5 minutes and resumes without a reconnection. AnyDash never refreshes a CLI sign-in itself.
- **Grok:** sign in to the `grok` CLI again. AnyDash resumes at its next cycle.

If the CLI reports that the device credentials are inactive, or that a new login is required, the AnyDash sign-in of the device was revoked or expired. This happens after you delete the device, after 90 days without an upload, or when AnyDash detects that an old device credential was used again. Run `anydash login` and approve the device again.

### Partial aggregation

"Partial aggregation · {n} records skipped. Update the collector on {device} to read them."

The collector could not read some records in the Codex or Claude Code session files, usually because the tool changed its file format. It still counts the records that it can read. Update the collector on that device. `anydash status` shows the parser state and the number of skipped records for each tool.

### Version conflict

"This dashboard changed. Someone saved changes while you were editing. We loaded the latest version. Make your edit again."

Another person saved the dashboard while you edited it. AnyDash rejects your save and never merges changes silently. Select **Continue editing**, and make your change again on the latest version.

### Couldn't refresh

A collection attempt failed, for example because of a network problem, a provider error, a missing token permission or a response that AnyDash could not read. The widget shows the last successful value, its time and when AnyDash tries again. AnyDash retries automatically, with waits from 1 minute up to 30 minutes. If the failure continues, check the connection in `anydash status`, the token permissions in [Provider connections](/services/anydash/providers), and the collector logs.

### Not provided

The provider does not offer this metric for your account or plan. This is not an error, and AnyDash does not retry it. See the "Not provided" list of each provider in [Provider connections](/services/anydash/providers).

### Can't be calculated

AnyDash has no public price for the model at that time, or a required token breakdown is missing. The token counts are still shown. See [estimated cost](/services/anydash/data#estimated-cost).

### Sharing ended

"The owner stopped sharing this widget. Its data was removed from this device."

The data owner stopped sharing the widget, left the team, or deleted the source. Remove the widget, or ask the owner to share it again. On a native widget, open AnyDash to choose another widget.

### You don't have access to this dashboard

Team dashboards are available to Owners, Admins and Members. Ask a team Owner or Admin for access. If you can edit the dashboard but cannot change a widget's period, filters or sharing, the widget belongs to another person; only that person can change them.

### This space is unavailable

The team is blocked or is being deleted. Contact the team Owner.

### This code no longer works

The connection code expired after 10 minutes, was already used, or was entered wrong 5 times. Run `anydash login` again, or restart sign-in in the app, to get a new code.

### Connect a device to start

Your personal dashboard needs at least one connected collector device. Install the desktop app or the CLI and approve the device. See [Getting started](/services/anydash/getting-started#connect-a-device). Team dashboards work without a device.

## Collector problems

- **External metrics do not refresh, and the connection shows "No collector running":** none of the devices registered for the connection is running a collector. Start the desktop app, run `anydash run`, or install the service with `anydash service install`.
- **`anydash run` or `anydash service install` fails because a collector is already running:** only one collector runs for each OS user. Use the running collector, or stop it first: quit the desktop app, stop the foreground `anydash run`, or run `anydash service uninstall`.
- **The desktop app cannot stop the collector:** the user service started it. Run `anydash service uninstall`.
- **"The AnyDash collector isn't running" in the desktop app:** quit and reopen AnyDash, then try again.
- **The encryption key is unavailable:** the collector cannot find the key that encrypted the stored credentials, or the key file can be read by other users. Restore the same key source (for example, set `ANYDASH_ENCRYPTION_KEY` again or unlock the keyring), or set the key file permissions to `0600`. See [credential storage](/services/anydash/cli#credential-storage-and-encryption-keys).
- **Key rotation failed:** the old key and the stored credentials stay unchanged. Correct the cause and run `anydash key rotate` again.
- **Usage seems too high:** the same session files may be on more than one device. See [cross-device duplication](/services/anydash/data#cross-device-duplication).
- **Claude Code history is shorter than 90 days:** Claude Code deletes its session files after 30 days by default. AnyDash can read only the files that still exist.
- **Installation fails:** check that your OS and CPU are supported, and keep npm optional dependencies enabled. A checksum mismatch stops the installation; download again from the official source and do not bypass the verification.

## Get support

- Report problems on [GitHub Issues](https://github.com/delinoio/apps/issues). Include the correlation ID, device ID, connection ID and collector version. **Do not post personal data, tokens, credentials, prompts, session contents or log files with personal data in public issues.** The maintainer handles issues without a guaranteed response time.
- Report security vulnerabilities privately through [GitHub private vulnerability reporting](https://github.com/delinoio/apps/security/advisories/new). Do not open a public issue for a vulnerability.
- To delete your data, use **Delete AnyDash data** in **Settings**. See [deletion](/services/anydash/data#deletion).
