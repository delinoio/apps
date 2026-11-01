---
"@repo/protos": minor
"@servers/delcore-api": minor
---

Add the read-only, service-scoped Delcore method `GetTenantMemberRoles`. Registered services with the `authorize` scope read whether 1 to 100 users are current members of one tenant, which fixed roles they hold and when their current membership started, in request order. Leaving and rejoining starts a new membership; role changes keep its start. A new forward migration records the membership start on Delcore members. Withdrawing users and deleted or unknown tenants report no membership. Existing tenant, role, `Authorize`, `GetTenantStates` and retention behavior is unchanged.
