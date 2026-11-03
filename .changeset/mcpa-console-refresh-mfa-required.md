---
"@servers/mcpa-api": patch
---

Report `ERROR_REASON_MFA_REQUIRED` with the reauthentication URL when a console request's in-request token refresh returns a token without MFA evidence, instead of `ERROR_REASON_UNAUTHENTICATED`. `GetViewer`, `ListTenantGroups`, `ListResources` and per-resource reads of that request fail with the MFA error and return no data, and the downgraded session can still read `GET /auth/session` and log out. A refresh rejected with `invalid_grant` still ends the session, and personal keys stay MFA-exempt.
