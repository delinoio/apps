# Cobrança do DeliTools

O DeliTools cobra do seu tenant pelos tokens de IA que suas solicitações usam. O Delcore calcula o valor do uso e mantém seu saldo, créditos e faturas. Esta página explica como o uso se torna uma cobrança.

## O que é medido

- O DeliTools lê os totais de **tokens de entrada** e **tokens de saída** que o provedor de IA informa para cada tentativa. Ele usa esses totais exatamente como são informados.
- Os totais informados já incluem quaisquer tokens em cache ou de raciocínio. O DeliTools não adiciona essas partes novamente, não oferece desconto separado por cache e não acrescenta sobretaxa por raciocínio.
- O DeliTools informa o uso de entrada e de saída ao Delcore separadamente, e o Delcore calcula o valor de cada um com seus respectivos preços.

## Preços

- O operador do serviço define no Delcore um preço por direção dos tokens (entrada e saída) para cada modelo. Modelos diferentes podem ter preços diferentes.
- Tradução e extração compartilham os mesmos preços. A extração sempre usa `openai/gpt-6-luna`, portanto sua cobrança usa os preços de entrada e de saída desse modelo.
- Um modelo sem ambos os preços cadastrados não pode ser executado. A solicitação falha com `ERROR_REASON_MODEL_UNAVAILABLE` e nenhum outro modelo é usado em seu lugar.
- Uma alteração de preço se aplica às execuções que começam após a alteração. O uso já iniciado mantém o preço vigente no início.
- Seu plano determina o uso incluído, seu saldo e se você pode continuar depois que o saldo se esgotar. Consulte seu plano e suas faturas no Delcore.

## O que é cobrado

- **Toda tentativa com uso conhecido é cobrada.** Isso inclui uma tentativa cujo resultado é inválido, cuja extração falha, cuja resposta chegou antes de você cancelar ou cujo resultado nunca chega até você.
- **Novas tentativas são tentativas separadas.** Uma falha transitória do provedor (um erro de rede ou uma resposta HTTP 5xx ou 429) não retorna informações de uso, então o DeliTools tenta novamente sem cobrar pela tentativa que falhou. Uma resposta concluída do provedor com uso conhecido encerra o ciclo de tentativas e é cobrada, mesmo quando sua saída é inválida, porque o DeliTools nunca tenta novamente para corrigir uma saída inválida. Uma resposta sem informações de uso não custa nada.
- **Uso desconhecido não é cobrado.** Se o provedor não informar totais de tokens utilizáveis, o DeliTools não estima uma cobrança nem consulta o uso posteriormente.
- **Chamadas interrompidas não são cobradas.** Se você cancelar, ou se o tempo limite de execução se esgotar, enquanto a chamada ao provedor ainda estiver em andamento, nenhum relatório de uso chega e nada é cobrado, mesmo que o provedor já tenha começado o trabalho. Esta é uma limitação conhecida: o DeliTools não consulta esse uso posteriormente.
- **O uso precisa ser registrado.** O DeliTools armazena o uso de cada tentativa antes de responder. Se não conseguir armazená-lo, a solicitação falha com `ERROR_REASON_DEPENDENCY_UNAVAILABLE` e você não recebe nenhum resultado.
- **Solicitações rejeitadas não são cobradas.** Solicitações que falham na validação, autenticação ou autorização, ou que atingem um limite ou usam um esquema não compatível, nunca chegam ao provedor de IA.
- **Toda chamada é nova.** O DeliTools não tem chave de idempotência nem reenvio de resultados anteriores. Se você enviar a mesma solicitação novamente, ela será executada novamente e cobrada novamente.

## Quando as cobranças aparecem

O DeliTools salva o uso de cada tentativa antes de responder a você e depois o informa ao Delcore. O envio das informações de uso é separado da sua solicitação, portanto uma cobrança pode aparecer no Delcore pouco depois da chamada. Se o Delcore estiver indisponível, o DeliTools continua tentando com os mesmos identificadores, para que as novas tentativas não criem cobranças duplicadas.

Os registros de uso que o DeliTools não consegue entregar são excluídos 30 dias após sua criação, e as novas tentativas não estendem esse período. O uso que não tiver sido entregue nesse prazo não é cobrado. O DeliTools não o estima.

Você pode consultar seu uso e suas cobranças no Delcore. As respostas do DeliTools não os incluem.

## Verificações de saldo e gastos excedentes

O Delcore verifica sua autorização e situação de cobrança antes de cada solicitação e novamente antes de cada nova tentativa. Ele não reserva dinheiro nem tokens para a chamada. O custo só é conhecido após o término da chamada. Por isso, várias solicitações executadas ao mesmo tempo podem consumir mais do que seu saldo restante ou uso incluído. Quando o saldo ou um limite se esgota, o Delcore bloqueia as solicitações posteriores com `ERROR_REASON_BILLING_BLOCKED`.

Se não for possível acessar o Delcore, o DeliTools recusa novas solicitações pagas em vez de executá-las.

## Solicitações bloqueadas

`ERROR_REASON_BILLING_BLOCKED` significa que o Delcore bloqueou o uso pago para seu tenant. As causas comuns são saldo esgotado, falha na renovação ou período pago expirado, contestação em aberto, bloqueio pelo operador ou um tenant em processo de exclusão. Resolva a causa no Delcore. Por exemplo, adicione saldo ou renove sua assinatura. O gerenciamento do tenant e a regularização de pagamentos continuam disponíveis enquanto o uso pago está bloqueado.
