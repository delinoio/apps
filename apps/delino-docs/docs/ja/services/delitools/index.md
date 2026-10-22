# DeliTools

DeliTools は、開発者向けに 2 つの有料 API を提供します。テキストのリストの翻訳と、テキストからの JSON 値の抽出です。独自のサーバーサイドコードから、任意の Connect クライアント、または HTTP 経由のプレーンな JSON を使って呼び出せます。Delcore が API キー、権限、請求を管理するため、すべてに同じ Delino アカウントとテナントを使用できます。

## 概念

- **翻訳：**テキストのリスト、翻訳先の言語、モデルを送信します。同じ順序、同じ件数の翻訳が返されます。
- **抽出：**テキストと JSON Schema を送信します。スキーマに従った JSON ドキュメントが 1 つ返されます。
- **テナント：**Delino の個人スペースまたは組織です。Delcore は使用量をテナントに請求します。
- **認証情報：**Delino ログイントークン、個人 API キー、またはテナントサービス API キーです。[API キー](/ja/services/delitools/api-keys)を参照してください。
- **リクエスト ID：**すべてのレスポンスにリクエスト ID が含まれます。サポートに問い合わせる際は、この ID を含めてください。

## サインインと権限

各リクエストで `Authorization: Bearer <credential>` ヘッダーを送信してください。

| 認証情報                | 形式                        | テナント                                         |
| ----------------------- | --------------------------- | ------------------------------------------------ |
| Delino ログイントークン | Delino アクセストークン     | リクエストに `tenantId` を設定します。必須です。 |
| 個人 API キー           | `dlk_` で始まるシークレット | キーのテナントです。`tenantId` は省略可能です。  |
| テナントサービスキー    | `dlk_` で始まるシークレット | キーのテナントです。`tenantId` は省略可能です。  |

API キーとともに `tenantId` を設定する場合、そのキーのテナントでなければなりません。そうでない場合、リクエストは `ERROR_REASON_PERMISSION_DENIED` で失敗します。すべてのリクエストは、その時点で Delcore によって許可される必要があります。認証情報には `translate` または `extract` の機能権限が必要で、テナントが有効であり、通常の請求チェックに合格している必要があります。これらの API は有料であり、残高不足など、テナントに請求上の問題がある場合、Delcore は API の利用をブロックします。

## クイックスタート

1. **認証情報を取得します。** Delino ログイントークンを使用するか、[API キー](/ja/services/delitools/api-keys)の説明に従って API キーを作成してください。
2. **テナント ID とモデルを確認します。** テナント ID は Delino アカウントに含まれています。利用可能な翻訳モデルはサービス運用者が設定します。
3. **API を呼び出します。** すべてのプロシージャは `delitools.v1.DelitoolsService` にあります。`DELITOOLS_URL` に、サービス運用者から提供された DeliTools API サーバーのベース URL を設定してください。

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

この例では、テナント情報を持つ API キーを使用しています。Delino ログイントークンを使用する場合は、JSON 本文に `"tenantId":"<tenant-id>"` を追加してください。

JSON では、フィールド名に lowerCamelCase を使用します。すべてのフィールド、サポートされるスキーマのサブセット、エラー理由については、[API リファレンス](/ja/services/delitools/api-reference)を参照してください。

DeliTools はサーバーサイドコードまたはコマンドラインから呼び出してください。フロントエンドのコードに API キーを含めないでください。DeliTools はブラウザーからの呼び出しをサポートしていません。

## コンテンツと AI プロバイダー

DeliTools は Vercel AI Gateway 上でリクエストを実行します。**送信したテキストとスキーマは、Vercel AI Gateway と、選択したモデルを提供するモデルプロバイダーに送信されます。**保存を含め、これらのサービスによるコンテンツの取り扱いには、DeliTools ではなく、各サービス独自の規約と設定が適用されます。

- DeliTools はテキスト、スキーマ、結果を保存せず、ログにも記録しません。これは DeliTools のみについての説明です。Vercel AI Gateway やモデルプロバイダーが何を保持するかを保証するものではありません。
- これらのプロバイダーとの共有が許可されていないコンテンツは送信しないでください。機密データや規制対象のデータを送信する前に、各プロバイダーの規約を確認してください。
- DeliTools は、テナントへの請求に必要な使用量の記録のみを保持し、作成から 30 日後に削除します。
- モデルの出力が正しいことは保証されません。結果を利用する前に確認してください。

## その他のガイド

- [API リファレンス](/ja/services/delitools/api-reference)：フィールド、言語タグ、スキーマのサブセット、エラー、制限。
- [API キー](/ja/services/delitools/api-keys)：個人キーとテナントキー、権限、有効期限、失効。
- [請求](/ja/services/delitools/billing)：トークン使用量が料金に換算される仕組み。
- [トラブルシューティング](/ja/services/delitools/troubleshooting)：クライアントの設定とエラーの解決。
