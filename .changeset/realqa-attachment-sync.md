---
"@delinoio/lane": patch
"@servers/lane-api": patch
"@apps/lane": patch
---

Store capture problem descriptions only in the prompt in the Rust store. Treat attachments awaiting another device as pending and block only images refused for size or invalid content, without failing the sync cycle. Apply native PNG structure rules in the CLI, add shared URL-prefix vectors for every runtime, align UI page-URL normalization with them and log capture and attachment operations without user content. Add attachment queue gauges to the server.

<!-- public-changelog:start -->

A capture image that cannot synchronize no longer stops synchronization of your other Lane data. Lane shows the affected image so that you can retry it. Images saved on another device wait quietly until that device uploads them. Saving a capture from the CLI no longer repeats the problem description in the task description.
<!-- public-changelog:end -->
