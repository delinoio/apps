# DeliTools

DeliTools 为开发者提供两种付费 API：翻译文本列表，以及从文本中提取 JSON 值。你可以在自己的服务端代码中，使用任意 Connect 客户端，或通过 HTTP 发送普通 JSON 来调用这些 API。Delcore 负责管理你的 API 密钥、权限和计费，因此你可以使用同一个 Delino 账户和租户来处理所有事务。

## 概念

- **翻译：**发送文本列表、目标语言和模型。返回的译文顺序和数量与原文一致。
- **提取：**发送一段文本和一个 JSON Schema。你会收到一个符合该模式的 JSON 文档。
- **租户：**Delino 个人空间或组织。Delcore 将用量费用计入该租户。
- **凭据：**Delino 登录令牌、个人 API 密钥或租户服务 API 密钥。请参阅 [API 密钥](/zh-Hans/services/delitools/api-keys)。
- **请求 ID：**每个响应都带有请求 ID。寻求支持时请提供此 ID。

## 登录与权限

在每个请求中发送 `Authorization: Bearer <credential>` 请求头。

| 凭据            | 形式                 | 租户                                      |
| --------------- | -------------------- | ----------------------------------------- |
| Delino 登录令牌 | Delino 访问令牌      | 在请求中设置 `tenantId`。此字段为必填项。 |
| 个人 API 密钥   | 以 `dlk_` 开头的密钥 | 该密钥所属的租户。`tenantId` 为可选项。   |
| 租户服务密钥    | 以 `dlk_` 开头的密钥 | 该密钥所属的租户。`tenantId` 为可选项。   |

如果你在使用 API 密钥时同时设置了 `tenantId`，它必须是该密钥所属的租户，否则请求会失败并返回 `ERROR_REASON_PERMISSION_DENIED`。每个请求都必须在发出时获得 Delcore 的许可：凭据需要具备 `translate` 或 `extract` 能力，租户必须处于活跃状态，并且必须通过常规计费检查。这些 API 是付费的，当你的租户出现余额耗尽等计费问题时，Delcore 会阻止调用这些 API。

## 快速入门

1. **获取凭据。**使用你的 Delino 登录令牌，或按照 [API 密钥](/zh-Hans/services/delitools/api-keys)中的说明创建 API 密钥。
2. **找到你的租户 ID 和模型。**租户 ID 是你的 Delino 账户的一部分。服务运营方会设置可用的翻译模型。
3. **调用 API。**所有过程都位于 `delitools.v1.DelitoolsService`。将 `DELITOOLS_URL` 设置为 DeliTools API 服务器的基础 URL，例如本地服务器可使用 `http://127.0.0.1:46322`。

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

在 JSON 中，字段名使用 lowerCamelCase。有关所有字段、支持的模式子集和错误原因，请参阅 [API 参考](/zh-Hans/services/delitools/api-reference)。

请从服务端代码或命令行调用 DeliTools。不要将 API 密钥放在前端代码中：DeliTools 不支持从浏览器发起调用。

## 你的内容与 AI 提供商

DeliTools 通过 Vercel AI Gateway 执行你的请求。**你提交的文本和模式会发送给 Vercel AI Gateway 以及你所选模型背后的模型提供商。**它们对你内容的处理方式（包括任何保留行为）受其自身的条款和设置约束，而非 DeliTools 的条款和设置。

- DeliTools 不会存储你的文本、模式或结果，也不会将其写入日志。这仅描述 DeliTools 的行为，并不保证 Vercel AI Gateway 或模型提供商会保留哪些内容。
- 不要提交你无权与这些提供商共享的内容。在提交敏感或受监管的数据之前，请查阅它们的条款。
- DeliTools 仅保留向你的租户计费所需的用量记录，并在记录创建 30 天后将其删除。
- 模型输出不保证正确。在依赖结果之前，请先进行检查。

## 更多指南

- [API 参考](/zh-Hans/services/delitools/api-reference)：字段、语言标签、模式子集、错误和限制。
- [API 密钥](/zh-Hans/services/delitools/api-keys)：个人和租户密钥、权限、到期与撤销。
- [计费](/zh-Hans/services/delitools/billing)：如何根据 token 用量计算费用。
- [故障排查](/zh-Hans/services/delitools/troubleshooting)：客户端配置与错误修复。
