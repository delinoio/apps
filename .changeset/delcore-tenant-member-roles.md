---
"@repo/protos": minor
"@servers/delcore-api": minor
---

Add the read-only, service-scoped Delcore method `GetTenantMemberRoles`. Registered services with the `authorize` scope read whether 1 to 100 users are current members of one tenant and which fixed roles they hold, in request order. Withdrawing users and deleted or unknown tenants report no membership. Existing tenant, role, `Authorize`, `GetTenantStates` and retention behavior is unchanged.
