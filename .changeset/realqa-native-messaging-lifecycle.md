---
"@apps/lane": patch
"@apps/lane-chrome-extension": patch
---

Register the RealQA Native Messaging relay from the running desktop host, because portable release archives have no installer step. Each start of a non-testing package with configured extension IDs rewrites a missing, unreadable, foreign-relay or changed-origin manifest, and the Windows registry value, for its own relay. It skips macOS App Translocation copies and logs only the result. The development uninstaller keeps a registration that a later-started installation owns and still removes its own application. The release desktop build reads the extension IDs from repository variables.

<!-- public-changelog:start -->

Lane desktop releases that include RealQA Chrome support now register their Chrome connection each time Lane starts, so a downloaded release needs no separate setup step. On macOS, move Lane to your Applications folder before you open it, so Chrome can connect.
<!-- public-changelog:end -->
