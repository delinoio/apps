---
"@actions/git-tracker": patch
---

Match column directions against own keys only so inherited property names such as constructor or **proto** are rejected, and validate the configuration and measurement files before requesting an Actions identity token on upload. The committed action.cjs bundle is rebuilt.

<!-- public-changelog:start -->

The Git Tracker Action now rejects column directions such as constructor or **proto** with the standard "lower, higher or neutral" error, and it checks the configuration and measurement files before requesting a GitHub identity token, so invalid input no longer triggers any request.
<!-- public-changelog:end -->
