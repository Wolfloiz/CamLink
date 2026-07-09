# Quickstart — Validação ponta a ponta do CamLink

Guia de validação por fase/checkpoint. Cada cenário mapeia critérios de sucesso
da spec (SC-001…SC-010). Rodar os gates automatizados antes de qualquer cenário
manual:

```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test   # em src-tauri/
```

## Pré-requisitos

- **Linux**: Ubuntu 22.04+/Arch; `adb`, `scrcpy ≥ 4.0`, `ffmpeg`,
  `v4l2loopback-dkms (≥ 0.13)`, `v4l-utils` (o `installer/linux/install.sh`
  instala tudo, incluindo policy polkit e udev rules).
- **Windows 10/11**: instalador NSIS/MSI do CamLink (inclui adb, scrcpy, ffmpeg
  e o driver akvirtualcamera).
- **Celular**: Android 12+, depuração USB habilitada, cabo USB.
- **Consumidores para teste**: OBS Studio; Chrome/Firefox (https://webrtc.github.io/samples/src/content/devices/input-output/); Discord; Linux: `cheese`/`ffplay`.
- Dev: `rustup` (stable), `cargo tauri`, `pnpm`; fork: JDK 17 + Android SDK
  (usuário final recebe o jar pronto no pacote).

```bash
git clone --recurse-submodules <repo> && cd camlink
pnpm install && pnpm tauri dev
```

## Cenário 1 — Android como webcam (US1 / SC-001, SC-002, SC-003)

1. Conectar o celular via USB → deve aparecer na lista em ≤ 3 s; se
   `unauthorized`, o app exibe o passo a passo (FR-002).
2. Clicar em Iniciar → preview ativo; dispositivo virtual criado
   (Linux: conferir com `v4l2-ctl --list-devices`, label "CamLink Android").
3. Selecionar a câmera no OBS, no Chrome (página WebRTC), no Firefox e no
   Discord → vídeo em todos, sem configuração extra.
4. Medir latência: filmar um cronômetro na tela do PC e comparar quadro exibido
   vs. atual → **≤ 70 ms** (tipicamente 35–70 ms), medido no Linux **e** no
   Windows (o caminho decode→push do Windows é o mais sujeito a exceder — ver
   T016/Spike B); do cabo ao vídeo ≤ 30 s no total.
5. Puxar o cabo com o OBS aberto → imagem de espera (sem travar o OBS);
   religar → stream retoma sozinho (FR-006).

## Cenário 2 — Controles em tempo real (US2 / SC-004)

Com stream ativo no OBS:

1. Zoom, EV, ISO (modo Pro), WB, EIS, torch → efeito visível em < 1 s, sem
   queda do stream.
2. Clicar no preview (tap-to-focus) → foco converge na região.
3. Alternar frontal/traseira → vídeo volta em ≤ 2 s, OBS não precisa
   reselecionar.
4. Num aparelho sem ISO manual/RAW → controles correspondentes
   ocultos/desabilitados com explicação (FR-016).

## Cenário 3 — Modos inteligentes (US3)

1. Alternar Auto → Night → Sport → Pro com stream ativo: sem interrupção;
   indicador de modo atualiza.
2. Sport: confirmar 60 fps no OBS (stats). Night: imagem visivelmente mais
   clara em ambiente escuro (+1 EV, fps 15–30). Pro: todos os campos manuais
   habilitados.

## Cenário 4 — RTSP (US4)

1. Sem câmera IP real, simular: `ffmpeg -re -stream_loop -1 -i sample.mp4 -f rtsp rtsp://localhost:8554/test`
   (com mediamtx) — ou usar câmera real com senha.
2. Adicionar fonte com credencial → senha vai ao cofre do SO (verificar que o
   arquivo de config NÃO contém a senha — FR-018a; Linux: `seahorse`, Windows:
   Gerenciador de Credenciais).
3. Fonte aparece como segunda webcam virtual no OBS; latência ≤ 300 ms.
4. Credencial errada → erro claro de autenticação; derrubar o stream → imagem
   de espera + reconexão automática.

## Cenário 5 — RAW (US5 / SC-006)

1. Aparelho com RAW: Snapshot → abrir o `.dng` no RawTherapee/Darktable;
   conferir resolução nativa do sensor.
2. Sequência 10 s → fps efetivo entre 1–3, nenhum arquivo corrompido, stream
   H.264 no OBS sem degradação (FR-020).
3. Aparelho sem RAW: painel oculto.

## Cenário 6 — Multi-fonte e preview (US6 / SC-007)

1. Android + RTSP simultâneos → duas câmeras virtuais independentes no OBS.
2. Derrubar uma fonte → a outra continua intacta.
3. Preview do app a 1 fps com OBS consumindo → CPU adicional < 5%
   (`htop`/Gerenciador de Tarefas).

## Cenário 7 — Sessão longa e instalação (SC-005, SC-008, SC-010)

1. Stream de 2 h → sem interrupção perceptível; RSS do processo estável.
2. Instalação limpa: Ubuntu 22.04/24.04 e Arch via pacotes; Windows 10 e 11
   via instalador → primeiro vídeo funcionando **sem abrir terminal** (SC-008).
3. Repetir Cenário 1 completo no Windows → resultados equivalentes (SC-010).

## Diagnósticos esperados (edge cases)

| Situação | Comportamento esperado |
|---|---|
| Secure Boot bloqueia v4l2loopback | Erro com guia de assinatura do módulo |
| scrcpy ausente/versão < 4.0 | Erro com instrução de instalação |
| Android < 12 | Listado como incompatível com motivo (FR-002a) |
| Firefox não enumera (Linux) | App oferece launcher com `v4l2compat.so` |
| Dois jobs RAW simultâneos | Segundo rejeitado com `BUSY` |
