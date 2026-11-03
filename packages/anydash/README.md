# AnyDash CLI

`anydash` is the AnyDash command-line collector. It connects a device to your AnyDash account, aggregates local Codex and Claude Code session usage, polls the provider connections you add, and uploads only aggregated usage to [AnyDash](https://anydash.delino.io). Provider credentials stay on the device.

## Install

With npm (Node.js 20 or later):

```sh
npm install --global @delinoio/anydash
```

npm installs the launcher and the one platform package that matches your OS and CPU: macOS 14 or later (Apple silicon and Intel), Windows 11 (x64 and arm64), and Linux with glibc (x64 and arm64; Ubuntu 24.04 and 26.04, Debian 12 and 13). Keep optional dependencies enabled. Before each run the launcher checks the platform package's name, version, OS, CPU and SHA-256 checksum. It never downloads or builds a replacement; if the check fails, reinstall the same version.

Without npm, use the install script, which verifies the archive checksum before installing:

```sh
curl -fsSL https://anydash.delino.io/install.sh | sh
```

```powershell
irm https://anydash.delino.io/install.ps1 | iex
```

## Get started

```sh
anydash login
anydash status
```

`anydash login` shows a connection code and opens the approval page in your browser. Run `anydash --help` for every command.

## Updates

Update through the channel you installed with: `npm update --global @delinoio/anydash`, or run the install script again. `anydash status` and the logs show when a new version is available. There is no self-update command.

## Support

Report problems in [GitHub Issues](https://github.com/delinoio/apps/issues) without personal data, and report security issues through GitHub private vulnerability reporting.
