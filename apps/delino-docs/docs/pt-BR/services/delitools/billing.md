# Cobrança do DeliTools

O DeliTools cobra do seu tenant pelos tokens de IA que suas solicitações usam. O Delcore calcula o valor do uso e mantém seu saldo, créditos e faturas. Esta página explica como o uso se transforma em cobrança.

## O que é medido

- O DeliTools lê os totais de **tokens de entrada** e **tokens de saída** que o provedor de IA informa para cada tentativa. Ele usa esses totais conforme são informados.
- Os totais informados já incluem quaisquer tokens em cache ou de raciocínio. O DeliTools não adiciona essas partes novamente, não oferece desconto separado para cache e não acrescenta sobretaxa por raciocínio.
- O DeliTools informa o uso de entrada e saída ao Delcore separadamente, e o Delcore calcula o valor de cada um com seus respectivos preços.

## Preços

- O operador do serviço define no Delcore um preço por direção de token (entrada e saída) para cada modelo. Modelos diferentes podem ter preços diferentes.
- Tradução e extração compartilham os mesmos preços. A extração sempre usa `openai/gpt-6-luna`, portanto sua cobrança usa os preços de entrada e saída desse modelo.
- Um modelo sem ambos os preços cadastrados não pode ser executado. A solicitação falha com `ERROR_REASON_MODEL_UNAVAILABLE` e nenhum outro modelo é usado em seu lugar.
- Uma alteração de preço se aplica às execuções que começam após a alteração. O uso que já começou mantém o preço vigente no início.
- Seu plano determina o uso incluído, seu saldo e se você pode continuar depois que o saldo se esgotar. Consulte seu plano e suas faturas no Delcore.

## O que é cobrado

- **Toda tentativa com uso conhecido é cobrada.** Isso inclui uma tentativa cujo resultado seja inválido, cuja extração falhe, cuja resposta tenha chegado antes de você cancelar ou cujo resultado nunca chegue até você.
- **Novas tentativas são tentativas separadas.** Se o DeliTools tentar novamente após uma falha transitória do provedor e a tentativa que falhou tiver informado uso, ambas as tentativas serão cobradas. Uma tentativa que falhou e não informou uso não tem custo.
- **Uso desconhecido não é cobrado.** Se o provedor não informar totais de tokens utilizáveis, o DeliTools não estima uma cobrança nem consulta o uso posteriormente.
- **Chamadas abortadas não são cobradas.** Se você cancelar, ou se o tempo limite da execução se esgotar, enquanto a chamada ao provedor ainda estiver em andamento, nenhum relatório de uso será recebido e nada será cobrado, mesmo que o provedor já tenha começado o trabalho. Esta é uma limitação conhecida: o DeliTools não consulta esse uso posteriormente.
- **O uso precisa ser registrado.** O DeliTools armazena o uso de cada tentativa antes de responder. Se não conseguir armazená-lo, a solicitação falhará com `ERROR_REASON_DEPENDENCY_UNAVAILABLE` e você não receberá nenhum resultado.
- **Solicitações rejeitadas não são cobradas.** Solicitações que falham na validação, autenticação ou autorização, ou que atingem um limite ou usam um esquema não compatível, nunca chegam ao provedor de IA.
- **Toda chamada é nova.** O DeliTools não tem chave de idempotência nem reenvio de resultados anteriores. Se você enviar a mesma solicitação novamente, ela será executada novamente e cobrada novamente.

## Quando as cobranças aparecem

O DeliTools salva o uso de cada tentativa antes de responder a você e, em seguida, o informa ao Delcore. O envio das informações de uso é separado da sua solicitação, portanto uma cobrança pode aparecer no Delcore pouco depois da chamada. Se o Delcore estiver indisponível, o DeliTools continuará tentando com os mesmos identificadores, para que as novas tentativas não criem cobranças duplicadas.

Os registros de uso que o DeliTools não conseguir entregar serão excluídos 30 dias após sua criação, e novas tentativas não estendem esse período. O uso que nunca foi entregue nesse prazo não é cobrado. O DeliTools não faz estimativas desse uso.

Você pode consultar seu uso e suas cobranças no Delcore. As respostas do DeliTools não os incluem.

## Verificações de saldo e gastos excedentes

O Delcore verifica sua autorização e seu status de cobrança antes de cada solicitação e novamente antes de cada nova tentativa. Ele não reserva dinheiro nem tokens para a chamada. O custo só é conhecido depois que a chamada termina. Por isso, várias solicitações executadas ao mesmo tempo podem consumir mais do que seu saldo restante ou uso incluído. Quando o saldo ou um limite se esgota, o Delcore bloqueia solicitações posteriores com `ERROR_REASON_BILLING_BLOCKED`.

Se não for possível acessar o Delcore, o DeliTools recusa novas solicitações pagas em vez de executá-las.

## Solicitações bloqueadas

`ERROR_REASON_BILLING_BLOCKED` significa que o Delcore bloqueou o uso pago para seu tenant. As causas comuns são saldo esgotado, falha na renovação ou período pago expirado, contestação em aberto, bloqueio pelo operador ou um tenant que está sendo excluído. Resolva a causa no Delcore. Por exemplo, recarregue seu saldo ou renove sua assinatura. O gerenciamento do tenant e a regularização de pagamentos continuam disponíveis enquanto o uso pago estiver bloqueado.
