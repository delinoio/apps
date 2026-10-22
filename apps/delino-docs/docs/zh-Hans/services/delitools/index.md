# DeliTools

DeliTools 通过 Connect API 为开发者提供 AI 工具。第 1 版有两个同步方法：对文本数组进行**翻译**，以及从文本中进行**数据提取**，生成遵循你的架构的 JSON。每次调用都会在同一个响应中返回完整结果或错误。无需轮询任务，也无需稍后获取结果。所有方法都位于 `delitools.v1.DeliToolsService` 上。

请从服务器端代码或命令行调用 DeliTools，可以使用任意 Connect 客户端，也可以通过 HTTP 使用纯 JSON。不要将 API 密钥放入前端代码中。将 `DELITOOLS_URL` 设置为 DeliTools API 服务器的基础 URL。

## 快速开始

1. **选择凭据。** 使用你的 Delino 登录令牌，或使用 `CreateKey` 创建 API 密钥。请参阅“身份验证和 API 密钥”一节。
2. **查找你的租户 ID。** 用量会计入你在 `tenantId` 中指定的 Delino 租户。
3. **调用方法**，并附带 `Authorization: Bearer <credential>`。

```sh
# Translate two texts into Korean. Detect the input language.
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/TranslateTextArray" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","outputLanguage":"ko","model":"<allowed-model>","texts":["Good morning","Where is the station?"]}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000001",
  "texts": ["좋은 아침이에요", "역이 어디예요?"]
}
```

在 JSON 中，字段名称使用 lowerCamelCase，枚举值使用其完整名称。

## 翻译文本

`TranslateTextArray` 会翻译数组中的每个字符串。

| 字段             | 必填 | 含义                                                                 |
| ---------------- | ---- | -------------------------------------------------------------------- |
| `tenantId`       | 是   | 接收用量费用的租户。请使用其规范形式的 UUID 版本 7：小写并带连字符。 |
| `outputLanguage` | 是   | 译文的 BCP 47 语言标签，例如 `ko`、`pt-BR` 或 `zh-Hans`。            |
| `inputLanguage`  | 否   | 源文本的 BCP 47 语言标签。省略它则会为整个数组检测一种语言。         |
| `model`          | 是   | 来自操作员允许列表的模型。                                           |
| `texts`          | 是   | 要翻译的字符串。                                                     |
| `idempotencyKey` | 否   | 请参阅幂等性。                                                       |

响应包含 `requestId` 和 `texts`，后者是译文，顺序和长度均与原文一致。

规则：

- 语言值为 BCP 47 标签。`outputLanguage` 为必填。不是有效 BCP 47 的标签会返回输入无效错误。
- `texts` 不能为空，并且任何字符串都不能为空。
- 如果你设置了 `inputLanguage` 并且它与 `outputLanguage` 相同，则请求无效。
- 如果你省略 `inputLanguage`，DeliTools 会为整个数组检测一种输入语言。混合语言的数组仍会被翻译。如果检测到的语言与 `outputLanguage` 相同，模型仍会处理每一项。DeliTools 不会原样返回未经处理的原始数组。
- 仅接受操作员允许列表中的模型。其他模型会返回模型无效错误。存在于 Vercel AI Gateway 目录中的模型不会自动被允许。
- 只有每一项都成功，调用才算成功。有一项失败，整个请求就会失败。DeliTools 绝不会返回部分译文，也绝不会用源文本替代失败的项。

## 提取数据

`ExtractDataFromText` 读取文本并返回遵循你的 JSON 架构的 JSON。该服务使用固定的模型 `gpt-6-luna`。你无法选择模型。

| 字段             | 必填 | 含义                                                                 |
| ---------------- | ---- | -------------------------------------------------------------------- |
| `tenantId`       | 是   | 接收用量费用的租户。请使用其规范形式的 UUID 版本 7：小写并带连字符。 |
| `text`           | 是   | 源文本。                                                             |
| `jsonSchema`     | 是   | 以字符串表示、使用受支持子集的 JSON 架构。                           |
| `idempotencyKey` | 否   | 请参阅幂等性。                                                       |

