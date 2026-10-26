---
"@apps/lane": patch
---

Bind task mutation completions in the Lane web editor to the editor lifetime that
submitted them. A late save, proposal change, prompt restoration, conflict
resolution or deletion from a closed or replaced editor now only refreshes read
data and cannot replace, clean, reopen or close another editor.

<!-- public-changelog:start -->

Fixed a case where a slow save could replace the task you were editing after you
had already moved on. Your unsaved input and current page now stay as you left them,
while the list still refreshes with the saved change.

<!-- public-changelog:end -->
