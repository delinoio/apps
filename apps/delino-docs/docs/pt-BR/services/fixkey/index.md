# FixKey

O FixKey corrige palavras digitadas com o layout de teclado errado. Ele oferece suporte ao Dubeolsik coreano e ao QWERTY de inglês dos EUA em ambas as direções. O FixKey é executado localmente no seu computador e não exige uma conta.

## Instalar o FixKey

Abra [versões do FixKey](https://github.com/delinoio/apps/releases?q=fixkey%40) e escolha o pacote correspondente ao seu sistema operacional e processador:

- macOS ARM64: DMG para Apple silicon. Mova o FixKey para Aplicativos e abra-o.
- Windows x64 ou ARM64: instalador EXE para o seu processador. Os pacotes do Windows não têm uma assinatura de editor Authenticode, portanto, o Windows pode exibir um aviso de editor desconhecido ou do SmartScreen.
- Linux X11 x64 ou ARM64: AppImage para o seu processador. Não há suporte ao Wayland.

Os requisitos mínimos de execução são macOS 11 em Apple silicon; Windows 10 com WebView2; ou Linux com X11, GTK 3, WebKitGTK 4.1 e um barramento de acessibilidade AT-SPI. Esses são pré-requisitos da plataforma. Eles não garantem compatibilidade com todos os aplicativos ou campos de texto.

## Conceder permissões

O FixKey precisa das permissões de acessibilidade e entrada do seu sistema operacional para ler e substituir a palavra atual em um campo editável. Siga a caixa de diálogo de permissões, conceda o acesso solicitado nas configurações do sistema e selecione **Verificar novamente**. A correção permanece inativa até que as permissões necessárias e os dicionários incluídos estejam prontos. Campos de senha e protegidos são excluídos.

No macOS, verifique **Acessibilidade** e **Monitoramento de Entrada** nos Ajustes do Sistema. A Acessibilidade pode conceder acesso efetivo à entrada sem uma entrada separada em Monitoramento de Entrada. Para verificar essa lista, abra **Geral** no FixKey e selecione **Configurar Monitoramento de Entrada**. Se o FixKey não estiver presente, use **+** na lista de permissões do sistema para adicionar o FixKey.app atual da pasta Aplicativos, depois retorne ao FixKey e verifique novamente. Uma entrada separada na lista não é necessária quando o macOS já concede acesso efetivo à entrada.

## Corrigir uma palavra

Ambos os layouts estão ativados por padrão. Digite em um campo editável e termine uma palavra com um espaço ou sinal de pontuação. O FixKey verifica os candidatos dos layouts ativados em seus dicionários incluídos. Ele substitui a palavra somente quando exatamente um idioma ativado reconhece seu candidato e esse candidato é diferente do original. Se nenhum idioma ou mais de um idioma reconhecer um candidato, a palavra permanecerá inalterada.

Por exemplo, as sequências de teclas `rPthrgo` e `zlqhemrk` podem se tornar `계속해` e `키보드가`. Esses exemplos ilustram a correção de layout; os resultados dependem do campo atual, do contexto de entrada e do reconhecimento do dicionário.

O FixKey preserva o separador após uma palavra corrigida. Enter e Tab não acionam a correção. A pontuação usada em URLs ou endereços de e-mail pode adiar a avaliação até que o FixKey consiga classificar o token inteiro. Apóstrofos e hifens entre letras permanecem dentro da palavra.

A correção automática ignora textos colados ou inseridos programaticamente, números, emojis, URLs, endereços de e-mail e tokens com sistemas de escrita mistos. Ela aguarda a conclusão da composição da entrada em coreano. Campos desconhecidos, permissões indisponíveis e campos que não podem ser alterados com segurança permanecem intactos.

Após uma correção automática bem-sucedida no macOS ou Windows, o FixKey também tenta selecionar uma fonte de entrada ativada para o idioma corrigido. No Windows, ele pode usar os modos Hangul e latino do Microsoft Korean IME, inclusive quando nenhuma fonte QWERTY de inglês dos EUA separada está ativada. Mantenha seu IME coreano configurado para Dubeolsik. Se a seleção da fonte de entrada não estiver disponível ou falhar, o texto corrigido e a opção de desfazer imediatamente continuarão disponíveis. IMEs fechados ou sem suporte no Windows não são alterados. O Linux mantém a fonte de entrada atual. Desfazer imediatamente não altera novamente a fonte de entrada.

### Desfazer uma correção

Pressione Backspace imediatamente após uma correção automática para restaurar a palavra original e seu separador. Os pressionamentos seguintes de Backspace excluem normalmente. Mover o cursor, alterar a seleção, trocar de campo ou começar outra palavra encerra esse contexto de desfazer imediatamente.

### Escolher um candidato manualmente

1. Abra **Atalhos** e atribua o atalho de candidato manual. Por padrão, ele não está atribuído. Se o registro falhar, verifique o motivo exibido ou escolha outro atalho.
2. Digite uma palavra em um campo editável e use o atalho.
3. Use as teclas de seta para selecionar um candidato. Pressione Enter para aplicá-lo ou Esc para fechar o seletor.

O seletor mostra os resultados de reconhecimento do dicionário. Você pode escolher explicitamente um candidato não reconhecido. Mover o cursor de origem ou trocar de aplicativo fecha o seletor. O seletor não fica disponível quando o FixKey não consegue estabelecer um destino de substituição seguro.

## Configurar a correção

Em **Geral**, pause ou retome a correção, selecione os idiomas ativados, ative a inicialização no login e escolha o idioma e o tema da interface. A inicialização no login fica desativada por padrão. As configurações são aplicadas imediatamente e salvas localmente.

Para manter um aplicativo inalterado, adicione-o em **Aplicativos excluídos**. As exclusões identificam o aplicativo, e não apenas seu nome exibido. Um aplicativo não resolvido permanece listado para que você possa revisá-lo.

## Privacidade e uso offline

O FixKey processa a entrada no seu dispositivo. Ele nunca salva nem transmite texto digitado, candidatos, conteúdo de documentos, títulos de janelas ou histórico de correções. Ele não usa a área de transferência para fazer substituições. Não há telemetria, relatórios remotos de falhas nem envio automático de diagnósticos.

Os dicionários estão incluídos, portanto, a correção funciona offline desde a primeira inicialização, assim que as permissões estiverem prontas. Os dicionários mudam com as atualizações do aplicativo; não há download separado de dicionários nem editor de dicionário do usuário. O uso da rede é limitado à verificação e ao download de atualizações.

## Atualizar o FixKey

O FixKey verifica a existência de atualizações estáveis na inicialização, a cada 24 horas enquanto está em execução e quando você solicita uma verificação em **Atualizações**. O download e a instalação exigem seu consentimento. As assinaturas dos pacotes de atualização são verificadas, e assinaturas inválidas impedem a instalação. A correção é interrompida antes da instalação, e as configurações são preservadas.

Após uma falha na verificação, tente novamente manualmente ou aguarde a próxima verificação agendada. Se a instalação falhar, reinicie o FixKey ou reinstale o pacote apropriado a partir das versões do FixKey. Você pode reinstalar manualmente uma versão anterior; o FixKey nunca faz downgrade automaticamente.

## Solução de problemas e suporte

Se a correção estiver inativa, abra **Diagnóstico e Ajuda** e leia o status e o código do motivo. Verifique novamente as permissões e tente usar um campo editável em outro aplicativo. Um status de espera por um campo editável significa que não foi possível identificar o campo atual com segurança. Verifique também se a correção está pausada ou se o aplicativo está excluído.

Use **Abrir pasta de logs** para inspecionar os logs de diagnóstico locais ou **Excluir logs** para removê-los. Os logs contêm resultados de diagnóstico e códigos de motivo, e não o conteúdo digitado. O FixKey mantém no máximo sete dias de logs e, no máximo, 10 MiB no total.

Para obter ajuda, use o link do GitHub Issues em **Diagnóstico e Ajuda** ou abra o [GitHub Issues](https://github.com/delinoio/apps/issues). Inclua a versão do FixKey, o sistema operacional, o aplicativo afetado e o código do motivo. Não inclua textos privados nem credenciais.
