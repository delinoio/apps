# DeliTools API リファレンス

このページでは、DeliTools の 2 つのメソッドについて説明します。どちらも `delitools.v1.DelitoolsService` の Connect プロシージャで、HTTP `POST` で JSON を受け付けます。JSON のフィールド名は lowerCamelCase です。`Content-Type: application/json` と `Authorization: Bearer <credential>` を送信してください。認証情報とテナントについては、[DeliTools](/ja/services/delitools/) を参照してください。

メソッドハンドラーが生成するすべてのレスポンスには、本文と `X-Delitools-Request-Id` ヘッダーにリクエスト ID が含まれます。トランスポートのサイズ制限によって拒否されたリクエスト（エラーの表を参照）には、リクエスト ID はありません。レスポンスにはトークン使用量や料金は含まれません。これらは Delcore で確認してください。[課金](/ja/services/delitools/billing)を参照してください。

呼び出しごとに新しい実行が行われます。DeliTools には冪等性キーも、保存された結果もありません。リクエストを繰り返すと、再度実行され、再度課金される可能性があります。

## Translate

`POST /delitools.v1.DelitoolsService/Translate`

| フィールド       | 型           | ルール                                                                                                            |
| ---------------- | ------------ | ----------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | 文字列       | Delino ログイントークンでは必須です。API キーの場合は、空にするか、そのキーのテナントを指定します。               |
| `texts`          | 文字列の配列 | 必須です。少なくとも 1 項目必要です。空の文字列は使用できません。項目数と合計サイズは運用者によって制限されます。 |
| `targetLanguage` | 文字列       | 必須です。`fr` や `pt-BR` などの BCP 47 言語タグです。                                                            |
| `sourceLanguage` | 文字列       | 任意です。BCP 47 言語タグです。省略すると、モデルが原文の言語を検出します。                                       |
| `model`          | 文字列       | 必須です。運用者が許可しているモデル識別子です。                                                                  |

レスポンスには `requestId` と `translations` が含まれます。`translations` の項目数と順序は `texts` と同じです。

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

- **言語タグ。** DeliTools は、大文字と小文字を区別せず、タグが BCP 47 に準拠した正しい形式であること、64 文字以内であること、主言語が 2 文字または 3 文字であることを確認します。私用サブタグのみで構成されるタグと、既得権タグは拒否されます。DeliTools は、モデルがその言語をサポートしているかどうかは確認しません。
- **すべて成功するか、すべて失敗するか。** 結果はアトミックです。いずれかの項目が失敗すると、リクエスト全体が失敗し、部分的な翻訳は返されません。
- **特別扱いはありません。** 原文の言語が省略されている場合、複数の言語が混在する入力、すでに翻訳先の言語になっているテキストは、いずれもモデルに送られます。DeliTools はこれらを拒否せず、代わりに原文をコピーすることもありません。
- **モデル。** 運用者が許可し、かつ Delcore に料金が登録されているモデルのみ使用できます。別のモデルに置き換えられることはありません。利用可能なモデル識別子については、サービス運用者にお問い合わせください。

## Extract

`POST /delitools.v1.DelitoolsService/Extract`

| フィールド   | 型     | ルール                                                                                                         |
| ------------ | ------ | -------------------------------------------------------------------------------------------------------------- |
| `tenantId`   | 文字列 | Delino ログイントークンでは必須です。API キーの場合は、空にするか、そのキーのテナントを指定します。            |
| `text`       | 文字列 | 必須で、空にはできません。サイズは運用者によって制限されます。                                                 |
| `jsonSchema` | 文字列 | 必須です。以下のサポート対象サブセットのみを使用する JSON Schema ドキュメントを、JSON 文字列として指定します。 |

モデルは `openai/gpt-6-luna` に固定されています。選択はできません。レスポンスには `requestId` と、指定したスキーマに準拠する JSON 文字列である `json` が含まれます。

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

