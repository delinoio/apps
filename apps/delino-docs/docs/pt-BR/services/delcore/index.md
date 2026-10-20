# Delcore

O Delcore fornece tenants compartilhados, controle de acesso e faturamento para os serviços da Delino. O servidor oficial da API é `https://delcore.api.delino.io`. Esta página descreve a verificação do estado do tenant para desenvolvedores de serviços.

## Verificar o estado do tenant

Um serviço registrado pode perguntar ao Delcore se um tenant está ativo, bloqueado ou excluído. Use essa verificação quando seu serviço armazena dados de um tenant, mas não possui um usuário da Delino para cada solicitação, por exemplo, quando uma chave de API do projeto realiza a chamada.

Chame `delcore.v1.DelcoreService/GetTenantStates` com 1 a 100 IDs de tenant. Cada ID deve ser um UUID versão 7 em letras minúsculas. Autentique-se com o token de acesso do aplicativo máquina a máquina do Logto do seu serviço. O serviço deve estar registrado no Delcore e ter o escopo `authorize`. Tokens de usuário, serviços não registrados e serviços sem esse escopo não podem consultar o estado do tenant.

A resposta repete cada ID de tenant solicitado, na ordem em que foram enviados, com um dos seguintes estados:

| Estado                  | Significado                                              |
| ----------------------- | -------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | O tenant existe e não está bloqueado nem sendo excluído. |
| `TENANT_STATE_BLOCKED`  | Um operador da plataforma bloqueou o tenant.             |
| `TENANT_STATE_DELETING` | A exclusão está pendente.                                |
| `TENANT_STATE_DELETED`  | A exclusão é definitiva.                                 |
| `TENANT_STATE_UNKNOWN`  | O Delcore não possui nenhum tenant com este ID.          |

Quando mais de uma condição se aplica, `DELETED` prevalece sobre `DELETING`, `DELETING` prevalece sobre `BLOCKED` e `BLOCKED` prevalece sobre `ACTIVE`.

O estado não depende do faturamento. Uma assinatura expirada, uma renovação malsucedida, uma contestação ou um saldo baixo não fazem com que um tenant fique `BLOCKED`. Para decidir se um usuário pode executar trabalhos pagos, use `Authorize`. A verificação apenas lê dados. Ela não altera um tenant, não autoriza o uso pago nem entra em contato com um provedor de pagamentos, e não retorna nomes de tenants, detalhes de usuários ou informações de faturamento.

## Tratar resultados e falhas

- Permita o acesso apenas para `TENANT_STATE_ACTIVE` ou quando suas próprias regras documentadas para proprietários ou administradores permitirem o acesso a um tenant bloqueado ou em processo de exclusão.
- Trate `TENANT_STATE_UNKNOWN`, erros de solicitação, tempos limite e a indisponibilidade do Delcore como ausência de acesso. Não exclua dados nesses casos.
- Exclua automaticamente os dados retidos do tenant somente após um `TENANT_STATE_DELETED` explícito. `BLOCKED` e `DELETING` nunca permitem a exclusão automática.
- IDs inválidos, uma solicitação vazia ou mais de 100 IDs retornam um erro de argumento inválido. Credenciais ausentes, expiradas ou inválidas retornam um erro de não autenticado. Um chamador sem permissão para consultar o estado do tenant recebe um erro de permissão negada. Outras falhas retornam um erro com um ID de correlação; ao solicitar suporte, inclua esse ID, encontrado nos detalhes do erro e no cabeçalho `X-Delcore-Correlation-Id`.
- Tente novamente mais tarde após uma solicitação malsucedida. Uma solicitação malsucedida nunca significa que um tenant está ativo ou excluído.