响应包含 `requestId` 和 `jsonData`，后者是 DeliTools 已根据你的架构验证过的 JSON 字符串。

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/ExtractDataFromText" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","text":"Ada Lovelace lives in London.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"},\"city\":{\"type\":\"string\"}},\"required\":[\"name\",\"city\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000002",
  "jsonData": "{\"name\":\"Ada Lovelace\",\"city\":\"London\"}"
}
```

提取的工作方式：

- 模型会被告知仅使用文本中明确陈述的信息。如果文本缺少架构所要求的信息，调用会失败并返回无法提取错误。该服务不会为了满足架构而编造值。
- DeliTools 会在服务器上根据你的架构检查模型输出。格式错误或不符合架构的输出属于失败，并返回输出无效错误。它绝不会作为结果返回，DeliTools 也不会重试或修复它。
- 架构检查和提示词并不能保证每个值在语义上都正确。在依赖提取的数据之前，请先进行审核。DeliTools 不会返回每个值的依据。
- DeliTools 会在调用任何 AI 模型之前检查架构。超出受支持子集的架构会返回架构无效错误，不会调用 AI，也不会收费。

### 架构子集

DeliTools 仅接受提取模型的结构化输出所支持的那部分 JSON Schema。

支持：

- 根部为对象。
- 类型 `object`、`array`、`string`、`number`、`integer`、`boolean` 和 `null`，嵌套深度最多为 8 层。
- `properties`，且每个属性都列在 `required` 中。
- 每个对象上的 `additionalProperties` 都设置为 `false`。
- 数组使用 `items`。
- 任何架构上的 `description`。
- 带标量值的 `enum`。

不支持，并会在执行前被拒绝：

- 上述列表之外的任何关键字，例如 `pattern`、`minimum`、`format` 或 `oneOf`。
- 可选属性。对象的每个属性都必须在 `required` 中。
- `additionalProperties` 设置为 `true` 或缺失。
- 引用：`$ref` 和递归架构。
- 超出提供商限制的架构：5,000 个属性、1,000 个枚举值，或属性名称与枚举值合计 120,000 个字符。包含超过 250 个值的枚举，其字符串值最多只能有 15,000 个字符。

受支持的架构：

```json
{
  "type": "object",
  "properties": {
    "invoiceNumber": { "type": "string", "description": "The invoice number" },
    "total": { "type": "number" },
    "status": { "type": "string", "enum": ["paid", "unpaid"] },
    "lines": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": { "item": { "type": "string" }, "quantity": { "type": "integer" } },
        "required": ["item", "quantity"],
        "additionalProperties": false
      }
    }
  },
  "required": ["invoiceNumber", "total", "status", "lines"],
  "additionalProperties": false
}
```

不受支持的架构，因为 `note` 是可选的，并且不支持 `pattern`：

```json
{
  "type": "object",
  "properties": {
    "code": { "type": "string", "pattern": "^[A-Z]{3}$" },
    "note": { "type": "string" }
  },
  "required": ["code"],
  "additionalProperties": false
}
```

## 请求行为

- 两个方法都是同步的。请保持连接打开，直到收到结果或错误。
- 每个成功响应和每个错误都包含一个请求 ID，即 UUID 版本 7。寻求支持时请提供该 ID。成功响应仅包含结果和请求 ID，不包含模型、令牌数量或费用。要查看用量，请参阅计费。
- DeliTools 会重试暂时性的 AI 提供商错误，重试上限由操作员设置。它不会重试格式错误的输出，也绝不会自行切换到其他模型。
- 如果你取消调用或断开连接，DeliTools 会尝试取消 AI 调用并停止后续重试。已经发生的用量仍会被计量并计费。
- 操作员设置输入大小、翻译项数量、执行超时、每个租户的请求速率以及每个租户的并发请求数的限制。这些是部署设置，并非固定的产品限制。超出限制的请求会返回限制错误。

## 身份验证和 API 密钥

每个请求都要发送 `Authorization: Bearer <credential>`。两种凭据类型均可用于这两个工具：

- **Delino 登录令牌**（Logto 用户访问令牌）以已登录用户的身份行事。
- **DeliTools API 密钥**按其密钥类型所述的方式行事。

每个请求还会根据你在 Delcore 中的租户进行检查。租户必须处于活动状态并被允许运行付费工作，且计费必须允许这样做。如果 DeliTools 无法完成这些检查，它会拒绝新的付费执行。

### 密钥类型和能力

- **个人密钥**属于一个用户。其权限是其能力与该用户当前权限的交集。当用户失去对租户的访问权限时，该密钥将停止工作。
- **服务密钥**属于租户。它不依赖于创建它的用户的成员身份。只有租户的所有者或管理员才能创建和管理服务密钥。
- 每个密钥具有其中一项或两项能力：`CAPABILITY_TRANSLATOR` 和 `CAPABILITY_EXTRACTOR`。密钥只能调用与其能力对应的工具。
- 用户管理自己的个人密钥。密钥管理需要 Delino 登录令牌。API 密钥不能创建或撤销密钥。

### 管理密钥

- `CreateKey`：发送 `tenantId`、`kind`（`KEY_KIND_PERSONAL` 或 `KEY_KIND_SERVICE`）、`capabilities` 和可选的 `expiresAt`。
- `ListKeys`：列出租户的密钥，可按 `kind` 筛选，并使用 `afterId` 和 `pageSize` 分页。
- `RevokeKey`：发送 `tenantId` 和 `keyId`。

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/CreateKey" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"tenantId":"<tenant-id>","kind":"KEY_KIND_SERVICE","capabilities":["CAPABILITY_TRANSLATOR"]}'
```

