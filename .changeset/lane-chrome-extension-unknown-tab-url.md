---
"@apps/lane-chrome-extension": none
---

Treat an omitted or empty active-tab URL as a missing tab grant instead of a
restricted page. Native selection requests now return `PERMISSION_REQUIRED`
without status, activation, injection or capture, while known restricted, file,
Web Store and incognito pages stay unsupported. Verified with mocked Chrome APIs
only, not an installed Chrome profile.

<!-- public-changelog:start -->

When Chrome has not yet given RealQA access to the current tab, Lane now asks you
to click the RealQA extension on that page instead of saying the page is unsupported.

<!-- public-changelog:end -->
