---
"@apps/lane": patch
---

Track desktop Store handles per window session and release them on retirement, including late opens.

<!-- public-changelog:start -->

Closing a Lane desktop window now releases its local data handles so a development data reset can proceed after all other owners release theirs.
<!-- public-changelog:end -->
