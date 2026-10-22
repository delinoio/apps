# DeliTools API 参考

本页介绍 DeliTools 的两种方法。两者都是 `delitools.v1.DelitoolsService` 上的 Connect 过程，并通过 HTTP `POST` 接受 JSON。JSON 中的字段名采用 lowerCamelCase 格式。请发送 `Content-Type: application/json` 和 `Authorization: Bearer <credential>`。有关凭据和租户的信息，请参阅 [DeliTools](/zh-Hans/services/delitools/)。

方法处理程序生成的每个响应都包含请求 ID，分别位于响应正文和 `X-Delitools-Request-Id` 标头中。因传输层大小限制而被拒绝的请求（参见错误表）没有请求 ID。响应不包含 token 用量或费用。请从 Delcore 中读取这些信息；参见[计费](/zh-Hans/services/delitools/billing)。

每次调用都是一次新的执行。DeliTools 不提供幂等键，也不存储结果。如果重复请求，它会再次执行，并可能再次计费。

## Translate

`POST /delitools.v1.DelitoolsService/Translate`

| 字段             | 类型       | 规则                                                                      |
| ---------------- | ---------- | ------------------------------------------------------------------------- |
| `tenantId`       | 字符串     | 使用 Delino 登录令牌时必填。使用 API 密钥时，留空或填写该密钥所属的租户。 |
| `texts`          | 字符串数组 | 必填。至少包含一项。不允许空字符串。项数和总大小受运营方限制。            |
| `targetLanguage` | 字符串     | 必填。BCP 47 语言标签，例如 `fr` 或 `pt-BR`。                             |
| `sourceLanguage` | 字符串     | 可选。BCP 47 语言标签。如果省略，模型会检测源语言。                       |
| `model`          | 字符串     | 必填。运营方允许的模型标识符。                                            |

响应包含 `requestId` 和 `translations`。`translations` 的项数及顺序与 `texts` 相同。

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"sourceLanguage":"en","targetLanguage":"fr","model":"<model-id>"}'
```

```json
{
  "requestId": "<request-id>",
  "translations": ["Bonjour", "Bonjour, bonne journée"]
}
```

- **语言标签。** DeliTools 会检查标签是否符合 BCP 47 格式，不区分大小写，最多包含 64 个字符，且主语言子标签由两个或三个字母组成。仅由私用子标签构成的标签和历史遗留标签会被拒绝。DeliTools 不检查模型是否支持该语言。
- **全部成功或全部失败。** 结果具有原子性。如果任何一项失败，整个请求就会失败，且不会返回部分翻译。
- **无特殊处理。** 省略源语言、混合语言输入以及已经是目标语言的文本都会发送给模型。DeliTools 不会拒绝这些输入，也不会替你直接复制源文本。
- **模型。** 只有运营方允许且已在 Delcore 中登记价格的模型才能使用。系统绝不会替换为其他模型。请向服务运营方咨询可用的模型标识符。

## Extract

`POST /delitools.v1.DelitoolsService/Extract`

| 字段         | 类型   | 规则                                                                        |
| ------------ | ------ | --------------------------------------------------------------------------- |
| `tenantId`   | 字符串 | 使用 Delino 登录令牌时必填。使用 API 密钥时，留空或填写该密钥所属的租户。   |
| `text`       | 字符串 | 必填且不能为空。大小受运营方限制。                                          |
| `jsonSchema` | 字符串 | 必填。以 JSON 字符串形式提供的 JSON Schema 文档，且只能使用下述支持的子集。 |

模型固定为 `openai/gpt-6-luna`，无法自行选择。响应包含 `requestId` 和 `json`，后者是符合你的 schema 的 JSON 字符串。

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Extract" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"text":"Please send three boxes of green tea.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"item\":{\"type\":\"string\"},\"quantity\":{\"type\":\"integer\",\"minimum\":1}},\"required\":[\"item\",\"quantity\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "<request-id>",
  "json": "{\"item\":\"green tea\",\"quantity\":3}"
}
```

- **基于原文是一项指令，而非检查。** DeliTools 会指示模型只从你的文本中获取值。当模型报告缺少必需信息时，请求会失败并返回 `ERROR_REASON_EXTRACTION_FAILED`。DeliTools 无法验证返回值是否来自你的文本，因此模型仍可能返回符合 schema 的虚构值。
- **输出检查。** DeliTools 会根据你的 schema 验证结果。格式错误或不符合 schema 的输出会导致失败，返回 `ERROR_REASON_INVALID_OUTPUT`。DeliTools 不会通过重试来修复无效输出。
- **准确性。** 符合 schema 的结果仍可能有误。请核查重要结果。
- **不支持的 schema 会在执行前失败。** DeliTools 会在运行模型之前拒绝超出支持子集的 schema，因此此类请求不会计费。

### 支持的 JSON Schema 子集

schema 按 JSON Schema 2020-12 读取。它必须是 JSON 对象。`$schema` 是可选的，且只允许出现在根级别。它必须是字符串，否则该 schema 不受支持。DeliTools 不使用此字符串来选择草案版本。每个 schema，包括根 schema、各属性的 schema 和 `items` schema，都需要包含 `type`、`enum` 或 `const`。schema 的深度最多为 8 层，最多包含 256 个 schema 节点，其大小受运营方限制。

