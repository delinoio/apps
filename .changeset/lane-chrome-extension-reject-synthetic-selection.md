---
"@apps/lane-chrome-extension": patch
---

Ignore page-dispatched click, pointermove and keyboard events in the RealQA
element selector. Only genuine user input (`isTrusted`) can change the selected
element, capture or cancel, and an ignored synthetic event leaves the selection
session intact. Verified with isolated DOM/VM fixtures only, not an installed
Chrome profile.

<!-- public-changelog:start -->

Lane's element selector now ignores selections made by the web page itself, so
only the element you choose is captured.

<!-- public-changelog:end -->
