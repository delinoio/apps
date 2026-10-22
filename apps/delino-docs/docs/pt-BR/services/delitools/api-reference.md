# Referência da API do DeliTools

Esta página descreve os dois métodos do DeliTools. Ambos são procedimentos Connect em `delitools.v1.DelitoolsService` e aceitam JSON via HTTP `POST`. Os nomes dos campos usam lowerCamelCase em JSON. Envie `Content-Type: application/json` e `Authorization: Bearer <credential>`. Consulte [DeliTools](/pt-BR/services/delitools/) para obter informações sobre credenciais e tenants.

Cada resposta inclui um ID de solicitação, no corpo e no cabeçalho `X-Delitools-Request-Id`. As respostas não incluem o uso de tokens nem cobranças. Consulte essas informações no Delcore; veja [Faturamento](/pt-BR/services/delitools/billing).

Cada chamada é uma nova execução. O DeliTools não tem chave de idempotência nem resultados armazenados. Se você repetir uma solicitação, ela será executada novamente e poderá ser cobrada novamente.

## Translate

`POST /delitools.v1.DelitoolsService/Translate`

| Campo            | Tipo             | Regras                                                                                                                      |
| ---------------- | ---------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | string           | Obrigatório para um token de login do Delino. Para uma chave de API, vazio ou o tenant da chave.                            |
| `texts`          | array de strings | Obrigatório. Pelo menos um item. Nenhuma string vazia. A quantidade de itens e o tamanho total são limitados pelo operador. |
| `targetLanguage` | string           | Obrigatório. Uma tag de idioma BCP 47, como `fr` ou `pt-BR`.                                                                |
| `sourceLanguage` | string           | Opcional. Uma tag de idioma BCP 47. Se omitida, o modelo detecta o idioma de origem.                                        |
| `model`          | string           | Obrigatório. Um identificador de modelo permitido pelo operador.                                                            |

A resposta contém `requestId` e `translations`. `translations` tem a mesma quantidade de itens, na mesma ordem de `texts`.

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

- **Tags de idioma.** O DeliTools verifica se uma tag está bem formada de acordo com o BCP 47, ignorando diferenças entre maiúsculas e minúsculas, com no máximo 64 caracteres e um idioma primário de duas ou três letras. Tags compostas apenas por subtags de uso privado e tags legadas (grandfathered) são rejeitadas. O DeliTools não verifica se o modelo oferece suporte ao idioma.
- **Tudo ou nada.** O resultado é atômico. Se qualquer item falhar, toda a solicitação falhará e você não receberá traduções parciais.
- **Sem casos especiais.** Um idioma de origem omitido, entradas com vários idiomas e textos que já estão no idioma de destino são todos enviados ao modelo. O DeliTools não os rejeita nem copia o texto de origem para você.
- **Modelo.** Somente modelos permitidos pelo operador e com preços registrados no Delcore funcionam. Nunca há substituição por outro modelo. Pergunte ao operador do serviço quais identificadores de modelo estão disponíveis.

## Extract

`POST /delitools.v1.DelitoolsService/Extract`

| Campo        | Tipo   | Regras                                                                                                       |
| ------------ | ------ | ------------------------------------------------------------------------------------------------------------ |
| `tenantId`   | string | Obrigatório para um token de login do Delino. Para uma chave de API, vazio ou o tenant da chave.             |
| `text`       | string | Obrigatório e não vazio. O tamanho é limitado pelo operador.                                                 |
| `jsonSchema` | string | Obrigatório. Um documento JSON Schema, como uma string JSON, que usa apenas o subconjunto compatível abaixo. |

O modelo é fixado em `openai/gpt-6-luna`. Você não pode escolhê-lo. A resposta contém `requestId` e `json`, uma string JSON que está em conformidade com seu esquema.

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

- **Baseado no seu texto.** O modelo deve extrair os valores do texto. Se o texto não contiver as informações obrigatórias, a solicitação falhará com `ERROR_REASON_EXTRACTION_FAILED`. O DeliTools não inventa valores.
- **Saída verificada.** O DeliTools valida o resultado em relação ao seu esquema. Saídas malformadas ou fora de conformidade falham com `ERROR_REASON_INVALID_OUTPUT`. O DeliTools não faz novas tentativas para corrigir uma saída inválida.
- **Precisão.** Um resultado que segue o esquema ainda pode estar errado. Verifique os resultados importantes.
- **Esquemas não compatíveis falham primeiro.** O DeliTools rejeita um esquema fora do subconjunto compatível antes de executar o modelo, portanto esse tipo de solicitação não é cobrado.

### Subconjunto compatível de JSON Schema

O esquema é interpretado como JSON Schema 2020-12. Ele deve ser um objeto JSON. `$schema` é opcional e permitido apenas na raiz, onde o DeliTools ignora seu valor. Todo esquema, incluindo a raiz, o esquema de cada propriedade e o esquema de `items`, precisa de `type`, `enum` ou `const`. Um esquema pode ter no máximo 8 níveis de profundidade e 256 nós de esquema, e seu tamanho é limitado pelo operador.

