# Lane

Lane manages repository tasks and prompts for individuals and their existing agents. Native apps and the CLI use local data without an account. Optional synchronization connects your devices. The web app requires login and an online connection.

Lane does not execute agents, call AI providers, fetch repository contents, or verify access to Git hosts. Repository URLs organize your work. Identical URLs in different accounts do not share data. There are no team workspaces or paid plans in this version.

The native application targets macOS, Windows, iOS, and Android. Linux uses the CLI, local MCP, or the web app. The CLI targets macOS, Windows, and Linux. Platform build and installation validation is separate from publication: these guides do not announce a released package, app-store listing, or live deployment.

- [Installation](./installation) describes reviewed CLI artifacts and npm installation.
- [User guide](./user-guide) covers tasks, prompts, synchronization, deletion, and notifications.
- [Connect an agent](./mcp) explains local and remote MCP.
- [Troubleshooting](./troubleshooting) explains failures and recovery.

The intended web address is `https://lane.delino.io`; the online API and remote MCP use `https://lane.api.delino.io`. Availability depends on deployment. Perfine and RealQA appear as disabled **Coming soon** entries; they do not run inside Lane.