- **一次性密钥。** `CreateKey` 的响应仅会包含一次 `secret`。DeliTools 仅存储验证哈希值，无法再次显示该密钥。如果你丢失了密钥，请创建一个新密钥。请将密钥保存在密钥管理器中。
- **过期。** `expiresAt` 是可选的。没有过期时间的密钥在你撤销它之前一直有效。个人密钥还要求其所有者保留对租户的访问权限。服务密钥不依赖创建它的用户，因此移除该用户并不会使其失效：请撤销服务密钥或为其设置过期时间。
- **撤销。** 在你撤销密钥后，或在个人密钥或登录令牌的用户失去权限后，新请求会被拒绝，正在运行的调用也不会再进行 AI 重试。已在运行的调用可以完成。其用量会被计费。

## 计费

DeliTools 向 Delcore 报告用量。Delcore 以美元对其进行计价和结算。

- **基准。** 基准是 Vercel AI Gateway 模型目录中该模型的公开令牌价格，乘以该调用实际计量到的令牌数。Gateway 折扣和促销额度不会降低基准。
- **加价。** 翻译费用为基准乘以 1.15。提取费用为基准乘以 1.30。金额使用精确小数和 Delcore 的舍入规则。
- **价格变动。** DeliTools 会自动更新目录价格。新价格对价格生效之后开始的请求适用于所有客户。一个请求的所有重试都沿用其开始时的价格。如果 DeliTools 没有某个模型的价格，它会拒绝新的 AI 执行。
- **失败也会计费。** 重试、失败的调用、超时、取消以及你未能收到的结果所使用的令牌仍然计入，并加上加价。
- **未知用量。** 如果 DeliTools 无法立即读取某次调用的用量，它会推迟收费，并稍后与 Gateway 对账。它绝不会估算用量来收费。如果记录在 30 天后过期时用量仍然未知，则该笔费用将被放弃。
- **在哪里查看用量。** 响应不包含用量。请使用 Delino 计费界面查看你的租户的用量和费用。
- **被阻止的租户。** 如果计费阻止了你的租户，新的执行会失败并返回计费被阻止错误。已在运行的调用不会被中断。由于 DeliTools 在执行之后才计量用量，因此并发调用可能会在阻止生效之前超出余额或限额。

