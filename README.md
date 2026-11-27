# Ultra Guardião

**English:** a lightweight Windows tray app (Rust) that watches RAM and CPU and acts before the machine freezes, e.g. by closing orphaned processes left behind by coding agents. UI in Brazilian Portuguese. The optional AI diagnosis (Codex CLI) sends data about your PC to the AI service; periodic analysis and auto-apply are off by default. Early stage; contributions welcome.

Programa leve para Windows que impede o PC de travar. Fica no relógio (bandeja), vigia RAM e CPU
a cada 3 segundos e age antes da memória acabar. Interface feita com [Rune](https://github.com/anomalyco/rune)
(Rust, renderizada na GPU), que só existe enquanto a janela está aberta.

## O que faz

| Quando | Ação |
|---|---|
| RAM ≥ 85% (Atenção) | Alivia a memória de programas **parados** (fora do primeiro plano), a cada 3 min |
| RAM ≥ 92% (Crítico) | Alivia a cada 1 min e **fecha processos órfãos parados há 3 min** e sem janela visível (node, claude, codex, llama-server… que ficaram rodando depois do programa que os abriu fechar) |
| CPU ≥ 85% por ~12 s | Baixa a prioridade de quem mais pesa em segundo plano; restaura quando acalma |
| Programa cresce +0,8 GB em 30 min e passa de 1,5 GB | Registra o alerta: parece vazamento de memória |
| Ligado há mais de 3 dias | Sugere reiniciar |
| C: com menos de 12% livre (verifica a cada 10 min) | Apaga temporários com mais de 2 dias e `CrashDumps`/`D3DSCache` com mais de 7 dias (sem "Desfazer") |
| Plano "Economia de energia" ativo (mesma verificação) | Troca para Equilibrado e ajusta o disco para não desligar na tomada |
| Sempre | **Caixa-preta**: uma linha por minuto em `%LOCALAPPDATA%\UltraGuardiao\caixa-preta\` (7 dias) |

As duas últimas linhas da tabela (disco e energia) valem mesmo com o modo automático desligado.

Por padrão não há notificações do Windows (`silencioso: true`): os avisos ficam na aba Ações e na
caixa-preta. Com `silencioso: false`, o Guardião notifica os alertas mais importantes.

O alívio de memória e as ações automáticas sobre processos poupam o programa em primeiro plano (e a
família dele) e os da lista `protegidos`. Os botões "Fechar" (no painel ou numa sugestão da IA) pedem
confirmação e respeitam `protegidos`, mas podem fechar um programa que você está usando.

O fechamento de órfãos, automático ou manual, pula `protegidos` e qualquer processo dono de uma janela
visível (inclusive minimizada); se o Windows não deixar listar as janelas, nada é fechado. A checagem
pode não detectar programas de console exibidos dentro de outro aplicativo (como o Windows Terminal),
porque a janela pertence a esse aplicativo. Os botões "Fechar órfãos" e "Otimizar tudo agora" não
esperam os 3 min parados.

### Abas
- **Agora**: RAM, memória pedida, CPU, quem mais consome (com botão Fechar + confirmação), "Aliviar memória agora", "Fechar órfãos".
- **Ações**: o que o Guardião fez e os alertas.
- **Saúde**: lê os registros do Windows: travamentos totais (e o que a caixa-preta viu antes), programas que congelaram/fecharam, atualizações de driver que falham, drivers de vídeo antigos, disco, tempo ligado.
- **Sistema**: disco, memória, energia, programas que iniciam com o Windows e autodiagnóstico.
- **Análise**: diagnóstico com IA (opcional; veja abaixo).
- **Faxina**: ajustes únicos e reversíveis: Edge e Paperclip na inicialização, telemetria Intel, assistente de drivers Intel, hibernar em vez de suspender, iniciar o Guardião com o Windows.

## Sempre ligado

- **Supervisor**: `UltraGuardiao.exe --supervisor` reinicia o Guardião se ele cair ou se o watchdog
  detectar o laço principal parado por 150 s. "Sair do Guardião" encerra de verdade (supervisor incluso).
- **Abre com o Windows**: chave `Run` + tarefa agendada "UltraGuardiao" ao entrar no Windows,
  auto-reparadas a cada início (config `iniciar_com_windows`).
- **Timeouts** nas consultas ao Windows; a análise com IA é interrompida após 6 minutos.
- **Intervalo adaptativo**: 3 s em Atenção/Crítico ou CPU alta, 8 s em calmaria.
- **Leve**: o Guardião roda em prioridade baixa + EcoQoS.
- **Protegidos** ampliada: WhatsApp, Chrome, Edge, Teams e outros programas de uso diário.

Honestidade: nenhum software garante que um PC jamais trave (falha de hardware, driver ou energia
podem travar qualquer máquina). O Guardião reduz o risco e se recupera sozinho. O reinício
automático do PC **não** é feito, para não perder trabalho não salvo.

## Análise com IA (opcional)

Usa o [Codex CLI](https://github.com/openai/codex), instalado e logado por você. **Ela envia dados do
seu PC ao serviço de IA configurado no Codex (normalmente a OpenAI).**

### Quando roda

- **Por clique** (`ia_ativa`, ligada por padrão): "Analisar agora" e também **"Otimizar tudo agora"**.
  O botão "Terminal" abre uma sessão do Codex.
- **Sem clique**: só com `ia_automatica: true` (**desligada por padrão**): rotina a cada
  `ia_intervalo_horas` (padrão 3; a primeira 10 min depois de abrir; 0 desliga só a rotina), ao sair
  do nível Crítico (no máximo a cada 2 h) e após um travamento total.
- **Aplicar sozinho**: só com `ia_aplicar_auto: true` (**desligado por padrão**). Vale para toda
  análise, exceto a do "Analisar agora", e só para: proteger (exceto programas da lista de órfãos),
  ajustar limites (atenção 70–90, crítico 80–95, CPU 70–95) e faxina sem administrador.
  `adicionar_orfao`, fechar processo e reiniciar sempre exigem seu clique.
- `ia_ativa: false` impede novas análises pelo Guardião.

Um `config.json` que já existe mantém os valores gravados (se ele tiver `ia_automatica` ou
`ia_aplicar_auto` como `true`, continuam ligados); chaves ausentes ficam desligadas.

### O que é enviado

O Guardião envia ao Codex suas instruções e um relatório (também salvo em
`%LOCALAPPDATA%\UltraGuardiao\analise\relatorio.md`) com:

- data, motivo da análise, RAM total, limite de memória e número de processadores;
- nível, uso de RAM, memória pedida e CPU, tempo ligado, número de processos e de órfãos, totais
  liberados e fechados;
- os grupos de programas que mais usam RAM (nome, quantidade, RAM, CPU);
- a verificação de saúde, se já feita: travamentos, nomes de programas que congelaram ou fecharam,
  atualizações que falharam, placa de vídeo e data do driver, disco e o último travamento total;
- até 30 ações e alertas recentes do Guardião (incluem nomes de programas, resumos de análises
  anteriores e mensagens de erro, que podem conter caminhos com seu nome de usuário);
- caixa-preta: até 45 linhas dos últimos 45 minutos e até 20 linhas das 6 horas antes do último
  travamento (horário, RAM, memória pedida, CPU, nível, 5 maiores programas, eventos);
- configuração: limites, opções e as listas `protegidos` e `orfaos_fechaveis`.

**O envio não se limita ao relatório.** As instruções permitem que o Codex investigue com comandos
PowerShell de leitura (`Get-Process`, `Get-WinEvent`, `Get-CimInstance` etc.), e a saída desses
comandos vai para o modelo. `--sandbox read-only` serve para restringir alterações e **não impede a
leitura de dados**: o que o sandbox do Codex deixar ler (por exemplo arquivos, registros e linhas de
comando de processos) pode ser enviado. O pedido para não alterar nada e usar poucos comandos é só uma
instrução ao modelo. O botão "Terminal" não passa `--sandbox`: valem as configurações do seu Codex.
Retenção e uso dos dados seguem os termos do provedor.

As últimas 30 respostas ficam em `%LOCALAPPDATA%\UltraGuardiao\analise\historico\`.

## Configuração

`%LOCALAPPDATA%\UltraGuardiao\config.json` (criado na primeira vez):
`automatico`, `limite_atencao`, `limite_critico`, `limite_cpu`, `protegidos`, `orfaos_fechaveis`,
`dias_para_reiniciar`, `dias_caixa_preta`, `iniciar_com_windows`, `ia_ativa`, `ia_modelo`,
`ia_modelo_rapido`, `ia_automatica`, `ia_intervalo_horas`, `ia_aplicar_auto`, `silencioso`.

## Compilar

Precisa do Rust (rustup) e do Visual Studio C++ Build Tools.

```bash
cargo build --release
```

O executável sai em `target\release\UltraGuardiao.exe` (ou na pasta definida em `CARGO_TARGET_DIR`).
`UltraGuardiao.exe --abrir` abre já com a janela.

## Licenças de terceiros

Ao distribuir o executável, inclua os avisos de terceiros:

- [Rune](https://github.com/anomalyco/rune): MIT, Copyright (c) 2026 Anomaly. Acompanhe o texto do
  `LICENSE` dele.
- Fonte Inter, embutida no executável do Windows pelo recurso `embedded-fonts`: Copyright (c) 2016
  The Inter Project Authors, SIL Open Font License 1.1. Inclua `crates/rune/assets/Inter-LICENSE.txt`
  (do repositório do Rune).
- O Rune também traz a IBM Plex Mono (OFL 1.1, `IBMPlexMono-LICENSE.txt`), que ele embute na versão
  web; inclua o aviso se distribuir um binário que a contenha.

As demais dependências do `Cargo.lock` (incluindo as cópias em `vendor/` no Rune) têm licenças
próprias, que também precisam ser verificadas.
