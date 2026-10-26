---
"@apps/lane": patch
---

Give Lane's web task-detail reads an editor-lifetime owner so a superseded response
cannot replace a newer draft, reopen a closed editor, change the page or report an
error. Accepted transitions, new reads and adapter disposal supersede earlier reads.

<!-- public-changelog:start -->

Fixed a case where a slow task response could replace the task you were editing or
switch you back to Tasks. Your unsaved input and current page now stay as you left them.

<!-- public-changelog:end -->
