---
"@servers/mcpa-api": patch
---

Reject provider `base_url` values that end in an empty query or fragment delimiter (`?`, `#` or `?#`). The validator inspected parsed URL fields, which keep neither an empty fragment nor an empty query, so these values passed the "no query or fragment" rule. They now fail with `INVALID_INPUT` and the existing message; URLs without those delimiters are unaffected.

<!-- public-changelog:start -->

A provider `base_url` that ends in `?`, `#` or `?#` is now rejected when you register an API key credential. Remove the trailing delimiter.

<!-- public-changelog:end -->
