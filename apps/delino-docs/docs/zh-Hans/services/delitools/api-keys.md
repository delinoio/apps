# DeliTools API 密钥

API 密钥让您的服务器无需 Delino 登录令牌即可调用 DeliTools。Delcore 为每项 Delino 服务创建和验证密钥，因此同一套密钥机制也适用于其他服务。您使用 Delino 登录令牌管理密钥，并在调用服务时将密钥用作 Bearer 令牌。

## 密钥类型

| 类型         | 代表的身份           | 谁可以创建、列出和撤销                   | 使用时的权限                                             |
| ------------ | -------------------- | ---------------------------------------- | -------------------------------------------------------- |
| 个人密钥     | 您在某个租户内的身份 | 仅限您本人，且仅当您仍是该租户的成员时。 | 密钥的授权和您当前在租户中的权限。两者都必须允许该调用。 |
| 租户服务密钥 | 租户，独立于任何用户 | 租户当前的所有者或管理员。               | 仅取决于密钥的授权。即使创建者离开，该密钥仍可继续使用。 |

- 其他用户（包括所有者和管理员）始终无法查看或撤销您的个人密钥。账单和成员角色无法管理租户服务密钥。
- 机器到机器身份无法管理密钥。密钥不能用作 Delcore 本身的凭据，只能用于其获准访问的服务。
- 两种类型都绑定到一个租户。密钥不能用于其他租户。

## 创建密钥

使用您的 Delino 登录令牌，在 `https://delcore.api.delino.io` 调用 `delcore.v1.DelcoreService/CreateApiKey`。

| 字段         | 规则                                                                                                                               |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------- |
| `requestKey` | 由您生成的 UUID v7。使用相同的键和输入重复请求，会返回之前的结果。                                                                 |
| `tenantId`   | 密钥所属的租户。                                                                                                                   |
| `ownerKind`  | `API_KEY_OWNER_KIND_PERSONAL` 或 `API_KEY_OWNER_KIND_TENANT`。                                                                     |
| `name`       | 1 到 100 字节，不能为空白，且不能包含控制字符。                                                                                    |
| `grants`     | 1 到 16 项授权。每项包含一个 `serviceId` 和 1 到 32 个 `capabilities`，每个能力最多 100 字节。同一服务只能出现一次，能力不能重复。 |
| `expiresAt`  | 可选，采用 RFC 3339 格式的未来时间。若未设置，密钥将一直有效，直到您撤销它。                                                       |

在 Delcore 目录（`GetCatalog`）中查找服务的 `serviceId` 和能力。对于 DeliTools，请根据需要授予 `translate` 和 `extract` 能力。Delcore 仅接受服务已声明的能力，因此创建密钥时，拼写错误的能力会被拒绝。

```sh
curl --request POST "https://delcore.api.delino.io/delcore.v1.DelcoreService/CreateApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"requestKey":"<uuid-v7>","tenantId":"<tenant-id>","ownerKind":"API_KEY_OWNER_KIND_TENANT","name":"Production translation","grants":[{"serviceId":"<deli-tools-service-id>","capabilities":["translate"]}]}'
```

此示例未设置到期时间，因此密钥将一直有效，直到您撤销它。要添加到期时间，请将 `expiresAt` 设置为采用 RFC 3339 格式的未来时间。

响应包含密钥详细信息以及以 `dlk_` 开头的 `secret`。

- **密钥机密值只会显示一次。** 请立即将其复制到您的机密管理器中。Delcore 仅保留哈希值，绝不会在列表或重复请求的响应中显示机密值。
- **机密值丢失。** 如果您丢失了机密值，或者重复请求的响应中 `secret` 为空，请创建新密钥并撤销旧密钥。
- **像对待密码一样对待它。** 请将其保存在服务端代码中，不要让它出现在日志、代码仓库或前端构建产物中。
- 限制：每个用户在每个租户中最多可拥有 100 个有效的个人密钥，每个租户最多可拥有 100 个有效的租户密钥。已撤销和已过期的密钥不计入有效密钥数量，但会保留 30 天。一个作用域最多可容纳 500 个任意状态的密钥。

授权和到期时间在创建密钥时即已固定。不提供编辑、续期或轮换调用。要更改密钥，请使用新的授权或到期时间创建替代密钥，将应用程序切换到该密钥，然后撤销旧密钥。

## 列出和撤销密钥

- `ListApiKeys` 接受 `tenantId` 和 `ownerKind`。它会返回密钥详细信息，包括已撤销和已过期的密钥，但绝不返回机密值。个人密钥列表仅显示您自己的密钥。租户密钥列表需要所有者或管理员角色。使用返回的 `nextAfterId` 作为 `page.afterId` 来读取更多结果。
- `RevokeApiKey` 接受 `requestKey`、`tenantId` 和 `keyId`。撤销会立即对新请求生效。重复撤销是安全的。

## 使用密钥

以 `Authorization: Bearer dlk_...` 的形式将机密值发送到密钥获准访问的服务。对于 DeliTools，这适用于 [API 参考](/zh-Hans/services/delitools/api-reference)中介绍的 `Translate` 和 `Extract`。

- 使用密钥时，`tenantId` 为可选项。如果发送该字段，它必须是密钥所属的租户。
- 密钥只能使用您为该服务授予的能力。某个服务的密钥不能用于其他服务。
- Delcore 会在每次请求时检查密钥，并在每次 AI 重试前再次检查。撤销、过期以及失去权限或计费访问资格，都会阻止新请求和后续重试。已经运行中的提供商调用可以完成。
- 当您不再获准使用租户中的某项能力时，个人密钥将无法使用该能力，例如您被移出租户或失去某项群组权限后。将您移除不会删除密钥。
- 租户密钥不依赖于创建者的成员资格。
- 使用密钥的调用会向密钥所属的租户计费，价格与使用登录令牌的调用相同。请参阅[计费](/zh-Hans/services/delitools/billing)。
- 删除租户时，Delcore 会在删除最终生效时删除其密钥。用户的退出申请一经接受，其个人密钥就会立即被删除。已删除租户的密钥无效。

## 常见错误

| 情况                                             | 结果                                                       |
| ------------------------------------------------ | ---------------------------------------------------------- |
| 机密值拼写错误、未知、已撤销或已过期             | `unauthenticated`。Delcore 不会说明具体属于哪种情况。      |
| 密钥缺少服务或能力授权                           | `permission_denied`                                        |
| `tenantId` 与密钥所属的租户不匹配                | `permission_denied`                                        |
| 您的角色无法管理此类型的密钥                     | 创建、列出或撤销密钥时，Delcore 返回 `permission_denied`。 |
| 您撤销其他用户的个人密钥或未知密钥               | Delcore 返回 `not_found`。                                 |
| 您在正在删除的租户中创建密钥，或超出密钥数量限制 | Delcore 返回 `failed_precondition`。                       |
| 授权、名称、服务、能力或到期时间无效             | 创建密钥时，Delcore 返回 `invalid_argument`。              |
| 租户计费被阻止                                   | DeliTools 返回 `ERROR_REASON_BILLING_BLOCKED`。            |
