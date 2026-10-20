# DevBase

DevBase 是一个免费的应用程序终端用户目录。你可以注册自己的用户，并通过 Connect API 查询和更改他们。你的用户不需要 Delino 账户，你的应用程序仍使用自己的登录机制。DevBase 会为每个用户提供一个稳定的 ID，你可以将其传递给其他服务。

## 概念

- **租户：** Delino 个人空间或组织。Delcore 管理租户及其成员。
- **项目：** 租户内一个具名的终端用户目录。即使位于同一租户中，不同项目也不会共享用户。
- **终端用户：** 你的应用程序中的个人或账户。它有一个由你选择的外部 ID、一个由 DevBase 创建的内部 ID、可选的姓名和电子邮件，以及一个 JSON 元数据对象。
- **项目 API 密钥：** 你的应用程序用于读取或更改某个项目的终端用户的密钥。

## 登录和权限

每个请求都应发送 `Authorization: Bearer <token>` 标头。

- **开发者**使用其 Delino 登录令牌。只有租户当前的所有者或管理员才能管理项目和 API 密钥。他们也可以使用登录令牌读取和更改终端用户。
- **应用程序**使用项目 API 密钥。密钥以 `dbk_` 开头。`READ_ONLY` 密钥可以查询和列出用户。`CRUD` 密钥还可以创建、更新和删除用户。密钥不能管理项目或其他密钥，并且仅适用于其所属项目。

终端用户不会登录 DevBase。他们的记录不会授予其访问 Delino 的权限。

## 快速开始

1. **创建项目。** 使用你的租户 ID 和名称调用 `CreateProject`。租户中的名称不区分大小写且必须唯一。
2. **签发密钥。** 使用项目 ID 和权限调用 `IssueApiKey`。响应仅会返回一次密钥。请将其存储在你的密钥管理器中。
3. **注册用户。** 使用密钥调用 `CreateEndUser`。将返回的 `id` 与你自己的用户记录存储在一起。
4. **查询用户。** 使用内部 ID 调用 `GetEndUser`，或使用外部 ID 调用 `LookupEndUser`。
5. **更改或删除用户。** 使用当前的 `version` 调用 `UpdateEndUser` 和 `DeleteEndUser`。

所有过程都位于 `devbase.v1.DevBaseService` 上。请从服务器端代码或命令行调用 DevBase，可以使用任意 Connect 客户端，也可以通过 HTTP 使用纯 JSON。不要从浏览器调用它，也不要将项目密钥放入前端代码中：除 DevBase 自己的控制台外，它会拒绝所有浏览器来源。将 `DEVBASE_URL` 设置为 DevBase API 服务器的基础 URL，例如本地服务器可使用 `http://127.0.0.1:46317`。

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

在 JSON 中，字段名称使用 lowerCamelCase，枚举值使用其完整名称，而 `version` 等 64 位数字使用字符串表示。

## 项目

- 项目名称的长度为 1 至 256 字节，开头或结尾不能有空格。
- `UpdateProject` 用于更改名称。`DeleteProject` 会一次性删除项目、其中的所有用户及其所有密钥。
- 每个租户最多可以有 100 个项目。

## 项目 API 密钥

- 每个项目最多可以有 20 个有效密钥。密钥在被撤销或过期之前一直有效。
- 如果密钥必须过期，请在签发时设置 `expire_time`。如果未设置，密钥不会自动过期。
- DevBase 仅存储密钥的哈希值。如果密钥丢失，请撤销该密钥并签发一个新密钥。若要轮换密钥，请先签发新密钥，然后撤销旧密钥。
- 如果 `IssueApiKey` 的响应丢失，请使用相同的幂等键重复请求。重复请求的响应会包含密钥 ID 和权限，但不会包含密钥本身。
- `RevokeApiKey` 会立即生效。已撤销或过期的密钥会返回 `unauthenticated`。

## 终端用户

| 字段          | 规则                                                                      |
| ------------- | ------------------------------------------------------------------------- |
| `external_id` | 必填。1 至 256 字节。区分大小写。在项目内唯一。开头或结尾不能有空格。     |
| `name`        | 可选。最多 256 字节。                                                     |
| `email`       | 可选。最多 254 字节的纯电子邮件地址。多个用户可以使用相同的电子邮件地址。 |
| `metadata`    | 最大 16 KiB 的 JSON 对象。整个请求最大为 64 KiB。                         |

- 相同的外部 ID 可以存在于不同项目中。每个项目最多可以有 1,000,000 个用户。
- 创建操作必须显式执行。DevBase 不提供 upsert，也不能按电子邮件、姓名或元数据进行搜索。
- `UpdateEndUser` 使用 `update_mask`，可包含 `external_id`、`name`、`email` 和 `metadata`。如果被掩码指定的 `name` 或 `email` 值为空，则会清除该字段。被掩码指定的 `metadata` 会替换整个对象，而空对象会将其清除。你不能清除外部 ID，但可以更改它，并且内部 ID 会保持不变。
- `DeleteEndUser` 会立即永久删除用户。你可以立即再次使用该外部 ID。之后使用该 ID 创建的用户会获得新的内部 ID，并且与已删除的用户没有任何关联。
- DevBase 不会将删除操作通知其他服务。请在你自己的应用程序中删除下游数据。

