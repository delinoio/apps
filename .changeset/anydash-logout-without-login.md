---
"@delinoio/anydash": patch
---

Report NOT_LOGGED_IN from collector logout before any data-key access. Logout on a device without stored device credentials no longer generates and stores an encryption key, and no longer fails with KEY_UNAVAILABLE when the configured key source has no key. The product has no released version yet, so no public note is added.
