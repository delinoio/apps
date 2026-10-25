# Install Lane

The CLI is named `lane`. The npm package is `@delinoio/lane`. Release artifacts are prepared for review; the commands below describe installation when the matching artifact is available. They do not establish that a public npm version or a signed native app has been published.

## npm

Use Node.js 24 or newer. After the package is published, install an exact reviewed version:

```sh
npm install -g @delinoio/lane@0.1.0
lane --version
lane --help
```

To review an unpublished npm archive, install the supplied `delinoio-lane-0.1.0.tgz` from its local path instead. The package contains platform executables. On each invocation, the launcher checks the package version, platform, artifact manifest, byte length, and SHA-256 checksum before execution. It does not download a mutable latest executable. Unsupported platforms, missing artifacts, or mismatches stop with an installation error.

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
