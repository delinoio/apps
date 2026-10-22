# DeliTools

O DeliTools oferece ferramentas de IA para desenvolvedores por meio de uma API Connect. A versão 1 tem dois métodos síncronos: **tradução** de uma matriz de textos e **extração de dados** de um texto para JSON que segue o seu esquema. Cada chamada retorna o resultado final ou um erro na mesma resposta. Não há tarefas para consultar e nenhum resultado para buscar depois. Todos os métodos estão em `delitools.v1.DeliToolsService`.

Chame o DeliTools a partir de código executado no servidor ou da linha de comando, com qualquer cliente Connect ou JSON simples sobre HTTP. Não coloque uma chave de API em código de frontend. Defina `DELITOOLS_URL` como a URL base do seu servidor de API do DeliTools.

## Início rápido

1. **Escolha uma credencial.** Use seu token de login da Delino ou crie uma chave de API com `CreateKey`. Consulte a seção Autenticação e chaves de API.
2. **Encontre o ID do seu tenant.** O uso é cobrado do tenant Delino que você informar em `tenantId`.
3. **Chame um método** com `Authorization: Bearer <credential>`.

```sh
# Translate two texts into Korean. Detect the input language.
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/TranslateTextArray" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","outputLanguage":"ko","model":"<allowed-model>","texts":["Good morning","Where is the station?"]}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000001",
  "texts": ["좋은 아침이에요", "역이 어디예요?"]
}
```

Em JSON, os nomes dos campos usam lowerCamelCase e os valores de enum usam seus nomes completos.

## Traduzir texto

`TranslateTextArray` traduz cada string de uma matriz.

| Campo            | Obrigatório | Significado                                                                                           |
| ---------------- | ----------- | ----------------------------------------------------------------------------------------------------- |
| `tenantId`       | Sim         | O tenant que recebe a cobrança de uso. Use o UUID versão 7 canônico dele: em minúsculas e com hifens. |
| `outputLanguage` | Sim         | Tag de idioma BCP 47 da tradução, por exemplo `ko`, `pt-BR` ou `zh-Hans`.                             |
| `inputLanguage`  | Não         | Tag de idioma BCP 47 da origem. Omita-a para detectar um único idioma para a matriz.                  |
| `model`          | Sim         | Um modelo da lista de permissões do operador.                                                         |
| `texts`          | Sim         | As strings a traduzir.                                                                                |
| `idempotencyKey` | Não         | Consulte Idempotência.                                                                                |

A resposta contém `requestId` e `texts`, as traduções na ordem original e com o comprimento original.

Regras:

- Os valores de idioma são tags BCP 47. `outputLanguage` é obrigatório. Uma tag que não seja BCP 47 válida, que não seja ASCII ou que tenha mais de 64 bytes retorna um erro de entrada inválida.
- `texts` não pode estar vazio, e nenhuma string pode estar vazia ou conter apenas espaços em branco.
- Se você definir `inputLanguage` e ele for igual a `outputLanguage`, a solicitação é inválida.
- Se você omitir `inputLanguage`, o DeliTools detecta um único idioma de entrada para toda a matriz. Uma matriz com vários idiomas ainda é traduzida. Se o idioma detectado for igual a `outputLanguage`, o modelo ainda processa cada item. O DeliTools não retorna a matriz original sem processamento.
- Somente os modelos da lista de permissões do operador são aceitos. Outros modelos retornam um erro de modelo inválido. Um modelo que existe no catálogo do Vercel AI Gateway não é permitido automaticamente.
- A chamada só é bem-sucedida se todos os itens forem bem-sucedidos. Um item com falha faz toda a solicitação falhar. O DeliTools nunca retorna traduções parciais e nunca substitui o texto de origem por um item com falha.

## Extrair dados

`ExtractDataFromText` lê um texto e retorna JSON que segue o seu esquema JSON. O serviço usa um modelo fixo, `gpt-6-luna`. Você não escolhe o modelo.

| Campo            | Obrigatório | Significado                                                                                           |
| ---------------- | ----------- | ----------------------------------------------------------------------------------------------------- |
| `tenantId`       | Sim         | O tenant que recebe a cobrança de uso. Use o UUID versão 7 canônico dele: em minúsculas e com hifens. |
| `text`           | Sim         | O texto de origem. Ele não pode estar vazio nem conter apenas espaços em branco.                      |
| `jsonSchema`     | Sim         | Um esquema JSON, como string, que usa o subconjunto compatível.                                       |
| `idempotencyKey` | Não         | Consulte Idempotência.                                                                                |