| Palavra-chave            | Regra                                                                                                                                                                          |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `type`                   | `object`, `array`, `string`, `number`, `integer`, `boolean`, `null` ou um array desses tipos, por exemplo `["string","null"]` para um valor que pode ser nulo.                 |
| `properties`, `required` | `properties` mapeia nomes para esquemas. Todo nome em `required` deve aparecer em `properties`. No máximo 64 propriedades por objeto, e os nomes em `required` não se repetem. |
| `additionalProperties`   | Apenas `true` ou `false` (não um esquema).                                                                                                                                     |
| `items`                  | Um esquema para todos os itens.                                                                                                                                                |
| `enum`, `const`          | `enum` tem de 1 a 64 valores distintos. `const` é um único valor.                                                                                                              |
| `minLength`, `maxLength` | Inteiros de 0 a 1.000.000, para strings. O mínimo não pode exceder o máximo.                                                                                                   |
| `minItems`, `maxItems`   | Inteiros de 0 a 1.000.000, para arrays. O mínimo não pode exceder o máximo.                                                                                                    |
| `minimum`, `maximum`     | Números finitos, para `number` e `integer`. O mínimo não pode exceder o máximo.                                                                                                |
| `title`, `description`   | Observações para o modelo. Não alteram a validação.                                                                                                                            |

Os seguintes elementos não são compatíveis e retornam `ERROR_REASON_UNSUPPORTED_SCHEMA`: `$ref`, `$defs`, `definitions`, `$id`, `$comment`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `patternProperties`, `propertyNames`, `pattern`, `format`, `uniqueItems`, `multipleOf`, `exclusiveMinimum`, `exclusiveMaximum`, `contains`, `prefixItems`, `default`, palavras-chave `unevaluated`, esquemas booleanos, palavras-chave desconhecidas e esquemas grandes demais ou com aninhamento profundo demais. O DeliTools nunca segue referências, portanto um esquema não pode fazê-lo buscar nada.

Defina `"additionalProperties": false` nos objetos quando quiser apenas as propriedades que você listou.

## Erros

Os erros usam um código Connect padrão. Os detalhes do erro incluem um motivo estável e o ID da solicitação. As mensagens estão em inglês e nunca contêm seu conteúdo.

| Motivo                                | Código Connect                              | Significado                                                                                                                                                                                                        |
| ------------------------------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                          | Um campo é inválido: array ou string vazios, tag de idioma inválida, tenant ausente ou malformado, modelo ausente ou não permitido, ou um esquema que não é JSON válido.                                           |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                           | A credencial está ausente, malformada, expirada, revogada ou é desconhecida.                                                                                                                                       |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                         | A credencial não tem a capacidade, a concessão da chave ou sua permissão no tenant, ou o `tenantId` enviado não é o tenant da chave.                                                                               |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                       | O Delcore bloqueou o uso pago, por exemplo, devido ao saldo, a uma renovação, a uma contestação, a um bloqueio do operador ou a um tenant em processo de exclusão.                                                 |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                        | A entrada, a quantidade de itens ou o esquema excede os limites do operador.                                                                                                                                       |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                        | Seu tenant atingiu o limite de taxa de solicitações ou de simultaneidade. Tente novamente mais tarde. Um erro de taxa de solicitações inclui um cabeçalho `Retry-After`.                                           |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` ou `failed_precondition` | O modelo não é permitido (`invalid_argument`) ou não tem preços de entrada e saída registrados (`failed_precondition`). Não há substituição por outro modelo.                                                      |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                          | O esquema usa uma palavra-chave fora do subconjunto compatível.                                                                                                                                                    |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` ou `internal`                 | O provedor de IA continuou falhando após as novas tentativas permitidas (`unavailable`) ou rejeitou a solicitação (`internal`).                                                                                    |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                  | A saída do modelo estava malformada, foi truncada ou não correspondeu ao contrato.                                                                                                                                 |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                       | O texto não contém as informações obrigatórias.                                                                                                                                                                    |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                         | O tempo limite de execução do operador foi excedido. Ele abrange as verificações e todas as tentativas.                                                                                                            |
| `ERROR_REASON_CANCELED`               | `canceled`                                  | A solicitação foi cancelada ou a conexão foi fechada.                                                                                                                                                              |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                               | Uma verificação de autorização, preço ou faturamento não pôde ser concluída, ou o DeliTools não conseguiu registrar o uso da chamada. O uso pago é bloqueado em caso de falha, e você não recebe nenhum resultado. |

Uma solicitação que exceda o limite geral de tamanho de solicitação do operador pode receber um erro simples `resource_exhausted` sem esses detalhes. Consulte [Solução de problemas](/pt-BR/services/delitools/troubleshooting) para saber o que fazer.

## Limites e novas tentativas

O operador configura estes limites, que podem mudar: tamanho da entrada, quantidade de itens de tradução, tamanho e profundidade do esquema, tempo limite de execução, taxa de solicitações e de rajada por tenant, execuções simultâneas por tenant e quantidade máxima de novas tentativas em caso de falhas transitórias. Entre em contato com o operador do serviço para saber os valores que se aplicam a você. Um limite é uma proteção, não uma garantia de vazão.

- O DeliTools faz novas tentativas apenas em caso de falhas transitórias do provedor de IA, como erros de rede, erros de servidor e respostas de limite de taxa, até a quantidade configurada. Ele nunca faz novas tentativas para corrigir uma saída inválida.
- Antes de cada nova tentativa, o DeliTools verifica sua autorização novamente. Se sua chave foi revogada ou expirou, você perdeu a permissão ou o faturamento está bloqueado, o DeliTools para e retorna o erro correspondente. Uma chamada ao provedor que já esteja em execução pode ser concluída.
- Se você cancelar a solicitação ou fechar a conexão, o DeliTools cancela a chamada ao provedor e para de fazer novas tentativas.
- Se seu cliente encerrar a chamada ou se o tempo limite do próprio cliente a encerrar, o DeliTools tratará isso como um cancelamento.
- O tempo limite de execução do operador abrange a autorização, a validação e todas as tentativas. Quando ele é excedido, o DeliTools aborta a chamada ao provedor e retorna `ERROR_REASON_TIMEOUT`.
