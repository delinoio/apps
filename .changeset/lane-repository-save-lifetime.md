---
"@apps/lane": patch
---

Bind repository editor saves to the editor lifetime that submitted them: lock URL and display name while the save is pending, and keep a late response from a closed editor from closing or reporting into a newer editor. Reuse the request ID across unchanged repository save retries, including after a lost response, so the server replays the original outcome instead of creating a second version or an `already_exists` error. The logic lives in `src/repositoryEditor.ts` with in-memory regression tests.

<!-- public-changelog:start -->

Saving a repository no longer loses text you type while the save is running, and a slow save can no longer close a different repository you opened meanwhile. Retrying an unchanged repository save after a lost connection now completes the original save instead of reporting a conflict or a duplicate repository.

<!-- public-changelog:end -->
