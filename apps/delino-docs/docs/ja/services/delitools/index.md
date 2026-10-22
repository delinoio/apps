# DeliTools

DeliTools は開発者向けに、テキストリストの翻訳と、テキストからの JSON 値の抽出という 2 つの有料 API を提供します。どちらも、ご自身のサーバーサイドコードから、任意の Connect クライアント、または HTTP 経由の通常の JSON で呼び出せます。Delcore が API キー、権限、請求を管理するため、すべてに同じ Delino アカウントとテナントを使用できます。

## 概念

- **翻訳：** テキストのリスト、翻訳先の言語、モデルを送信します。同じ順序・件数の翻訳結果を受け取ります。
- **抽出：** テキストと JSON Schema を送信します。そのスキーマに従う 1 つの JSON ドキュメントを受け取ります。
- **テナント：** Delino の個人スペースまたは組織です。Delcore は使用料金をテナントに請求します。
- **認証情報：** Delino ログイントークン、個人 API キー、またはテナントサービス API キーです。[API キー](/ja/services/delitools/api-keys)を参照してください。
- **リクエスト ID：** すべてのレスポンスにリクエスト ID が含まれます。サポートに問い合わせる際は、この ID を添えてください。

## サインインと権限

各リクエストに `Authorization: Bearer <credential>` ヘッダーを付けて送信してください。

| 認証情報                | 形式                        | テナント                                               |
| ----------------------- | --------------------------- | ------------------------------------------------------ |
| Delino ログイントークン | Delino アクセストークン     | リクエストに `tenantId` を設定してください。必須です。 |
| 個人 API キー           | `dlk_` で始まるシークレット | キーに紐づくテナントです。`tenantId` は省略可能です。  |
| テナントサービスキー    | `dlk_` で始まるシークレット | キーに紐づくテナントです。`tenantId` は省略可能です。  |

API キーとともに `tenantId` を設定する場合は、そのキーに紐づくテナントでなければなりません。そうでない場合、リクエストは `ERROR_REASON_PERMISSION_DENIED` で失敗します。すべてのリクエストは、その時点で Delcore によって許可されている必要があります。認証情報には `translate` または `extract` の機能権限が必要であり、テナントが有効で、通常の請求チェックを通過している必要があります。これらの API は有料であり、残高の枯渇など、テナントに請求上の問題がある場合、Delcore は API の利用をブロックします。

## クイックスタート

1. **認証情報を取得します。** Delino ログイントークンを使用するか、[API キー](/ja/services/delitools/api-keys)の説明に従って API キーを作成してください。
2. **テナント ID とモデルを確認します。** テナント ID は Delino アカウントに含まれます。利用可能な翻訳モデルはサービス運営者が設定します。
3. **API を呼び出します。** すべてのプロシージャは `delitools.v1.DelitoolsService` にあります。`DELITOOLS_URL` に DeliTools API サーバーのベース URL を設定してください。たとえば、ローカルサーバーの場合は `http://127.0.0.1:46322` です。

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

JSON のフィールド名には lowerCamelCase を使用します。すべてのフィールド、サポートされるスキーマのサブセット、エラー理由については、[API リファレンス](/ja/services/delitools/api-reference)を参照してください。

DeliTools はサーバーサイドコードまたはコマンドラインから呼び出してください。フロントエンドコードに API キーを含めないでください。DeliTools はブラウザーからの呼び出しをサポートしていません。

## コンテンツと AI プロバイダー

DeliTools は Vercel AI Gateway 上でリクエストを実行します。**送信したテキストとスキーマは、Vercel AI Gateway と、選択したモデルを提供するモデルプロバイダーに送信されます。** コンテンツの保持を含む取り扱いには、DeliTools ではなく、それぞれの規約と設定が適用されます。

- DeliTools は、テキスト、スキーマ、結果を保存せず、ログにも記録しません。これは DeliTools のみについての説明です。Vercel AI Gateway やモデルプロバイダーが何を保持するかを保証するものではありません。
- これらのプロバイダーとの共有が許可されていないコンテンツを送信しないでください。機密データや規制対象のデータを送信する前に、各プロバイダーの規約を確認してください。
- DeliTools は、テナントへの請求に必要な使用記録のみを保持し、作成から 30 日後に削除します。
- モデルの出力が正しいことは保証されません。結果に依拠する前に、その内容を確認してください。

## その他のガイド

- [API リファレンス](/ja/services/delitools/api-reference)：フィールド、言語タグ、スキーマのサブセット、エラー、制限。
- [API キー](/ja/services/delitools/api-keys)：個人キーとテナントキー、権限、有効期限、失効。
- [請求](/ja/services/delitools/billing)：トークン使用量が料金に換算される仕組み。
- [トラブルシューティング](/ja/services/delitools/troubleshooting)：クライアントの設定とエラーの解決。
