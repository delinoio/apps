# Delcore

O Delcore fornece tenants compartilhados, controle de acesso e faturamento para os serviços Delino. O servidor oficial da API é `https://delcore.api.delino.io`. Esta página descreve os preços de uso, a expiração de registros e a verificação do estado do tenant para desenvolvedores de serviços.

## Verificar o estado do tenant

Um serviço registrado pode consultar o Delcore para saber se um tenant está ativo, bloqueado ou excluído. Use essa verificação quando seu serviço armazenar dados de um tenant, mas não tiver um usuário Delino para cada requisição, por exemplo, quando uma chave de API de projeto fizer a chamada.

Chame `delcore.v1.DelcoreService/GetTenantStates` com 1 a 100 IDs de tenant. Cada ID deve ser um UUID versão 7 em letras minúsculas. Autentique-se com o token de acesso da aplicação máquina a máquina do seu serviço no Logto. O serviço deve estar registrado no Delcore e ter o escopo `authorize`. Tokens de usuário, serviços não registrados e serviços sem esse escopo não podem ler o estado do tenant.

A resposta repete cada ID de tenant solicitado, na ordem em que você os enviou, com um estado:

| Estado                  | Significado                                                       |
| ----------------------- | ----------------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | O tenant existe e não está bloqueado nem em processo de exclusão. |
| `TENANT_STATE_BLOCKED`  | Um operador da plataforma bloqueou o tenant.                      |
| `TENANT_STATE_DELETING` | A exclusão está pendente.                                         |
| `TENANT_STATE_DELETED`  | A exclusão é definitiva.                                          |
| `TENANT_STATE_UNKNOWN`  | O Delcore não tem um tenant com esse ID.                          |

Quando mais de uma condição se aplica, `DELETED` prevalece sobre `DELETING`, `DELETING` prevalece sobre `BLOCKED` e `BLOCKED` prevalece sobre `ACTIVE`.

O estado não depende do faturamento. Uma assinatura expirada, uma falha na renovação, uma contestação ou um saldo baixo não tornam um tenant `BLOCKED`. Para decidir se um usuário pode executar tarefas pagas, use `Authorize`. A verificação apenas lê dados. Ela não altera um tenant, não autoriza uso pago nem entra em contato com um provedor de pagamentos, e não retorna nomes de tenants, detalhes de usuários ou informações de faturamento.

## Tratar resultados e falhas

- Permita o acesso apenas para `TENANT_STATE_ACTIVE`, ou quando suas próprias regras documentadas para proprietários ou administradores permitirem acesso a um tenant bloqueado ou em processo de exclusão.
- Trate `TENANT_STATE_UNKNOWN`, erros de requisição, tempos limite excedidos e a indisponibilidade do Delcore como acesso negado. Não exclua dados nesses casos.
- Para a limpeza decorrente da exclusão de um tenant, exclua automaticamente os dados retidos do tenant apenas após um `TENANT_STATE_DELETED` explícito. `BLOCKED` e `DELETING` nunca permitem essa limpeza. A expiração fixa dos registros se aplica independentemente do estado do tenant.
- IDs inválidos, uma requisição vazia ou mais de 100 IDs retornam um erro de argumento inválido. Credenciais ausentes, expiradas ou inválidas retornam um erro de não autenticado. Um chamador sem autorização para ler o estado do tenant recebe um erro de permissão negada. Outras falhas retornam um erro com um ID de correlação; inclua esse ID, encontrado nos detalhes do erro e no cabeçalho `X-Delcore-Correlation-Id`, ao solicitar suporte.
- Tente novamente mais tarde se uma requisição falhar. Uma requisição com falha nunca significa que um tenant está ativo ou excluído.

## Preços de uso e expiração de registros

Os preços unitários de uso se aplicam no início da execução inicial para todos os clientes, incluindo assinantes existentes. Informe esse início em `IngestEvent` como `occurred_at` e mantenha-o nas novas tentativas de execução e na conciliação de uso. Uma alteração de preço se aplica a execuções posteriores; entregas tardias e correções mantêm a atribuição de preço original. Os preços de pagamentos recorrentes, os recursos selecionados e as concessões de uso incluído mantêm seus termos de assinatura.

A deduplicação de requisições dura 30 dias a partir da criação inicial do registro. Uma nova tentativa não estende esse período. Após a expiração, a mesma chave de requisição pode iniciar uma nova operação, sujeita às permissões atuais e a verificações independentes de identidade de pagamento ou evento. Os eventos de uso aceitos mantêm suas identidades duráveis para evitar efeitos contábeis duplicados.

Os serviços consumidores devem excluir os registros de entrega de uso e de conciliação 30 dias após a criação inicial, mesmo quando a execução ou o uso permanecerem desconhecidos ou um relatório não tiver chegado ao Delcore. Repita as tentativas de entrega e de consulta de uso desconhecido até essa expiração e, então, abandone a cobrança pendente. Não estime o uso para fins de faturamento. Isso pode causar a perda permanente de uso faturável, e uma chave de deduplicação de execução expirada pode permitir uma nova execução e cobrança. Os registros de auditoria da plataforma e os logs operacionais também expiram 30 dias após a criação. Esses limites não excluem lançamentos contábeis já aceitos pelo Delcore nem alteram a validade das chaves de API.