## 列表

`ListProjects`、`ListApiKeys` 和 `ListEndUsers` 按创建顺序返回项目。默认页面大小为 50，最大为 100。使用 `next_page_token` 重复调用，直到其为空。应将该令牌视为不透明值。列表不包含总数。如果你在读取页面期间创建或删除项目，某个页面可能会漏掉或跳过这些项目。在整个读取期间始终存在的项目会出现一次。

## 重试和冲突

- 每个更改操作都需要一个长度为 8 至 128 个字符的 `idempotency_key`，可使用 `A-Z`、`a-z`、`0-9`、`.`、`_`、`:` 和 `-`。UUID 是很好的选择。
- 如果你在 24 小时内使用相同的键和相同的内容重复请求，则会获得第一次请求的结果，并且 DevBase 不会再次进行任何更改。可利用这一点处理响应丢失的情况。24 小时后，该键将被视为一个新请求。
- 使用相同的键但不同的内容会返回 `already_exists`。每个调用者都有自己的键，因此其他调用者无法查看或获取你的结果。
- 重复请求会根据当前权限再次进行检查。在第一次调用和重复调用之间，你的访问权限可能会终止。
- 删除用户后，重复该用户的创建或更新请求会返回 `not_found`，并且 DevBase 不会再次创建该用户。重复删除请求则会成功。删除项目后，该项目的密钥会停止工作。
- `UpdateProject`、`UpdateEndUser`、`DeleteProject` 和 `DeleteEndUser` 需要当前的 `version`。如果其他人先更改了该项目，则会收到 `aborted`。请重新读取该项目，并使用新版本重试。

## 限制

| 限制                 | 默认值                          |
| -------------------- | ------------------------------- |
| 每个租户的项目数     | 100                             |
| 每个项目的用户数     | 1,000,000                       |
| 每个项目的有效密钥数 | 20                              |
| 每个项目的用户请求数 | 每秒 1,000 个，突发上限为 2,000 |
| 每个租户的管理请求数 | 每秒 50 个，突发上限为 100      |

一个项目的所有密钥和登录令牌发出的用户请求会合并计数。一个租户的管理请求会合并计数。新密钥不会增加容量。如果超出限制，你会收到 `resource_exhausted`。对于速率限制，错误和 `Retry-After` 标头会告知你需要等待多长时间。操作员可以更改这些限制。它们用于保护服务，并非速度保证。

## 错误

| 代码                 | 含义                                                     |
| -------------------- | -------------------------------------------------------- |
| `unauthenticated`    | 令牌或密钥缺失、无效、已过期或已撤销。                   |
| `permission_denied`  | 你没有所需的角色或密钥权限，或者租户状态阻止了该操作。   |
| `not_found`          | 该项目不存在、属于其他项目，或者已被删除。               |
| `invalid_argument`   | 输入值无效。                                             |
| `already_exists`     | 名称或外部 ID 已存在，或者幂等键已用于其他内容。         |
| `aborted`            | 版本已过时。                                             |
| `resource_exhausted` | 受到速率限制或资源限制。                                 |
| `unavailable`        | 依赖项或数据库发生故障。更改可能已应用，也可能尚未应用。 |

每个响应以及 DevBase 自身返回的每个错误都包含一个跟踪 ID。寻求支持时请提供该 ID。大于 256 KiB 的请求会被传输层提前拒绝，并返回 `resource_exhausted`，且不提供跟踪 ID；此时应减小请求大小。如果更改操作返回 `unavailable`，请使用相同的幂等键重复相同请求，以确认更改是否已应用。

## 租户状态

DevBase 免费提供。你的套餐、余额和账单绝不会限制其使用。只有操作员封禁和租户删除会造成限制。

- 当租户被封禁或正在删除时，项目 API 密钥无法使用。所有者和管理员仍然可以读取项目和密钥、撤销密钥，以及删除用户和项目。他们不能创建或更改任何内容、签发密钥或读取用户。
- 如果 DevBase 无法确认租户状态，它会以 `unavailable` 或 `permission_denied` 拒绝请求，而不会进行猜测。
- 租户被永久删除后，DevBase 会自动删除其项目、用户和密钥。

## DevBase 不提供的功能

DevBase 不会让你的用户登录，不会为他们创建 Delino 账户，也不会代表他们执行操作。它不会在项目之间共享用户，不会在租户之间移动项目，不会导入或导出数据，也不会搜索用户。备份（如果有）不属于 DevBase 对其活动数据执行的即时删除范围。
