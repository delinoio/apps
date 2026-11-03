---
"@servers/git-tracker-api": patch
---

Acknowledge GitHub webhooks after storing them durably and apply them from the maintenance cycle, so a delivery that waits for a repository lock is no longer lost. Migrations are now version tracked: run git-tracker-api migrate on every database before deploying this version, including retained ones that hold only the earlier bootstrap schema, and /readyz stays unready until it has run.