A resposta contém `requestId` e `jsonData`, uma string JSON que o DeliTools validou em relação ao seu esquema.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/ExtractDataFromText" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","text":"Ada Lovelace lives in London.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"},\"city\":{\"type\":\"string\"}},\"required\":[\"name\",\"city\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000002",
  "jsonData": "{\"name\":\"Ada Lovelace\",\"city\":\"London\"}"
}
```

Como a extração funciona:

- O modelo é instruído a usar somente as informações que o texto declara. Se o texto não tiver informações exigidas pelo esquema, a chamada falha com um erro de extração impossível. O serviço não inventa um valor para satisfazer o esquema.
- O DeliTools verifica a saída do modelo em relação ao seu esquema no servidor. Uma saída malformada ou não conforme é uma falha com um erro de saída inválida. Ela nunca é retornada como resultado, e o DeliTools não a repete nem a corrige.
- Uma verificação de esquema e um prompt não garantem que todos os valores estejam semanticamente corretos. Revise os dados extraídos antes de confiar neles. O DeliTools não retorna evidências para cada valor.
- O DeliTools verifica o esquema antes de chamar qualquer modelo de IA. Um esquema fora do subconjunto compatível retorna um erro de esquema inválido, sem chamada de IA e sem cobrança.

### Subconjunto de esquema

O DeliTools aceita apenas a parte do JSON Schema que a saída estruturada do modelo de extração suporta.

Compatível:

- Um objeto na raiz.
- Os tipos `object`, `array`, `string`, `number`, `integer`, `boolean` e `null`, aninhados até uma profundidade de 8 níveis.
- `properties`, com todas as propriedades listadas em `required`.
- `additionalProperties` definido como `false` em todos os objetos.
- `items` para matrizes.
- `description` em qualquer esquema.
- `enum` com valores escalares.

Não compatível e rejeitado antes da execução:

- Qualquer palavra-chave fora da lista acima, por exemplo `pattern`, `minimum`, `format` ou `oneOf`.
- Propriedades opcionais. Toda propriedade de um objeto deve estar em `required`.
- `additionalProperties` definido como `true` ou ausente.
- Referências: `$ref` e esquemas recursivos.
- Esquemas acima dos limites do provedor: 5.000 propriedades, 1.000 valores de enum ou 120.000 caracteres de nomes de propriedades e valores de enum juntos. Um enum com mais de 250 valores pode conter no máximo 15.000 caracteres de valores de string.

Um esquema compatível:

```json
{
  "type": "object",
  "properties": {
    "invoiceNumber": { "type": "string", "description": "The invoice number" },
    "total": { "type": "number" },
    "status": { "type": "string", "enum": ["paid", "unpaid"] },
    "lines": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": { "item": { "type": "string" }, "quantity": { "type": "integer" } },
        "required": ["item", "quantity"],
        "additionalProperties": false
      }
    }
  },
  "required": ["invoiceNumber", "total", "status", "lines"],
  "additionalProperties": false
}
```

Um esquema não compatível, porque `note` é opcional e `pattern` não é compatível:

```json
{
  "type": "object",
  "properties": {
    "code": { "type": "string", "pattern": "^[A-Z]{3}$" },
    "note": { "type": "string" }
  },
  "required": ["code"],
  "additionalProperties": false
}
```

## Comportamento das solicitações

- Ambos os métodos são síncronos. Mantenha a conexão aberta até receber um resultado ou um erro.
- Todo sucesso e todo erro contém um ID de solicitação, um UUID versão 7. Inclua-o ao solicitar suporte. As respostas de sucesso contêm apenas o resultado e o ID da solicitação. Elas não contêm o modelo, a contagem de tokens nem a cobrança. Consulte Faturamento para inspecionar o uso.
- O DeliTools repete erros transitórios do provedor de IA, até um limite definido pelo operador. Ele não repete uma saída malformada e nunca troca de modelo por conta própria.
- Se você cancelar a chamada ou se desconectar, o DeliTools tenta cancelar a chamada de IA e interrompe novas tentativas. O uso que já ocorreu continua sendo medido e faturado.
- O operador define os limites de tamanho da entrada, de quantidade de itens da tradução, de tempo limite de execução, de taxa de solicitações por tenant e de solicitações simultâneas por tenant. Essas são configurações da implantação, não limites fixos do produto. Uma solicitação acima de um limite retorna um erro de limite.

## Autenticação e chaves de API

Envie `Authorization: Bearer <credential>` em cada solicitação. Dois tipos de credencial funcionam para ambas as ferramentas:

- **Um token de login da Delino** (um token de acesso de usuário do Logto) atua como o usuário conectado.
- **Uma chave de API do DeliTools** atua conforme o tipo da chave descreve.

Toda solicitação também é verificada em relação ao seu tenant no Delcore. O tenant deve estar ativo e autorizado a executar trabalho pago, e o faturamento deve permiti-lo. Se o DeliTools não puder concluir essas verificações, ele rejeita novas execuções pagas.

### Tipos de chave e capacidades

- Uma **chave pessoal** pertence a um usuário. Suas permissões são a interseção entre suas capacidades e as permissões atuais desse usuário. Quando o usuário perde o acesso ao tenant, a chave deixa de funcionar.
- Uma **chave de serviço** pertence ao tenant. Ela não depende da participação do usuário que a criou. Somente um Proprietário ou Administrador do tenant pode criar e gerenciar chaves de serviço.
- Cada chave tem uma ou ambas as capacidades: `CAPABILITY_TRANSLATOR` e `CAPABILITY_EXTRACTOR`. Uma chave pode chamar somente as ferramentas de suas capacidades.
- Os usuários gerenciam suas próprias chaves pessoais. O gerenciamento de chaves exige um token de login da Delino. Uma chave de API não pode criar nem revogar chaves.

### Gerenciar chaves

- `CreateKey`: envie `tenantId`, `kind` (`KEY_KIND_PERSONAL` ou `KEY_KIND_SERVICE`), `capabilities` e um `expiresAt` opcional.
- `ListKeys`: lista as chaves de um tenant, filtradas por `kind`, com `afterId` e `pageSize` para paginação.
- `RevokeKey`: envie `tenantId` e `keyId`.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/CreateKey" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"tenantId":"<tenant-id>","kind":"KEY_KIND_SERVICE","capabilities":["CAPABILITY_TRANSLATOR"]}'
```

