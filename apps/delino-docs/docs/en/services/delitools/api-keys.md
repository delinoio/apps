# DeliTools API keys

API keys let your servers call DeliTools without a Delino login token. Delcore creates and verifies the keys for every Delino service, so the same key facility works for other services too. You manage keys with your Delino login token, and you use a key as a bearer token when you call a service.

## Key types

| Type               | Acts as                             | Who can create, list and revoke it                               | Permission at use                                                             |
| ------------------ | ----------------------------------- | ---------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| Personal key       | You, inside one tenant              | Only you, and only while you are a current member of the tenant. | The key grants and your current tenant permissions. Both must allow the call. |
| Tenant service key | The tenant, independent of any user | A current Owner or Admin of the tenant.                          | The key grants only. It keeps working if the person who created it leaves.    |

- Other users never see or revoke your personal keys, including Owners and Admins. Billing and Member roles cannot manage tenant service keys.
- A machine-to-machine identity cannot manage keys. Keys never work as credentials for Delcore itself, only for the services they grant.
- Both types are tied to one tenant. A key cannot be used for another tenant.

## Create a key

Call `delcore.v1.DelcoreService/CreateApiKey` at `https://delcore.api.delino.io` with your Delino login token.

| Field        | Rules                                                                                                                                            |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `requestKey` | A UUID v7 you generate. Repeating the request with the same key and input replays the result.                                                    |
| `tenantId`   | The tenant the key belongs to.                                                                                                                   |
| `ownerKind`  | `API_KEY_OWNER_KIND_PERSONAL` or `API_KEY_OWNER_KIND_TENANT`.                                                                                    |
| `name`       | 1 to 100 bytes, not blank, with no control characters.                                                                                           |
| `grants`     | 1 to 16 grants. Each has a `serviceId` and 1 to 32 `capabilities` of up to 100 bytes. A service appears once, and capabilities are not repeated. |
| `expiresAt`  | Optional RFC 3339 time in the future. Without it the key is valid until you revoke it.                                                           |

Find the `serviceId` and the capabilities of a service in the Delcore catalog (`GetCatalog`). For DeliTools, grant the capabilities `translate` and `extract` that you need. Delcore only accepts capabilities that the service declared, so a mistyped capability is rejected when you create the key.

```sh
curl --request POST "https://delcore.api.delino.io/delcore.v1.DelcoreService/CreateApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"requestKey":"<uuid-v7>","tenantId":"<tenant-id>","ownerKind":"API_KEY_OWNER_KIND_TENANT","name":"Production translation","grants":[{"serviceId":"<deli-tools-service-id>","capabilities":["translate"]}]}'
```

This example sets no expiry, so the key stays valid until you revoke it. To add one, set `expiresAt` to a future RFC 3339 time.

The response contains the key details and the `secret`, which begins with `dlk_`.

- **The secret appears only once.** Copy it into your secret manager now. Delcore keeps only a hash, and it never shows the secret in lists or in a repeated response.
- **Lost secret.** If you lose the secret, or if the response to a repeated request has an empty `secret`, create a new key and revoke the old one.
- **Treat it like a password.** Keep it in server-side code and out of logs, repositories and frontend bundles.
- Limits: you can have 100 active personal keys for each user and tenant, and 100 active tenant keys per tenant. Revoked and expired keys do not count as active, but they stay for 30 days. A scope holds at most 500 keys of any state.

Grants and the expiry time are fixed when you create a key. There is no edit, renewal or rotation call. To change a key, create a replacement with the new grants or expiry, switch your application to it, and then revoke the old key.

## List and revoke keys

- `ListApiKeys` takes `tenantId` and `ownerKind`. It returns key details, including revoked and expired keys, and never secrets. Personal listings show only your own keys. Tenant listings require the Owner or Admin role. Use `page.afterId` with the returned `nextAfterId` to read more.
- `RevokeApiKey` takes a `requestKey`, `tenantId` and `keyId`. It takes effect immediately for new requests. Revoking again is safe.

## Use a key

Send the secret as `Authorization: Bearer dlk_...` to the service the key grants. For DeliTools, this works for `Translate` and `Extract` as described in the [API reference](/services/delitools/api-reference).

- `tenantId` is optional with a key. If you send it, it must be the tenant of the key.
- A key can use only the capabilities that you granted for that service. A key for one service never works for another.
- Delcore checks the key on every request, and again before each AI retry. Revocation, expiry, and a loss of permission or billing access stop new requests and further retries. A provider call that is already running can finish.
- A personal key stops working while you are no longer allowed to use the capability in the tenant, for example after you are removed from the tenant or lose a group permission. Removing you does not delete the key.
- A tenant key does not depend on the creator's membership.
- Calls with a key are billed to the key's tenant at the same prices as calls with a login token. See [Billing](/services/delitools/billing).
- When you delete a tenant, Delcore deletes its keys when the deletion becomes final. Personal keys are deleted as soon as the user's withdrawal is accepted. A key of a deleted tenant is invalid.

## Common errors

| Situation                                                                 | Result                                                               |
| ------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| Secret is mistyped, unknown, revoked or expired                           | `unauthenticated`. Delcore does not say which of these applies.      |
| Key lacks the service or capability                                       | `permission_denied`                                                  |
| `tenantId` does not match the key's tenant                                | `permission_denied`                                                  |
| Your role cannot manage this key type                                     | `permission_denied` from Delcore when you create, list or revoke it. |
| You revoke another user's personal key, or an unknown key                 | `not_found` from Delcore.                                            |
| You create a key in a tenant that is being deleted, or over the key limit | `failed_precondition` from Delcore.                                  |
| Invalid grant, name, service, capability or expiry                        | `invalid_argument` from Delcore when you create the key.             |
| Tenant billing is blocked                                                 | DeliTools returns `ERROR_REASON_BILLING_BLOCKED`.                    |
