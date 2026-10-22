# DeliTools

O DeliTools oferece duas APIs pagas para desenvolvedores: tradução de uma lista de textos e extração de um valor JSON de um texto. Você pode chamá-las com qualquer cliente Connect ou com JSON simples via HTTP, a partir do seu próprio código do lado do servidor. O Delcore gerencia suas chaves de API, permissões e faturamento, para que você use a mesma conta e o mesmo tenant do Delino para tudo.

## Conceitos

- **Tradução:** envie uma lista de textos, um idioma de destino e um modelo. Você recebe as traduções na mesma ordem e quantidade.
- **Extração:** envie um texto e um JSON Schema. Você recebe um documento JSON que segue o esquema.
- **Tenant:** um espaço pessoal ou uma organização do Delino. O Delcore cobra o uso do tenant.
- **Credencial:** um token de login do Delino, uma chave de API pessoal ou uma chave de API de serviço do tenant. Consulte [Chaves de API](/pt-BR/services/delitools/api-keys).
- **ID da solicitação:** toda resposta contém um ID de solicitação. Inclua-o ao pedir suporte.

## Login e permissões

Envie um cabeçalho `Authorization: Bearer <credential>` em cada solicitação.

| Credencial                 | Formato                          | Tenant                                               |
| -------------------------- | -------------------------------- | ---------------------------------------------------- |
| Token de login do Delino   | Um token de acesso do Delino     | Defina `tenantId` na solicitação. Ele é obrigatório. |
| Chave de API pessoal       | Um segredo que começa com `dlk_` | O tenant da chave. `tenantId` é opcional.            |
| Chave de serviço do tenant | Um segredo que começa com `dlk_` | O tenant da chave. `tenantId` é opcional.            |

Se você definir `tenantId` junto com uma chave de API, ele deverá ser o tenant dessa chave; caso contrário, a solicitação falhará com `ERROR_REASON_PERMISSION_DENIED`. Toda solicitação precisa ser autorizada pelo Delcore naquele momento: a credencial precisa ter a capacidade `translate` ou `extract`, o tenant deve estar ativo e deve passar pelas verificações habituais de faturamento. Essas APIs são pagas, e o Delcore as bloqueia quando seu tenant tem um problema de faturamento, como saldo esgotado.

## Início rápido

1. **Obtenha uma credencial.** Use seu token de login do Delino ou crie uma chave de API conforme descrito em [Chaves de API](/pt-BR/services/delitools/api-keys).
2. **Encontre o ID do seu tenant e um modelo.** O ID do tenant faz parte da sua conta do Delino. O operador do serviço define quais modelos de tradução estão disponíveis.
3. **Chame a API.** Todos os procedimentos estão em `delitools.v1.DelitoolsService`. Defina `DELITOOLS_URL` com a URL base do servidor da API do DeliTools fornecida pelo operador do serviço.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

Este exemplo usa uma chave de API, que já contém seu tenant. Com um token de login do Delino, adicione `"tenantId":"<tenant-id>"` ao corpo JSON.

Em JSON, os nomes dos campos usam lowerCamelCase. Consulte a [Referência da API](/pt-BR/services/delitools/api-reference) para ver todos os campos, o subconjunto de esquema compatível e os motivos de erro.

Chame o DeliTools a partir de código do lado do servidor ou da linha de comando. Não coloque uma chave de API no código do frontend: o DeliTools não oferece suporte a chamadas de navegadores.

## Seu conteúdo e o provedor de IA

O DeliTools executa sua solicitação no Vercel AI Gateway. **Os textos e esquemas que você envia são encaminhados ao Vercel AI Gateway e ao provedor do modelo que você escolher.** O tratamento do seu conteúdo por esses provedores, incluindo qualquer retenção, é regido pelos termos e configurações deles, não pelo DeliTools.

- O DeliTools não armazena seus textos, esquemas ou resultados e não os grava em seus logs. Isso descreve apenas o DeliTools. Não é uma garantia sobre o que o Vercel AI Gateway ou o provedor do modelo mantém.
- Não envie conteúdo que você não tenha permissão para compartilhar com esses provedores. Analise os termos deles antes de enviar dados sensíveis ou sujeitos a regulamentação.
- O DeliTools mantém apenas os registros de uso necessários para cobrar seu tenant e os remove 30 dias após a criação.
- Não há garantia de que a saída do modelo esteja correta. Verifique os resultados antes de confiar neles.

## Mais guias

- [Referência da API](/pt-BR/services/delitools/api-reference): campos, tags de idioma, subconjunto de esquema, erros e limites.
- [Chaves de API](/pt-BR/services/delitools/api-keys): chaves pessoais e de tenant, permissões, expiração e revogação.
- [Faturamento](/pt-BR/services/delitools/billing): como o uso de tokens se transforma em cobranças.
- [Solução de problemas](/pt-BR/services/delitools/troubleshooting): configuração do cliente e correção de erros.
