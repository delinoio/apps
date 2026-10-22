# DeliTools API 参考

本页介绍 DeliTools 的两种方法。两者都是 `delitools.v1.DelitoolsService` 上的 Connect 过程，通过 HTTP `POST` 接受 JSON。JSON 中的字段名采用 lowerCamelCase。请发送 `Content-Type: application/json` 和 `Authorization: Bearer <credential>`。有关凭据和租户的信息，请参阅 [DeliTools](/zh-Hans/services/delitools/)。

每个响应都会在响应体和 `X-Delitools-Request-Id` 响应头中包含请求 ID。响应不包含 token 用量或费用。请从 Delcore 中读取这些信息；参阅[计费](/zh-Hans/services/delitools/billing)。

每次调用都是一次新的执行。DeliTools 没有幂等键，也不存储结果。如果重复发送请求，请求会再次执行，并可能再次计费。

## 翻译

`POST /delitools.v1.DelitoolsService/Translate`

| 字段             | 类型       | 规则                                                                      |
| ---------------- | ---------- | ------------------------------------------------------------------------- |
| `tenantId`       | 字符串     | 使用 Delino 登录令牌时必填。使用 API 密钥时，留空或填写该密钥所属的租户。 |
| `texts`          | 字符串数组 | 必填。至少包含一项。不允许空字符串。项数和总大小受运营方限制。            |
| `targetLanguage` | 字符串     | 必填。BCP 47 语言标签，例如 `fr` 或 `pt-BR`。                             |
| `sourceLanguage` | 字符串     | 可选。BCP 47 语言标签。如果省略，模型会检测源语言。                       |
| `model`          | 字符串     | 必填。运营方允许使用的模型标识符。                                        |

响应包含 `requestId` 和 `translations`。`translations` 的项数和顺序与 `texts` 相同。

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

- **语言标签。** DeliTools 会检查标签是否符合 BCP 47 格式，忽略大小写，最多包含 64 个字符，且主语言子标签由两个或三个字母组成。仅由私用子标签组成的标签和祖父标签会被拒绝。DeliTools 不会检查模型是否支持该语言。
- **全有或全无。** 结果具有原子性。如果任何一项失败，整个请求都会失败，且不会返回部分翻译结果。
- **无特殊处理。** 省略源语言、混合语言输入以及已经是目标语言的文本，都会发送给模型。DeliTools 不会拒绝这些输入，也不会直接为你复制源文本。
- **模型。** 只有运营方允许使用且已在 Delcore 中登记价格的模型才能使用。系统绝不会替换为其他模型。请向服务运营方咨询可用的模型标识符。

## 提取

`POST /delitools.v1.DelitoolsService/Extract`

| 字段         | 类型   | 规则                                                                      |
| ------------ | ------ | ------------------------------------------------------------------------- |
| `tenantId`   | 字符串 | 使用 Delino 登录令牌时必填。使用 API 密钥时，留空或填写该密钥所属的租户。 |
| `text`       | 字符串 | 必填且不能为空。大小受运营方限制。                                        |
| `jsonSchema` | 字符串 | 必填。以 JSON 字符串形式提供的 JSON Schema 文档，只能使用下方支持的子集。 |

模型固定为 `openai/gpt-6-luna`，无法自行选择。响应包含 `requestId` 和 `json`，后者是符合你的模式的 JSON 字符串。

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

- **以你的文本为依据。** 模型必须从文本中提取值。如果文本缺少必需的信息，请求会失败并返回 `ERROR_REASON_EXTRACTION_FAILED`。DeliTools 不会编造值。
- **校验输出。** DeliTools 会根据你的模式验证结果。格式错误或不符合模式的输出会导致失败，并返回 `ERROR_REASON_INVALID_OUTPUT`。DeliTools 不会通过重试来修复无效输出。
- **准确性。** 符合模式的结果仍然可能有误。请核查重要的结果。
- **不支持的模式会提前失败。** 在运行模型之前，DeliTools 会拒绝超出所支持子集范围的模式，因此此类请求不会计费。

### 支持的 JSON Schema 子集

模式按 JSON Schema 2020-12 解析，必须是 JSON 对象。`$schema` 为可选项，仅允许出现在根级别，DeliTools 会忽略其值。每个模式，包括根模式、每个属性模式和 `items` 模式，都需要包含 `type`、`enum` 或 `const`。模式深度最多为 8 层，最多包含 256 个模式节点，其大小受运营方限制。

