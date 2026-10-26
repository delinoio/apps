---
"@apps/lane": patch
---

Keep the latest macOS APNs token in memory and reconcile `RegisterPush` per space on the host's observation cadence. A token received while a retained account was disconnected now registers when that account reconnects, an initial helper or RPC failure retries after 30 seconds, and authentication failures wait for the next verified reconnection. A forbidden result, such as a space that is not the active account selection, retries on the same delay instead of blocking registration. Registration stays tied to each space's own device, never runs for disabled, disconnected, removed or stopped spaces, and is covered by a native fixture target.

<!-- public-changelog:start -->

Lane on macOS now keeps notifications working after reconnecting an account or recovering from a temporary connection problem, without turning notifications off and on again.
<!-- public-changelog:end -->