- **Segredo de uso único.** A resposta de `CreateKey` contém o `secret` uma única vez. O DeliTools armazena apenas um hash de verificação e não pode mostrar o segredo novamente. Se você perder uma chave, crie uma nova. Mantenha o segredo em um gerenciador de segredos.
- **Expiração.** `expiresAt` é opcional. Uma chave sem expiração permanece válida até você revogá-la. Uma chave pessoal também exige que o proprietário dela mantenha o acesso ao tenant. Uma chave de serviço não depende do usuário que a criou, portanto remover esse usuário não a interrompe: revogue a chave de serviço ou defina uma expiração para ela.
- **Revogação.** Depois que você revoga uma chave, ou depois que o usuário de uma chave pessoal ou de um token de login perde a permissão, novas solicitações são rejeitadas e uma chamada em andamento não faz mais tentativas de IA. Uma chamada que já está em andamento pode terminar. Seu uso é faturado.

## Faturamento

O DeliTools informa o uso ao Delcore. O Delcore o avalia e o liquida em USD.

- **Base.** A base são os preços públicos de tokens do modelo no catálogo de modelos do Vercel AI Gateway, multiplicados pelos tokens que a chamada efetivamente mediu. Descontos do Gateway e créditos promocionais não reduzem a base.
- **Margem.** A tradução custa a base multiplicada por 1.15. A extração custa a base multiplicada por 1.30. Os valores usam decimais exatos e as regras de arredondamento do Delcore.
- **Alterações de preço.** O DeliTools atualiza automaticamente os preços do catálogo. Um novo preço se aplica a todos os clientes nas solicitações iniciadas depois que ele entra em vigor. Uma solicitação mantém o preço do seu início em todas as suas novas tentativas. Se o DeliTools não tiver um preço para um modelo, ele rejeita novas execuções de IA.
- **Falhas são faturáveis.** Os tokens usados por novas tentativas, chamadas com falha, tempos limite, cancelamentos e resultados que você não conseguiu receber continuam sendo contados, junto com a margem.
- **Uso desconhecido.** Se o DeliTools não conseguir ler o uso de uma chamada imediatamente, ele adia a cobrança e a reconcilia com o Gateway depois. Ele nunca estima o uso para uma cobrança. Se o uso ainda for desconhecido quando o registro expirar após 30 dias, essa cobrança é descartada.
- **Onde ver o uso.** As respostas não contêm uso. Use a interface de faturamento da Delino para o seu tenant para inspecionar o uso e as cobranças.
- **Tenants bloqueados.** Se o faturamento bloquear o seu tenant, novas execuções falham com um erro de faturamento bloqueado. Uma chamada que já está em andamento não é interrompida. Como o DeliTools mede o uso após a execução, chamadas simultâneas podem exceder um saldo ou limite antes de o bloqueio ser aplicado.

## Idempotência