| 关键字                   | 规则                                                                                                                                         |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `type`                   | `object`、`array`、`string`、`number`、`integer`、`boolean`、`null`，或由这些类型组成的数组，例如用 `["string","null"]` 表示可为 null 的值。 |
| `properties`, `required` | `properties` 将名称映射到模式。每个 `required` 名称都必须出现在 `properties` 中。每个对象最多有 64 个属性，且 `required` 名称不能重复。      |
| `additionalProperties`   | 只能为 `true` 或 `false`（不能是模式）。                                                                                                     |
| `items`                  | 所有项共用一个模式。                                                                                                                         |
| `enum`, `const`          | `enum` 包含 1 到 64 个不同的值。`const` 为单个值。                                                                                           |
| `minLength`, `maxLength` | 0 到 1,000,000 之间的整数，用于字符串。最小值不能超过最大值。                                                                                |
| `minItems`, `maxItems`   | 0 到 1,000,000 之间的整数，用于数组。最小值不能超过最大值。                                                                                  |
| `minimum`, `maximum`     | 有限数值，用于 `number` 和 `integer`。最小值不能超过最大值。                                                                                 |
| `title`, `description`   | 提供给模型的说明，不会改变验证规则。                                                                                                         |

以下内容不受支持，会返回 `ERROR_REASON_UNSUPPORTED_SCHEMA`：`$ref`、`$defs`、`definitions`、`$id`、`$comment`、`oneOf`、`anyOf`、`allOf`、`not`、`if`/`then`/`else`、`patternProperties`、`propertyNames`、`pattern`、`format`、`uniqueItems`、`multipleOf`、`exclusiveMinimum`、`exclusiveMaximum`、`contains`、`prefixItems`、`default`、`unevaluated` 关键字、布尔模式、未知关键字，以及过大或嵌套过深的模式。DeliTools 从不解析引用目标，因此模式无法使其获取任何内容。

如果只希望对象包含你列出的属性，请在对象上设置 `"additionalProperties": false`。

## 错误

错误使用标准的 Connect 代码。错误详情包含稳定的原因标识和请求 ID。消息使用英语，且绝不会包含你的内容。

| 原因                                  | Connect 代码                                | 含义                                                                                                                |
| ------------------------------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                          | 字段无效：数组或字符串为空、语言标签错误、租户缺失或格式错误、模型缺失或不被允许，或者模式不是有效的 JSON。         |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                           | 凭据缺失、格式错误、已过期、已撤销或未知。                                                                          |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                         | 凭据缺少所需的能力、密钥授权或你的租户权限，或者你发送的 `tenantId` 不是该密钥所属的租户。                          |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                       | Delcore 阻止了付费使用，例如由于余额、续订、争议、运营方封禁或租户正在被删除。                                      |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                        | 输入、项数或模式超出了运营方的限制。                                                                                |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                        | 你的租户已达到请求速率或并发限制。请稍后重试。请求速率错误包含 `Retry-After` 响应头。                               |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` 或 `failed_precondition` | 模型不被允许（`invalid_argument`），或未登记输入和输出价格（`failed_precondition`）。系统不会替换为其他模型。       |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                          | 模式使用了所支持子集之外的关键字。                                                                                  |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` 或 `internal`                 | 在允许的重试次数用尽后，AI 提供商仍然失败（`unavailable`），或者提供商拒绝了请求（`internal`）。                    |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                  | 模型输出格式错误、被截断或不符合约定。                                                                              |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                       | 文本不包含必需的信息。                                                                                              |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                         | 已超过运营方设置的执行超时时间。该时限涵盖各项检查和所有尝试。                                                      |
| `ERROR_REASON_CANCELED`               | `canceled`                                  | 请求已取消或连接已关闭。                                                                                            |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                               | 无法完成授权、价格或计费检查，或者 DeliTools 无法记录此次调用的用量。付费使用会在此情况下拒绝继续，且不会返回结果。 |

如果请求超出了运营方的整体请求大小限制，可能会收到不带这些详情的普通 `resource_exhausted` 错误。有关处理方法，请参阅[故障排查](/zh-Hans/services/delitools/troubleshooting)。

## 限制与重试

以下限制由运营方配置，且可能发生变化：输入大小、翻译项数、模式大小和深度、执行超时时间、每个租户的请求速率和突发额度、每个租户的并发执行数，以及瞬时故障的最大重试次数。请联系服务运营方，了解适用于你的具体值。限制是一种保护措施，并非吞吐量保证。

- DeliTools 仅对 AI 提供商的瞬时故障进行重试，例如网络错误、服务器错误和速率限制响应，最多重试配置的次数。它绝不会通过重试来修复无效输出。
- 每次重试前，DeliTools 都会重新检查你的授权。如果密钥已被撤销或过期、你失去了权限，或者计费被阻止，DeliTools 会停止并返回相应错误。已经运行中的提供商调用仍可完成。
- 如果你取消请求或关闭连接，DeliTools 会取消提供商调用并停止重试。
- 如果客户端关闭调用，或因其自身超时而结束调用，DeliTools 会将其视为取消。
- 运营方设置的执行超时时间涵盖授权、验证和每次尝试。超时后，DeliTools 会中止提供商调用并返回 `ERROR_REASON_TIMEOUT`。