- **グラウンディングは指示であり、検証ではありません。** DeliTools は、指定されたテキストからのみ値を取得するようモデルに指示します。必須情報が不足しているとモデルが報告した場合、リクエストは `ERROR_REASON_EXTRACTION_FAILED` で失敗します。DeliTools は、返された値がテキストに由来するかどうかを検証できないため、モデルがスキーマに従った架空の値を返す可能性は残ります。
- **出力の検証。** DeliTools は、結果が指定されたスキーマに適合するか検証します。形式が不正な出力やスキーマに適合しない出力は、`ERROR_REASON_INVALID_OUTPUT` で失敗します。DeliTools は、不正な出力を修正するための再試行は行いません。
- **正確性。** スキーマに従った結果でも、誤っている可能性があります。重要な結果は確認してください。
- **未対応のスキーマは最初に拒否されます。** DeliTools は、サポート対象サブセットに含まれないスキーマをモデル実行前に拒否するため、そのリクエストには課金されません。

### サポートされる JSON Schema のサブセット

スキーマは JSON Schema 2020-12 として読み取られます。JSON オブジェクトである必要があります。`$schema` は任意で、ルートでのみ使用できます。文字列である必要があり、そうでなければスキーマは未対応と見なされます。DeliTools は、この文字列を使ってドラフトを選択しません。ルート、各プロパティのスキーマ、`items` のスキーマを含むすべてのスキーマに、`type`、`enum`、`const` のいずれかが必要です。スキーマの深さは最大 8 階層、スキーマノード数は最大 256 で、サイズは運用者によって制限されます。

| キーワード               | ルール                                                                                                                                                                                      |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `type`                   | `object`、`array`、`string`、`number`、`integer`、`boolean`、`null`、またはこれらの型の配列です。たとえば、null を許容する値には `["string","null"]` を指定します。                         |
| `properties`, `required` | `properties` は名前をスキーマに対応付けます。`required` のすべての名前は `properties` に存在する必要があります。オブジェクトごとに最大 64 プロパティで、`required` の名前は重複できません。 |
| `additionalProperties`   | `true` または `false` のみです（スキーマは指定できません）。                                                                                                                                |
| `items`                  | すべての項目に適用する 1 つのスキーマです。                                                                                                                                                 |
| `enum`, `const`          | `enum` は 1 ～ 64 個の異なる値を持ちます。`const` は単一の値です。                                                                                                                          |
| `minLength`, `maxLength` | 文字列用の、0 ～ 1,000,000 の整数です。最小値は最大値を超えることができません。                                                                                                             |
| `minItems`, `maxItems`   | 配列用の、0 ～ 1,000,000 の整数です。最小値は最大値を超えることができません。                                                                                                               |
| `minimum`, `maximum`     | `number` と `integer` 用の有限の数値です。最小値は最大値を超えることができません。                                                                                                          |
| `title`, `description`   | モデル向けの注記です。検証には影響しません。                                                                                                                                                |

以下はサポートされておらず、`ERROR_REASON_UNSUPPORTED_SCHEMA` が返されます：`$ref`、`$defs`、`definitions`、`$id`、`$comment`、`oneOf`、`anyOf`、`allOf`、`not`、`if`/`then`/`else`、`patternProperties`、`propertyNames`、`pattern`、`format`、`uniqueItems`、`multipleOf`、`exclusiveMinimum`、`exclusiveMaximum`、`contains`、`prefixItems`、`default`、`unevaluated` キーワード、ブール値スキーマ、不明なキーワード、大きすぎるスキーマ、ネストが深すぎるスキーマ。DeliTools は参照をたどらないため、スキーマによって何かを取得させることはできません。

列挙したプロパティのみを許可する場合は、オブジェクトに `"additionalProperties": false` を設定してください。

## エラー

エラーには標準の Connect コードが使用されます。エラーの詳細には、固定の理由とリクエスト ID が含まれます。メッセージは英語で、送信したコンテンツが含まれることはありません。

