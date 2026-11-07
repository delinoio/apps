---
"@servers/mcpa-api": patch
---

Update the indirect Go modules github.com/andybalholm/brotli to v1.2.7 and github.com/klauspost/compress to v1.20.2 in the shared root module. These releases fix GO-2026-6888 and GO-2026-6865, which the mcpa-api govulncheck gate reported. The change touches no mcpa-api source.
