---
"@servers/mcpa-api": patch
---

Reset the credential change listener reconnect delay after a successful connection. A listener that was healthy and then drops now reconnects after one second instead of continuing from the delay left by earlier failures, so credential changes from other replicas are picked up sooner. Consecutive failures before the listener connects still back off from one second up to one minute.