| 理由                                  | Connect コード                              | 意味                                                                                                                                                                                                                               |
| ------------------------------------- | ------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                          | フィールドが無効です。空の配列や文字列、不正な言語タグ、テナントの欠落や形式不正、モデルの欠落、有効な JSON ではない `jsonSchema` が該当します。運用者が許可していないモデルの場合は `ERROR_REASON_MODEL_UNAVAILABLE` になります。 |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                           | 認証情報が欠落しているか、形式不正、期限切れ、失効済み、または不明です。                                                                                                                                                           |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                         | 認証情報に必要なケーパビリティ、キーの付与権限、またはテナント権限がないか、送信した `tenantId` がキーのテナントではありません。                                                                                                   |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                       | 残高、更新、異議申し立て、運用者によるブロック、テナントの削除中などの理由で、Delcore が有料利用をブロックしました。                                                                                                               |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                        | 入力、項目数、またはスキーマが運用者の制限を超えています。                                                                                                                                                                         |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                        | テナントがリクエストレートまたは同時実行数の上限に達しました。後でもう一度お試しください。リクエストレートのエラーには `Retry-After` ヘッダーが含まれます。                                                                        |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` or `failed_precondition` | モデルが許可されていないか（`invalid_argument`）、入力と出力の料金が登録されていません（`failed_precondition`）。別のモデルに置き換えられることはありません。                                                                      |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                          | スキーマがサポート対象サブセット外のキーワードを使用しています。                                                                                                                                                                   |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` or `internal`                 | 許可された回数の再試行後も AI プロバイダーで失敗が続いたか（`unavailable`）、プロバイダーがリクエストを拒否しました（`internal`）。                                                                                                |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                  | モデルの出力が形式不正、途中で切れた、または規定に適合していませんでした。                                                                                                                                                         |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                       | テキストに必須情報が含まれていません。                                                                                                                                                                                             |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                         | 運用者が設定した実行タイムアウトを超過しました。この時間には、各種チェックとすべての試行が含まれます。                                                                                                                             |
| `ERROR_REASON_CANCELED`               | `canceled`                                  | リクエストがキャンセルされたか、接続が閉じられました。                                                                                                                                                                             |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                               | 認可、料金、課金のチェックを完了できなかったか、DeliTools が呼び出しの使用量を記録できませんでした。有料利用は安全側に倒して拒否され、結果は返されません。                                                                         |

運用者が設定したリクエスト全体のサイズ制限を超えるリクエストには、これらの詳細を伴わない単純な `resource_exhausted` エラーが返される場合があります。対処方法については、[トラブルシューティング](/ja/services/delitools/troubleshooting)を参照してください。

値の型が誤っている、本文が途中で切れている、構文エラーがあるなどの理由でデコードできない JSON リクエスト本文には、HTTP 400 が返されます。これには Connect コード `invalid_argument`、理由 `ERROR_REASON_INVALID_INPUT`、リクエスト ID、および正確に `request body is invalid` というメッセージが含まれます。DeliTools は、認証情報を確認する前に本文を確認します。メッセージにユーザーのコンテンツの一部が再掲されることはありません。JSON のエンコードまたはフィールドの型を修正して、リクエストを再送信してください。

## 制限と再試行

次の制限は運用者が設定し、変更される可能性があります：入力サイズ、翻訳項目数、スキーマのサイズと深さ、実行タイムアウト、テナントごとのリクエストレートとバースト、テナントごとの同時実行数、一時的な障害に対する最大再試行回数。適用される値については、サービス運用者にお問い合わせください。制限は保護のためのものであり、スループットを保証するものではありません。

- DeliTools は、ネットワークエラー、サーバーエラー、レート制限レスポンスなど、AI プロバイダーの一時的な障害に対してのみ、設定された回数を上限に再試行します。不正な出力を修正するための再試行は行いません。
- DeliTools は、再試行のたびに認可を再確認します。キーが失効または期限切れになった場合、権限を失った場合、あるいは課金がブロックされた場合は、処理を停止し、対応するエラーを返します。すでに実行中のプロバイダー呼び出しは完了する場合があります。
- リクエストをキャンセルするか接続を閉じると、DeliTools はプロバイダー呼び出しをキャンセルし、再試行を停止します。
- クライアントが呼び出しを閉じた場合、またはクライアント自身のタイムアウトによって呼び出しを終了した場合、DeliTools はこれをキャンセルとして扱います。
- 運用者が設定した実行タイムアウトには、認可、検証、およびすべての試行が含まれます。タイムアウトすると、DeliTools はプロバイダー呼び出しを中止し、`ERROR_REASON_TIMEOUT` を返します。
