---
"@servers/lane-api": patch
---

Accept an optional typed web navigation intent (task or repository plus an account
guard) on `/auth/login`, reject invalid or conflicting values before creating the
flow, store it only in that flow's request, revalidate it at callback completion and
append only the recognized fields to the registered callback. Session `redirect_uri`,
reauthentication and native/CLI OAuth are unchanged.
