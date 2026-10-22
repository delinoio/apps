# DeliTools

DeliTools provides two paid APIs for developers: translation of a list of texts, and extraction of a JSON value from text. You call them with any Connect client, or with plain JSON over HTTP, from your own server-side code. Delcore handles your API keys, permissions and billing, so you use the same Delino account and tenant for everything.

## Concepts

- **Translation:** send a list of texts, a target language and a model. You receive the translations in the same order and count.
- **Extraction:** send a text and a JSON Schema. You receive one JSON document that follows the schema.
- **Tenant:** a Delino personal space or organization. Delcore bills usage to the tenant.
- **Credential:** a Delino login token, a personal API key or a tenant service API key. See [API keys](/services/delitools/api-keys).
- **Request ID:** every response carries a request ID. Include it when you ask for support.

## Sign in and permissions

Send an `Authorization: Bearer <credential>` header with each request.

| Credential         | How it looks             | Tenant                                         |
| ------------------ | ------------------------ | ---------------------------------------------- |
| Delino login token | A Delino access token    | Set `tenantId` in the request. It is required. |
| Personal API key   | A secret starting `dlk_` | The tenant of the key. `tenantId` is optional. |
| Tenant service key | A secret starting `dlk_` | The tenant of the key. `tenantId` is optional. |

If you set `tenantId` together with an API key, it must be the tenant of that key, or the request fails with `ERROR_REASON_PERMISSION_DENIED`. Every request must be allowed by Delcore at that moment: the credential needs the `translate` or `extract` capability, the tenant must be active, and the tenant must pass the usual billing checks. These APIs are paid, and Delcore blocks them when your tenant has a billing problem such as an exhausted balance.

## Quick start

1. **Get a credential.** Use your Delino login token, or create an API key as described in [API keys](/services/delitools/api-keys).
2. **Find your tenant ID and a model.** The tenant ID is part of your Delino account. The service operator sets which translation models are available.
3. **Call the API.** All procedures are on `delitools.v1.DelitoolsService`. Set `DELITOOLS_URL` to the base URL of the DeliTools API server that your service operator gives you.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

This example uses an API key, which carries its tenant. With a Delino login token, add `"tenantId":"<tenant-id>"` to the JSON body.

In JSON, field names use lowerCamelCase. See the [API reference](/services/delitools/api-reference) for all fields, the supported schema subset and the error reasons.

Call DeliTools from server-side code or the command line. Do not put an API key in frontend code: DeliTools does not support calls from browsers.

## Your content and the AI provider

DeliTools runs your request on Vercel AI Gateway. **The texts and schemas you submit are sent to Vercel AI Gateway and to the model provider behind the model you choose.** Their handling of your content, including any retention, is governed by their own terms and settings, not by DeliTools.

- DeliTools does not store your texts, schemas or results, and does not write them to its logs. This describes DeliTools only. It is not a guarantee about what Vercel AI Gateway or the model provider keeps.
- Do not submit content that you are not allowed to share with those providers. Review their terms before you submit sensitive or regulated data.
- DeliTools keeps only the usage records it needs to bill your tenant, and removes them 30 days after they were created.
- Model output is not guaranteed to be correct. Check results before you rely on them.

## More guides

- [API reference](/services/delitools/api-reference): fields, language tags, schema subset, errors and limits.
- [API keys](/services/delitools/api-keys): personal and tenant keys, permissions, expiry and revocation.
- [Billing](/services/delitools/billing): how token usage becomes charges.
- [Troubleshooting](/services/delitools/troubleshooting): client configuration and fixing errors.
