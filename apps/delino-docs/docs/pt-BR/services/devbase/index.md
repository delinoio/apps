# DevBase

O DevBase é um diretório gratuito dos usuários finais dos seus aplicativos. Você cadastra seus próprios usuários, consulta-os e altera-os por meio de uma API Connect. Seus usuários não precisam de contas Delino, e seu aplicativo mantém o próprio login. O DevBase fornece a cada usuário um ID estável que você pode passar para outros serviços.

## Conceitos

- **Tenant:** Um espaço pessoal ou uma organização Delino. O Delcore gerencia os tenants e seus membros.
- **Projeto:** Um diretório nomeado de usuários finais dentro de um tenant. Os projetos não compartilham usuários, mesmo quando estão no mesmo tenant.
- **Usuário final:** Uma pessoa ou conta no seu aplicativo. Possui um ID externo escolhido por você, um ID interno criado pelo DevBase, nome e e-mail opcionais e um objeto JSON de metadados.
- **Chave de API do projeto:** Um segredo que seu aplicativo usa para ler ou alterar os usuários finais de um projeto.

## Login e permissões

Envie um cabeçalho `Authorization: Bearer <token>` em cada solicitação.

- **Desenvolvedores** usam o token de login da Delino. Somente um Proprietário ou Administrador atual do tenant pode gerenciar projetos e chaves de API. Eles também podem ler e alterar usuários finais com o token de login.
- **Aplicativos** usam uma chave de API do projeto. Uma chave começa com `dbk_`. Uma chave `READ_ONLY` consulta e lista usuários. Uma chave `CRUD` também cria, atualiza e exclui usuários. Uma chave nunca gerencia projetos ou chaves e funciona somente em seu próprio projeto.

Os usuários finais nunca fazem login no DevBase. Seus registros não lhes dão acesso à Delino.

## Início rápido

1. **Crie um projeto.** Chame `CreateProject` com o ID do tenant e um nome. Os nomes são únicos em um tenant, sem diferenciar maiúsculas de minúsculas.
2. **Emita uma chave.** Chame `IssueApiKey` com o ID do projeto e uma permissão. A resposta contém o segredo uma única vez. Armazene-o no seu gerenciador de segredos.
3. **Cadastre usuários.** Chame `CreateEndUser` com a chave. Armazene o `id` retornado junto ao seu próprio registro de usuário.
4. **Consulte usuários.** Use `GetEndUser` com o ID interno ou `LookupEndUser` com o ID externo.
5. **Altere ou exclua usuários.** Use `UpdateEndUser` e `DeleteEndUser` com a `version` atual.

Todos os procedimentos estão em `devbase.v1.DevBaseService`. Chame o DevBase a partir de código executado no servidor ou da linha de comando, com qualquer cliente Connect ou JSON simples sobre HTTP. Não o chame de um navegador nem coloque uma chave de projeto no código de frontend: o DevBase rejeita todas as origens de navegador, exceto a do próprio console. Defina `DEVBASE_URL` como a URL base do seu servidor de API do DevBase, por exemplo, `http://127.0.0.1:46317` para um servidor local.

```sh
# Create a project with your Delino login token.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateProject" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"3f1f6d1e-5a0a-4d7b-9a53-0c1d9f7a7d01","tenantId":"<tenant-id>","name":"My app"}'

# Issue a CRUD key for the project. The secret appears only in this response.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/IssueApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"8c2b7a60-91a0-4a53-a9c3-6f7f0b2f1e02","projectId":"<project-id>","permission":"API_KEY_PERMISSION_CRUD"}'

# Register an end user with the project key.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateEndUser" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DEVBASE_KEY" \
  --data '{"idempotencyKey":"d5f0c3a2-2b1e-4f59-8f0a-5b7c9e4d3a03","projectId":"<project-id>","externalId":"user-1001","email":"ada@example.com","metadata":{"plan":"pro"}}'
```

