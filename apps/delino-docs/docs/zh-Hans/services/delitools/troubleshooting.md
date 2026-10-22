# DeliTools 配置与故障排查

本页帮助你为 DeliTools 配置客户端并解决错误。有关字段规则，请参阅 [API 参考](/zh-Hans/services/delitools/api-reference)。

## 配置客户端

- **基础 URL。** 使用服务运营方提供的 DeliTools API 服务器 URL。过程位于 `/delitools.v1.DelitoolsService/Translate` 和 `/delitools.v1.DelitoolsService/Extract`。
- **协议。** 使用 Connect 客户端，或通过 `POST` 发送 JSON，并设置 `Content-Type: application/json`。JSON 字段名使用 lowerCamelCase。
- **凭据。** 发送 `Authorization: Bearer <credential>`。使用 Delino 登录令牌，或以 `dlk_` 开头的 API 密钥。将密钥保存在服务端代码中。DeliTools 不支持来自浏览器的调用。
- **租户。** 登录令牌需要 `tenantId`。API 密钥本身已标识其所属租户；如果你同时发送 `tenantId`，它必须与该租户匹配。
- **模型。** 使用运营方允许的模型标识符。提取使用固定模型，没有 `model` 字段。
- **超时。** 根据最大请求的需要设置客户端超时时间。一次提供方调用可能需要数秒，且 DeliTools 可能会重试暂时性故障，因此完整调用可能耗时更长。请求超过运营方的执行超时时间时，会返回 `ERROR_REASON_TIMEOUT`；如果客户端自身的超时先关闭了调用，DeliTools 会将其视为取消。
- **重试。** 对于 `ERROR_REASON_RATE_LIMITED`、`ERROR_REASON_DEPENDENCY_UNAVAILABLE` 和 `ERROR_REASON_TIMEOUT`，应采用指数退避并加入少量随机延迟进行重试；对于 `ERROR_REASON_PROVIDER_FAILED`，仅在 Connect 代码为 `unavailable` 时才重试。每次重试都是一次新的执行，可能会产生费用。请参阅[计费](/zh-Hans/services/delitools/billing)。对于验证失败的请求，不要在未修改的情况下重试。

## 解决错误

| 原因                                  | 处理方法                                                                                                                                                                                           |
| ------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | 修正请求。检查是否存在空字符串，并检查语言标签、租户 ID、模型以及模式是否为有效的 JSON。                                                                                                           |
| `ERROR_REASON_UNAUTHENTICATED`        | 检查令牌或密钥。它可能输入有误、已过期、已撤销，或属于已删除的租户。如果你丢失了密钥的秘密值，请创建新密钥。如果所有有效密钥突然都无法使用，请告知服务运营方：服务身份配置错误会使密钥看起来无效。 |
| `ERROR_REASON_PERMISSION_DENIED`      | 检查密钥是否授予了 `translate` 或 `extract` 权限、用户是否仍在租户中拥有权限，以及你发送的任何 `tenantId` 是否为密钥所属的租户。                                                                   |
| `ERROR_REASON_BILLING_BLOCKED`        | 在 Delcore 中解决余额或续订等计费问题，然后重试。                                                                                                                                                  |
| `ERROR_REASON_LIMIT_EXCEEDED`         | 减少发送的数据：减少条目、缩短文本或缩小模式。将大型任务拆分为多个请求。                                                                                                                           |
| `ERROR_REASON_RATE_LIMITED`           | 降低请求速度，减少并发请求，并在短暂延迟后重试。                                                                                                                                                   |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | 如果错误为 `invalid_argument`，则该模型不被允许：请选择其他模型。如果为 `failed_precondition`，请让运营方注册该模型的价格。                                                                        |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | 移除[支持的子集](/zh-Hans/services/delitools/api-reference)以外的关键字。根节点必须是对象模式。                                                                                                    |
| `ERROR_REASON_PROVIDER_FAILED`        | 如果 Connect 代码为 `unavailable`，请等待后重试。如果为 `internal`，则表示提供方拒绝了请求：不要在未修改的情况下重试，请检查请求，并携带请求 ID 联系运营方。                                       |
| `ERROR_REASON_INVALID_OUTPUT`         | 重试一次。如果再次发生，请简化模式或输入。本次尝试会计费。                                                                                                                                         |
| `ERROR_REASON_EXTRACTION_FAILED`      | 文本缺少必需的信息。请提供缺失的详细信息，或在模式中将该属性设为可选。本次尝试会计费。                                                                                                             |
| `ERROR_REASON_TIMEOUT`                | 发送更小的请求。稍后重试。                                                                                                                                                                         |
| `ERROR_REASON_CANCELED`               | 客户端关闭了连接。检查客户端是否超时或关闭。                                                                                                                                                       |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | 稍后重试。DeliTools 无法检查授权时，会拒绝执行付费任务。                                                                                                                                           |

## 常见问题

- **密钥在一次请求中可用，但在下一次请求中失败。** 密钥过期、被撤销，或其所属用户失去权限后，该密钥会立即失效。使用 `ListApiKeys` 检查。
- **只有 `resource_exhausted` 错误，没有原因。** 请求体超过了运营方设置的总体大小限制。请减少发送的数据。
- **结果看起来不正确。** 模型可能会出错，符合模式的结果也不一定正确。请让模式中 `description` 字段内的指示更加清晰，并核查重要值。
- **出现意外费用。** 用量已知的失败尝试以及重复请求都会计费。请参阅[计费](/zh-Hans/services/delitools/billing)。
- **暂时看不到用量。** 用量会在调用后不久传送到 Delcore。请稍后再次查看。

## 寻求支持

请提供响应体或 `X-Delitools-Request-Id` 标头中的请求 ID、过程名称、时间和错误原因。不要发送你的 API 密钥、登录令牌或提交的文本。
