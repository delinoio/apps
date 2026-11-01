# AnyDash

AnyDash shows your AI coding subscription limits, your local AI session usage from many devices, and the status, usage and cost of your deployment platforms in one dashboard. It is made for solo businesses. Personal dashboards show your own data. Team dashboards let a Delino team share selected widgets without sharing the credentials behind them.

AnyDash works with these providers:

- **AI coding subscriptions:** Codex (ChatGPT plans), Claude Code (claude.ai plans) and Grok (SuperGrok and X Premium through Grok Build).
- **Local AI session usage:** Codex and Claude Code session files on your computers.
- **Deployment platforms (read only):** Vercel, Supabase and Cloudflare.

Your provider credentials stay on your own computers. A collector on each computer reads local usage and calls the providers, and then uploads only aggregated results to AnyDash. See [Data, retention and sharing](/services/anydash/data) for details.

AnyDash runs in the web browser at `https://anydash.delino.io`, as a desktop app for macOS and Windows, as a mobile app for iOS, iPadOS and Android, and as the `anydash` command-line collector. The interface is in English. AnyDash is free and has no paid plans. These guides describe the supported behavior. They do not announce that a specific app, package or store listing is available.

- [Getting started](/services/anydash/getting-started) explains sign-in, spaces, installation, device connection and the supported platforms.
- [CLI reference](/services/anydash/cli) lists every `anydash` command, option, status field, file location and environment variable.
- [Provider connections](/services/anydash/providers) explains the token or sign-in each provider needs, the metrics AnyDash shows, the metrics that show "Not provided", and how data refreshes.
- [Data and sharing](/services/anydash/data) explains what AnyDash uploads, how long it keeps data, how deletion and team sharing work, and what estimated cost means.
- [Troubleshooting](/services/anydash/troubleshooting) explains common errors, the correlation ID and how to get support.