## 幂等性

`idempotencyKey` 是可选的。如果没有它，每次调用都是一次新的执行，并可能产生新的费用，包括在网络故障后重试的情况。

使用键时，DeliTools 按**租户、方法和键**跟踪请求。这适用于同一租户的不同凭据。在告知你请求的状态之前，DeliTools 会检查你当前的授权。

| 情况                           | 结果                                                    |
| ------------------------------ | ------------------------------------------------------- |
| 新键                           | 调用正常运行。                                          |
| 相同的键和相同的输入，仍在运行 | 返回处理中错误，并附带第一次调用的请求 ID。它不会等待。 |
| 相同的键和相同的输入，已完成   | 返回已处理错误。结果不会重放。                          |
| 相同的键和不同的输入           | 返回输入冲突错误。                                      |
| 第一次调用以未知状态结束       | 返回结果未知错误。DeliTools 不会自行重复 AI 调用。      |
| 30 天的记录过期后使用相同的键  | 新的调用，正常收费。                                    |

DeliTools 保留键、输入的哈希值和状态。它不保留输入或结果。去重仅在第一次调用后的 30 天内有效。请在收到结果时保存它：使用相同键的重复调用无法再次返回该结果。

## 错误

每个错误都有一个 Connect 代码、一个稳定的原因、一个请求 ID 和一条英文消息。原因和请求 ID 位于 `delitools.v1.ErrorDetail` 错误详情中。请在代码中使用原因，而不是消息。