Em JSON, os nomes dos campos usam lowerCamelCase, os valores de enum usam seus nomes completos e números de 64 bits, como `version`, são strings.

## Projetos

- O nome de um projeto tem de 1 a 256 bytes, sem espaço no início ou no final.
- `UpdateProject` altera o nome. `DeleteProject` exclui o projeto, todos os seus usuários e todas as suas chaves em uma única etapa.
- Um tenant pode ter até 100 projetos.

## Chaves de API do projeto

- Um projeto pode ter até 20 chaves ativas. Uma chave permanece ativa até ser revogada ou expirar.
- Defina `expire_time` ao emitir uma chave se ela precisar expirar. Sem esse campo, a chave não expira automaticamente.
- O DevBase armazena apenas um hash do segredo. Se você perder um segredo, revogue essa chave e emita uma nova. Para alternar uma chave, primeiro emita a nova chave e depois revogue a antiga.
- Se a resposta de `IssueApiKey` for perdida, repita a solicitação com a mesma chave de idempotência. A resposta repetida contém o ID e a permissão da chave, mas não o segredo.
- `RevokeApiKey` entra em vigor imediatamente. Uma chave revogada ou expirada retorna `unauthenticated`.

## Usuários finais

| Campo         | Regra                                                                                                                   |
| ------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `external_id` | Obrigatório. De 1 a 256 bytes. Diferencia maiúsculas de minúsculas. Único no projeto. Sem espaço no início ou no final. |
| `name`        | Opcional. Até 256 bytes.                                                                                                |
| `email`       | Opcional. Um endereço simples de até 254 bytes. Vários usuários podem ter o mesmo e-mail.                               |
| `metadata`    | Um objeto JSON de até 16 KiB. Uma solicitação inteira pode ter até 64 KiB.                                              |

- O mesmo ID externo pode existir em projetos diferentes. Um projeto pode ter até 1.000.000 de usuários.
- A criação é explícita. O DevBase não oferece upsert nem pesquisa por e-mail, nome ou metadados.
- `UpdateEndUser` usa `update_mask` com `external_id`, `name`, `email` e `metadata`. Um `name` ou `email` incluído na máscara com um valor vazio é removido. Um `metadata` incluído na máscara substitui o objeto inteiro, e um objeto vazio o remove. Você não pode remover o ID externo. É possível alterá-lo, e o ID interno permanece o mesmo.
- `DeleteEndUser` remove o usuário imediata e permanentemente. Você pode voltar a usar o ID externo imediatamente. O próximo usuário com esse ID recebe um novo ID interno e não tem vínculo com o usuário excluído.
- O DevBase não informa outros serviços sobre exclusões. Exclua os dados dos serviços dependentes no seu próprio aplicativo.

## Listas

`ListProjects`, `ListApiKeys` e `ListEndUsers` retornam itens na ordem de criação. O tamanho padrão da página é 50, e o máximo é 100. Repita a chamada com `next_page_token` até que ele esteja vazio. Trate o token como opaco. As listas não têm uma contagem total. Se você criar ou excluir itens enquanto lê as páginas, uma página pode não incluir ou pode pular esses itens. Um item que exista durante toda a leitura aparece uma vez.

## Novas tentativas e conflitos

- Toda alteração exige uma `idempotency_key` de 8 a 128 caracteres dentre `A-Z`, `a-z`, `0-9`, `.`, `_`, `:` e `-`. Um UUID funciona bem.
- Se você repetir uma solicitação com a mesma chave e o mesmo conteúdo dentro de 24 horas, receberá o primeiro resultado, e o DevBase não fará nenhuma alteração novamente. Use isso para resolver respostas perdidas. Após 24 horas, a chave representa uma nova solicitação.
- A mesma chave com conteúdo diferente retorna `already_exists`. Cada chamador tem suas próprias chaves, portanto outro chamador não pode ver nem obter seu resultado.
- Uma solicitação repetida é verificada novamente em relação às permissões atuais. Seu acesso pode terminar entre a primeira chamada e a repetição.
- Depois que você excluir um usuário, a repetição da criação ou atualização desse usuário retornará `not_found`, e o DevBase não criará o usuário novamente. A repetição da exclusão será bem-sucedida. Depois que você excluir um projeto, as chaves desse projeto deixarão de funcionar.
- `UpdateProject`, `UpdateEndUser`, `DeleteProject` e `DeleteEndUser` exigem a `version` atual. Se alguém tiver alterado o item primeiro, você receberá `aborted`. Leia o item novamente e tente outra vez com a nova versão.

