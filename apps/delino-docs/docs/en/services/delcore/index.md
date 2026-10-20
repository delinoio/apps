# Delcore

Delcore provides shared tenants, access control and billing for Delino services. The official API server is `https://delcore.api.delino.io`. This page describes the tenant state check for service developers.

## Check tenant state

A registered service can ask Delcore whether a tenant is active, blocked or deleted. Use this check when your service stores data for a tenant but does not have a Delino user for each request, for example when a project API key makes the call.

Call `delcore.v1.DelcoreService/GetTenantStates` with 1 to 100 tenant IDs. Each ID must be a lowercase UUID version 7. Authenticate with the access token of your service's Logto machine-to-machine application. The service must be registered with Delcore and have the `authorize` scope. User tokens, unregistered services and services without this scope cannot read tenant state.

The response repeats each requested tenant ID, in the order you sent them, with one state:

| State                   | Meaning                                                |
| ----------------------- | ------------------------------------------------------ |
| `TENANT_STATE_ACTIVE`   | The tenant exists and is not blocked or being deleted. |
| `TENANT_STATE_BLOCKED`  | A platform operator blocked the tenant.                |
| `TENANT_STATE_DELETING` | Deletion is pending.                                   |
| `TENANT_STATE_DELETED`  | Deletion is final.                                     |
| `TENANT_STATE_UNKNOWN`  | Delcore has no tenant with this ID.                    |

When more than one condition applies, `DELETED` wins over `DELETING`, `DELETING` wins over `BLOCKED`, and `BLOCKED` wins over `ACTIVE`.

The state does not depend on billing. An expired subscription, a failed renewal, a dispute or a low balance does not make a tenant `BLOCKED`. To decide whether a user may run paid work, use `Authorize` instead. The check only reads data. It does not change a tenant, authorize paid use or contact a payment provider, and it returns no tenant names, user details or billing information.

## Handle results and failures

- Allow access only for `TENANT_STATE_ACTIVE`, or when your own documented rules for owners or administrators permit access to a blocked or deleting tenant.
- Treat `TENANT_STATE_UNKNOWN`, request errors, timeouts and an unavailable Delcore as no access. Do not delete data in these cases.
- Delete your retained tenant data automatically only after an explicit `TENANT_STATE_DELETED`. `BLOCKED` and `DELETING` never permit automatic deletion.
- Invalid IDs, an empty request or more than 100 IDs return an invalid-argument error. Credentials that are missing, expired or invalid return an unauthenticated error. A caller that is not allowed to read tenant state receives a permission-denied error. Other failures return an error with a correlation ID; include this ID, found in the error details and the `X-Delcore-Correlation-Id` header, when you ask for support.
- Retry a failed request later. A failed request never means a tenant is active or deleted.
