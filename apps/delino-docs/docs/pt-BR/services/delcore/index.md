# Delcore

O Delcore fornece tenants compartilhados, controle de acesso e faturamento para os serviços Delino. O servidor oficial da API é `https://delcore.api.delino.io`. Esta página descreve a verificação de chaves de API, os preços de uso, a expiração de registros e a consulta ao estado do tenant para desenvolvedores de serviços.

## Consultar o estado do tenant

Um serviço registrado pode perguntar ao Delcore se um tenant está ativo, bloqueado ou excluído. Use essa consulta quando seu serviço armazenar dados de um tenant, mas não tiver um usuário Delino para cada solicitação, por exemplo, quando uma chave de API de projeto fizer a chamada.

Chame `delcore.v1.DelcoreService/GetTenantStates` com 1 a 100 IDs de tenant. Cada ID deve ser um UUID versão 7 em letras minúsculas. Autentique-se com o token de acesso do aplicativo máquina a máquina do Logto do seu serviço. O serviço deve estar registrado no Delcore e ter o escopo `authorize`. Tokens de usuário, serviços não registrados e serviços sem esse escopo não podem consultar o estado do tenant.

A resposta repete cada ID de tenant solicitado, na ordem em que você os enviou, com um estado:

| Estado                  | Significado                                                       |
| ----------------------- | ----------------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | O tenant existe e não está bloqueado nem em processo de exclusão. |
| `TENANT_STATE_BLOCKED`  | Um operador da plataforma bloqueou o tenant.                      |
| `TENANT_STATE_DELETING` | A exclusão está pendente.                                         |
| `TENANT_STATE_DELETED`  | A exclusão é definitiva.                                          |
| `TENANT_STATE_UNKNOWN`  | O Delcore não tem um tenant com esse ID.                          |

Quando mais de uma condição se aplica, `DELETED` tem precedência sobre `DELETING`, `DELETING` tem precedência sobre `BLOCKED`, e `BLOCKED` tem precedência sobre `ACTIVE`.

O estado não depende do faturamento. Uma assinatura expirada, uma renovação malsucedida, uma contestação ou um saldo baixo não tornam um tenant `BLOCKED`. Para decidir se um usuário pode executar trabalho pago, use `Authorize`. A consulta apenas lê dados. Ela não altera um tenant, não autoriza uso pago nem entra em contato com um provedor de pagamentos, e não retorna nomes de tenants, detalhes de usuários ou informações de faturamento.

## Lidar com resultados e falhas

- Permita o acesso apenas para `TENANT_STATE_ACTIVE`, ou quando suas próprias regras documentadas para proprietários ou administradores permitirem acesso a um tenant bloqueado ou em processo de exclusão.
- Trate `TENANT_STATE_UNKNOWN`, erros de solicitação, tempos limite excedidos e a indisponibilidade do Delcore como ausência de acesso. Não exclua dados nesses casos.
- Para a limpeza causada pela exclusão de um tenant, exclua automaticamente os dados retidos do tenant somente após um `TENANT_STATE_DELETED` explícito. `BLOCKED` e `DELETING` nunca permitem essa limpeza. A expiração fixa dos registros se aplica independentemente do estado do tenant.
- IDs inválidos, uma solicitação vazia ou mais de 100 IDs retornam um erro de argumento inválido. Credenciais ausentes, expiradas ou inválidas retornam um erro de não autenticado. Um chamador sem permissão para consultar o estado do tenant recebe um erro de permissão negada. Outras falhas retornam um erro com um ID de correlação; inclua esse ID, encontrado nos detalhes do erro e no cabeçalho `X-Delcore-Correlation-Id`, ao solicitar suporte.
- Tente novamente uma solicitação malsucedida mais tarde. Uma solicitação malsucedida nunca significa que um tenant está ativo ou excluído.

## Preços de uso e expiração de registros

