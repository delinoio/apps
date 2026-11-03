# Git Tracker

[git-tracker.delino.io](https://git-tracker.delino.io) で Git コミットの数値測定値を追跡できます。Git Tracker は無料です。フォークからのプルリクエストを含め、公開および非公開の GitHub リポジトリに対応しています。ファイルサイズ、処理時間、スループット、件数などのスカラー測定値にご利用ください。

## リポジトリを接続する

1. Delino アカウントでサインインし、個人または組織のスペースを選択します。
2. スペースのオーナーに、スペースのグループを通じて Git Tracker の View、Edit、Manage のいずれかの権限を付与してもらいます。View では読み取りが可能です。Edit ではさらにアップロードと完了処理が可能になります。Manage ではさらに接続、トークン管理、完全削除が可能になります。
3. リポジトリに Git Tracker GitHub App をインストールします。
4. **リポジトリを管理**を開きます。`owner/repository` と、リポジトリの管理者アクセス権を持つ、連携済み GitHub アカウントの GitHub アクセストークンを入力します。サービスは GitHub での権限を確認しますが、そのアクセストークンは保持しません。
5. リポジトリを接続します。Delino スペースの Owner/Admin 権限も必要です。リポジトリを同時に接続できるスペースは 1 つだけです。Action や API の呼び出しに使う接続 ID を、リポジトリ管理からコピーします。接続を解除すると、別の権限を持つスペースが新しい接続 ID で接続できます。元のスペースに保持された履歴は非公開のままです。

公開リポジトリの測定値は、サインインせずに読み取れます。非公開の測定値を読み取るには、スペースのメンバーであり、Git Tracker の読み取り権限を持つ必要があります。スペースのアクセス権は、そのスペースに接続されているすべてのリポジトリに適用されます。

接続の解除または App の削除により、アップロードが停止し、保持された履歴は非公開になります。権限のあるメンバーは引き続き読み取れます。公開から非公開への変更が確認されると、過去のデータは非公開になります。GitHub で現在の公開状態を確認できない場合、Git Tracker は同期が成功するまで最後に確認できた公開状態を維持します。

## データセットを定義する

リポジトリ内に YAML 設定を保存します。ID は固定で、名前は履歴を失わずに変更できます。行内のすべての列は、まとめてアップロードする必要があります。

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

たとえば、SWC 形式のファイルサイズ表など、JSON の行をアップロードします。

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

Action は、二進浮動小数点数への変換を行わず、JSON の十進数表記を保持します。Connect を直接呼び出す場合は、`"9007199254740993.0000000000000001"` のように十進数値を文字列として送信します。負数と有限小数に対応しています。表示上の丸め処理がリグレッション判定を変えることはありません。

対応する単位は、`B`、`kB`、`MB`、`GB`、`KiB`、`MiB`、`GiB`、`ns`、`μs`、`ms`、`s`、`count`、`%`、`unitless`、および `ops/s`、`B/s`、`kB/s`、`MB/s`、`GB/s`、`KiB/s`、`MiB/s`、`GiB/s` です。互換性のある単位は比較前に変換されます。互換性のない種類同士は比較できません。たとえば、1048576 B は 1 MiB と表示されますが、表では正確な値を確認できます。

方向は `lower`、`higher`、`neutral` です。neutral の列ではリグレッションは発生しません。しきい値は列内のすべての行に適用されます。絶対値と割合の両方のしきい値がある場合、いずれかを超えるとリグレッションとなり、等しい場合は該当しません。しきい値がない場合、変化は表示されますが、リグレッションによる失敗は発生しません。悪化する方向の変化のみが判定対象になります。

変化率は `(new - baseline) / abs(baseline) * 100` です。ベースラインがゼロの場合、変化率はありません。その場合でも絶対値のしきい値で判定できますが、割合のみのルールでは判定を保留します。比較には、後から変更された設定ではなく、対象の実行に保存された設定が使用されます。

## 公式 GitHub Action を使用する

`delinoio/apps/actions/git-tracker` を、レビュー済みの完全なコミット SHA に固定します。以下のプレースホルダーを、Git Tracker の接続 ID、チェックアウトのリビジョン、Action のリビジョンに置き換えます。OIDC 用に `id-token: write` を付与します。代わりにリポジトリのアップロードトークンも使用できます。その場合は GitHub シークレットとして保存し、`token` に渡します。

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

独立したジョブは、デフォルトの実行 ID `repository_id/run_id/run_attempt` を共有し、重複しない行をアップロードできます。再実行には新しい実行 ID を使用します。Actions OIDC ではデフォルトの識別情報が必要です。カスタム実行 ID は、リポジトリトークンを使用する呼び出し元向けです。同一リポジトリの PR ジョブでは `baseline` を設定できます。フォークからの OIDC アップロードは PR の head に限定され、ベースラインの上書き指定や、アップストリームの push 履歴への書き込みはできません。

各アップロードジョブは、自身がアップロードした行のみを判定します。デフォルトでは、リグレッションは報告されますが CI は失敗しません。リグレッション発生時にそのジョブを失敗させるには、`fail-on-regression: 'true'` を設定します。比較できない場合も CI は失敗しません。完了ジョブは、測定が終了したことを示すだけです。CI の成功・失敗・キャンセル状態の送信や、集約結果に基づく合否判定は行いません。すべてのジョブがスキップされた場合は、空の完了処理も有効です。

ネットワークエラーや一時的な API 障害は、最大 3 回再試行されます。デフォルトでは、再試行後も解消しないとジョブは失敗します。代わりに警告を出して成功させるには、`warn-on-api-error: 'true'` を設定します。PR コメントの失敗は警告のみとなり、保存済みデータが無効になったり、それ自体で CI が失敗したりすることはありません。Action は詳細リンクをログに記録し、Job Summary を出力します。

1 つの PR コメントに、PR の最新コミットに対して最後に登録された実行の累積結果が反映されます。古い実行も Web の履歴から引き続き参照できます。

## Connect API を呼び出す

API のオリジンは `https://git-tracker.api.delino.io` です。Connect の JSON リクエストでは、`Content-Type: application/json` と `Connect-Protocol-Version: 1` を使用します。管理者は、将来の有効期限を必須としてリポジトリトークンを発行し、リポジトリ管理から失効させることができます。

```sh
curl --fail-with-body \
  'https://git-tracker.api.delino.io/gittracker.v1.GitTrackerService/Upload' \
  -H "Authorization: Bearer ${GIT_TRACKER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -H 'Connect-Protocol-Version: 1' \
  --data-binary @upload.json
```

`upload.json` には `context`、`dataset`、`rows` を含めます。

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

同じ `context` で `/gittracker.v1.GitTrackerService/Complete` を呼び出します。PR のアップロードでは `pullRequest` を含め、PR のベースブランチと head コミットを使用します。`baselineCommit` では、ベースラインの完全な SHA を明示的に指定します。過去のコミットにも同じアップロード API を使用します。十進数文字列は、送信前に JavaScript の `Number` を経由して変換しないでください。

## 実行と比較のルール

最初のアップロードで、その実行における各データセットの設定が固定されます。それ以降、異なる設定は拒否されます。既存の行を同一の数値で繰り返し送信する操作は冪等です。競合する値は拒否されます。バッチはアトミックです。無効な行や競合する行がある場合、そのリクエストのデータは一切保存されません。

データはすぐに**測定中**として表示されます。代表結果になるのは完了後のみです。コミットの代表結果は、リグレッションのある実行も含め、完了済みの実行のうち最後に登録されたものです。古い実行が遅れて完了しても、それより後に登録された完了済みの実行を置き換えることはありません。完了処理を繰り返しても、完了時刻や順序は変わりません。

測定データが新たに保存されないまま 6 時間経過すると、未完了の実行は**中断**になります。同一データの再送信では、このタイマーはリセットされません。完了または中断した実行では、それ以降のアップロードは拒否されます。中断した実行では、完了処理も拒否されます。代わりに新しい実行を開始してください。データは削除するまで保持されます。

PR の自動比較では、最初のアップロード時に、merge-base コミットと、その代表となる完了済みの実行が固定されます。ベースラインが存在しない場合、後からアップロードされても利用不可のままです。push の自動比較では、各行について個別に第 1 親の祖先をたどり、最初のアップロード時点までに完了している、測定済みの最も近い祖先を使用します。後続のジョブも同じ基準時点を使用します。表には、実際の参照元となった各コミットと実行が表示されます。ベースライン SHA を明示的に指定した比較では、行単位で祖先にフォールバックしません。

Web サイトの 2 コミット比較では、選択したコミットの代表となる完了済みの実行のみを使用します。新しい行や列、欠落したデータ、互換性のないデータは比較できず、リグレッションによる失敗の原因にはなりません。削除されたベースラインデータは復元されません。存在しない行は、完了前には未収集、完了後には欠落として扱われます。

グラフでは、第 1 親の順序でコミットを等間隔に配置します。スキップされたコミットをまたいで実際の測定点を結び、値の引き継ぎや架空の値の生成は行いません。データセット、ブランチ、列、項目、日付、コミット範囲で絞り込めます。正確な値や、キーボードでアクセスできる実行リンクについては、データ表を使用してください。

## 削除とトラブルシューティング

管理者は、確認後にリポジトリの測定値、データセット、または 1 つの実行を完全に削除できます。ベースラインが削除された比較は利用できなくなります。Delcore がスペースの削除を確定すると、Git Tracker はそのスペースの接続、トークン、設定、履歴を削除します。

権限エラーが発生した場合は、選択中のスペースと現在の Git Tracker ロールを確認してください。アップロードの競合が発生した場合は、実行の識別情報、データセット設定、行の値がすべて揃っていることを確認してください。中断または完了した実行には、新しい実行 ID を使用してください。ネットワークやサーバーのエラーが発生した場合は再試行し、サポートを求める際に備えてレスポンスの相関 ID を保管してください。公開の報告では、トークン、認証情報、非公開の測定値を決して共有しないでください。

PR 実行の測定値は引き続き確認できますが、アップストリームブランチのグラフの点や、自動ベースラインには使用されません。これにより、共通のコミット SHA に対して、フォークの測定値がベースラインブランチのデータや設定を置き換えることを防ぎます。
