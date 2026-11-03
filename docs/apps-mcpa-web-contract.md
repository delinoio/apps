# Managed CPA web console contract

## Scope

`apps/mcpa` (`@apps/mcpa`) owns the Managed CPA web console served at `https://mcpa.delino.io`. Tenant members use it to manage resources, provider credentials, call keys, routing, ACL, budgets, usage, errors and the audit log. The files use Apache-2.0. The [server contract](servers-mcpa-api-contract.md) owns authorization and behavior; the [protocol contract](protos-mcpa-v1-contract.md) owns the wire types.

## Runtime and Language

TypeScript, React and Rsbuild follow the repository frontend stack. Node.js 24 runs development and build tooling. The console is a static single-page application deployed to Cloudflare Pages under the repository default; the default Pages project name is the app directory name `mcpa`. Server state uses React Query through `@connectrpc/connect-query` over the `mcpa.v1` bindings exported by `@repo/protos` (`@repo/protos/mcpa/v1/mcpa_pb`). Shared React primitives come from `@packages/ui` where they fit. UX and UI decisions follow the Toss Design Guidelines. The console is English only; localization into other languages is out of scope.

## Users and Operators

- Resource managers and invokers in Delcore personal and organization tenants.
- Holders of the tenant budget capability.
- Maintainers who build and publish the static site.

## Interfaces and Contracts

### Session, CSRF and transport

- The console never handles Logto tokens. It starts login by navigating to the API's `/auth/login?return_to=<exact console URL>` and relies on the `mcpa_session` cookie that the API sets on its own origin.
- On load and after navigation from login, it calls `GET /auth/session` with credentials:
  - 200 provides `user_id`, `csrf_token` and `expires_at`.
  - 401 shows the signed-out state with a sign-in action.
  - 403 `mfa_required` shows an MFA screen that explains the requirement and links to `reauthentication_url` for enrollment or reauthentication. Because the server never adds MFA evidence to an existing session, the screen also offers to sign in again: the console ends the restricted session with `POST /auth/logout` and the response's `csrf_token`, then starts a new login.
- Every Connect call uses the API origin with `credentials: "include"` and sends `X-Mcpa-CSRF` with the session CSRF token. The console refreshes with `POST /auth/refresh` before `expires_at` and on an authentication failure, then retries once. It signs out with `POST /auth/logout`, also with the CSRF header.
- Cached management responses belong to the signed-in user. Signing out drops them. When a session refetch or recovery finds a different user, for example after another tab signed in again, the console drops the previous user's cached responses and mutations and starts the console views again for the new user.
- A Connect error with `ERROR_REASON_MFA_REQUIRED` routes to the MFA screen. `ERROR_REASON_UNAUTHENTICATED` routes to sign-in.
- The API origin is public build configuration (`PUBLIC_MCPA_API_URL`, default `https://mcpa.api.delino.io`). Only `PUBLIC_*` values enter the browser bundle; the bundle contains no service credentials.

### Navigation and tenancy

- `GetViewer` lists the user's Delcore tenants. A tenant switcher selects the active tenant; personal tenants are labeled. The selected tenant scopes resource lists, tenant usage and the tenant budget. Switching tenants opens the new tenant's views at their first page with default filters.
- Each resource view shows the caller's effective role (`caller_role`) and hides or disables actions the role does not permit. Server responses remain authoritative.

### Screens

| Screen                         | Content and actions                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Role                                               |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------- |
| Resource list and creation     | Paginated resources of the tenant with name, description, role and creation time. Create a resource with a name and an optional description, using a generated UUID v7 request key. The creator receives `manage`.                                                                                                                                                                                                                                                                                                                                                                              | List: any role; create: `create_resources`         |
| Credentials                    | List credentials with provider, kind, label, account label or secret hint, base URL and models. Connect an OAuth account: start the flow, then either open `authorization_url` and paste the final redirect URL (authorization code) or show `user_code` and `verification_uri` and poll the flow (device code), showing pending, completed, failed and expired states. Register an API key for any supported provider, with base URL and models when the provider requires them. Remove a credential after confirmation.                                                                       | `manage`                                           |
| Call keys                      | List keys with name, display prefix, issuer, creation, expiry, last use and revocation. Issue a key with a name and an optional expiry; show the secret once in a dialog with a copy action and a warning that it cannot be shown again. Each opened form uses one generated UUID v7 request key, and a retry with the same expiry choice resends the first expiry timestamp. When the server replays an earlier issuance without the secret, explain that the key was created but cannot be shown and offer to revoke it so the user can issue a replacement. Revoke a key after confirmation. | Own keys: `invoke`; all keys: `manage`             |
| Endpoint guide                 | The OpenAI, Claude and Gemini base URLs of the resource with copy actions and short request examples that use a placeholder key, never a real secret.                                                                                                                                                                                                                                                                                                                                                                                                                                           | `invoke`                                           |
| Routing                        | Load balancing (round-robin) and the model allowlist editor, explaining that an empty list allows every model the credentials can serve.                                                                                                                                                                                                                                                                                                                                                                                                                                                        | `manage`                                           |
| ACL                            | List user and group entries with roles. Add or change an entry by choosing a tenant group from `ListTenantGroups` or entering a user ID, and choosing `manage` or `invoke`. Remove an entry after confirmation. Show the last-manager rule when the server rejects a removal.                                                                                                                                                                                                                                                                                                                   | `manage`                                           |
| Budgets                        | Resource budget: limit, estimated spend, Delcore billing period and exhausted state; set or remove the limit. Tenant budget: the same fields for the tenant.                                                                                                                                                                                                                                                                                                                                                                                                                                    | Resource: `manage`; tenant: `manage_tenant_budget` |
| Usage and errors               | Usage summary (request bytes, response bytes, total GiB and estimated USD at USD 0.07 per GiB) with a statement that Delcore issues the final bill, and daily rows. Error view of call records with time, resource, key, provider, model, status, provider status class, error class, bytes per segment, latency and request ID. Both filter by resource, model, provider, call key, status and time range.                                                                                                                                                                                     | Usage: `invoke`; errors: `manage`                  |
| Audit log                      | Paginated audit events with time, action, actor kind, actor, target and details.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | `manage`                                           |
| Resource settings and deletion | Edit name and description. Delete the resource after typing its name to confirm, explaining that keys and credentials are removed immediately and records stay until retention ends.                                                                                                                                                                                                                                                                                                                                                                                                            | `manage`                                           |

