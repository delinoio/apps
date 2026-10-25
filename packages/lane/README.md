# @delinoio/lane

Lane manages repository tasks and prompts locally, with optional account synchronization. Lane does not execute agents or call AI providers.

The package is prepared for review; this source does not announce an npm publication. After publication, install an exact version with `npm install -g @delinoio/lane@0.1.0`. For an unpublished candidate, install the supplied npm archive from its local path. Run `lane --version` and `lane --help`.

Node.js 24 or newer is required. Bundled Rust executables target macOS, Windows and Linux on amd64/arm64. The launcher validates the package/artifact version, platform and SHA-256 checksum before execution, with no runtime download. A missing or mismatched artifact stops execution. Arguments, stdio, signals and exit status pass through to the executable.

Read [installation](guides/installation.md), [MCP connections](guides/mcp.md), [full management](guides/user-guide.md), and [troubleshooting](guides/troubleshooting.md). Agents should read [SKILL.md](SKILL.md).

Repository-owned CLI/package code uses Apache-2.0. Distributions include LICENSE, NOTICE and dependency license texts in THIRD_PARTY_NOTICES. Server software is separately licensed; installing this package does not install a server.
