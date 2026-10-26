---
"@apps/lane-chrome-extension": none
---

Match the Chrome Web Store exclusion against the parsed HTTPS hostname and path
instead of a prefix expression, so hostname lookalikes and `/webstorefront` are no
longer rejected before native status and selector injection. Verified with mocked
Chrome APIs only, not an installed Chrome profile.

<!-- public-changelog:start -->

RealQA no longer treats ordinary websites whose address merely resembles the
Chrome Web Store as unsupported pages.

<!-- public-changelog:end -->
