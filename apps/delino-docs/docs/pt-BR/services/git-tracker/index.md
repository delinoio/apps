# Git Tracker

Acompanhe medições numéricas de commits do Git em [git-tracker.delino.io](https://git-tracker.delino.io). O Git Tracker é gratuito. Ele oferece suporte a repositórios públicos e privados do GitHub, incluindo pull requests de forks. Use-o para tamanhos de arquivos, tempos, taxa de transferência, contagens e outras medições escalares.

## Conecte um repositório

1. Entre com sua conta Delino e selecione seu espaço pessoal ou de organização.
2. Peça a um proprietário do espaço que conceda a permissão View, Edit ou Manage do Git Tracker por meio dos grupos do espaço. View permite leituras. Edit acrescenta uploads e conclusão. Manage acrescenta conexão, gerenciamento de tokens e exclusão permanente.
3. Instale o GitHub App do Git Tracker no repositório.
4. Abra **Gerenciar repositórios**. Insira `owner/repository` e um token de acesso do GitHub para sua conta vinculada do GitHub com acesso de administrador ao repositório. O serviço verifica sua autoridade no GitHub e não retém esse token de acesso.
5. Conecte o repositório. Você também precisa ter autoridade de Owner/Admin no espaço Delino. Um repositório pode estar conectado a apenas um espaço por vez. Copie o ID de conexão dele no gerenciamento de repositórios para chamadas da Action e da API. Após a desconexão, outro espaço autorizado poderá conectá-lo com um novo ID de conexão; o histórico retido pelo espaço antigo permanecerá privado.

As medições de repositórios públicos podem ser lidas sem entrar em uma conta. Medições privadas exigem associação ao espaço e permissão de leitura do Git Tracker. O acesso ao espaço se aplica a todos os repositórios conectados nesse espaço.

Desconectar ou remover o App interrompe os uploads e torna privado o histórico retido. Membros autorizados ainda podem lê-lo. Uma alteração confirmada de público para privado torna privados os dados históricos. Se o GitHub não puder confirmar a visibilidade atual, o Git Tracker manterá a última visibilidade confirmada até que a sincronização seja bem-sucedida.

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

Envie linhas JSON, por exemplo, uma tabela de tamanhos de arquivos no estilo do SWC:

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

A Action preserva a representação textual dos decimais no JSON sem conversão para ponto flutuante binário. Chamadores do Connect direto enviam valores decimais como strings, como `"9007199254740993.0000000000000001"`. Números negativos e decimais finitos são aceitos. O arredondamento de exibição nunca altera as decisões sobre regressões.

As unidades aceitas são `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`; `ns`, `μs`, `ms`, `s`; `count`, `%`, `unitless`; e `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`. Unidades compatíveis são convertidas antes da comparação. Tipos incompatíveis não são comparáveis. Por exemplo, 1048576 B é exibido como 1 MiB; os valores exatos permanecem visíveis nas tabelas.

As direções são `lower`, `higher` e `neutral`. Colunas neutras nunca produzem regressões. Os limites se aplicam a todas as linhas da coluna. Quando há limites absolutos e percentuais, ultrapassar qualquer um deles é uma regressão; igualá-los não é. Sem limite, as alterações são exibidas, mas não causam falhas por regressão. Somente alterações na direção de piora são avaliadas.

A alteração percentual é `(new - baseline) / abs(baseline) * 100`. Uma linha de base igual a zero não tem alteração percentual. Um limite absoluto ainda pode determinar o resultado; uma regra somente percentual adia a avaliação. As comparações usam a configuração armazenada da execução de destino, não uma configuração posterior.

## Use a GitHub Action oficial

Fixe `delinoio/apps/actions/git-tracker` em um SHA de commit completo e revisado. Substitua os espaços reservados abaixo pelo ID de conexão do Git Tracker, pela revisão do checkout e pela revisão da Action. Conceda `id-token: write` para OIDC. Um token de upload do repositório é uma alternativa; armazene-o como um segredo do GitHub e passe `token`.

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

Jobs independentes compartilham o ID de execução padrão, `repository_id/run_id/run_attempt`, e podem enviar linhas distintas. Use um novo ID de execução para novas tentativas. O OIDC de Actions exige a identidade padrão; IDs de execução personalizados destinam-se a chamadores que usam token de repositório. Uploads OIDC de forks são limitados ao head do PR e não podem fornecer substituições de linha de base nem gravar no histórico de push do repositório upstream.

Cada job de upload avalia somente as próprias linhas enviadas. Por padrão, regressões são relatadas sem causar falha na CI. Defina `fail-on-regression: 'true'` para fazer esse job falhar em caso de regressões. Comparações ausentes não causam falha na CI. O job de conclusão apenas informa que a medição foi finalizada; ele não envia o estado de sucesso, falha ou cancelamento da CI nem executa um bloqueio agregado. Uma conclusão vazia é válida quando todos os jobs foram ignorados.

Erros de rede e falhas transitórias da API são repetidos no máximo três vezes. Por padrão, o job falha após essas tentativas. Defina `warn-on-api-error: 'true'` para emitir um aviso e permitir que o job seja aprovado. Falhas em comentários de PR apenas geram avisos; elas não invalidam os dados salvos nem causam falha na CI de forma independente. A Action registra um link de detalhes e gera um Job Summary.

Um comentário no PR reflete os resultados cumulativos da execução registrada mais recente no commit mais recente do PR. Execuções anteriores permanecem disponíveis no histórico da web.

## Chame a API Connect

A origem da API é `https://git-tracker.api.delino.io`. Solicitações JSON do Connect usam `Content-Type: application/json` e `Connect-Protocol-Version: 1`. Gerentes podem emitir tokens de repositório com uma data de expiração futura obrigatória e revogá-los no gerenciamento de repositórios.

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

Chame `/gittracker.v1.GitTrackerService/Complete` com o mesmo `context`. Para uploads de PR, inclua `pullRequest` e use o branch base e o commit head do PR. `baselineCommit` seleciona explicitamente um SHA completo como linha de base. Commits históricos usam a mesma API de upload. Não converta strings decimais por meio de `Number` do JavaScript antes de enviá-las.

## Regras de execução e comparação

O primeiro upload fixa a configuração de cada conjunto de dados para essa execução. Uma configuração diferente enviada posteriormente é rejeitada. Repetir uma linha existente com valores numéricos idênticos é idempotente. Valores conflitantes são rejeitados. Um lote é atômico: uma linha inválida ou conflitante faz com que nada dessa solicitação seja salvo.

Os dados aparecem imediatamente como **Em medição**. Eles só se tornam representativos após a conclusão. O resultado representativo de um commit é sua execução concluída e registrada mais recente, incluindo execuções com regressões. Uma execução anterior que termina com atraso não substitui uma execução concluída mais recente que já tenha sido registrada. Repetir a conclusão não altera nem o horário de conclusão nem a ordenação.

Após seis horas sem novos dados de medição salvos, uma execução não concluída se torna **Interrompida**. Retransmissões idênticas não reiniciam esse temporizador. Execuções concluídas e interrompidas rejeitam uploads adicionais; execuções interrompidas também rejeitam a conclusão. Em vez disso, inicie uma nova execução. Os dados permanecem até que você os exclua.

Comparações automáticas de PR fixam o commit de merge-base e sua execução concluída representativa no primeiro upload. Uma linha de base ausente permanece indisponível mesmo se for enviada depois. Comparações automáticas de push percorrem separadamente a ancestralidade de primeiro pai para cada linha e usam o ancestral medido mais próximo que tenha sido concluído até o momento limite do primeiro upload. Jobs posteriores usam esse mesmo momento limite. A tabela identifica cada commit e execução efetivamente usados como origem. Comparações com um SHA de linha de base explícito não usam fallback de ancestral no nível da linha.

A comparação de dois commits do site usa apenas as execuções concluídas representativas dos commits selecionados. Novas linhas ou colunas e dados ausentes ou incompatíveis não são comparáveis e não causam falha por regressão. Dados excluídos da linha de base não são reconstruídos. Antes da conclusão, uma linha ausente não foi coletada; após a conclusão, ela está ausente.

Os gráficos usam commits igualmente espaçados na ordem do primeiro pai. Eles conectam pontos reais medidos entre commits ignorados e nunca propagam nem inventam valores. Filtre por conjunto de dados, branch, coluna, itens, datas e intervalo de commits. Use a tabela de dados para ver valores exatos e links de execução acessíveis pelo teclado.

## Exclusão e solução de problemas

Gerentes podem excluir permanentemente, após confirmação, as medições de um repositório, um conjunto de dados ou uma execução. Uma comparação cuja linha de base foi excluída fica indisponível. Quando o Delcore finaliza a exclusão de um espaço, o Git Tracker exclui as conexões, os tokens, a configuração e o histórico do espaço.

Em caso de erro de permissão, verifique o espaço selecionado e sua função atual no Git Tracker. Em caso de conflitos de upload, confirme a identidade da execução, a configuração do conjunto de dados e os valores completos das linhas. Para execuções interrompidas ou concluídas, use um novo ID de execução. Em caso de erros de rede ou do servidor, tente novamente e guarde o ID de correlação da resposta ao pedir ajuda. Nunca compartilhe tokens, credenciais ou medições privadas em relatórios públicos.

As medições de execuções de PR permanecem disponíveis para inspeção, mas não fornecem pontos para o gráfico do branch upstream nem linhas de base automáticas. Isso impede que medições de forks substituam dados do branch de linha de base ou a configuração de um SHA de commit compartilhado.
