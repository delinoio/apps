---
"@apps/git-tracker": patch
---

Mark malformed First and Last commit values as invalid inline instead of sending them, and show a notice when a valid commit bound is not on the branch's first-parent path and so does not limit the graph. Add a pure range-bound helper with node tests.

<!-- public-changelog:start -->

The history graph now tells you when a First or Last commit filter cannot be applied. Values that are not a full 40-character lowercase SHA are marked as invalid, and a commit that is not on the selected branch's first-parent path shows a notice instead of being silently ignored.

<!-- public-changelog:end -->
