---
"@servers/mcpa-api": patch
---

Evict idle resource runtimes from the data plane. A janitor drops runtimes unused for 30 minutes and the least recently used idle runtimes above 5000 loaded, and the new mcpa_runtimes_loaded gauge reports the loaded count. In-flight calls are never evicted and later calls rebuild the runtime from stored credentials.
