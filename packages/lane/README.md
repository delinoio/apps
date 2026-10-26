# @delinoio/lane

Lane manages repository tasks and prompts locally, with optional account synchronization. Lane does not execute agents or call AI providers.

Delino GitHub Releases include direct CLI binaries, a lightweight npm launcher and six platform npm archives and macOS/Windows x64 desktop archives with checksums. CLI/npm versions can differ from the Delino tag. The release workflow does not publish to the npm registry. After publication, install an exact version with `npm install -g @delinoio/lane@0.1.0`. For an unpublished candidate, install the supplied npm archive from its local path. Run `lane --version` and `lane --help`.

Node.js 24 or newer is required. Rust executables target macOS, Windows and Linux on amd64/arm64. Exact-version optional dependencies install only the matching platform package. Keep optional dependencies enabled. The launcher package contains no native binaries. The launcher validates the package/artifact version, platform and SHA-256 checksum before execution, with no runtime download. A missing or mismatched artifact stops execution. Arguments, stdio, signals and exit status pass through to the executable.

Read [installation](guides/installation.md), [MCP connections](guides/mcp.md), [full management](guides/user-guide.md), and [troubleshooting](guides/troubleshooting.md). Agents should read [SKILL.md](SKILL.md).

Tasks can carry optional priority, type, labels, start and due dates, story points, a milestone, a cycle and same-repository relationships. Labels, milestones and cycles belong to a repository. Task lists filter and sort by these fields. Lane has no assignees, bulk edits or saved views, and the web app is online-only. Opening data from an earlier development format fails with `UNSUPPORTED_FORMAT`; the explicit `lane --space <uuid> data reset --confirm-development-reset` discards one local space's development data and must not be run by an agent unless the user asks. See the [user guide](guides/user-guide.md#plan-tasks) and the [reset procedure](guides/user-guide.md#development-data-reset).

Repository-owned CLI/package code uses Apache-2.0. Distributions include LICENSE and NOTICE; native packages also include dependency license texts in THIRD_PARTY_NOTICES. Server software is separately licensed; installing this package does not install a server.
