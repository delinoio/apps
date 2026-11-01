# AnyDash Distribution Contract

## Scope

`packages/anydash` (`implemented`) owns the npm executable launcher `@delinoio/anydash` and its platform packages, CLI archive packaging with manifests and checksums, and the `sh` and PowerShell install scripts served from `anydash.delino.io`. This contract also defines desktop, mobile and server image distribution requirements, the release automation that produces them and the public documentation pages. CLI source stays in `cmds/anydash`, app hosts in `apps/anydash` and the server image in `servers/anydash-api`. Release steps live in `scripts/release/anydash.mjs` and the reusable `.github/workflows/deploy-anydash.yml` and `.github/workflows/deploy-anydash-mobile.yml`, called through the [unified release procedure](repository-workflow-contract.md#project-release-preparation). This contract does not establish that any artifact has been published.

## Runtime and Language

The npm launcher is a small Node.js package (Node.js 20 or later, built-in modules only); the product CLI is Rust. Install scripts are POSIX `sh` (tested with `sh` and `dash`) and PowerShell (Windows PowerShell 5.1 and PowerShell 7). Release helpers are Node.js modules. Follow the pinned repository Node and pnpm tooling and Rstack formatting. CLI targets: macOS arm64 and x64, Windows x64 and arm64, Linux x64 and arm64 (glibc; validated on Ubuntu 24.04 and 26.04 and Debian 12 and 13). CLI architecture support does not imply a desktop app for the same architecture.

## Users and Operators

Users install the CLI by install script or npm, desktop apps by direct download, and mobile apps through stores that the maintainer manages. The maintainer handles store listings, review submission and the general public release. Private operator automation deploys the server image.

## Interfaces and Contracts

### Platform keys and versions

- Platform keys are `darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64`, `windows-arm64` and `windows-x64` (`bin/verify.mjs` `Platform`). Desktop update keys are `darwin-arm64`, `darwin-x64` and `windows-x64`, matching the app updater.
- One release version covers every AnyDash binary in a release: the GitHub Release tag is the component success tag `anydash@vX.Y.Z`; the CLI (`cmds/anydash/Cargo.toml`), `@delinoio/anydash`, its exact optional platform dependencies and the embedded desktop version are `X.Y.Z`. Release preparation mirrors the component version into the launcher and the CLI (`scripts/release/versions.mjs`); the desktop build receives it as `ANYDASH_RELEASE_VERSION`. The `@apps/anydash` package keeps its own version.

### CLI archives and install scripts

- Each release publishes `anydash-<os>-<arch>-<version>.tar.gz` for the six platform keys. The archive has one top-level directory `anydash-<os>-<arch>/` that contains `anydash` (`anydash.exe` on Windows), `SHA256SUMS` for the executable, `manifest.json` (platform, version, path, size, SHA-256 and source revision), the repository `LICENSE` and `NOTICE`, and `THIRD_PARTY_NOTICES/` with the license texts of every linked crate for that target, the Rust standard library notices and `modules.json`. Packaging stops when a linked crate ships no license text. Archives are deterministic ustar archives (sorted entries, `root:root`, zero timestamps, mode 0755 for the executable), so repackaging identical inputs gives identical bytes.
- macOS executables are signed with the Developer ID Application certificate, the hardened runtime and a secure timestamp, and notarized (a standalone executable cannot hold a stapled ticket; Gatekeeper checks it online). Windows executables are Authenticode-signed with SHA-256 digests and an RFC 3161 timestamp. Linux executables have no platform code signature; the Ed25519-signed `SHA256SUMS` covers them. One release-wide `SHA256SUMS` covers every published file, and `SHA256SUMS.sig` is its base64 Ed25519 signature with the desktop update key.
- Linux CLI builds run in Debian 12 (glibc 2.36), the oldest supported glibc, so the same executables run on the newer supported distributions.
- Install scripts `https://anydash.delino.io/install.sh` and `https://anydash.delino.io/install.ps1` detect the OS and architecture, download the matching archive and `SHA256SUMS` from the GitHub Release, verify the archive checksum before extracting and the executable against the archive's own checksum after extracting, and install the executable into a per-user location (`~/.local/bin` on macOS and Linux, `%LOCALAPPDATA%\Programs\AnyDash` on Windows) without elevated privileges. They replace an existing executable atomically and never leave a partial file. A checksum mismatch, a missing checksum entry or an unsupported platform (another OS, another CPU, or Linux with musl libc) stops with a clear error and installs nothing.
  - The latest version is the highest `anydash@vX.Y.Z` tag in the public GitHub release list. `--version X.Y.Z` (`sh`) or `ANYDASH_VERSION` (both) selects an explicit version; `--install-dir` or `ANYDASH_INSTALL_DIR` selects another directory. On macOS, a shell running under Rosetta installs the Apple silicon build.
  - `install.ps1` adds its directory to the user `PATH` unless `ANYDASH_NO_MODIFY_PATH=1`; `install.sh` prints a `PATH` hint instead of editing shell profiles.
  - `ANYDASH_RELEASES_URL` and `ANYDASH_DOWNLOAD_URL` replace the GitHub release list and download base for mirrors and tests. Without them, `install.sh` accepts HTTPS only, including redirects.
  - The web build copies both scripts unchanged from `packages/anydash/install/` into the `apps/anydash` output root, and `apps/anydash/web/_headers` serves them as `text/plain; charset=utf-8` with `Cache-Control: no-cache` on Cloudflare Pages. `apps/anydash` `pnpm test` checks the copies after the build.
- npm: `npm install -g @delinoio/anydash` installs a launcher that declares exact-version `optionalDependencies` on `@delinoio/anydash-<os>-<arch>` packages for all six targets. Each platform package contains only its executable, `manifest.json`, `SHA256SUMS`, licenses and notices, selected by npm `os` and `cpu`. The launcher package contains no executable; its `manifest.json` records every target's size and SHA-256 and the source revision. The launcher verifies the installed platform package's name, version, OS, CPU and checksum and propagates arguments, stdio, signals and exit codes. A missing or mismatched platform package fails with a reinstall instruction; the launcher never downloads or compiles a fallback.
- Updates come only through the install channel (rerun the install script, `npm update -g` or the desktop app update). There is no CLI self-update command, no APT repository and no `.deb` packages. Logs and `status` show new-version and unsupported-parser notices.

### Desktop apps

- Signed direct downloads: macOS arm64 and x64 apps signed and notarized; a signed Windows x64 app. There is no Windows arm64 app, following the [application contract](apps-anydash-foundation.md) fallback while a native Lynxtron arm64 build is not established; add one here and in the release matrix if Lynxtron supports it. Installers and the signed auto-update feed are published on GitHub Releases, and the `anydash.delino.io` download links point there.
  - Installers: `AnyDash-<version>-macos-<arch>.dmg` (signed, notarized and stapled disk image with the app and an `Applications` link) and `AnyDash-Setup-<version>-windows-x64.exe` (signed per-user Inno Setup installer from `apps/anydash/native/desktop/windows/AnyDash.iss`). The Windows installer needs no elevation, installs the app into `%LOCALAPPDATA%\Programs\AnyDash Desktop\App` with the uninstaller in `...\AnyDash Desktop\Uninstall`, adds a Start menu shortcut and registers the widget MSIX for the user.
  - Update feed: each release carries `anydash-desktop-<platform>-<version>.tar.gz` with one top-level entry (`AnyDash.app` on macOS, `application` on Windows), covered by `SHA256SUMS` and `SHA256SUMS.sig`. The app installs an update only after the Ed25519 signature, the archive checksum and the OS code signature verify (the embedded Apple team ID on macOS; the embedded Authenticode publisher subject on `lynxtron.exe` on Windows) and the user confirms.
- Each desktop package embeds the matching verified `anydash` executable as its collector (the same signed bytes as the CLI archive for that platform, which assembly checks), its license notices and the separate native widget host: the signed WidgetKit containing app at `AnyDash.app/Contents/Helpers/AnyDash Widgets.app` on macOS, and the signed Widgets Board MSIX at `resources/app/widgets/AnyDash.Widgets.msix` on Windows. The MSIX identity publisher and version follow the signing certificate subject and the release version.
- macOS signing runs inside-out: nested Mach-O files and code bundles first, then the app, with the hardened runtime, a secure timestamp and the JIT entitlements in `apps/anydash/native/desktop/release.entitlements` for app bundles. The already signed collector and widget host keep their signatures. Windows signs every `.exe`, `.dll` and `.node` file except the already signed collector.

### Mobile apps

- The unified release produces signed iOS and Android builds with their widgets through the repository-wide `release-signing` environment, with optional TestFlight and Google Play internal-testing uploads through `release-publish`, following the [native publication handoff](repository-workflow-contract.md#native-publication-handoff). The iOS archive uses automatic signing with the App Store Connect API key and exports for App Store Connect, and the job rejects an IPA without the compiled `AppIcon` asset catalog before any TestFlight upload; the Android App Bundle is signed with the upload key, whose certificate fingerprint the job checks. Both use the release version and the release run number as the build number (1 to 9999).
- The maintainer handles store listings, review submission and the general public release. Automatic store submission is out of scope. Privacy policy and terms of service must exist before store publication; they are a separate legal deliverable.

### Release automation

- Selection (`scripts/release/plan.mjs`): an `@apps/anydash` record selects the web host, the AnyDash binaries (CLI, desktop and npm), the mobile builds and the `anydash-api` image with its Delcore prerequisite. An `@delinoio/anydash` record selects the binaries and redeploys the web host, which serves the install scripts. An `@servers/anydash-api` record selects the image only.
- `deploy-anydash.yml` jobs: `source` (release version check), `cli-linux` (Debian 12 container on `ubuntu-24.04` and `ubuntu-24.04-arm`, no credentials), `native` (`macos-15`, `macos-15-intel`, `windows-2025`, `release-signing`), `assemble` (`release-signing`, signs `SHA256SUMS`) and `publish` (`release-publish`, the only job with `contents: write`; it also holds `actions: read` to download a retained set). `deploy-anydash-mobile.yml` mirrors the DeliTools mobile workflow. The component job runs both in signed build-only mobile mode and with `publish: true` for the binaries. Environment names are listed in the [environment contract](repository-environment-contract.md#anydash-release-environment).
- Publication is immutable and resumable: GitHub Release assets and npm versions that already exist must match the local bytes and are reused; a different existing artifact stops publication for manual reconciliation. Platform npm packages publish before the launcher, with npm provenance.
- Signed and notarized builds are not reproducible (signature timestamps and repackaged desktop archives change the bytes), so recovery never rebuilds the set. `assemble` retains the signed release set as the `anydash-release-<revision>` workflow artifact for 30 days. Rerunning the failed publication job uses that artifact. On a repeated dispatch of the same publication identity, the `source` job (`actions: read`) looks for an unexpired artifact of that name from an earlier unified release run on main (`release.yml`, `repository_dispatch`); the earliest one wins. When one exists, `cli-linux`, `native` and `assemble` are skipped and `publish` downloads that set from the earlier run, verifies its record, checksums and update-key signature, then resumes GitHub Release and npm publication. Without a retained set the workflow builds again, and a rebuilt set that differs from already published assets stops for manual reconciliation.

### CI runner rule

Root `AGENTS.md` allows macOS runners only for the Perfine and Lane binary workflows and the AnyDash binary/desktop (`deploy-anydash.yml` `native`) and mobile iOS (`deploy-anydash-mobile.yml` `ios` and `ios-publish`) jobs called by the main-only unified release. `scripts/ci/workflow.test.mjs` enforces that no other job uses a macOS runner and that these workflows run only from component publication. Ordinary CI validates AnyDash on Ubuntu: `anydash-frontend` and `anydash-cli` (launcher and install-script tests, Linux x64 packaging and packed smoke).

### Server image and web

The unified release publishes the `anydash-api` OCI image to GHCR through the [server image publication](repository-workflow-contract.md#server-image-publication) procedure and deploys the web client to Cloudflare Pages through [web publication](repository-workflow-contract.md#web-publication) with the domain `anydash.delino.io`. Runtime deployment and migrations belong to private operator automation.

### Public documentation

Public pages in `apps/delino-docs` (`implemented`) under `/services/anydash/`, curated from these contracts without repository-internal details:

- `index`: overview.
- `getting-started`: sign-in, personal and team spaces, the supported platforms matrix, desktop app, install-script and npm installation, the connection code and approval flow, collector operation, logout and reattachment.
- `cli`: every command and option, JSON output and exit codes, the status fields, user service install and uninstall, credential storage and key rotation, source paths, file locations, environment variables, logs and the color opt-out.
- `providers`: per provider, the required token or CLI sign-in and its permissions, provided and "Not provided" metrics, the Vercel write-capable token warning, refresh, backoff and collection-device behavior, and the rule that AnyDash never refreshes CLI sign-ins and resumes after the user signs in to that CLI again.
- `data`: uploaded data, retention and deletion deadlines, account data deletion, team sharing scope and revocation, the meaning of estimated cost, cross-device duplication, quotas and rate limits.
- `troubleshooting`: the correlation ID, common errors (update required, rate limited, quota exceeded, authentication expired, partial aggregation, version conflict and the other approved states), collector problems and support: public GitHub Issues without personal data, and GitHub private vulnerability reporting for security reports.

The pages are English-only, because the AnyDash product is English-only and localization beyond English is out of scope. They and their dedicated navigation labels are declared as English-only units, and the AnyDash entry of the shared service directory is a registered English-only section, as the [localization contract](apps-delino-site-localization-contract.md#english-only-documentation-coverage) defines. The pages use the approved UI copy for state names. They describe supported behavior and do not announce that an artifact, store listing or deployment is available. Update them together with the CLI, provider adapter, application and server contracts when user-visible behavior changes.

### Changesets

Record component behavior changes under the [changeset contract](repository-changeset-contract.md) with the owning npm names: `@apps/anydash`, `@servers/anydash-api`, `@delinoio/anydash` (CLI and collector) and `@repo/protos` for schema changes. Public notes describe user-visible behavior for AnyDash users.

## Storage

All `dist` directories are ignored generated output and are removed from the final worktree; release jobs also remove desktop, widget and mobile build directories. Never commit built executables, signing material or release credentials. Assets of at least 512 KiB use exact-path Git LFS attributes.

## Security

Verify archive checksums in install scripts and the npm launcher. Keep signing, notarization, update-feed, npm publication and store credentials in the protected release environments, never in Git or artifacts; jobs delete temporary keychains and key files even on failure. Linux CLI builds and assembly inputs carry no signing credential. Preserve Apache-2.0 for CLI, launcher, app and protocol distributions and FSL-1.1-ALv2 for the server image, with third-party licenses and notices.

## Logging

Install scripts and the launcher report platform, version and safe failure categories only, never environment dumps or credentials. Installation uploads no diagnostics. Release helpers log structured JSON events with artifact names, platforms and notarization status, never credentials or provider responses.

## Build and Test

From `packages/anydash`:

- `pnpm test`: launcher verification and stdio, signal and exit-code propagation; packed npm installation through an isolated loopback registry (host-only platform selection, every target by `--os`/`--cpu`, global install, missing optional dependencies); deterministic archives; `install.sh` under `sh` and `dash` and `install.ps1` under PowerShell 7 against local release directories (latest and explicit versions, platform selection through fake `uname`, `sysctl` and `ldd`, unsupported platforms, outer and inner checksum mismatches, missing entries, invalid versions). PowerShell tests skip when `pwsh` is absent.
- `pnpm build [--platform <key>]... [--no-compile]`: packages committed sources into `dist/` (`--no-compile` packages executables already built and signed under `target/<triple>/release/`).
- `pnpm smoke`: checks the real build on the host: checksums, archive and npm package contents, the packed npm installation, the direct archive and the install script against a local copy of the release, each running `--version` and `--help`, and records `dist/smoke-<platform>.json`.

Root tooling tests cover release selection and version mirroring (`scripts/release/project.test.mjs`), the release helpers (`scripts/release/anydash.test.mjs`: identity, signing order, Windows signing lists, assembly rejections, the signed checksum list, the updater feed names and resumable npm publication) and the runner rule (`scripts/ci/workflow.test.mjs`).

Packaging checks in the release:

- Signed and notarized macOS arm64 and x64 apps and disk images, and a signed Windows x64 app and installer, with signed auto-update feed assembly.
- iOS and Android signed builds with widgets.
- CLI archives, install-script checksum verification and npm installation on each native release host (macOS arm64 and x64, Windows x64, and Linux x64 and arm64 in Debian 12), including checksum mismatch and unsupported-platform failures in the package tests.

Packaging tests use local candidates and never publish. Packaging and per-OS builds do not replace real-device validation or store acceptance.

### Known limits

- Windows arm64 CLI executables are cross-built and not executed in the release; Ubuntu 24.04 and 26.04 and Debian 13 execution is not part of the release run. Record native checks on those systems separately.
- The Windows widget MSIX depends on the Windows App SDK runtime; the installer registers the package but does not install that runtime. In-app updates replace the desktop app, including the embedded MSIX file, but do not re-register the widget package; a new installer run updates the registered widgets.
- Developer ID signing, notarization, Artifact Signing, store uploads and npm publication have not run with real credentials; record the first real release as platform acceptance evidence.

## Dependencies and Integrations

The unified release workflow, `release-signing` and `release-publish` environments, GitHub Releases, npm, Cloudflare Pages, GHCR, Apple notarization, Azure Artifact Signing for Windows code signing, Inno Setup on the Windows runner and the App Store Connect and Google Play APIs. Follow the [license](repository-license-contract.md), [dependency security](repository-dependency-security-contract.md), [workflow](repository-workflow-contract.md) and [environment](repository-environment-contract.md) contracts.

## Change Triggers

Change the CLI, application and project contracts and the public guides together when installation, targets, update behavior or signing changes. Update root `AGENTS.md` and the CI runner test in the same change that adds or moves an AnyDash macOS or iOS release gate. Follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Project](project-anydash.md)
- [Repository defaults](repository-defaults.md)
- [CLI and collector](cmds-anydash-cli-contract.md)
- [Application](apps-anydash-foundation.md)
- [Server](servers-anydash-api-contract.md)
- [Repository workflow](repository-workflow-contract.md)
- [Environment](repository-environment-contract.md#anydash-release-environment)
- [Azure Artifact Signing action](https://github.com/Azure/artifact-signing-action)