## Limites

| Limite                                   | Padrão                           |
| ---------------------------------------- | -------------------------------- |
| Projetos por tenant                      | 100                              |
| Usuários por projeto                     | 1.000.000                        |
| Chaves ativas por projeto                | 20                               |
| Solicitações de usuários por projeto     | 1.000 por segundo, pico de 2.000 |
| Solicitações de gerenciamento por tenant | 50 por segundo, pico de 100      |

As solicitações de usuários são contabilizadas em conjunto para todas as chaves e tokens de login de um projeto. As solicitações de gerenciamento são contabilizadas em conjunto para um tenant. Uma nova chave não adiciona capacidade. Se você exceder um limite, receberá `resource_exhausted`. Para um limite de taxa, o erro e o cabeçalho `Retry-After` informam quanto tempo esperar. Um operador pode alterar esses limites. Eles protegem o serviço e não garantem velocidade.

## Erros

| Código               | Significado                                                                                          |
| -------------------- | ---------------------------------------------------------------------------------------------------- |
| `unauthenticated`    | O token ou a chave está ausente, é inválido, expirou ou foi revogado.                                |
| `permission_denied`  | Você não tem a função ou a permissão da chave necessária, ou o estado do tenant bloqueia a operação. |
| `not_found`          | O item não existe, pertence a outro projeto ou foi excluído.                                         |
| `invalid_argument`   | Um valor de entrada não é válido.                                                                    |
| `already_exists`     | Um nome ou ID externo já existe, ou uma chave de idempotência foi usada com outro conteúdo.          |
| `aborted`            | A versão está desatualizada.                                                                         |
| `resource_exhausted` | Um limite de taxa ou de recursos foi atingido.                                                       |
| `unavailable`        | Uma dependência ou o banco de dados falhou. Uma alteração pode ou não ter sido aplicada.             |

Toda resposta e todo erro retornado pelo próprio DevBase têm um ID de rastreamento. Inclua-o ao solicitar suporte. Uma solicitação maior que 256 KiB é rejeitada antes, pela camada de transporte, com `resource_exhausted` e sem um ID de rastreamento; em vez disso, reduza o tamanho da solicitação. Se você receber `unavailable` para uma alteração, repita a mesma solicitação com a mesma chave de idempotência para descobrir se ela foi aplicada.

## Estado do tenant

O DevBase é gratuito. Seu plano, saldo e suas faturas nunca o limitam. Somente um bloqueio pelo operador e a exclusão do tenant podem fazer isso.

- Enquanto um tenant estiver bloqueado ou sendo excluído, as chaves de API do projeto não funcionarão. Proprietários e Administradores ainda poderão ler projetos e chaves, revogar chaves e excluir usuários e projetos. Eles não poderão criar nem alterar nada, emitir chaves ou ler usuários.
- Se o DevBase não puder confirmar o estado do tenant, ele negará a solicitação com `unavailable` ou `permission_denied` e não fará suposições.
- Depois que um tenant for excluído permanentemente, o DevBase excluirá automaticamente seus projetos, usuários e chaves.

## O que o DevBase não faz

O DevBase não autentica seus usuários, não cria contas Delino para eles nem atua em nome deles. Ele não compartilha usuários entre projetos, não move projetos entre tenants, não importa nem exporta dados e não pesquisa usuários. Backups, se houver, ficam fora da exclusão imediata que o DevBase aplica aos dados ativos.
