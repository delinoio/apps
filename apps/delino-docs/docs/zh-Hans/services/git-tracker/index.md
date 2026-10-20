# Git Tracker

在 [git-tracker.delino.io](https://git-tracker.delino.io) 跟踪 Git 提交的数值测量结果。Git Tracker 免费使用。它支持公开和私有 GitHub 仓库，包括来自复刻仓库的拉取请求。可将其用于文件大小、耗时、吞吐量、计数及其他标量测量。

## 连接仓库

1. 使用你的 Delino 账户登录，然后选择你的个人空间或组织空间。
2. 请空间所有者通过空间的群组授予你 Git Tracker 查看、编辑或管理权限。查看权限允许读取。编辑权限还允许上传和完成执行。管理权限还允许连接仓库、管理令牌和永久删除数据。
3. 为该仓库安装 Git Tracker GitHub App。
4. 打开**管理仓库**。输入 `owner/repository`，并提供与你已关联的 GitHub 账户对应、具有仓库管理员访问权限的 GitHub 访问令牌。该服务会验证你的 GitHub 权限，但不会保留此访问令牌。
5. 连接仓库。你还需要拥有 Delino 空间的所有者/管理员权限。同一仓库一次只能连接到一个空间。从仓库管理页面复制其连接 ID，以用于 Action 和 API 调用。断开连接后，另一个获得授权的空间可以使用新的连接 ID 连接该仓库；原空间保留的历史记录仍为私有。

无需登录即可读取公开仓库的测量结果。私有测量结果要求用户是空间成员，并拥有 Git Tracker 读取权限。空间访问权限适用于该空间中所有已连接的仓库。

断开连接或移除 App 会停止上传，并使保留的历史记录变为私有。获得授权的成员仍可读取这些记录。经确认从公开变为私有后，历史数据也会变为私有。如果 GitHub 无法确认当前可见性，Git Tracker 会继续使用上次确认的可见性，直至同步成功。

## 定义数据集

在仓库中保存一份 YAML 配置。ID 是稳定的；更改名称不会丢失历史记录。同一行中的所有列必须一起上传。

```yaml
id: file-size
name: JavaScript file sizes
columns:
  - id: original
    name: Original size
    unit: B
    direction: lower
    absoluteThreshold: 1024
  - id: minified
    name: Minified size
    unit: B
    direction: lower
    percentageThreshold: 1
  - id: gzip
    name: Gzipped size
    unit: B
    direction: lower
    absoluteThreshold: 128
    percentageThreshold: 2
```

上传 JSON 行，例如 SWC 风格的文件大小表：

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

Action 会保留 JSON 小数的原始拼写，不进行二进制浮点数转换。Direct Connect 调用方应将十进制值作为字符串发送，例如 `"9007199254740993.0000000000000001"`。支持负数和有限小数。显示时的舍入绝不会改变回归判定。

支持的单位包括 `B`、`kB`、`MB`、`GB`、`KiB`、`MiB`、`GiB`；`ns`、`μs`、`ms`、`s`；`count`、`%`、`unitless`；以及 `ops/s`、`B/s`、`kB/s`、`MB/s`、`GB/s`、`KiB/s`、`MiB/s`、`GiB/s`。比较前会转换兼容的单位。不兼容的类型无法比较。例如，1048576 B 会显示为 1 MiB；表格中仍可查看精确值。

方向包括 `lower`、`higher` 和 `neutral`。中性列绝不会产生回归。阈值适用于该列中的所有行。如果同时设置绝对阈值和百分比阈值，超过其中任意一个即为回归；等于阈值不算回归。未设置阈值时，系统会显示变化，但不会导致回归失败。只有向恶化方向的变化才会被判定。

百分比变化为 `(new - baseline) / abs(baseline) * 100`。基线为零时没有百分比变化。绝对阈值仍可用于判定；仅设置百分比的规则会推迟判定。比较使用目标执行中存储的配置，而非之后的配置。

## 使用官方 GitHub Action

将 `delinoio/apps/actions/git-tracker` 固定到经过审查的完整提交 SHA。将下方占位符替换为你的 Git Tracker 连接 ID、检出版本和 Action 版本。为 OIDC 授予 `id-token: write`。也可以使用仓库上传令牌；将其存储为 GitHub 机密并通过 `token` 传入。

```yaml
permissions:
  contents: read
  id-token: write
jobs:
  measure:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<reviewed-checkout-commit-sha>
      - run: ./measure.sh
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          repository-id: '019b0000-0000-7000-8000-000000000001'
          configuration: measurements.yaml
          measurements: measurements.json
          fail-on-regression: 'false'
  mark-as-done:
    needs: measure
    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          operation: complete
          repository-id: '019b0000-0000-7000-8000-000000000001'
```

相互独立的作业共享默认执行 ID，即 `repository_id/run_id/run_attempt`，并且可以上传互不重叠的行。重新运行时请使用新的执行 ID。Actions OIDC 要求使用默认身份；自定义执行 ID 仅适用于使用仓库令牌的调用方。来自复刻仓库的 OIDC 上传仅限于 PR 头部，且不能提供基线覆盖，也不能写入上游推送历史记录。

每个上传作业只判定其自身上传的行。默认情况下，系统会报告回归，但不会使 CI 失败。设置 `fail-on-regression: 'true'` 可在出现回归时使该作业失败。缺少比较不会使 CI 失败。完成作业仅表示测量已结束；它不会提交 CI 成功、失败或取消状态，也不会执行聚合门禁。如果所有作业都被跳过，则空完成操作也是有效的。

网络错误和暂时性 API 故障最多重试三次。默认情况下，重试后仍失败会使作业失败。设置 `warn-on-api-error: 'true'` 可改为发出警告并让作业通过。PR 评论失败只会产生警告；它不会使已保存的数据失效，也不会单独导致 CI 失败。Action 会记录详情链接并写入作业摘要。

一条 PR 评论会反映该 PR 最新提交上最近注册执行的累计结果。较早的运行仍可在 Web 历史记录中查看。

## 调用 Connect API

API 源地址为 `https://git-tracker.api.delino.io`。Connect JSON 请求使用 `Content-Type: application/json` 和 `Connect-Protocol-Version: 1`。管理员可以签发必须具有未来到期时间的仓库令牌，并可在仓库管理页面撤销这些令牌。

```sh
curl --fail-with-body \
  'https://git-tracker.api.delino.io/gittracker.v1.GitTrackerService/Upload' \
  -H "Authorization: Bearer ${GIT_TRACKER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -H 'Connect-Protocol-Version: 1' \
  --data-binary @upload.json
```

`upload.json` 包含 `context`、`dataset` 和 `rows`：

```json
{
  "context": {
    "repositoryId": "019b0000-0000-7000-8000-000000000001",
    "executionId": "my-historical-run",
    "commit": "0123456789012345678901234567890123456789",
    "branch": "main"
  },
  "dataset": {
    "id": "size",
    "name": "Size",
    "columns": [{ "id": "bytes", "name": "Bytes", "unit": "B", "direction": "DIRECTION_LOWER", "absoluteThreshold": "1024" }]
  },
  "rows": [{ "id": "bundle", "name": "Bundle", "values": { "bytes": "1048576" } }]
}
```

使用相同的 `context` 调用 `/gittracker.v1.GitTrackerService/Complete`。对于 PR 上传，请包含 `pullRequest`，并使用 PR 的基础分支和头部提交。`baselineCommit` 用于选择明确的完整基线 SHA。历史提交使用相同的上传 API。发送十进制字符串前，不要通过 JavaScript `Number` 对其进行转换。

## 执行和比较规则

首次上传会冻结该次执行中每个数据集的配置。之后上传不同配置会被拒绝。使用完全相同的数值重复上传已有行是幂等操作。存在冲突的值会被拒绝。批次具有原子性：如果某一行无效或发生冲突，则该请求中的所有行都不会被保存。

数据会立即显示为**测量中**。只有完成执行后，它才会成为代表性结果。某个提交的代表性结果是其最近注册且已完成的执行，包括出现回归的执行。较早的运行即使较晚完成，也不会取代较新注册且已完成的运行。重复执行完成操作既不会更改完成时间，也不会更改顺序。

如果连续六小时没有新保存的测量数据，未完成的执行会变为**已中断**。重复传输相同数据不会重置该计时器。已完成和已中断的执行会拒绝继续上传；已中断的执行还会拒绝完成操作。请改为开始新的执行。数据会一直保留，直至你将其删除。

自动 PR 比较会在首次上传时冻结合并基点提交及其代表性的已完成执行。缺失的基线会一直不可用，即使稍后上传也不例外。自动推送比较会针对每一行分别沿第一父提交祖先链回溯，并使用在首次上传截止时间之前完成的最近已测量祖先。后续作业使用相同的截止时间。表格会标识每个实际的源提交和执行。使用明确基线 SHA 的比较不会采用行级祖先回退。

网站的双提交比较仅使用所选提交各自代表性的已完成执行。新增的行/列以及缺失或不兼容的数据无法比较，也不会导致回归失败。已删除的基线数据不会重建。完成执行前，缺少的行属于未采集；完成执行后则属于缺失。

图表使用按第一父提交顺序等距排列的提交。它们会跨过已跳过的提交连接实际测量点，绝不会沿用或虚构数值。可按数据集、分支、列、项目、日期和提交范围进行筛选。使用数据表查看精确值和可通过键盘访问的执行链接。

## 删除和故障排除

管理员可以在确认后永久删除仓库的测量结果、某个数据集或某次执行。如果某项比较的基线已被删除，该比较将变为不可用。当 Delcore 最终完成空间删除时，Git Tracker 会删除该空间的连接、令牌、配置和历史记录。

遇到权限错误时，请验证所选空间和你当前的 Git Tracker 角色。遇到上传冲突时，请确认执行身份、数据集配置和完整的行值。对于已中断或已完成的运行，请使用新的执行 ID。遇到网络/服务器错误时，请重试，并在寻求帮助时保留响应关联 ID。切勿在公开报告中分享令牌、凭据或私有测量结果。

PR 执行的测量结果仍可供检查，但不会为上游分支图提供数据点或自动基线。这可以防止复刻仓库的测量结果替换共享提交 SHA 对应的基线分支数据或配置。
