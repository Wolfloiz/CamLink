# Research — CamLink (001-phone-webcam-bridge)

**Date**: 2026-07-09. Consolida as decisões do plano legado (validadas contra a
spec atual) e resolve os desconhecidos introduzidos pela paridade Windows.
Nenhum NEEDS CLARIFICATION permanece.

## R1. Controles de câmera: fork do scrcpy-server (não APK companion)

- **Decision**: fork do `scrcpy-server` (Java) partindo da tag estável mais
  recente do scrcpy (≥ 4.0), adicionando uma thread `CamLinkControlServer` que
  escuta em `localabstract:camlink` e aplica comandos JSON via
  `setRepeatingRequest` na `CameraCaptureSession` existente. Cliente scrcpy
  intocado; jar apontado por `SCRCPY_SERVER_PATH`.
- **Rationale**: a câmera Android é exclusiva por processo — só quem possui a
  sessão pode alterar `CaptureRequest` (foco, exposição, ISO, WB). O servidor do
  scrcpy já possui a sessão e roda com UID shell (sem APK, sem permissão de
  câmera, sem foreground service — mantém a premissa "nada instalado no
  celular"). scrcpy 4.0 só expõe zoom/torch e apenas com janela (inviável em
  headless `--no-playback`); demanda upstream aberta (issues #5695, #4452) indica
  que não virá tão cedo.
- **Alternatives considered**: (a) APK companion controlando a câmera — evictado
  pela arbitragem de câmera do Android 10+ ao abrir a mesma câmera do scrcpy, e
  viola a premissa de produto; (b) scrcpy stock + atalhos de teclado — exige
  janela, sem foco/ISO/WB de qualquer forma; (c) pipeline própria completa
  (Camera2 → MediaCodec → socket em APK) — reimplementa o que o scrcpy já faz
  bem; fica como **plano B do spike** (critério de abortar da Fase 0.5).
- **Risco/mitigação**: manutenção do fork a cada release do scrcpy (cliente e
  servidor devem casar de versão). Mudanças isoladas em pacote `camlink/` + hooks
  mínimos; avaliar PR upstream. Spike A prova `set_zoom` em runtime antes de
  qualquer investimento nas fases 4–5.
- **Resultado do Spike A (2026-07-09)**: ✅ APROVADO em Samsung Galaxy S20 FE
  (Android 13). `hello`/`set_zoom`/`OUT_OF_RANGE`/`BAD_REQUEST` conforme o
  contrato; zoom 3× visível no stream headless sem reinício de sessão.
  Detalhes e roteiro em `scrcpy/README.camlink.md`. **Quirks Samsung** que a
  Phase 3 (T024) deve absorver: (a) hooks da libstagefright One UI copiam a
  cmdline do processo p/ buffer fixo ao configurar encoder → argv do servidor
  deve permanecer CURTO (~130 chars ok, ~230+ crasha com stack smash);
  (b) `adaptivebrightnessgo` (prioridade 999) evicta o cliente de câmera UID
  shell em ~2 s → desativar brilho adaptativo durante o stream ou emitir
  diagnóstico acionável (FR-010).

## R2. Modos inteligentes: Camera2 direto, sem SCENE_MODE e sem Extensions

- **Decision**: modos Auto/Night/Sport/Pro implementados como tabelas de
  parâmetros Camera2 (`CONTROL_AF_MODE`, `CONTROL_AE_MODE`,
  `AE_TARGET_FPS_RANGE`, `AE_EXPOSURE_COMPENSATION`, `CONTROL_AWB_MODE`,
  `VIDEO_STABILIZATION`, `NOISE_REDUCTION_MODE`, face-AF via
  `STATISTICS_FACE_DETECT_MODE_SIMPLE` → `CONTROL_AF_REGIONS`).
- **Rationale**: Camera2 Extensions API (Night/HDR/Bokeh) é stills-only — não
  aceita superfície de `MediaCodec` para vídeo contínuo. `CONTROL_SCENE_MODE`
  está deprecated (API 30+) e é ignorado por muitos fabricantes.
- **Alternatives considered**: Extensions API (inviável p/ vídeo); SCENE_MODE
  (deprecated/não confiável).
- **Nota**: Night = AE_TARGET_FPS_RANGE [15,30] (exposições longas) + NR
  HIGH_QUALITY + compensação +1 EV; Sport = [60,60] + EIS off + NR FAST; Pro =
  AF/AE/AWB off, tudo manual. Tap-to-focus: `AF_TRIGGER_START` → lock →
  `AF_TRIGGER_CANCEL` retoma contínuo.

## R3. Câmera virtual Linux: v4l2loopback ≥ 0.13 com criação em runtime

- **Decision**: módulo `v4l2loopback` carregado com `exclusive_caps=1`
  (persistência via `modules-load.d`); devices criados/removidos em runtime via
  `v4l2loopback-ctl add/delete` com número alocado dinamicamente; escalação via
  `pkexec` com policy polkit instalada pelo instalador.
- **Rationale**: `exclusive_caps=1` é mandatório para Chrome/WebRTC e OBS;
  alocação dinâmica evita colisão com webcams físicas; add/delete individual
  evita que `modprobe -r` derrube todos os devices.
- **Alternatives considered**: `modprobe` com `video_nr` fixo (colisões, requer
  reload global); virtual camera do OBS (não cobre apps fora do OBS, acopla ao
  OBS).
- **Riscos tratados**: v4l2loopback < 0.13 → fallback modprobe com parâmetros;
  Secure Boot bloqueando módulo não assinado → detectar e guiar o usuário;
  Firefox pode exigir `v4l2compat.so` via `LD_PRELOAD` — o app oferece launcher.

## R4. Câmera virtual Windows: akvirtualcamera (DirectShow) via FFI

- **Decision**: `akvirtualcamera` (webcamoid, GPL-3.0, C++/C API) como backend
  Windows do trait `VirtualCameraBackend`: o instalador registra o driver
  DirectShow; o Rust cria a câmera e empurra frames RGB/NV12 via a API IPC.
  Pipeline Android no Windows: `scrcpy --record=-` (H.264 em stdout) → `ffmpeg`
  decodifica para rawvideo em pipe → Rust → akvirtualcamera. RTSP idem (ffmpeg →
  rawvideo → push).
- **Rationale**: scrcpy não tem sink de câmera virtual no Windows (o `--v4l2-sink`
  é Linux-only) e ffmpeg não tem muxer de câmera virtual no Windows — é
  necessária uma camada IPC de qualquer forma. akvirtualcamera é a opção madura,
  GPL-compatível, com API C chamável via FFI e suportada pelo Webcamoid.
- **Alternatives considered**: `MFCreateVirtualCamera` (Media Foundation, só
  Win11 22H2+, exclui Win10 — pode virar backend adicional futuro); OBS Virtual
  Camera DLL (acoplada ao OBS); softcam (menos mantida); driver próprio
  (esforço/assinatura de driver desproporcionais).
- **Risco/mitigação**: apps que enumeram apenas câmeras Media Foundation podem
  não ver fontes DirectShow → **Spike B** valida OBS, Chrome, Firefox e Discord
  no Windows 10 e 11 antes das fases dependentes; latência extra do
  decode→push é orçada dentro dos 70 ms (medir no spike; H.264 decode local
  ≈ 5–15 ms).

## R5. Pipeline RTSP: ffmpeg low-delay

- **Decision**: subprocess ffmpeg por fonte:
  `-fflags nobuffer -flags low_delay -analyzeduration 0 -probesize 32
  -rtsp_transport tcp -i <url>` → Linux: `-f v4l2 /dev/videoX`; Windows:
  rawvideo em pipe → push akvirtualcamera. Validação de URL com timeout de 3 s
  antes de iniciar.
- **Rationale**: atende o alvo ≤ 300 ms com flags de buffer mínimo; TCP evita
  perda em redes domésticas; ffmpeg é dependência já exigida pelo caminho
  Windows.
- **Alternatives considered**: GStreamer (segunda dependência pesada sem ganho
  claro); crate `retina` (RTSP puro-Rust, ainda exigiria decode + conversão —
  reavaliar se o ffmpeg se mostrar frágil).

## R6. Captura RAW: DNG via DngCreator no fork, framing binário

- **Decision**: `ImageReader` com `RAW_SENSOR` (resolução nativa do sensor) como
  surface adicional da sessão; `DngCreator` gera DNG por frame; transmissão pelo
  socket de controle com framing binário length-prefixed; sequência limitada a
  1–3 fps com cadência calculada por tamanho de frame ÷ throughput medido do
  túnel ADB; stream H.264 principal sempre com prioridade.
- **Rationale**: RAW contínuo em full-fps exigiria 75–499 MB/s, acima do limite
  prático do ADB USB (~20–80 MB/s); a maioria dos devices só expõe RAW na
  resolução nativa. Base64 desperdiçaria ~33% do gargalo — daí framing binário.
- **Alternatives considered**: RAW full-fps (inviável por banda); compressão
  intermediária (perde o propósito do RAW); salvar no celular e puxar depois
  (viola premissa de nada persistir/instalar no aparelho).
- **Gate de capacidade**: `REQUEST_AVAILABLE_CAPABILITIES_RAW` + suporte à
  combinação de streams PRIV+RAW verificados via `get_capabilities`; UI oculta
  controles quando ausente (FR-016).

## R7. GUI: Tauri 2.x + Svelte (fixa TODO(GUI_FRAMEWORK) da constituição)

- **Decision**: Tauri 2.x (backend Rust) com frontend SvelteKit.
- **Rationale**: backend permanece Rust puro (constituição); bundler cobre
  .deb/AppImage/NSIS/MSI (FR-024); UI de painéis/sliders/preview itera rápido em
  Svelte; system tray e autostart nativos no Tauri.
- **Alternatives considered**: egui/iced/Slint (100% Rust, porém componentes de
  UI rica e theming mais custosos; sem bundler multi-formato integrado); Electron
  (peso e RAM injustificáveis; backend Node violaria a constituição).
- Desvio (frontend não-Rust) registrado na Complexity Tracking do plan.md.

## R8. Segredos RTSP: crate `keyring`

- **Decision**: crate `keyring` — Secret Service/libsecret no Linux, Credential
  Manager no Windows; a config persiste a URL sem credenciais + referência ao
  segredo (FR-018a).
- **Rationale**: cobre as duas plataformas com uma API; requisito direto da
  clarificação de 2026-07-09.
- **Alternatives considered**: arquivo com permissões 600 (senha em claro,
  rejeitado na clarificação); não persistir (UX pior, rejeitado).

## R9. Detecção de dispositivos: adb + hotplug por plataforma

- **Decision**: parsing de `adb devices -l` como fonte de verdade; gatilho de
  rescan por udev rule no Linux e polling de 500 ms como fallback universal
  (Windows usa somente polling na v1). Estado `unauthorized` vira fluxo guiado de
  autorização (FR-002).
- **Rationale**: adb já arbitra acesso USB e estados de autorização; udev
  acelera a reação no Linux sem criar dependência dura.
- **Alternatives considered**: falar protocolo ADB direto em Rust (crate
  `adb_client`) — menos maduro; USB raw via libusb — reimplementa o adb.

## R10. Licença e compatibilidade

- **Decision**: projeto GPL-3.0 (clarificação 2026-07-09). Compatível com:
  scrcpy (Apache-2.0, consumido como programa externo + fork mantém Apache-2.0),
  akvirtualcamera (GPL-3.0), ffmpeg (chamado como binário externo — sem link),
  Tauri (MIT/Apache-2.0), crates permissivas.
- **Rationale**: copyleft garante derivados abertos; nenhuma dependência do
  desenho impede GPL-3.0.
- **Alternatives considered**: Apache-2.0/MIT (rejeitadas na clarificação).

## R11. Estratégia de teste (Princípio III em domínio hardware-dependente)

- **Decision**: três camadas — (1) unit: parsers (`adb devices`, capabilities,
  serialização de comandos), máquinas de estado de sessão, cálculo de cadência
  RAW; (2) integração: subprocessos fake de adb/scrcpy/ffmpeg (binários de teste
  no repo) validando lifecycle, reconexão e tratamento de erro; contratos JSON
  com golden files compartilhados entre Rust e Java (JUnit no fork); (3)
  hardware-in-the-loop: roteiro manual versionado no quickstart.md com critérios
  mensuráveis (SC-001…SC-010), executado por fase.
- **Rationale**: a maior parte da lógica (orquestração, protocolo, estados) é
  testável sem hardware; o que exige celular/OBS fica roteirizado e vira gate de
  checkpoint, mantendo o Test-First aplicável.
- **Alternatives considered**: só testes manuais (viola Princípio III); emulador
  Android em CI (câmera virtual do emulador não exercita Camera2 real — valor
  baixo pelo custo; reavaliar depois).
