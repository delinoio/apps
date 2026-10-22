# Chaves de API do DeliTools

As chaves de API permitem que seus servidores chamem o DeliTools sem um token de login do Delino. O Delcore cria e verifica as chaves de todos os serviços do Delino, portanto o mesmo mecanismo de chaves também funciona para outros serviços. Você gerencia as chaves com seu token de login do Delino e usa uma chave como token bearer ao chamar um serviço.

## Tipos de chave

| Tipo                       | Atua como                                       | Quem pode criar, listar e revogar                           | Permissão no uso                                                                           |
| -------------------------- | ----------------------------------------------- | ----------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Chave pessoal              | Você, dentro de um único tenant                 | Somente você, e apenas enquanto for membro atual do tenant. | As concessões da chave e suas permissões atuais no tenant. Ambas devem permitir a chamada. |
| Chave de serviço do tenant | O tenant, independentemente de qualquer usuário | Um membro atual com a função Owner ou Admin do tenant.      | Apenas as concessões da chave. Ela continua funcionando se a pessoa que a criou sair.      |

- Outros usuários nunca veem nem revogam suas chaves pessoais, incluindo Owners e Admins. As funções Billing e Member não podem gerenciar chaves de serviço do tenant.
- Uma identidade de máquina a máquina não pode gerenciar chaves. As chaves nunca funcionam como credenciais para o próprio Delcore, apenas para os serviços aos quais concedem acesso.
- Ambos os tipos estão vinculados a um único tenant. Uma chave não pode ser usada para outro tenant.

## Criar uma chave

Chame `delcore.v1.DelcoreService/CreateApiKey` em `https://delcore.api.delino.io` com seu token de login do Delino.

| Campo        | Regras                                                                                                                                                        |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `requestKey` | Um UUID v7 que você gera. Repetir a solicitação com a mesma chave e os mesmos dados de entrada retorna o resultado anterior.                                  |
| `tenantId`   | O tenant ao qual a chave pertence.                                                                                                                            |
| `ownerKind`  | `API_KEY_OWNER_KIND_PERSONAL` ou `API_KEY_OWNER_KIND_TENANT`.                                                                                                 |
| `name`       | De 1 a 100 bytes, não pode estar em branco nem conter caracteres de controle.                                                                                 |
| `grants`     | De 1 a 16 concessões. Cada uma tem um `serviceId` e de 1 a 32 `capabilities` de até 100 bytes. Cada serviço aparece uma vez, e as capacidades não se repetem. |
| `expiresAt`  | Data e hora futuras opcionais no formato RFC 3339. Sem esse campo, a chave é válida até que você a revogue.                                                   |

Encontre o `serviceId` e as capacidades de um serviço no catálogo do Delcore (`GetCatalog`). Para o DeliTools, conceda as capacidades `translate` e `extract` de que você precisa. O Delcore só aceita capacidades declaradas pelo serviço, portanto uma capacidade digitada incorretamente é rejeitada quando você cria a chave.

```sh
curl --request POST "https://delcore.api.delino.io/delcore.v1.DelcoreService/CreateApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"requestKey":"<uuid-v7>","tenantId":"<tenant-id>","ownerKind":"API_KEY_OWNER_KIND_TENANT","name":"Production translation","grants":[{"serviceId":"<deli-tools-service-id>","capabilities":["translate"]}],"expiresAt":"2027-01-01T00:00:00Z"}'
```

A resposta contém os detalhes da chave e o `secret`, que começa com `dlk_`.

- **O segredo aparece apenas uma vez.** Copie-o agora para seu gerenciador de segredos. O Delcore mantém apenas um hash e nunca mostra o segredo em listagens nem em uma resposta a uma solicitação repetida.
- **Segredo perdido.** Se você perder o segredo, ou se a resposta a uma solicitação repetida tiver um `secret` vazio, crie uma nova chave e revogue a antiga.
- **Trate-o como uma senha.** Mantenha-o no código do lado do servidor e fora de logs, repositórios e bundles de frontend.
- Cada tenant pode ter um número limitado de chaves ativas: 20 chaves pessoais para cada usuário e 20 chaves de tenant no total. Chaves revogadas e expiradas não entram na contagem.

As concessões e o prazo de expiração são definidos quando você cria uma chave. Não há chamada para edição, renovação ou rotação. Para alterar uma chave, crie uma substituta com as novas concessões ou o novo prazo de expiração, passe sua aplicação a usá-la e, em seguida, revogue a chave antiga.

## Listar e revogar chaves

- `ListApiKeys` recebe `tenantId` e `ownerKind`. Retorna os detalhes das chaves, incluindo chaves revogadas e expiradas, e nunca os segredos. As listagens pessoais mostram apenas suas próprias chaves. As listagens do tenant exigem a função Owner ou Admin. Use `page.afterId` com o `nextAfterId` retornado para ler mais resultados.
- `RevokeApiKey` recebe um `requestKey`, `tenantId` e `keyId`. A revogação entra em vigor imediatamente para novas solicitações. É seguro revogar novamente.

## Usar uma chave

Envie o segredo como `Authorization: Bearer dlk_...` ao serviço ao qual a chave concede acesso. Para o DeliTools, isso funciona para `Translate` e `Extract`, conforme descrito na [Referência da API](/pt-BR/services/delitools/api-reference).

- `tenantId` é opcional ao usar uma chave. Se você o enviar, ele deve ser o tenant da chave.
- Uma chave só pode usar as capacidades que você concedeu para aquele serviço. Uma chave de um serviço nunca funciona para outro.
- O Delcore verifica a chave em todas as solicitações e novamente antes de cada nova tentativa de IA. A revogação, a expiração e a perda de permissão ou de acesso ao faturamento interrompem novas solicitações e tentativas adicionais. Uma chamada ao provedor que já estiver em execução pode ser concluída.
- Uma chave pessoal deixa de funcionar enquanto você não tiver mais permissão para usar a capacidade no tenant, por exemplo, após ser removido do tenant ou perder uma permissão de grupo. Sua remoção não exclui a chave.
- Uma chave de tenant não depende de seu criador continuar sendo membro.
- As chamadas com uma chave são cobradas do tenant da chave pelos mesmos preços das chamadas com um token de login. Consulte [Faturamento](/pt-BR/services/delitools/billing).
- Quando você exclui um tenant, o Delcore exclui suas chaves quando a exclusão se torna definitiva. As chaves pessoais são excluídas assim que a solicitação de saída do usuário é aceita. Uma chave de um tenant excluído é inválida.

## Erros comuns

| Situação                                                                              | Resultado                                                                 |
| ------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| O segredo foi digitado incorretamente, é desconhecido, foi revogado ou expirou        | `unauthenticated`. O Delcore não informa qual dessas situações se aplica. |
| A chave não tem o serviço ou a capacidade                                             | `permission_denied`                                                       |
| `tenantId` não corresponde ao tenant da chave                                         | `permission_denied`                                                       |
| Sua função não pode gerenciar esse tipo de chave                                      | `permission_denied` do Delcore ao criar, listar ou revogar a chave.       |
| Você revoga a chave pessoal de outro usuário ou uma chave desconhecida                | `not_found` do Delcore.                                                   |
| Você cria uma chave em um tenant que está sendo excluído ou excede o limite de chaves | `failed_precondition` do Delcore.                                         |
| Concessão, nome, serviço, capacidade ou prazo de expiração inválido                   | `invalid_argument` do Delcore ao criar a chave.                           |
| O faturamento do tenant está bloqueado                                                | O DeliTools retorna `ERROR_REASON_BILLING_BLOCKED`.                       |
