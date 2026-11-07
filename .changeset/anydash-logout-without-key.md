---
"@delinoio/anydash": patch
---

Check for stored device credentials by name in `Core::logout` instead of decrypting them, so logging out of a device that was never logged in returns `NOT_LOGGED_IN` without loading, creating or persisting the data key (no `encryption.key` file or OS keychain item) and without `KEY_UNAVAILABLE` when `ANYDASH_KEY_SOURCE=env` has no key.
