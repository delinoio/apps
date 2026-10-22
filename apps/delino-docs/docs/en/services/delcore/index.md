# Delcore

Delcore provides shared tenants, access control and billing for Delino services. The official API server is `https://delcore.api.delino.io`. This page describes usage pricing, record expiry and the tenant state check for service developers.

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
- For cleanup caused by tenant deletion, delete your retained tenant data automatically only after an explicit `TENANT_STATE_DELETED`. `BLOCKED` and `DELETING` never permit that cleanup. Fixed record expiry applies independently of tenant state.
- Invalid IDs, an empty request or more than 100 IDs return an invalid-argument error. Credentials that are missing, expired or invalid return an unauthenticated error. A caller that is not allowed to read tenant state receives a permission-denied error. Other failures return an error with a correlation ID; include this ID, found in the error details and the `X-Delcore-Correlation-Id` header, when you ask for support.
- Retry a failed request later. A failed request never means a tenant is active or deleted.

## Usage prices and record expiry

Usage unit prices apply at the initial execution start for every customer, including existing subscribers. Report that start in `IngestEvent` as `occurred_at` and keep it across execution retries and usage reconciliation. A price change applies to later executions; late delivery and corrections keep the original price attribution. Recurring payment prices, selected features and included-usage grants retain their subscription terms.

Request deduplication lasts 30 days from the record's first creation. A retry does not extend this period. After expiry, the same request key can start a new operation subject to current permissions and independent payment or event identity checks. Accepted usage events retain their durable identities to prevent duplicate accounting effects.

Consumer services must delete usage-delivery and reconciliation records 30 days after first creation, even when execution or usage remains unknown or a report has not reached Delcore. Retry delivery and unknown-usage lookup until that expiry, then abandon the pending charge. Do not estimate usage for billing. This can permanently lose billable usage, and an expired execution deduplication key can permit a new execution and charge. Platform audit records and operational logs also expire 30 days after creation. These limits do not delete accounting entries already accepted by Delcore or change API-key validity.
