# CamLink

Transforme seu celular Android e câmeras IP em webcams virtuais — sem instalar
nada no celular.

O CamLink é um aplicativo desktop (Linux e Windows 10/11) que detecta
smartphones Android conectados por cabo USB e fontes de vídeo IP/RTSP, e os
expõe como webcams virtuais reconhecidas por OBS Studio, navegadores (Chrome,
Firefox — WebRTC), Discord e qualquer aplicativo que use câmera.

## Visão

- **Zero instalação no celular**: a comunicação com o Android acontece via
  USB/ADB; nenhum aplicativo é instalado no aparelho (Android 12+).
- **Controles de câmera em tempo real**: zoom, foco (tap-to-focus), exposição,
  balanço de branco, torch e estabilização, direto do desktop.
- **Modos inteligentes**: Auto, Night, Sport e Pro trocam parâmetros de
  câmera (foco, exposição, fps, estabilização, redução de ruído, AF por
  rosto) em runtime, sem interromper o stream; Pro libera controle manual
  completo.
- **Fontes IP/RTSP**: câmeras de rede como webcams virtuais, com credenciais
  guardadas apenas no cofre de segredos do sistema operacional.
- **Múltiplos dispositivos simultâneos**: cada fonte vira um dispositivo de
  webcam independente.
- **Captura RAW (DNG)**: fotos em RAW a partir do sensor do celular.
- **Paridade Linux + Windows**: funcionalidades equivalentes nas duas
  plataformas desde a v1.

## Status

🚧 Em desenvolvimento — ainda não há release. Acompanhe o progresso em
[`specs/001-phone-webcam-bridge/`](specs/001-phone-webcam-bridge/).

## Stack

- **Backend**: Rust (Tauri 2.x) — `src-tauri/`
- **UI**: SvelteKit — `src/`
- **Android**: fork do [scrcpy](https://github.com/Genymobile/scrcpy)-server
  (Java 17, submodule em `scrcpy/`, branch `camlink`)
- **Runtime**: adb, scrcpy ≥ 4.0, ffmpeg, v4l2loopback ≥ 0.13 (Linux),
  filtro DirectShow próprio (Windows — sem driver de terceiros)

## Desenvolvimento

```bash
pnpm install
pnpm tauri dev
```

Gates de qualidade (obrigatórios, ver `.specify/memory/constitution.md`):

```bash
cd src-tauri
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

## Privacidade

O CamLink não envia nada para lugar nenhum. Não há telemetria, analytics,
verificação de atualização nem "phone home" de qualquer espécie. Isto é um
requisito do projeto (FR-026), não uma escolha de configuração — e foi
auditado, não apenas declarado:

- **Dependências**: das 274 crates na árvore de compilação, nenhuma é
  cliente HTTP, stack TLS ou SDK de telemetria. As únicas com capacidade de
  rede são `mio` e `socket2`, que são as primitivas de I/O do `tokio`.
- **Sockets do aplicativo**: existem exatamente dois no código, e ambos
  conectam em `127.0.0.1` — são os túneis que o `adb` abre para falar com o
  celular pelo cabo USB. Nenhum endereço externo é alcançável por eles.
- **Interface**: nenhum `fetch`, `XMLHttpRequest` ou `WebSocket`; nenhuma
  fonte, folha de estilo, script ou imagem vinda de CDN. Tudo que a tela
  carrega está dentro do pacote instalado.
- **Única saída de rede**: o `ffmpeg`, quando você configura uma fonte
  IP/RTSP, conecta no endereço que **você** digitou. É o propósito do
  recurso. Nenhum outro destino é contatado, e a saída do ffmpeg é sempre
  local (o dispositivo de câmera virtual ou a própria memória do app).
- **Credenciais**: senhas de câmeras RTSP ficam apenas no cofre do sistema
  operacional (Secret Service no Linux, Gerenciador de Credenciais no
  Windows). Nunca no arquivo de configuração, e os logs censuram a
  credencial na URL antes de escrever.
- **Vídeo**: os frames nunca saem da máquina. Vão do celular (USB) ou da
  câmera IP direto para o dispositivo de câmera virtual local, consumido
  por OBS/navegador/Discord na mesma máquina.

A política de segurança de conteúdo (CSP) da janela restringe o que a
interface pode carregar, de modo que nem um erro futuro nosso consiga
introduzir uma requisição externa sem que isso apareça como violação.

Os binários que o CamLink executa (`adb`, `scrcpy`, `ffmpeg`,
`v4l2loopback-ctl`) são de terceiros e vêm da sua distribuição ou embutidos
no instalador; o projeto não os modifica.

## Limitações conhecidas

- **Trocar de câmera (frontal/traseira), espelhar ou girar (qualquer ângulo)
  no Linux pode exigir atualizar a página no Meet/Chrome uma vez.** No
  Linux, o scrcpy escreve os frames direto no device v4l2loopback
  (`--v4l2-sink`) sem passar pelo CamLink — não há como aplicar espelho/giro
  "ao vivo" nesse caminho, então **toda** mudança de orientação reinicia o
  processo do scrcpy (não só 90°/270° como no Windows, onde o pipeline passa
  pelo Rust e permite atualização ao vivo — FR-016a). Isso gera um instante
  sem frame novo; o device virtual continua saudável durante esse instante
  (confirmado: nenhum evento de add/remove/change no kernel), mas o Meet às
  vezes marca a câmera como indisponível e não recupera sozinho — nem
  esperando alguns segundos. **F5 na aba (ou sair e entrar de novo na
  chamada) resolve.** Reproduzido também num Moto G55 (2026-08-03), então
  não é peculiaridade do S20 FE — parece ser inerente ao caminho de restart
  do scrcpy no Linux (`--v4l2-sink`), independente de fabricante.
- **Em alguns aparelhos Samsung, reconectar após um restart pode entrar num
  ciclo de falhas/artefatos que não se autorrecupera** (visto em bancada com
  um SM-G781B — o app tenta de novo com backoff, mas o dispositivo às vezes
  não libera a câmera a tempo, ciclando entre "Address already in use",
  "Demuxer error" e `CAMERA_DISCONNECTED`). Depois de várias tentativas
  seguidas sem sucesso, o app desiste e mostra um erro pedindo pra
  desconectar/reconectar o cabo USB ou reiniciar o app, em vez de martelar o
  device indefinidamente (circuit breaker). **Causa raiz**: parece ser um bug
  conhecido e ainda aberto do próprio scrcpy com o Camera2 HAL de aparelhos
  Samsung (não é específico do CamLink) — mesmo padrão relatado em S22,
  SM-S906B e outros: [#6514](https://github.com/Genymobile/scrcpy/issues/6514),
  [#5977](https://github.com/Genymobile/scrcpy/issues/5977),
  [#5311](https://github.com/Genymobile/scrcpy/issues/5311). Tentativas de
  isolar o gatilho (resolução, sequência de restart de rotação, leitor de
  preview concorrente) não reproduziram em testes isolados — o disparo real
  parece exigir o padrão de uso completo do app. **Ainda sem decisão de
  como tratar definitivamente; será decidido antes da versão release.**

## Licença

[GPL-3.0](LICENSE). O fork do scrcpy-server permanece sob Apache-2.0 (licença
do upstream).
