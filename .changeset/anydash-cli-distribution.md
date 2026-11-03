---
"@delinoio/anydash": minor
---

Add the AnyDash CLI and shared collector (`cmds/anydash`) distribution in `packages/anydash`: the `@delinoio/anydash` npm launcher that verifies and runs the matching platform package, six platform packages (macOS, Windows and Linux on x64 and arm64), deterministic direct archives with a release-wide `SHA256SUMS` and its Ed25519 signature, and the `sh` and PowerShell install scripts that verify checksums before installing and are served from `anydash.delino.io`. Register the AnyDash release component with the unified release: signed and notarized CLI and desktop builds with installers and the desktop update feed on GitHub Releases, signed mobile builds, the `anydash-api` image, the web deployment and npm publication, plus an affected `anydash-cli` CI job.

<!-- public-changelog:start -->

Install the AnyDash CLI on macOS and Linux with `curl -fsSL https://anydash.delino.io/install.sh | sh`, on Windows with `irm https://anydash.delino.io/install.ps1 | iex`, or anywhere with `npm install --global @delinoio/anydash`. The installers check each download against its published checksum before installing. Run `anydash login` to connect a device. The CLI then collects your local Codex and Claude Code usage and checks the provider connections you add. Your provider credentials stay on the device. To update, run the install script again or use `npm update --global @delinoio/anydash`.

<!-- public-changelog:end -->
