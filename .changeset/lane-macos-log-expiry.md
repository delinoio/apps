---
"@apps/lane": patch
---

Write macOS desktop diagnostic logs to UTC daily files and expire them by bucket date plus
30 days instead of the refreshed modification time of one long-lived file. Retire the legacy
timestamp-less `native-macos.log` under the shared rotation lock without copying its records.
Add native regression coverage for migration, boundary expiry, the shared 100 MiB budget,
concurrent writers, failure handling and test-root isolation.
