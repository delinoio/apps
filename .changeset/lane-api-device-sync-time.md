---
"@servers/lane-api": patch
---

Record the authenticated device's last successful synchronization time using the database
server clock in the same account-locked transaction as each successful pull, validated empty
push and committed push item, so `ListDevices` reports `lastSyncAt`. Rejected, rolled-back,
negotiation-only and subscription-only calls leave it unchanged.
