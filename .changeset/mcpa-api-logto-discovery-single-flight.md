---
"@servers/mcpa-api": patch
---

Share one Logto discovery attempt among concurrent callers during an outage. Callers now fail together after at most one discovery timeout instead of queueing one full attempt each, a caller that cancels stops waiting without aborting the shared attempt, and a failed discovery is still retried on the next use.
