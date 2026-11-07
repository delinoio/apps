---
"@servers/anydash-api": patch
---

Limit retries of failed Logto key retrievals to one per 30 seconds. While the key endpoint is failing, concurrent sign-ins now receive `DEPENDENCY_UNAVAILABLE` without each triggering their own upstream request.
