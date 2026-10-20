# DevBase

DevBase is a free directory of the end users of your applications. You register your own users, look them up and change them through a Connect API. Your users do not need Delino accounts, and your application keeps its own login. DevBase gives each user a stable ID that you can pass to other services.

## Concepts

- **Tenant:** A Delino personal space or organization. Delcore manages tenants and their members.
- **Project:** A named directory of end users inside a tenant. Projects do not share users, even in the same tenant.
- **End user:** A person or account in your application. It has an external ID that you choose, an internal ID that DevBase creates, an optional name and email, and a JSON object of metadata.
- **Project API key:** A secret that your application uses to read or change the end users of one project.

## Sign in and permissions

Send an `Authorization: Bearer <token>` header with each request.

- **Developers** use their Delino login token. Only a current Owner or Admin of the tenant can manage projects and API keys. They can also read and change end users with the login token.
- **Applications** use a project API key. A key starts with `dbk_`. A `READ_ONLY` key looks up and lists users. A `CRUD` key also creates, updates and deletes users. A key never manages projects or keys, and it works only for its own project.

End users never sign in to DevBase. Their records do not give them access to Delino.

## Quick start

1. **Create a project.** Call `CreateProject` with your tenant ID and a name. Names are unique in a tenant, ignoring case.
2. **Issue a key.** Call `IssueApiKey` with the project ID and a permission. The response contains the secret one time. Store it in your secret manager.
3. **Register users.** Call `CreateEndUser` with the key. Store the returned `id` next to your own user record.
4. **Look users up.** Use `GetEndUser` with the internal ID or `LookupEndUser` with the external ID.
5. **Change or delete users.** Use `UpdateEndUser` and `DeleteEndUser` with the current `version`.

All procedures are on `devbase.v1.DevBaseService`. Use any Connect client, or send plain JSON over HTTP. Set `DEVBASE_URL` to the base URL of your DevBase API server, for example `http://127.0.0.1:46317` for a local server.

```sh
# Create a project with your Delino login token.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateProject" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"3f1f6d1e-5a0a-4d7b-9a53-0c1d9f7a7d01","tenantId":"<tenant-id>","name":"My app"}'

# Issue a CRUD key for the project. The secret appears only in this response.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/IssueApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"8c2b7a60-91a0-4a53-a9c3-6f7f0b2f1e02","projectId":"<project-id>","permission":"API_KEY_PERMISSION_CRUD"}'

# Register an end user with the project key.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateEndUser" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DEVBASE_KEY" \
  --data '{"idempotencyKey":"d5f0c3a2-2b1e-4f59-8f0a-5b7c9e4d3a03","projectId":"<project-id>","externalId":"user-1001","email":"ada@example.com","metadata":{"plan":"pro"}}'
```

In JSON, field names use lowerCamelCase, enum values use their full names, and 64-bit numbers such as `version` are strings.

## Projects

- A project name has 1 to 256 bytes, with no space at the start or end.
- `UpdateProject` changes the name. `DeleteProject` deletes the project, all of its users and all of its keys in one step.
- A tenant can have up to 100 projects.

## Project API keys

- A project can have up to 20 active keys. A key is active until it is revoked or expires.
- Set `expire_time` when you issue a key if it must expire. Without it, the key does not expire automatically.
- DevBase stores only a hash of the secret. If you lose a secret, revoke that key and issue a new one. To rotate a key, issue the new key first and then revoke the old key.
- If the response to `IssueApiKey` is lost, repeat the request with the same idempotency key. The repeated response contains the key ID and permission, but not the secret.
- `RevokeApiKey` takes effect immediately. A revoked or expired key returns `unauthenticated`.

## End users

| Field         | Rule                                                                                           |
| ------------- | ---------------------------------------------------------------------------------------------- |
| `external_id` | Required. 1 to 256 bytes. Case-sensitive. Unique in the project. No space at the start or end. |
| `name`        | Optional. Up to 256 bytes.                                                                     |
| `email`       | Optional. A plain address of up to 254 bytes. Several users can have the same email.           |
| `metadata`    | A JSON object of up to 16 KiB. A whole request is up to 64 KiB.                                |

