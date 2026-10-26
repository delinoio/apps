# Install Lane

The CLI is named `lane`. The npm package is `@delinoio/lane`. Release artifacts are prepared for review; the commands below describe installation when the matching artifact is available. They do not establish that a public npm version or a signed native app has been published.

## npm

Use Node.js 24 or newer. After the package is published, install an exact reviewed version:

```sh
npm install -g @delinoio/lane@0.1.0
lane --version
lane --help
```

The launcher uses exact-version optional dependencies. npm installs only the native package matching your OS and CPU. Keep optional dependencies enabled; do not use `--omit=optional` or `--no-optional`.

To review unpublished candidates, obtain both `delinoio-lane-0.1.0.tgz` and the matching `delinoio-lane-<os>-<arch>-0.1.0.tgz`. Install both local paths together with `npm install -g --offline <launcher-archive> <platform-archive>`. The launcher archive contains no native binaries; each platform archive contains one executable and its licenses. On each invocation, the launcher checks the launcher and native package versions, OS/CPU metadata, artifact manifest, byte length, and SHA-256 checksum before execution. It does not download a mutable latest executable. Unsupported platforms, missing artifacts, or mismatches stop with an installation error.

The launcher forwards command arguments, standard input/output/error, process signals, and the child exit status. It stores no user data in the installation directory.

## Direct executable

Select the archive for your OS and CPU: `lane-darwin-amd64`, `lane-darwin-arm64`, `lane-linux-amd64`, `lane-linux-arm64`, `lane-windows-amd64`, or `lane-windows-arm64`. The archive name includes its version. Windows contains `lane.exe`; other targets contain `lane`.

Obtain the archive from your trusted distributor. Extract it into a new directory. Verify the executable against `SHA256SUMS`, and check that `manifest.json` names the expected platform and version. For example, use `sha256sum -c SHA256SUMS` on Linux or `shasum -a 256 -c SHA256SUMS` on macOS. On Windows, compare `Get-FileHash .\lane.exe -Algorithm SHA256` with the manifest. A checksum detects corruption; trust still depends on the source of the archive and manifest.

Keep the included licenses and notices. Add the executable directory to your PATH, then run `lane --version` and `lane --help`. CLI architecture support does not imply matching native GUI architecture support.

## Choose local or remote data

```sh
lane --json repository list
lane mcp serve
```

The CLI defaults to local data, including after login. Only an explicit `--remote` targets server data. `--space <id>` selects an existing local space and cannot be combined with `--remote`.

Default output is human-readable. `--json` returns a structured result or error; it never includes terminal colors. Use `NO_COLOR` or `--color=never` to disable colors in human output. Exit status 0 means success, 1 means an operation failed, and 2 means the invocation was invalid.

Native desktop packages include their matching background synchronization helper. They do not depend on a globally installed npm package. Development builds use `lane-dev://` and distribution builds use `lane://` for login callbacks, opening Lane, and repository/task links. Use links supplied by Lane; do not substitute one build channel's callback scheme for another.

## Upgrade existing local data

Before using RealQA with data from an older Lane release, update every Lane app, CLI, and MCP client that accesses that local data. Save your work, close other Lane windows, and stop CLI, MCP, and background synchronization processes before migration. Closing a desktop window alone can leave Lane running; quit other Lane instances.

Opening older data does not automatically migrate it or create an empty replacement. In the native app's compatibility error screen, enter the **Existing data space ID**, select **Review migration**, check the space, and confirm **Migrate existing space**. Keep only the instance performing migration open.

Alternatively, quit Lane and run the updated CLI:

```sh
lane data migrate --space <existing-space-id>
```

Replace `<existing-space-id>` with the UUID of the existing local space you intend to upgrade. This command requires an explicit space ID, operates locally, and does not accept `--remote` or a request payload.

Migration preserves saved work, history, and pending synchronization changes. If it fails, it does not leave a partially migrated space. Correct the reported cause and retry; do not delete the data or initialize a replacement. After success, reopen the space with updated clients. Older Lane versions refuse the upgraded format. Repeating migration on an already upgraded space makes no changes.

## RealQA Chrome companion

RealQA capture runs inside the Lane desktop app. Its Chrome extension connects to an already running Lane installation. Use the extension and native component from the same trusted distributor. Native Messaging registration is part of Lane installation and removal; there is no separate RealQA desktop app.

A Chrome Web Store listing is not yet claimed by this guide. Until a reviewed listing is available, maintainers can supply a review build for **Load unpacked** in `chrome://extensions`. The exact extension ID must be registered by the matching Lane installation. An unpacked review build is not evidence of Chrome Web Store publication. Never use an arbitrary extension ID or grant access to an unrelated extension.

After installation, open Lane and approve each normal Chrome profile on that device. See [Chrome element capture](./user-guide#chrome-element-capture). Removing the extension stops its access; disconnect the profile in Lane to remove its stored approval as well. Lane removal unregisters its Native Messaging component. Windows signed installation and runtime acceptance remain separate from build validation.
