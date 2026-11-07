---
"@apps/mcpa": patch
---

Show the console's Page not found state for URLs with malformed percent-encoding instead of failing to render. Valid encoded tenant and resource IDs, generated navigation URLs and server routing are unchanged.

<!-- public-changelog:start -->

Opening a console link with a broken character escape, such as `/tenants/%`, now shows the Page not found screen with a link back to your tenants instead of a blank page.

<!-- public-changelog:end -->