| 关键字                   | 规则                                                                                                                                             |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `type`                   | `object`、`array`、`string`、`number`、`integer`、`boolean`、`null`，或由这些类型组成的数组，例如用 `["string","null"]` 表示可为 null 的值。     |
| `properties`, `required` | `properties` 将名称映射到 schema。每个 `required` 名称都必须出现在 `properties` 中。每个对象最多包含 64 个属性，且 `required` 中的名称不得重复。 |
| `additionalProperties`   | 只能为 `true` 或 `false`（不能是 schema）。                                                                                                      |
| `items`                  | 用于所有项的单个 schema。                                                                                                                        |
| `enum`, `const`          | `enum` 包含 1 到 64 个不同的值。`const` 为单个值。                                                                                               |
| `minLength`, `maxLength` | 用于字符串，为 0 到 1,000,000 之间的整数。最小值不能大于最大值。                                                                                 |
| `minItems`, `maxItems`   | 用于数组，为 0 到 1,000,000 之间的整数。最小值不能大于最大值。                                                                                   |
| `minimum`, `maximum`     | 用于 `number` 和 `integer`，为有限数值。最小值不能大于最大值。                                                                                   |
| `title`, `description`   | 提供给模型的说明。它们不会改变验证行为。                                                                                                         |

以下内容不受支持，并会返回 `ERROR_REASON_UNSUPPORTED_SCHEMA`：`$ref`、`$defs`、`definitions`、`$id`、`$comment`、`oneOf`、`anyOf`、`allOf`、`not`、`if`/`then`/`else`、`patternProperties`、`propertyNames`、`pattern`、`format`、`uniqueItems`、`multipleOf`、`exclusiveMinimum`、`exclusiveMaximum`、`contains`、`prefixItems`、`default`、`unevaluated` 关键字、布尔值 schema、未知关键字，以及过大或嵌套过深的 schema。DeliTools 从不跟随引用，因此 schema 无法使其获取任何内容。

如果只希望对象包含你列出的属性，请为对象设置 `"additionalProperties": false`。

## 错误

错误使用标准 Connect 代码。错误详情包含稳定的原因标识和请求 ID。错误消息为英文，且绝不会包含你的内容。

| 原因                                  | Connect 代码                                | 含义                                                                                                                                                                   |
| ------------------------------------- | ------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                          | 字段无效：空数组或空字符串、错误的语言标签、缺失或格式错误的租户、缺失模型，或 `jsonSchema` 不是有效的 JSON。运营方不允许的模型对应 `ERROR_REASON_MODEL_UNAVAILABLE`。 |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                           | 凭据缺失、格式错误、已过期、已撤销或未知。                                                                                                                             |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                         | 凭据缺少所需能力、密钥授权或你的租户权限，或者你发送的 `tenantId` 不是密钥所属的租户。                                                                                 |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                       | Delcore 阻止了付费使用，例如由于余额、续订、争议、运营方封禁或租户正在被删除。                                                                                         |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                        | 输入、项数或 schema 超过运营方的限制。                                                                                                                                 |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                        | 你的租户已达到请求速率或并发限制。请稍后重试。请求速率错误包含 `Retry-After` 标头。                                                                                    |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` 或 `failed_precondition` | 模型不被允许（`invalid_argument`），或未登记输入和输出价格（`failed_precondition`）。系统不会替换为其他模型。                                                          |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                          | schema 使用了支持子集之外的关键字。                                                                                                                                    |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` 或 `internal`                 | AI 提供商在允许的重试后仍持续失败（`unavailable`），或拒绝了请求（`internal`）。                                                                                       |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                  | 模型输出格式错误、被截断或不符合约定。                                                                                                                                 |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                       | 文本不包含必需的信息。                                                                                                                                                 |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                         | 已超过运营方设置的执行超时时间。该时间涵盖检查和所有尝试。                                                                                                             |
| `ERROR_REASON_CANCELED`               | `canceled`                                  | 请求已取消或连接已关闭。                                                                                                                                               |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                               | 无法完成授权、价格或计费检查，或者 DeliTools 无法记录该调用的用量。付费使用会采用失败即拒绝策略，你将不会收到结果。                                                    |

超过运营方总体请求大小限制的请求，可能会收到不含这些详情的普通 `resource_exhausted` 错误。有关处理方法，请参阅[故障排查](/zh-Hans/services/delitools/troubleshooting)。

## 限制与重试

以下限制由运营方配置，且可能发生变化：输入大小、翻译项数、schema 大小和深度、执行超时时间、每个租户的请求速率及突发请求量、每个租户的并发执行数，以及暂时性故障的最大重试次数。请联系服务运营方，了解适用于你的具体数值。限制是一种保护措施，而非吞吐量保证。

- DeliTools 仅针对暂时性的 AI 提供商故障进行重试，例如网络错误、服务器错误和速率限制响应，重试次数不超过配置值。它绝不会通过重试来修复无效输出。
- 每次重试前，DeliTools 都会重新检查你的授权。如果你的密钥已撤销或过期、你已失去权限或计费被阻止，DeliTools 会停止并返回相应错误。已经运行的提供商调用可以完成。
- 如果你取消请求或关闭连接，DeliTools 会取消提供商调用并停止重试。
- 如果你的客户端关闭调用，或因自身超时而结束调用，DeliTools 会将其视为取消。
- 运营方设置的执行超时时间涵盖授权、验证和每次尝试。超时后，DeliTools 会中止提供商调用并返回 `ERROR_REASON_TIMEOUT`。
