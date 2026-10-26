---
"@apps/lane": patch
---

Validate stored normalized repository identifiers in the native store without assigning a transport, so an explicit `:443` port from an SSH repository URL is no longer rejected by conflict resolution, accepted pull and push results or the standalone merge. The repository editor now pre-fills such a repository with an `ssh://git@` URL, so saving only a display name keeps the identifier instead of renaming it to the portless repository. Shared normalization fixtures cover SSH `:443` and HTTPS `:22`.

<!-- public-changelog:start -->

Saving only the display name of a repository that uses a nondefault SSH port such as `:443` no longer changes its URL to a different repository, and resolving conflicts or synchronizing such a repository no longer fails with an invalid-input error.

<!-- public-changelog:end -->
