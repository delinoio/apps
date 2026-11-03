# Git Tracker

Acompanhe medições numéricas de commits Git em [git-tracker.delino.io](https://git-tracker.delino.io). O Git Tracker é gratuito. Ele oferece suporte a repositórios GitHub públicos e privados, incluindo pull requests de forks. Use-o para tamanhos de arquivos, tempos, taxas de transferência, contagens e outras medições escalares.

## Conectar um repositório

1. Entre com sua conta Delino e selecione seu espaço pessoal ou da organização.
2. Peça a um proprietário do espaço para conceder a permissão View, Edit ou Manage do Git Tracker por meio dos grupos do espaço. View permite leituras. Edit acrescenta uploads e conclusão. Manage acrescenta conexão, gerenciamento de tokens e exclusão permanente.
3. Instale o Git Tracker GitHub App no repositório.
4. Abra **Gerenciar repositórios**. Insira `owner/repository` e um token de acesso do GitHub para sua conta GitHub vinculada com acesso de administrador ao repositório. O serviço verifica sua autoridade no GitHub e não retém esse token de acesso.
5. Conecte o repositório. Você também precisa da autoridade de Owner/Admin do espaço Delino. Um repositório só pode estar conectado a um espaço por vez. Copie seu ID de conexão no gerenciamento de repositórios para chamadas da Action e da API. Após a desconexão, outro espaço autorizado pode conectá-lo com um novo ID de conexão; o histórico retido pelo espaço anterior permanece privado.

As medições de repositórios públicos podem ser lidas sem fazer login. Medições privadas exigem participação no espaço e permissão de leitura do Git Tracker. O acesso ao espaço se aplica a todos os repositórios conectados nesse espaço.

Desconectar ou remover o App interrompe os uploads e torna privado o histórico retido. Membros autorizados ainda podem lê-lo. Uma mudança confirmada de público para privado torna os dados históricos privados. Se o GitHub não puder confirmar a visibilidade atual, o Git Tracker mantém a última visibilidade confirmada até que a sincronização seja bem-sucedida.

## Defina seu conjunto de dados

Mantenha uma configuração YAML no seu repositório. Os IDs são estáveis; os nomes podem mudar sem perda de histórico. Todas as colunas de uma linha devem ser enviadas juntas.

```yaml
id: file-size
name: JavaScript file sizes
columns:
  - id: original
    name: Original size
    unit: B
    direction: lower
    absoluteThreshold: 1024
  - id: minified
    name: Minified size
    unit: B
    direction: lower
    percentageThreshold: 1
  - id: gzip
    name: Gzipped size
    unit: B
    direction: lower
    absoluteThreshold: 128
    percentageThreshold: 2
```

Envie linhas JSON, por exemplo, uma tabela de tamanhos de arquivos no estilo SWC:

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

A Action preserva a representação decimal do JSON sem conversão para ponto flutuante binário. Clientes que chamam o Connect diretamente enviam valores decimais como strings, como `"9007199254740993.0000000000000001"`. Números negativos e decimais finitos são aceitos. O arredondamento na exibição nunca altera as decisões sobre regressões.

As unidades aceitas são `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`; `ns`, `μs`, `ms`, `s`; `count`, `%`, `unitless`; e `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`. Unidades compatíveis são convertidas antes da comparação. Tipos incompatíveis não são comparáveis. Por exemplo, 1048576 B é exibido como 1 MiB; os valores exatos permanecem visíveis nas tabelas.

As direções são `lower`, `higher` e `neutral`. Colunas neutras nunca produzem regressões. Os limites se aplicam a todas as linhas da coluna. Com limites absolutos e percentuais, exceder qualquer um deles é uma regressão; a igualdade não é. Sem limite, as alterações são exibidas, mas não causam falhas por regressão. Somente alterações na direção de piora são avaliadas.

A variação percentual é `(new - baseline) / abs(baseline) * 100`. Uma referência igual a zero não tem variação percentual. Um limite absoluto ainda pode determinar o resultado; uma regra apenas percentual não emite julgamento. As comparações usam a configuração armazenada da execução de destino, não uma configuração posterior.

## Use a GitHub Action oficial

Fixe `delinoio/apps/actions/git-tracker` em um SHA completo de commit revisado. Substitua os placeholders abaixo pelo ID de conexão do Git Tracker, pela revisão do checkout e pela revisão da Action. Conceda `id-token: write` para OIDC. Um token de upload do repositório é uma alternativa; armazene-o como um segredo do GitHub e passe `token`.

```yaml
permissions:
  contents: read
  id-token: write
jobs:
  measure:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<reviewed-checkout-commit-sha>
      - run: ./measure.sh
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          repository-id: '019b0000-0000-7000-8000-000000000001'
          configuration: measurements.yaml
          measurements: measurements.json
          fail-on-regression: 'false'
  mark-as-done:
    needs: measure
    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          operation: complete
          repository-id: '019b0000-0000-7000-8000-000000000001'
```

Jobs independentes compartilham o ID de execução padrão, `repository_id/run_id/run_attempt`, e podem enviar conjuntos de linhas sem sobreposição. Use um novo ID de execução para reexecuções. O OIDC do Actions exige a identidade padrão; IDs de execução personalizados são destinados a clientes que usam tokens de repositório. Jobs de PR do mesmo repositório podem definir `baseline`. Uploads via OIDC de forks são limitados ao commit de ponta do PR e não podem fornecer substituições da referência nem gravar no histórico de push do repositório de origem.

Cada job de upload avalia apenas as próprias linhas enviadas. Por padrão, as regressões são relatadas sem causar falha na CI. Defina `fail-on-regression: 'true'` para que esse job falhe em caso de regressões. Comparações ausentes não causam falha na CI. O job de conclusão apenas informa que a medição terminou; ele não envia o status de sucesso/falha/cancelamento da CI nem realiza uma verificação agregada de aprovação. Uma conclusão vazia é válida quando todos os jobs foram ignorados.

Erros de rede e falhas transitórias da API são tentados novamente no máximo três vezes. Por padrão, o job falha após as novas tentativas. Defina `warn-on-api-error: 'true'` para emitir um aviso e passar em vez disso. Falhas em comentários de PR apenas emitem avisos; elas não invalidam os dados salvos nem causam, por si só, falha na CI. A Action registra um link de detalhes e grava um Resumo do job.

Um único comentário de PR reflete os resultados cumulativos da execução registrada mais recente no commit mais recente do PR. Execuções anteriores permanecem disponíveis no histórico da web.

## Chame a API Connect

A origem da API é `https://git-tracker.api.delino.io`. As solicitações JSON do Connect usam `Content-Type: application/json` e `Connect-Protocol-Version: 1`. Gerentes podem emitir tokens de repositório com uma expiração futura obrigatória e revogá-los no gerenciamento de repositórios.

```sh
curl --fail-with-body \
  'https://git-tracker.api.delino.io/gittracker.v1.GitTrackerService/Upload' \
  -H "Authorization: Bearer ${GIT_TRACKER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -H 'Connect-Protocol-Version: 1' \
  --data-binary @upload.json
```

`upload.json` contém `context`, `dataset` e `rows`:

```json
{
  "context": {
    "repositoryId": "019b0000-0000-7000-8000-000000000001",
    "executionId": "my-historical-run",
    "commit": "0123456789012345678901234567890123456789",
    "branch": "main"
  },
  "dataset": {
    "id": "size",
    "name": "Size",
    "columns": [{ "id": "bytes", "name": "Bytes", "unit": "B", "direction": "DIRECTION_LOWER", "absoluteThreshold": "1024" }]
  },
  "rows": [{ "id": "bundle", "name": "Bundle", "values": { "bytes": "1048576" } }]
}
```

Chame `/gittracker.v1.GitTrackerService/Complete` com o mesmo `context`. Para uploads de PR, inclua `pullRequest` e use a branch base e o commit de ponta do PR. `baselineCommit` seleciona um SHA completo explícito de referência. Commits históricos usam a mesma API de upload. Não converta strings decimais usando `Number` do JavaScript antes de enviá-las.

## Regras de execução e comparação

O primeiro upload fixa a configuração de cada conjunto de dados para essa execução. Uma configuração posterior diferente é rejeitada. Repetir uma linha existente com valores numéricos idênticos é idempotente. Valores conflitantes são rejeitados. Um lote é atômico: uma linha inválida ou conflitante impede que qualquer dado daquela solicitação seja salvo.

Os dados aparecem imediatamente como **Medindo**. Eles só se tornam representativos após a conclusão. O resultado representativo de um commit é sua execução concluída registrada mais recentemente, incluindo execuções com regressões. Uma execução mais antiga que termina com atraso não substitui uma execução concluída registrada mais recentemente. Repetir a conclusão não altera o horário de conclusão nem a ordenação.

Após seis horas sem novos dados de medição salvos, uma execução inacabada passa a **Interrompida**. Retransmissões idênticas não reiniciam esse temporizador. Execuções concluídas e interrompidas rejeitam uploads adicionais; execuções interrompidas também rejeitam a conclusão. Inicie uma nova execução em vez disso. Os dados permanecem até você excluí-los.

As comparações automáticas de PR fixam o commit de base de mesclagem e sua execução concluída representativa no primeiro upload. Uma referência ausente permanece indisponível mesmo que seja enviada posteriormente. As comparações automáticas de push percorrem a ancestralidade pelo primeiro pai separadamente para cada linha e usam o ancestral medido mais próximo concluído até o momento de corte do primeiro upload. Jobs posteriores usam esse mesmo momento de corte. A tabela identifica cada commit e execução de origem efetivamente usados. Comparações com um SHA de referência explícito não recorrem a ancestrais por linha.

A comparação de dois commits no site usa apenas as execuções concluídas representativas dos commits selecionados. Novas linhas/colunas e dados ausentes ou incompatíveis não são comparáveis e não causam falha por regressão. Dados de referência excluídos não são reconstruídos. Antes da conclusão, uma linha ausente é considerada ainda não coletada; após a conclusão, ela é considerada faltante.

Os gráficos usam commits igualmente espaçados na ordem do primeiro pai. Eles conectam pontos efetivamente medidos através de commits ignorados e nunca propagam nem inventam valores. Filtre por conjunto de dados, branch, coluna, itens, datas e intervalo de commits. Use a tabela de dados para obter valores exatos e links de execução acessíveis por teclado.

## Exclusão e solução de problemas

Gerentes podem excluir permanentemente as medições de um repositório, um conjunto de dados ou uma execução após confirmação. Uma comparação cuja referência foi excluída fica indisponível. Quando o Delcore finaliza a exclusão de um espaço, o Git Tracker exclui as conexões, os tokens, a configuração e o histórico do espaço.

Em caso de erro de permissão, verifique o espaço selecionado e sua função atual no Git Tracker. Em caso de conflitos de upload, confirme a identidade da execução, a configuração do conjunto de dados e os valores completos das linhas. Para execuções interrompidas ou concluídas, use um novo ID de execução. Em caso de erros de rede/servidor, tente novamente e guarde o ID de correlação da resposta ao pedir ajuda. Nunca compartilhe tokens, credenciais ou medições privadas em relatos públicos.

As medições de execuções de PR continuam disponíveis para inspeção, mas não fornecem pontos para os gráficos da branch de origem nem referências automáticas. Isso impede que medições de forks substituam os dados ou a configuração da branch de referência para um SHA de commit compartilhado.
