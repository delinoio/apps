---
"@apps/lane": patch
---

Preserve the exact prompt Markdown when the already active Include prompt option is selected again in the task editor. Absent prompts still initialize as present empty and No prompt still removes presence.

<!-- public-changelog:start -->

Selecting Include prompt again on a task that already has a prompt no longer clears the prompt text.
<!-- public-changelog:end -->
