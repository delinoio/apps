---
"@servers/lane-api": patch
---

Derive prompt-history membership of interactive task versions from the submitted
parent snapshots instead of the accepted head, for stale saves, specialized task
edits and conflict resolutions. Interactive and sync writes share one helper so
equivalent version graphs classify identically. Historical flags are not backfilled.

<!-- public-changelog:start -->

Prompt history now lists exactly the task versions that change the prompt or its
captures, including edits saved from an older version and conflict resolutions.

<!-- public-changelog:end -->
