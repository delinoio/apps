# AnyDash Distribution Contract

## Scope

`packages/anydash` (`planned`) owns the npm executable launcher `@delinoio/anydash` and its platform packages, release manifests and checksums for CLI archives, and the `sh` and PowerShell install scripts served from `anydash.delino.io`. This contract also defines desktop, mobile and server image distribution requirements and the planned public documentation pages. CLI source stays in `cmds/anydash`, app hosts in `apps/anydash` and the server image in `servers/anydash-api`. Release automation follows the [unified release procedure](repository-workflow-contract.md#project-release-preparation); this contract does not establish that any artifact has been published.

## Runtime and Language

The npm launcher is a small Node.js package; the product CLI is Rust. Install scripts are POSIX `sh` and PowerShell. Follow the pinned repository Node and pnpm tooling and Rstack formatting. CLI targets: macOS arm64 and x64, Windows x64 and arm64, Linux x64 and arm64 (glibc; validated on Ubuntu 24.04 and 26.04 and Debian 12 and 13). CLI architecture support does not imply a desktop app for the same architecture.

## Users and Operators

Users install the CLI by install script or npm, desktop apps by direct download, and mobile apps through stores that the maintainer manages. The maintainer handles store listings, review submission and the general public release. Private operator automation deploys the server image.

## Interfaces and Contracts

### CLI archives and install scripts

- Signed archives with checksums are published on GitHub Releases: `anydash-<os>-<arch>` with `darwin`, `windows` and `linux` and `x64` and `arm64`, containing `anydash` (`anydash.exe` on Windows), the repository license and notices and a version and platform manifest. macOS executables are signed and notarized; Windows executables are Authenticode-signed. A `SHA256SUMS` file covers every archive.
- Install scripts `https://anydash.delino.io/install.sh` and `https://anydash.delino.io/install.ps1` detect the OS and architecture, download the matching archive and `SHA256SUMS` from the GitHub Release, verify the checksum before extracting, and install the executable into a per-user location (`~/.local/bin` on macOS and Linux, `%LOCALAPPDATA%\Programs\AnyDash` on Windows) without elevated privileges. A checksum mismatch or unsupported platform stops with a clear error and installs nothing. The scripts accept an explicit version.
- npm: `npm install -g @delinoio/anydash` installs a launcher that declares exact-version `optionalDependencies` on `@delinoio/anydash-<os>-<arch>` packages for all six targets. Each platform package contains only its executable, manifest, checksum and licenses, selected by npm `os` and `cpu`. The launcher verifies the installed platform package's name, version, OS, CPU and checksum and propagates arguments, stdio, signals and exit codes. A missing or mismatched platform package fails with a reinstall instruction; the launcher never downloads or compiles a fallback.
- Updates come only through the install channel (rerun the install script, `npm update -g` or the desktop app update). There is no CLI self-update command, no APT repository and no `.deb` packages. Logs and `status` show new-version and unsupported-parser notices.

### Desktop apps

- Signed direct downloads: macOS arm64 and x64 apps signed and notarized; a signed Windows x64 app, plus a signed Windows arm64 app only if Lynxtron supports a native arm64 build. Installers and the signed auto-update feed are published on GitHub Releases, and the `anydash.delino.io` download links point there.
- The app installs an update only after its signature verifies and the user confirms.
- Each desktop package embeds the matching verified `anydash` executable as its collector, its license notices and any separate native widget host (a WidgetKit extension in the macOS app bundle; a Widgets Board provider in an MSIX package on Windows).

### Mobile apps

- The unified release produces signed iOS and Android builds with their widgets through the repository-wide `release-signing` environment, with optional TestFlight and Google Play internal-testing uploads through `release-publish`, following the [native publication handoff](repository-workflow-contract.md#native-publication-handoff).
- The maintainer handles store listings, review submission and the general public release. Automatic store submission is out of scope. Privacy policy and terms of service must exist before store publication; they are a separate legal deliverable.

### CI runner rule

Root `AGENTS.md` currently allows macOS runners only for the Perfine and Lane binary workflows called by the main-only unified release. The change that adds the AnyDash macOS and iOS distribution gates extends that rule for those gates in the same change. Until then, no AnyDash workflow uses macOS runners, and existing CI macOS-runner restrictions stay unchanged outside the AnyDash distribution gates.

### Server image and web

The unified release publishes the `anydash-api` OCI image to GHCR through the [server image publication](repository-workflow-contract.md#server-image-publication) procedure and deploys the web client to Cloudflare Pages through [web publication](repository-workflow-contract.md#web-publication) with the domain `anydash.delino.io`. Runtime deployment and migrations belong to private operator automation.

### Public documentation

Public pages in `apps/delino-docs` (`planned`), curated from these contracts without repository-internal details:

- Getting started and device connection.
- CLI reference.
- Provider connection guides: required tokens, permissions, provided and "Not provided" metrics, and the Vercel write-capable token warning.
- Data and troubleshooting: uploaded data, retention and deletion, team sharing scope, meaning of estimated cost, cross-device duplication and common errors.
- Support: public GitHub Issues without personal data, and GitHub private vulnerability reporting for security reports.

### Changesets

Record component behavior changes under the [changeset contract](repository-changeset-contract.md) with the owning npm names: `@apps/anydash`, `@servers/anydash-api`, `@delinoio/anydash` (CLI and collector) and `@repo/protos` for schema changes. Public notes describe user-visible behavior for AnyDash users.

## Storage

All `dist` directories are ignored generated output and are removed from the final worktree. Never commit built executables, signing material or release credentials. Assets of at least 512 KiB use exact-path Git LFS attributes.

## Security

Verify archive checksums in install scripts and the npm launcher. Keep signing, notarization, npm publication and store credentials in the protected release environments, never in Git. Preserve Apache-2.0 for CLI, launcher, app and protocol distributions and FSL-1.1-ALv2 for the server image, with third-party licenses and notices.

## Logging

Install scripts and the launcher report platform, version and safe failure categories only, never environment dumps or credentials. Installation uploads no diagnostics.

## Build and Test

Packaging checks:

- Signed and notarized macOS arm64 and x64 apps, and a signed Windows x64 app (plus arm64 if Lynxtron supports it), with signed auto-update verification.
- iOS and Android signed builds with widgets.
- CLI archives, install-script checksum verification and npm installation on every listed OS and architecture, including checksum mismatch and unsupported-platform failures.

Packaging tests use local candidates and never publish. Packaging and per-OS builds do not replace real-device validation or store acceptance.

## Dependencies and Integrations

The unified release workflow, `release-signing` and `release-publish` environments, GitHub Releases, npm, Cloudflare Pages, GHCR, Apple notarization and Windows code signing. Follow the [license](repository-license-contract.md), [dependency security](repository-dependency-security-contract.md), [workflow](repository-workflow-contract.md) and [environment](repository-environment-contract.md) contracts.

## Change Triggers

Change the CLI, application and project contracts and the public guides together when installation, targets, update behavior or signing changes. Update root `AGENTS.md` in the same change that adds AnyDash macOS or iOS release gates. Follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Project](project-anydash.md)
- [Repository defaults](repository-defaults.md)
- [CLI and collector](cmds-anydash-cli-contract.md)
- [Application](apps-anydash-foundation.md)
- [Server](servers-anydash-api-contract.md)
- [Repository workflow](repository-workflow-contract.md)