- The same external ID can exist in different projects. A project can have up to 1,000,000 users.
- Creation is explicit. DevBase has no upsert, and no search by email, name or metadata.
- `UpdateEndUser` uses `update_mask` with `external_id`, `name`, `email` and `metadata`. A masked `name` or `email` with an empty value clears it. A masked `metadata` replaces the whole object, and an empty object clears it. You cannot clear the external ID. You can change it, and the internal ID stays the same.
- `DeleteEndUser` removes the user immediately and permanently. You can use the external ID again at once. The next user with that ID gets a new internal ID and has no link to the deleted user.
- DevBase does not tell other services about deletions. Delete downstream data in your own application.

## Lists

`ListProjects`, `ListApiKeys` and `ListEndUsers` return items in creation order. The default page size is 50 and the maximum is 100. Repeat the call with `next_page_token` until it is empty. Treat the token as opaque. Lists have no total count. If you create or delete items while you read the pages, a page can miss or skip those items. An item that exists during the whole read appears one time.

## Retries and conflicts

- Every change needs an `idempotency_key` of 8 to 128 characters from `A-Z`, `a-z`, `0-9`, `.`, `_`, `:` and `-`. A UUID works well.
- If you repeat a request with the same key and the same content within 24 hours, you get the first result and DevBase does not change anything again. Use this to resolve lost responses. After 24 hours, the key is a new request.
- The same key with different content returns `already_exists`. Each caller has its own keys, so another caller cannot see or take your result.
- A repeated request is checked again against current permissions. Your access can end between the first call and the repeat.
- After you delete a user, a repeat of that user's create or update returns `not_found`, and DevBase does not create the user again. A repeat of the delete succeeds. After you delete a project, keys of that project stop working.
- `UpdateProject`, `UpdateEndUser`, `DeleteProject` and `DeleteEndUser` need the current `version`. If someone changed the item first, you get `aborted`. Read the item again and retry with the new version.

## Limits

| Limit                          | Default                          |
| ------------------------------ | -------------------------------- |
| Projects per tenant            | 100                              |
| Users per project              | 1,000,000                        |
| Active keys per project        | 20                               |
| User requests per project      | 1,000 per second, burst of 2,000 |
| Management requests per tenant | 50 per second, burst of 100      |

User requests count together for all keys and login tokens of a project. Management requests count together for a tenant. A new key does not add capacity. If you exceed a limit, you get `resource_exhausted`. For a rate limit, the error and the `Retry-After` header tell you how long to wait. An operator can change these limits. They protect the service and are not a speed guarantee.

## Errors

| Code                 | Meaning                                                                          |
| -------------------- | -------------------------------------------------------------------------------- |
| `unauthenticated`    | The token or key is missing, invalid, expired or revoked.                        |
| `permission_denied`  | You do not have the role or key permission, or the tenant state blocks it.       |
| `not_found`          | The item does not exist, belongs to another project, or was deleted.             |
| `invalid_argument`   | An input value is not valid.                                                     |
| `already_exists`     | A name or external ID exists, or an idempotency key was used with other content. |
| `aborted`            | The version is old.                                                              |
| `resource_exhausted` | A rate limit or a resource limit applies.                                        |
| `unavailable`        | A dependency or the database failed. A change may or may not have been applied.  |

Every response, and every error that DevBase itself returns, has a trace ID. Include it when you ask for support. A request larger than 256 KiB is rejected earlier, by the transport, with `resource_exhausted` and no trace ID; reduce the request size instead. If you get `unavailable` for a change, repeat the same request with the same idempotency key to find out whether it was applied.

## Tenant state

DevBase is free. Your plan, balance and invoices never limit it. Only operator blocking and tenant deletion can.

- While a tenant is blocked or is being deleted, project API keys do not work. Owners and Admins can still read projects and keys, revoke keys, and delete users and projects. They cannot create or change anything, issue keys, or read users.
- If DevBase cannot confirm the tenant state, it denies the request with `unavailable` or `permission_denied` and does not guess.
- After a tenant is permanently deleted, DevBase deletes its projects, users and keys automatically.

## What DevBase does not do

DevBase does not sign your users in, create Delino accounts for them or act on their behalf. It does not share users between projects, move projects between tenants, import or export data, or search users. Backups, if any, are outside the immediate deletion that DevBase applies to its active data.
