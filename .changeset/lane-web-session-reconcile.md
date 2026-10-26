---
"@apps/lane": patch
---

Reconcile the Lane web API session with GET `/auth/session` on focus, online resume
and deletion reauthentication, serialize reconciliation so the newest response wins,
retire in-flight results of replaced credentials, and block account-bound work after
session failure until an explicit retry.

<!-- public-changelog:start -->

After you approve a different account or re-approve Lane in the sign-in popup, the
original Lane tab now switches to the new session when you return to it instead of
failing to save until you reload. If Lane cannot confirm your session, your input is
kept and you can retry the connection explicitly.

<!-- public-changelog:end -->
