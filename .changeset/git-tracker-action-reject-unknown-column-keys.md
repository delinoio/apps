---
"@actions/git-tracker": patch
---

Reject unknown column keys in the Action configuration before uploading, naming the first unknown key and the column, so that a misspelled threshold such as absoluteTreshold no longer silently removes regression gating. Dataset-level keys are unchanged. The committed action.cjs bundle is rebuilt.

<!-- public-changelog:start -->

The Git Tracker Action now fails with a clear error when a column in the configuration has an unrecognized key, such as a misspelled absoluteThreshold or percentageThreshold, instead of uploading without that regression threshold.
<!-- public-changelog:end -->