`idempotencyKey` é opcional. Se você a enviar, ela deve ter de 1 a 256 caracteres ASCII visíveis (sem espaços nem caracteres de controle); qualquer outra chave retorna um erro de entrada inválida. Sem ela, cada chamada é uma nova execução e pode gerar uma nova cobrança, inclusive quando você repete após uma falha de rede.

Com uma chave, o DeliTools rastreia a solicitação por **tenant, método e chave**. Isso vale para diferentes credenciais do mesmo tenant. O DeliTools verifica sua autorização atual antes de informar o estado de uma solicitação.

| Situação                                            | Resultado                                                                                    |
| --------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Chave nova                                          | A chamada é executada normalmente.                                                           |
| Mesma chave e mesma entrada, ainda em execução      | Um erro de processamento com o ID de solicitação da primeira chamada. Ele não espera.        |
| Mesma chave e mesma entrada, concluída              | Um erro de já processado. O resultado não é reproduzido.                                     |
| Mesma chave e entrada diferente                     | Um erro de conflito de entrada.                                                              |
| A primeira chamada terminou em estado desconhecido  | Um erro de resultado desconhecido. O DeliTools não repete a chamada de IA por conta própria. |
| Mesma chave após a expiração do registro de 30 dias | Uma nova chamada, cobrada normalmente.                                                       |

O DeliTools mantém a chave, um hash da entrada e o estado. Ele não mantém a entrada nem o resultado. A deduplicação vale apenas por 30 dias após a primeira chamada. Salve o resultado ao recebê-lo: uma chamada repetida com a mesma chave não pode retorná-lo novamente.

## Erros

Todo erro tem um código Connect, um motivo estável, um ID de solicitação e uma mensagem em inglês. O motivo e o ID da solicitação estão no detalhe de erro `delitools.v1.ErrorDetail`. Use o motivo, não a mensagem, no seu código.