The console never displays request or response bodies, because the server stores none. Estimated USD values are labeled as estimates and shown with at least two and at most six fraction digits, rounded half up. The API returns exact decimals, so this rounding is display-only.

Byte counts use binary units (B to PiB) and round to one fraction digit below 100 and none from 100. The value is rounded before the unit is chosen, so a count that rounds to 1024 of a unit is shown in the next unit (1,048,575 bytes shows as `1.0 MiB`). A budget meter is full whenever the budget is exhausted, including a zero limit.

### Interaction rules

- Dialogs close with `Esc`, and focus returns to the control that opened them.
- Forms check the server's input limits before sending a request and show a field error instead: call key, resource and credential names up to 100 UTF-8 bytes, descriptions up to 1,000 bytes, model allowlist entries 1 to 200 bytes without whitespace or control characters, API-key base URLs over `https` only, and typed budget limits of up to 12 integer and 6 fraction digits (narrower than the server's 64-character limit; an untouched prefilled limit is sent back unchanged). The server stays the source of truth for all other validation.
- A form with a single critical input focuses that input when it opens, for example the delete confirmation, the call key name and the budget limit.
- Destructive actions (delete resource, remove credential, revoke key, remove ACL entry) require explicit confirmation.
- Every data view has explicit empty, loading, error and permission states:
  - Empty states explain the next step, such as connecting a credential.
  - Error states show the safe message and trace ID and offer a retry.
  - Permission states explain which role or capability is missing.
  - `ERROR_REASON_DEPENDENCY_UNAVAILABLE` shows a temporary-unavailability message.

### Development

Run package-local `pnpm dev` to serve the console on `127.0.0.1:46328` through Portless route `mcpa`. The `dev` script injects Infisical development variables from the default path and fails on a port conflict. Root `pnpm dev` includes it. For local API use, set `PUBLIC_MCPA_API_URL` to the printed `mcpa-api` origin and configure the API's `WEB_ORIGIN` and `MCPA_WEB_RETURN_URLS` with the printed console origin.

## Storage

The console persists no account data. The session cookie belongs to the API origin; the console stores only the active tenant selection in local storage. React Query caches responses in memory. `dist` is ignored generated build output.

## Security

- The bundle contains no service credentials or secrets.
- API keys entered for registration are sent once and cleared from component state after submission. The query client's mutation cache drops the request when it fails or when the dialog closes or unmounts.
- Call key secrets are held only in the issuance dialog's state and discarded, including from the query client's mutation cache, when the dialog closes or unmounts.
- The console sends CSRF headers on every Connect call and session mutation.
- It does not log secrets or tokens to the browser console or telemetry.
- Deployment credentials remain outside the repository.

## Logging

Rsbuild reports startup and build failures. Runtime errors surface the Connect error reason and trace ID to the user for support correlation, without secrets.

## Build and Test

- Run `pnpm test` in `apps/mcpa` for TypeScript checks, unit and interaction tests, and a production build. `pnpm build` emits `dist`; remove generated `dist` after validation.
- Tests cover these behaviors:
  - Session states (signed out, MFA required, signed in), CSRF headers and the refresh retry.
  - `Esc` closing dialogs and focus on single-input forms.
  - Empty, loading, error and permission states.
  - One-time secret display and secret removal from the mutation cache.
  - The OAuth flow states.
- Affected CI runs these checks on Ubuntu. Cloudflare Pages publishes `apps/mcpa/dist` with repository `LICENSE` and `NOTICE`. Domain binding and deployment require separate platform configuration; a local build does not establish deployment acceptance.

## Dependencies and Integrations

- `@repo/protos` for the `mcpa.v1` bindings and `@connectrpc/connect-query` with `@connectrpc/connect-web` for transport.
- React Query for server state and `@packages/ui` for shared primitives.
- The API at `https://mcpa.api.delino.io` for `/auth/*` and Connect. The console calls no other service directly: Delcore and Logto are reached only through the API.

## Change Triggers

Update this contract with the [server contract](servers-mcpa-api-contract.md), the [protocol contract](protos-mcpa-v1-contract.md) and the [project index](project-mcpa.md) when screens, session handling or interfaces change. Update the development port table in [repository defaults](repository-defaults.md) and the [workflow contract](repository-workflow-contract.md) when the port or route changes.

## References

- [Project index](project-mcpa.md)
- [Server contract](servers-mcpa-api-contract.md)
- [Protocol contract](protos-mcpa-v1-contract.md)
- [Repository defaults](repository-defaults.md)
- [Environment contract](repository-environment-contract.md)
