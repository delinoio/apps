---
"@servers/lane-api": patch
---

Validate normalized repository identifiers without assigning a transport, so an explicit
`:443` port from an SSH repository URL survives synchronization, replay and conflict
resolution instead of being stripped like an HTTPS default port. The server no longer maps
such a repository to a distinct portless repository or rejects its replayed version as
different immutable content.
