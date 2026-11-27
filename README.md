# Ultra Guardião

**English:** a lightweight Windows tray app (Rust) that watches RAM and CPU and acts before the machine freezes, e.g. by closing orphaned processes left behind by coding agents. UI in Brazilian Portuguese. Early stage; contributions welcome.

Programa leve para Windows que impede o PC de travar. Fica no relógio (bandeja), vigia RAM e CPU
a cada 3 segundos e age antes da memória acabar. Interface feita com [Rune](https://github.com/anomalyco/rune)
(Rust, renderizada na GPU), que só existe enquanto a janela está aberta.

## O que faz

| Quando | Ação |
|---|---|
| RAM ≥ 85% (Atenção) | Alivia a memória de programas **parados** (fora do primeiro plano), a cada 3 min |
| RAM ≥ 92% (Crítico) | Alivia a cada 1 min e **fecha processos órfãos parados há 3 min** (node, claude, codex, llama-server… que ficaram rodando depois do programa que os abriu fechar). Avisa por notificação |
| CPU ≥ 85% por ~12 s | Baixa a prioridade de quem mais pesa em segundo plano; restaura quando acalma |
| Programa cresce +0,8 GB em 30 min | Avisa: parece vazamento de memória |
| Ligado há mais de 3 dias | Sugere reiniciar |
| Sempre | **Caixa-preta**: uma linha por minuto em `%LOCALAPPDATA%\UltraGuardiao\caixa-preta\` (7 dias) |

Nunca mexe no programa que você está usando (nem na família de processos dele), nem nos
processos do sistema da lista `protegidos`.

### Abas
- **Agora**: RAM, memória pedida, CPU, quem mais consome (com botão Fechar + confirmação), "Aliviar memória agora", "Fechar órfãos".
- **Ações**: tudo que o Guardião fez.
- **Saúde**: lê os registros do Windows: travamentos totais (e o que a caixa-preta viu antes), programas que congelaram/fecharam, atualizações de driver que falham, drivers de vídeo antigos, disco, tempo ligado.
- **Faxina**: ajustes únicos e reversíveis: Edge e Paperclip na inicialização, telemetria Intel, assistente de drivers Intel, hibernar em vez de suspender, iniciar o Guardião com o Windows.

## Sempre ligado

- **Supervisor**: `UltraGuardiao.exe --supervisor` reinicia o Guardião se ele cair ou se o watchdog
  detectar o laço principal parado por 150 s. "Sair do Guardião" encerra de verdade (supervisor incluso).
- **Abre com o Windows**: chave `Run` + tarefa agendada "UltraGuardiao" ao entrar no Windows,
  auto-reparadas a cada início (config `iniciar_com_windows`).
- **Timeouts** em todos os comandos externos, para nada ficar pendurado.
- **Intervalo adaptativo**: 3 s em Atenção/Crítico ou CPU alta, 8 s em calmaria.
- **Leve**: o Guardião roda em prioridade baixa + EcoQoS.
- **IA de rotina** a cada `ia_intervalo_horas` (padrão 3). Com `ia_aplicar_auto`, aplica sozinha só
  ações seguras: proteger, ajustar_limite (limites 70–90 para atenção e 80–95 para crítico),
  adicionar_orfao e faxina sem administrador. Nunca fecha programas com janela nem reinicia.
- **Silencioso**: sem notificações por padrão (`silencioso`).
- **Protegidos** ampliada: WhatsApp, Chrome, Edge, Teams e outros programas de uso diário.

Honestidade: nenhum software garante que um PC jamais trave (falha de hardware, driver ou energia
podem travar qualquer máquina). O Guardião reduz o risco e se recupera sozinho. O reinício
automático do PC **não** é feito, para não perder trabalho não salvo.

## Configuração

`%LOCALAPPDATA%\UltraGuardiao\config.json` (criado na primeira vez):
`automatico`, `limite_atencao`, `limite_critico`, `limite_cpu`, `protegidos`, `orfaos_fechaveis`,
`dias_para_reiniciar`, `dias_caixa_preta`, `iniciar_com_windows`, `ia_intervalo_horas`,
`ia_aplicar_auto`, `silencioso`.

## Compilar

Precisa do Rust (rustup) e do Visual Studio C++ Build Tools.

```bash
cargo build --release
```

O executável sai em `target\release\UltraGuardiao.exe`
(a pasta de compilação fica fora do OneDrive). `UltraGuardiao.exe --abrir` abre já com a janela.
