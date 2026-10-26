---
"@servers/lane-api": patch
---

Admit the exact configured API origin for the API-hosted OAuth consent POST only, and send `Referrer-Policy: strict-origin` on the consent page so browsers emit the API Origin instead of `null`. Hostile and opaque origins, the API origin on every other route, frontend CORS and the consent token, cookie, expiry, one-use and PKCE semantics are unchanged.
