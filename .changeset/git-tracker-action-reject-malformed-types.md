---
"@actions/git-tracker": patch
---

Validate JSON and YAML value types in the Action before uploading: row ids and values containers, dataset and column scalars and column directions must have the documented types. Failures name the row or column by index and ID and send no request. The committed action.cjs bundle is rebuilt.

<!-- public-changelog:start -->

The Git Tracker Action now rejects configuration and measurement files with wrongly typed values, such as a numeric row id, non-object row values or a list used as a column direction, before uploading. The error names the affected row or column.
<!-- public-changelog:end -->