| Motivo                               | Código Connect        | Significado                                                                                                                        |
| ------------------------------------ | --------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`         | `invalid_argument`    | A solicitação é inválida, por exemplo uma matriz vazia ou idiomas idênticos.                                                       |
| `ERROR_REASON_INVALID_SCHEMA`        | `invalid_argument`    | O esquema está fora do subconjunto compatível.                                                                                     |
| `ERROR_REASON_INVALID_MODEL`         | `invalid_argument`    | O modelo não está na lista de permissões.                                                                                          |
| `ERROR_REASON_UNAUTHENTICATED`       | `unauthenticated`     | A credencial está ausente, é inválida, expirou ou foi revogada.                                                                    |
| `ERROR_REASON_FORBIDDEN`             | `permission_denied`   | A credencial ou o usuário não pode usar esta ferramenta neste tenant.                                                              |
| `ERROR_REASON_BILLING_BLOCKED`       | `failed_precondition` | O faturamento não permite novas execuções pagas para o tenant.                                                                     |
| `ERROR_REASON_PRICE_UNAVAILABLE`     | `failed_precondition` | Não há preço público disponível para o modelo, então o DeliTools não o executa.                                                    |
| `ERROR_REASON_LIMIT`                 | `resource_exhausted`  | Um limite configurado foi excedido, como tamanho da entrada, itens, taxa ou simultaneidade.                                        |
| `ERROR_REASON_PROCESSING`            | `aborted`             | Uma solicitação com a mesma chave de idempotência ainda está em execução.                                                          |
| `ERROR_REASON_ALREADY_PROCESSED`     | `already_exists`      | Uma solicitação com a mesma chave de idempotência e a mesma entrada já foi concluída.                                              |
| `ERROR_REASON_INPUT_CONFLICT`        | `failed_precondition` | A chave de idempotência foi usada com uma entrada diferente.                                                                       |
| `ERROR_REASON_UNKNOWN_OUTCOME`       | `unknown`             | O resultado da primeira execução é desconhecido.                                                                                   |
| `ERROR_REASON_EXTRACTION_IMPOSSIBLE` | `failed_precondition` | O texto não contém informações que o esquema exige.                                                                                |
| `ERROR_REASON_INVALID_OUTPUT`        | `data_loss`           | O modelo retornou uma saída malformada ou que não corresponde ao esquema.                                                          |
| `ERROR_REASON_PROVIDER`              | `unavailable`         | O provedor de IA falhou após as novas tentativas permitidas.                                                                       |
| `ERROR_REASON_TIMEOUT`               | `deadline_exceeded`   | O tempo limite de execução passou.                                                                                                 |
| `ERROR_REASON_CANCELED`              | `canceled`            | A chamada foi cancelada.                                                                                                           |
| `ERROR_REASON_INTERNAL`              | `internal`            | Uma falha interna, por exemplo no armazenamento.                                                                                   |
| `ERROR_REASON_INTERNAL`              | `unavailable`         | Não foi possível concluir a verificação de autorização ou de faturamento do tenant, por isso novos trabalhos pagos são rejeitados. |

Tratamento:

- Corrija a solicitação nos erros `invalid_argument`, `permission_denied`, `unauthenticated` e `failed_precondition` antes de tentar novamente. Resolva o bloqueio de faturamento em um erro de faturamento bloqueado.
- Um erro de provedor, de tempo limite ou interno pode ser repetido mais tarde. Falhas de provedor e de tempo limite ainda podem ser faturadas. Use uma chave de idempotência para evitar uma segunda execução quando a primeira pode ter sido bem-sucedida. Um erro `unavailable` com `ERROR_REASON_INTERNAL` significa que o DeliTools não conseguiu concluir a verificação de autorização ou de faturamento. Trate o código e o motivo em conjunto, porque `ERROR_REASON_INTERNAL` também aparece com `internal`.
- Os erros de resultado desconhecido, de processamento e de já processado contêm o ID de solicitação da primeira chamada. Entre em contato com o suporte com esse ID se o resultado continuar desconhecido.

## Retenção de dados

O DeliTools nunca armazena seu texto de entrada, seu esquema nem os resultados. Ele também os mantém fora de logs, rastreamentos e eventos de auditoria. Credenciais e segredos são excluídos da telemetria.

O DeliTools armazena apenas os dados de que precisa para executar o serviço e faturá-lo: hashes de verificação de chaves de API, quem e qual tenant são donos de uma solicitação, hashes de entrada para idempotência, estados das solicitações, identificadores de tentativas, uso de tokens medido, instantâneos de preços e registros seguros de auditoria e operação.

| Dados                                                    | Mantidos por                               |
| -------------------------------------------------------- | ------------------------------------------ |
| Registros de idempotência                                | 30 dias a partir da primeira criação       |
| Registros de entrega e reconciliação de uso              | 30 dias a partir da primeira criação       |
| Registros de auditoria da plataforma e logs operacionais | 30 dias a partir da criação                |
| Chaves de API                                            | Não são excluídas pela retenção de 30 dias |
| Lançamentos contábeis aceitos no Delcore                 | Conforme as regras de retenção do Delcore  |

A retenção de 30 dias não se aplica às chaves de API. A limpeza de retenção nunca as exclui. A expiração e a revogação apenas impedem que uma chave seja autenticada, portanto o hash de verificação e a atribuição dela continuam armazenados.

Os 30 dias começam quando o registro é criado pela primeira vez. Novas tentativas e mudanças de estado não os prorrogam. O DeliTools exclui um registro quando ele expira em qualquer estado, inclusive solicitações que não terminaram e uso que ainda não foi entregue. Você aceita dois riscos:

- Depois que um registro de idempotência expira, uma chamada repetida com a mesma chave é executada e cobrada novamente.
- O uso que ainda está pendente quando seu registro expira é perdido, e o DeliTools não o cobra.

## Tratamento de dados por provedores externos

Para fazer seu trabalho, o DeliTools envia seu texto, e na extração seu esquema, ao Vercel AI Gateway e, a partir dele, ao provedor do modelo que executa a sua solicitação. Esses serviços tratam os dados conforme suas próprias políticas. Leia-as antes de enviar dados ao DeliTools:

- [Vercel AI Gateway zero data retention](https://vercel.com/docs/ai-gateway/security-and-compliance/zdr)
- [Vercel AI Gateway models](https://vercel.com/ai-gateway/models): a página de cada modelo lista seus provedores.
- A política de dados do provedor do modelo que você usa.

O DeliTools não promete retenção zero de dados em nenhum provedor. Ele não detecta dados sensíveis e não os remove da sua entrada. Não envie dados que você não possa compartilhar com esses provedores.

## Limites

O operador do servidor de API do DeliTools define os limites de tamanho da entrada, de número de itens em uma tradução, de tempo limite de execução, de solicitações por minuto por tenant, de solicitações simultâneas por tenant, de número de novas tentativas de IA e a lista de permissões de modelos de tradução. Essas são configurações da implantação e podem mudar. Elas não são limites fixos do produto, e o DeliTools não oferece nenhuma garantia de taxa de transferência ou disponibilidade. Uma solicitação que excede um limite retorna `ERROR_REASON_LIMIT` ou, para um modelo inválido, `ERROR_REASON_INVALID_MODEL`. Pergunte ao operador da sua implantação quais são os valores atuais.
