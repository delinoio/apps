---
"@apps/lane": patch
---

Compare each Task history version with its declared parent instead of the previous list entry.
Defer the field delta while the parent is not loaded and omit it for multi-parent resolutions.
Add component-level regression coverage through a Task editor Node harness.

<!-- public-changelog:start -->

Task history now shows only the changes each version made. After a conflict, a version
no longer appears to undo another editor's change that it never replaced.

<!-- public-changelog:end -->