| 原因                                 | Connect 代码          | 含义                                                     |
| ------------------------------------ | --------------------- | -------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`         | `invalid_argument`    | 请求无效，例如空数组或相同的语言。                       |
| `ERROR_REASON_INVALID_SCHEMA`        | `invalid_argument`    | 架构超出受支持的子集。                                   |
| `ERROR_REASON_INVALID_MODEL`         | `invalid_argument`    | 模型不在允许列表中。                                     |
| `ERROR_REASON_UNAUTHENTICATED`       | `unauthenticated`     | 凭据缺失、无效、已过期或已撤销。                         |
| `ERROR_REASON_FORBIDDEN`             | `permission_denied`   | 该凭据或用户不得在此租户中使用此工具。                   |
| `ERROR_REASON_BILLING_BLOCKED`       | `failed_precondition` | 计费不允许该租户进行新的付费执行。                       |
| `ERROR_REASON_PRICE_UNAVAILABLE`     | `failed_precondition` | 该模型没有可用的公开价格，因此 DeliTools 不会运行它。    |
| `ERROR_REASON_LIMIT`                 | `resource_exhausted`  | 超出了已配置的限制，例如输入大小、项数、速率或并发。     |
| `ERROR_REASON_PROCESSING`            | `aborted`             | 具有相同幂等键的请求仍在运行。                           |
| `ERROR_REASON_ALREADY_PROCESSED`     | `already_exists`      | 具有相同幂等键和输入的请求已经完成。                     |
| `ERROR_REASON_INPUT_CONFLICT`        | `failed_precondition` | 该幂等键已与不同的输入一起使用过。                       |
| `ERROR_REASON_UNKNOWN_OUTCOME`       | `unknown`             | 第一次执行的结果未知。                                   |
| `ERROR_REASON_EXTRACTION_IMPOSSIBLE` | `failed_precondition` | 文本不包含架构所要求的信息。                             |
| `ERROR_REASON_INVALID_OUTPUT`        | `data_loss`           | 模型返回了格式错误的输出，或与架构不匹配的输出。         |
| `ERROR_REASON_PROVIDER`              | `unavailable`         | AI 提供商在允许的重试之后仍然失败。                      |
| `ERROR_REASON_TIMEOUT`               | `deadline_exceeded`   | 已超过执行超时。                                         |
| `ERROR_REASON_CANCELED`              | `canceled`            | 调用已被取消。                                           |
| `ERROR_REASON_INTERNAL`              | `internal`            | 内部故障，例如存储方面的故障。                           |
| `ERROR_REASON_INTERNAL`              | `unavailable`         | 无法完成租户的授权或计费检查，因此新的付费工作会被拒绝。 |

处理方式：

- 对于 `invalid_argument`、`permission_denied`、`unauthenticated` 和 `failed_precondition` 错误，请先修正请求再重试。对于计费被阻止错误，请先解决计费阻止问题。
- 提供商、超时或内部错误可以稍后重试。提供商和超时故障仍可能被计费。当第一次执行可能已经成功时，请使用幂等键以避免第二次执行。 带有 `ERROR_REASON_INTERNAL` 的 `unavailable` 错误表示 DeliTools 无法完成授权或计费检查。请同时根据错误码和原因分支处理，因为 `ERROR_REASON_INTERNAL` 也会与 `internal` 一起出现。
- 结果未知、处理中和已处理错误包含第一次调用的请求 ID。如果结果始终未知，请使用该 ID 联系支持。

## 数据保留

DeliTools 绝不会存储你的输入文本、你的架构或结果。它也会将它们排除在日志、跟踪和审计事件之外。凭据和密钥不会进入遥测数据。

DeliTools 仅存储运行和计费该服务所需的数据：API 密钥的验证哈希值、请求属于哪个用户和哪个租户、用于幂等性的输入哈希值、请求状态、尝试标识符、计量的令牌用量、价格快照，以及安全的审计和运营记录。

| 数据                       | 保留时间                    |
| -------------------------- | --------------------------- |
| 幂等记录                   | 自首次创建起 30 天          |
| 用量交付和对账记录         | 自首次创建起 30 天          |
| 平台审计记录和运营日志     | 自创建起 30 天              |
| API 密钥                   | 不会被 30 天保留清理删除    |
| Delcore 中已接受的会计分录 | 遵循 Delcore 自身的保留规则 |

30 天保留期不适用于 API 密钥。保留清理从不删除它们。过期和撤销只会让密钥无法通过认证，因此其验证哈希和归属信息仍会保存。

这 30 天从记录首次创建时开始计算。重试和状态变更不会延长这一期限。DeliTools 会在记录过期时将其删除，无论其处于何种状态，包括未完成的请求和尚未交付的用量。你需要接受两项风险：

- 幂等记录过期后，使用相同键的重复调用会再次运行并再次收费。
- 记录过期时仍处于待处理状态的用量会丢失，DeliTools 不会对其收费。

## 外部提供商的数据处理

为了完成工作，DeliTools 会将你的文本（提取时还包括你的架构）发送到 Vercel AI Gateway，并由其转发给运行你的请求的模型的提供商。这些服务依据各自的政策处理数据。在向 DeliTools 发送数据之前，请先阅读这些政策：

- [Vercel AI Gateway 零数据保留](https://vercel.com/docs/ai-gateway/security-and-compliance/zdr)
- [Vercel AI Gateway 模型](https://vercel.com/ai-gateway/models)：每个模型页面都会列出其提供商。
- 你所用模型的提供商的数据政策。

DeliTools 不承诺任何提供商实行零数据保留。它不会检测敏感数据，也不会从你的输入中移除敏感数据。请勿提交你无权与这些提供商共享的数据。

## 限制

DeliTools API 服务器的操作员设置以下限制：输入大小、翻译中的项数、执行超时、每个租户每分钟的请求数、每个租户的并发请求数、AI 重试次数以及翻译模型允许列表。这些是部署设置，可能会变化。它们不是固定的产品限制，DeliTools 也不提供吞吐量或可用性保证。超出限制的请求会返回 `ERROR_REASON_LIMIT`，如果是无效模型，则返回 `ERROR_REASON_INVALID_MODEL`。请向你所用部署的操作员询问其当前值。
