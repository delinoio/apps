# Delcore

Delcore 为 Delino 服务提供共享租户、访问控制和计费功能。官方 API 服务器为 `https://delcore.api.delino.io`。本页介绍供服务开发者使用的租户状态检查功能。

## 检查租户状态

已注册的服务可以询问 Delcore 某个租户是处于活跃、已阻止还是已删除状态。当您的服务为租户存储数据，但并非每次请求都有对应的 Delino 用户时，请使用此检查，例如由项目 API 密钥发起调用时。

使用 1 到 100 个租户 ID 调用 `delcore.v1.DelcoreService/GetTenantStates`。每个 ID 都必须是小写的 UUID 版本 7。使用您服务的 Logto 机器到机器应用程序的访问令牌进行身份验证。该服务必须已在 Delcore 中注册，并具有 `authorize` 范围。用户令牌、未注册的服务以及没有此范围的服务无法读取租户状态。

响应会按照您发送的顺序，返回每个请求的租户 ID 及其以下一种状态：

| 状态                    | 含义                                   |
| ----------------------- | -------------------------------------- |
| `TENANT_STATE_ACTIVE`   | 租户存在，且未被阻止或处于删除过程中。 |
| `TENANT_STATE_BLOCKED`  | 平台运营者已阻止该租户。               |
| `TENANT_STATE_DELETING` | 删除操作待完成。                       |
| `TENANT_STATE_DELETED`  | 删除已最终完成。                       |
| `TENANT_STATE_UNKNOWN`  | Delcore 中不存在使用此 ID 的租户。     |

当同时符合多个条件时，`DELETED` 的优先级高于 `DELETING`，`DELETING` 的优先级高于 `BLOCKED`，而 `BLOCKED` 的优先级高于 `ACTIVE`。

该状态与计费无关。订阅过期、续订失败、争议或余额不足都不会使租户变为 `BLOCKED`。要确定用户是否可以运行付费工作，请改用 `Authorize`。此检查只会读取数据。它不会更改租户、授权付费使用或联系支付提供商，也不会返回租户名称、用户详细信息或计费信息。

## 处理结果和故障

- 仅对 `TENANT_STATE_ACTIVE` 允许访问；如果您自己已记录的所有者或管理员规则允许访问已阻止或正在删除的租户，则也可以允许访问。
- 将 `TENANT_STATE_UNKNOWN`、请求错误、超时以及 Delcore 不可用视为不允许访问。在这些情况下，请勿删除数据。
- 仅在明确收到 `TENANT_STATE_DELETED` 后，才自动删除您保留的租户数据。`BLOCKED` 和 `DELETING` 绝不允许自动删除。
- 无效 ID、空请求或超过 100 个 ID 会返回无效参数错误。凭据缺失、过期或无效会返回未通过身份验证错误。无权读取租户状态的调用方会收到权限被拒绝错误。其他故障会返回包含关联 ID 的错误；请求支持时，请附上此 ID。它可以在错误详细信息和 `X-Delcore-Correlation-Id` 标头中找到。
- 稍后重试失败的请求。请求失败绝不意味着租户处于活跃或已删除状态。