Os preços unitários de uso se aplicam no início da execução inicial para todos os clientes, incluindo assinantes existentes. Informe esse início em `IngestEvent` como `occurred_at` e mantenha-o nas novas tentativas de execução e na reconciliação de uso. Uma alteração de preço se aplica a execuções posteriores; entregas tardias e correções mantêm a atribuição de preço original. Os preços de pagamentos recorrentes, os recursos selecionados e as concessões de uso incluído mantêm os termos da assinatura.

A deduplicação de solicitações dura 30 dias a partir da primeira criação do registro. Uma nova tentativa não estende esse período. Após a expiração, a mesma chave de solicitação pode iniciar uma nova operação, sujeita às permissões atuais e a verificações independentes de identidade de pagamento ou evento. Os eventos de uso aceitos mantêm suas identidades duráveis para evitar efeitos contábeis duplicados.

Os serviços consumidores devem excluir os registros de entrega de uso e de reconciliação 30 dias após a primeira criação, mesmo quando a execução ou o uso permanecerem desconhecidos ou um relatório não tiver chegado ao Delcore. Repita as tentativas de entrega e de consulta de uso desconhecido até essa expiração e, então, abandone a cobrança pendente. Não estime o uso para fins de faturamento. Isso pode resultar na perda permanente de uso faturável, e uma chave de deduplicação de execução expirada pode permitir uma nova execução e cobrança. Os registros de auditoria da plataforma e os logs operacionais também expiram 30 dias após a criação. Esses limites não excluem lançamentos contábeis já aceitos pelo Delcore nem alteram a validade das chaves de API.

## Verificar uma chave de API

O Delcore emite chaves de API reutilizáveis, pessoais e de serviço do tenant, para qualquer serviço registrado. Usuários e Proprietários e Administradores do tenant as criam com um token de login Delino; consulte [Chaves de API do DeliTools](/pt-BR/services/delitools/api-keys) para ver o fluxo voltado ao usuário. Uma chave começa com `dlk_`, concede capacidades selecionadas de serviços selecionados e pertence a um tenant.

Um serviço registrado verifica uma chave recebida de seus chamadores com `delcore.v1.DelcoreService/AuthorizeApiKey`. Autentique-se com o token de acesso máquina a máquina do Logto do seu serviço. O serviço deve estar registrado e ter o escopo `authorize`.

- Envie o `secret` completo, a `capability` em uso e `paid` para trabalho pago. Você também pode enviar `feature_key` e `group_values`, como em `Authorize`.
- O Delcore avalia apenas as concessões que indicam o serviço chamador, portanto seu serviço nunca pode autorizar as capacidades de outro serviço.
- Um segredo malformado, desconhecido, revogado ou expirado, ou o segredo de um tenant excluído, retorna um erro de não autenticado e nenhum detalhe da chave. O Delcore também responde dessa forma quando o token do seu próprio serviço é rejeitado, portanto verifique as credenciais do seu serviço se todas as chaves válidas falharem. Trate isso como uma credencial inválida.
- Para um segredo válido, a resposta contém `allowed`, `reasons` e o campo opcional `feature`, além de `keyId`, `tenantId`, `ownerKind` e `userId`. Uma chave sem a capacidade retorna `allowed: false` com `BLOCK_REASON_PERMISSION`.
- Uma chave pessoal também precisa da permissão atual de seu usuário para a capacidade no tenant. Uma chave de tenant não depende do usuário que a criou. Ambas precisam de um tenant ativo e das mesmas verificações de faturamento de `Authorize` quando `paid` estiver definido.
- Compare o tenant fornecido pelo chamador, se houver, com o `tenantId` retornado e rejeite qualquer divergência.
- Verifique cada solicitação. Não armazene um resultado em cache. Trate erros, tempos limite excedidos e a indisponibilidade do Delcore como ausência de acesso para novos trabalhos pagos. Um resultado de autorização não reserva nada.
