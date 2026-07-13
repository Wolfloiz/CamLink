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
- **Entrega de frames (T022)**: cada câmera virtual roda um subprocesso
  `ffmpeg -f rawvideo -pixel_format rgba -i pipe:0 -pix_fmt yuyv422 -f v4l2
  <device>` dedicado; `feed_frame`/`set_standby` escrevem RGBA no stdin, o
  ffmpeg converte para YUYV422 e escreve no device. Evita bindings ioctl
  manuais (VIDIOC_S_FMT + write) não verificáveis na máquina onde o backend
  foi escrito (Windows); reaproveita uma dependência já pinada do projeto.
- **Riscos tratados**: v4l2loopback < 0.13 → fallback modprobe com parâmetros;
  Secure Boot bloqueando módulo não assinado → detectar e guiar o usuário;
  Firefox pode exigir `v4l2compat.so` via `LD_PRELOAD` — o app oferece launcher.

## R4. Câmera virtual Windows: filtro DirectShow próprio (substitui akvirtualcamera)

- **Spike B — resultado (2026-07-10)**: `akvirtualcamera` **reprovado**.
  Sequência de achados, todos verificados nesta máquina (Windows 11, build
  ≥ 22000):
  1. `win_vcam_spike.rs` falhava em `GetProcAddress("vcam_system_api")` —
     símbolo ausente na release 9.4.0 instalada (adicionado só a partir da
     9.4.1 do upstream, confirmado comparando `capi.h` das duas tags no
     GitHub). Corrigido atualizando o driver para 9.4.1.
  2. Com o driver atualizado, o spike roda e empurra frames sem nenhum erro
     de API (`vcam_update`/`vcam_stream_send` sempre retornam 0), mas
     `AkVCamManager system-api` mostra que em Windows 11 o driver
     **auto-seleciona o backend Media Foundation** (não DirectShow, ao
     contrário do que este documento assumia originalmente).
  3. O device nunca aparece em `Get-PnpDevice -Class Camera` (nem
     `AkVCamManager -p devices` depois que o processo termina) — ou seja, o
     registro real no SO falha, mesmo com a API reportando sucesso. Não
     aparece nem no Chrome/Meet (MF) nem no OBS (DirectShow) nem no `ffmpeg
     -f dshow`.
  4. Isso bate com bugs abertos/recém-fechados no upstream
     ([webcamoid/akvirtualcamera#95](https://github.com/webcamoid/akvirtualcamera/issues/95),
     [#96](https://github.com/webcamoid/akvirtualcamera/issues/96)): o
     próprio mantenedor admite que a detecção de câmeras Media Foundation
     por outros programas é intermitente ("not sure why").
  5. **Controle**: a OBS Virtual Camera (embutida no OBS, filtro DirectShow
     próprio em `plugins/win-dshow/virtualcam-module/`, não relacionado ao
     akvirtualcamera) funciona normalmente no Chrome/Meet na mesma máquina —
     confirma que DirectShow continua sendo enumerado corretamente pelos
     consumidores; o problema é específico do codepath MF do
     akvirtualcamera, não do ambiente.
- **Decision**: abandonar `akvirtualcamera`; implementar um filtro DirectShow
  **próprio** (push-source, Rust via `windows-rs` — feature
  `Win32_Media_DirectShow` já expõe as interfaces COM necessárias, sem
  precisar handwritear vtables) como backend Windows do trait
  `VirtualCameraBackend`. Pipeline inalterado: `scrcpy --record=-` → `ffmpeg`
  decodifica para rawvideo em pipe → Rust → filtro DirectShow. RTSP idem.
- **Rationale**: scrcpy não tem sink de câmera virtual no Windows e ffmpeg não
  tem muxer de câmera virtual no Windows — necessária uma camada IPC de
  qualquer forma. DirectShow é a API madura (décadas de uso, bem documentada,
  `windows-rs` já tem os bindings) e comprovadamente funcional nesta stack
  (via o controle com a OBS Virtual Camera); implementação própria elimina a
  dependência de um driver de terceiros com bugs de registro não resolvidos.
- **Alternatives considered**: `MFCreateVirtualCamera` — reavaliado com a doc
  oficial da Microsoft: exige implementar um `IMFMediaSource`/`IMFMediaStream`
  COM completo e registrado por CLSID (não há método de push de sample direto
  na interface `IMFVirtualCamera`), e requer Windows Build 22000+ (Win11
  puro, exclui Win10) — esforço desproporcional confirmado, mantém-se
  rejeitado. Código da OBS Virtual Camera (`virtualcam-filter.cpp`): é
  GPL-2.0 sem cláusula "or later" nos cabeçalhos — **não pode ser copiado**
  para este projeto (GPL-3.0); usamos só a mesma arquitetura (filtro
  DirectShow), implementação própria via BaseClasses/`windows-rs`. softcam
  (menos mantida) e driver assinado próprio (custo de assinatura
  desproporcional) seguem descartados.
- **Risco/mitigação**: implementação de filtro COM DirectShow em Rust é
  território pouco trilhado (poucas referências prontas) → **Spike C**
  (T074) valida um filtro mínimo (registro + 1 formato + push de frames)
  antes de consolidar `virtualcam/dshow.rs` (T023 revisado); latência extra
  do decode→push segue orçada dentro dos 70 ms (H.264 decode local
  ≈ 5–15 ms, sem mudança nessa parte do pipeline).
- **Spike C — resultado (2026-07-10)**: **APROVADO**. Filtro push-source
  mínimo (`src-tauri/examples/win_dshow_spike.rs`, RGB24 640×480@30,
  `windows-rs` puro) registrado em `HKEY_CURRENT_USER\Software\Classes` (sem
  elevação) e validado end-to-end via Chrome real (headless, CDP,
  `getUserMedia`+`enumerateDevices`): device aparece, stream abre sem erro,
  frames com conteúdo real e não-estático (luminância não-nula, pixels
  mudando entre capturas). Também validado via cliente DirectShow genérico
  próprio (`win_dshow_connect_probe.rs`: `IGraphBuilder::Connect` → Null
  Renderer → `IMediaControl::Run` → `GetState` = Running → `Stop`, sem erros).
  Dois bugs reais encontrados e corrigidos durante o processo (relevantes
  para T023, evitar reintroduzir):
  1. **`IPin::QueryPinInfo.pFilter` e `IBaseFilter::QueryFilterInfo.pGraph`
     não podem ser `None`** — o contrato DirectShow exige referência contada
     válida ao filtro/grafo dono; um pino sem filtro dono ou um filtro que
     nunca "lembra" ter sido unido ao grafo (`JoinFilterGraph` como no-op)
     derruba o processo hospedeiro (Chrome/ffmpeg) quando o Filter Graph
     Manager desreferencia sem checar null — sem esse contrato, nem chegava
     a chamar `IPin::Connect`. Corrigido guardando auto-referências
     (`self_filter`/`self_pin`, AddRef só na entrega; `graph` como ponteiro
     bruto sem AddRef, por convenção, para evitar ciclo).
  2. **`IPin::ReceiveConnection` com `pConnector = None` é rejeitado
     (`E_POINTER`)** por vários consumidores (ex.: Null Renderer) — a maioria
     dos exemplos/tutoriais trata esse parâmetro como opcional, mas na
     prática é preciso passar uma referência real a si mesmo.
  3. **`AMPROPERTY_PIN_CATEGORY` (valor `0`, propriedade de
     `AMPROPSETID_Pin`) ≠ `KSPROPERTY_PIN_CATEGORY` (valor `11`, enum
     diferente)** — bug sutil de nome parecido: o filtro respondia à
     propriedade errada em `IKsPropertySet::Get`. Câmeras físicas e a OBS
     Virtual Camera também "falhavam" contra a propriedade errada nos testes
     manuais, mascarando o bug (parecia que só o nosso filtro respondia).
     Esse é exatamente o property ID que `VideoCaptureDeviceWin::Init()` do
     Chromium usa para achar o pino de captura (`media/capture/video/win/
     video_capture_device_win.cc`, confirmado lendo o código-fonte do
     Chromium) — sem a correção, `Init()` falhava silenciosamente
     ("Launching device has failed", sem detalhe do motivo).
  **Validação manual em OBS (2026-07-12)**: revelou 2 bugs adicionais,
  ausentes do teste automatizado via Chrome porque o Chromium não bate nesses
  caminhos de código — só apareceram com um consumidor DirectShow "real"
  (OBS/`libdshowcapture`). Ambos corrigidos, código-fonte da OBS
  (`obsproject/libdshowcapture`) lido diretamente do GitHub para confirmar o
  contrato exato esperado:
  4. **Trava permanente da OBS ao adicionar a fonte (precisava matar pelo
     Gerenciador de Tarefas)** — causa raiz: `IEnumMediaTypes::Skip` era um
     no-op (só logava e retornava `Ok(())`, sem marcar o enumerador como
     esgotado). A OBS sonda quantos formatos um pino oferece com o idioma
     "criar enumerador novo → `Skip(i)` → `Next(1, ...)`, incrementando `i`"
     — com `Skip` não fazendo nada, `Next` sempre reportava sucesso não
     importa o `i`, então a sondagem nunca via `Next` falhar e girava para
     sempre incrementando `i" (chegou a >80000 no log antes do usuário matar o
     processo), 100% de CPU numa thread da OBS sem nunca devolver o controle.
     Diagnosticado via logging próprio em arquivo (`dbg_log`, grava em
     `%TEMP%\camlink_dshow_debug.log`, técnica necessária porque a OBS não
     tem console visível) mostrando o padrão `Skip(0), Skip(1), Skip(2), ...`
     intercalado com criações de enumerador novo, nunca terminando. Corrigido
     implementando `Skip` de verdade: com um único formato disponível,
     qualquer `Skip(n>=1)` esgota o enumerador (marca `done=true`) e retorna
     `S_FALSE` se `n` excedia o que sobrava — aí `Next` correndo depois
     reporta corretamente "acabou" e a sondagem da OBS termina. (Duas pistas
     falsas investigadas e descartadas antes de achar isso: teoria de
     apartment/marshaling cross-thread via Global Interface Table — revelou
     que `IMemInputPin`/`IMemAllocator` não têm proxy/stub COM registrado no
     Windows, são interfaces "fast path" propositalmente não-marshaláveis, uso
     de ponteiro cru cross-thread é o padrão correto, não um bug; e teoria de
     `join()` bloqueante no `Stop()` — removida por ser mais correta de
     qualquer forma, mas não era a causa da trava.)
  5. **Tela preta na OBS / preview travado no último frame de outra fonte**
     (depois do bug #4 corrigido — a OBS parava de travar, conectava,
     `IMemInputPin::Receive` retornava `S_OK`, mas nada aparecia) — causa
     raiz: nunca chamávamos `IMediaSample::SetTime`. Lendo
     `HDevice::Receive` em `libdshowcapture/source/device.cpp`: o frame só é
     encaminhado pro pipeline de vídeo real da OBS
     (`SendToCallback`/`videoConfig.callback`) se `sample->GetTime(...)`
     tiver sucesso (`hasTime`); sem timestamp, `Receive()` aceita a amostra
     no nível do protocolo COM (retorna `S_OK`, daí não travar mais) mas o
     conteúdo é descartado silenciosamente antes de chegar à renderização.
     Corrigido setando `start`/`end` em unidades de 100ns (`REFERENCE_TIME`)
     por frame, casado com o `AvgTimePerFrame` já declarado no media type.
  **Validado (2026-07-12)**: OBS (padrão de cores real, sem travar, sem tela
  preta) e Google Meet via Chrome real (não-headless). Diferença de
  espelhamento horizontal observada entre Meet e OBS é comportamento
  esperado do lado de cada app (Meet espelha o preview local pra parecer
  natural; OBS mostra o frame cru) — não é bug do filtro.
  **Pendente**: validação manual em Firefox e Discord (não bloqueante — o
  mesmo contrato COM já provado em dois consumidores reais distintos).
  Antes de consolidar T023: remover a instrumentação `dbg_log`/arquivo de
  debug do spike (comentário `// Remover antes de consolidar T023` já no
  código-fonte).

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

## R12. Aquisição de vídeo no Windows: bypass do cliente scrcpy, socket direto

- **Contexto**: plan.md/tasks.md (T024) assumiam `--record=-` com decode via
  ffmpeg no Windows. **Falso** — verificado em 2026-07-13 contra o scrcpy 4.0
  real (`scrcpy --help` local + `app/src/recorder.c` upstream): o recorder
  sempre resolve a saída via `avio_open(&ctx->pb, "file:" + filename, ...)`,
  sem caso especial para stdout; `--record-format` só aceita containers
  mux ados (mp4/mkv/m4a/mka/opus/aac/flac/wav), nunca H264 elementar; e o
  muxer não liga flags de streaming (`frag_keyframe`/`empty_moov`/`movflags`
  — busca em `recorder.c` não encontrou nenhuma). Ou seja, `--record` sempre
  produz um arquivo seekable finalizado só no fechamento; não dá para "ler
  enquanto grava" nem para apontar para um pipe nomeado do Windows com
  garantia de funcionar.
- **Decision**: no Windows, `stream_manager.rs` **não invoca o binário
  cliente `scrcpy`** para o caminho de vídeo. Em vez disso, replica em Rust
  só a parte de bootstrap que o cliente faz (documentada e estável em
  `scrcpy/app/src/server.c`, tag v4.0 — mesmo submodule já fixado por R1):
  1. `adb push <jar> /data/local/tmp/scrcpy-server.jar`;
  2. `adb forward tcp:<porta> localabstract:scrcpy_<scid>` (tunnel forward,
     igual ao cliente);
  3. `adb shell CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process /
     com.genymobile.scrcpy.Server <version> scid=... tunnel_forward=true
     video_source=camera control=false ...` (jar **stock** em T024/US1; o
     jar forkado com o socket de controle só entra em T037/US2 — variável de
     ambiente já modelada como `SCRCPY_SERVER_PATH`, ver plan.md);
  4. conectar no socket TCP encaminhado, ler o handshake (nome do device) e o
     `codec_id` (`u32`), depois consumir o stream de *frame headers* de 12
     bytes (config flag `u1` + key-frame flag `u1` + PTS `u62` + tamanho
     `u32`, protocolo documentado em `scrcpy/doc/develop.md` §Protocol,
     estável dentro da versão 4.0) seguido do payload H264 Annex-B;
  5. alimentar um subprocesso `ffmpeg -f h264 -i pipe:0 -f rawvideo -pix_fmt
     rgba pipe:1` (stdin/stdout, sem arquivo) e empurrar os frames RGBA
     decodificados para `VirtualCameraBackend::feed_frame` (T023).
  Linux **não muda**: continua usando o binário cliente `scrcpy` stock com
  `--v4l2-sink` direto (already-validated, sem decode nosso).
- **Rationale**: a alternativa (named pipe + `--record-format=mkv`, apostando
  que o muxer do ffmpeg dentro do scrcpy detecta saída não-seekável e
  transmite em vez de exigir arquivo finalizado) depende de comportamento
  interno do libavformat não confirmado e não testável agora (nenhum Android
  conectado nesta máquina) — risco de investigar e descobrir que não
  funciona, sem fallback pronto. O protocolo do socket de vídeo, em
  contraste, é documentado oficialmente pelo próprio projeto scrcpy e já
  temos precedente de falar diretamente com o servidor forkado via socket
  `localabstract` (R1, controle). O acoplamento extra (replicar os argumentos
  de lançamento do servidor) é de baixo risco: já fixamos a versão do scrcpy
  via submodule (R1), então cliente e o "mini-lançador" em Rust andam juntos
  na mesma tag.
- **Alternatives considered**: (a) named pipe + `--record-format=mkv`/`mp4`
  no cliente stock — descartado por depender de comportamento não verificado
  do avio/muxer com destino não-seekável no Windows; (b) pipeline própria
  completa (Camera2→MediaCodec→socket custom, sem scrcpy-server) — já
  descartada em R1 como "Plano B do spike", não revivida aqui (o problema é
  só a entrega do H264 ao desktop, não a captura no Android).
- **Risco/mitigação**: os argumentos de `Server.main()` fazem parte do
  "protocolo entre cliente e servidor" que scrcpy documenta como podendo
  mudar entre versões (`doc/develop.md`); mitigado por já estarmos pinados à
  tag do submodule (R1) e por cobrir a montagem dos argumentos com teste de
  unidade (T020) contra os valores conhecidos da v4.0. Isolamento
  Linux/Windows via abstração dedicada (trait `VideoSource` ou equivalente,
  com impls `#[cfg(target_os)]`), conforme Princípio IV da constituição —
  não é código platform-specific solto em `stream_manager.rs`.
- **Decisão do usuário**: confirmada em 2026-07-13 via pergunta direta
  (opção "bypass do cliente scrcpy no Windows" vs. "named pipe + mkv/mp4 no
  cliente stock") — usuário escolheu a primeira.
- **Implementação (T024, 2026-07-13)**: protocolo do socket de vídeo
  implementado em `stream_manager.rs` e verificado byte-a-byte contra
  `scrcpy/server/.../device/DesktopConnection.java` e `Streamer.java` do
  submodule (não apenas contra a doc em prosa): 1 byte dummy + 64 bytes de
  nome do device (handshake) → `u32` codec id → session packet (12 bytes:
  bit7 do byte0 = flag, bytes4..12 = width/height BE) → frame headers (12
  bytes: bits 7/6/5 do byte0 = media/config/key, 61 bits de PTS, bytes8..12
  = tamanho do pacote BE) + payload H264 → stdin de um subprocesso `ffmpeg
  -f h264 -i pipe:0 -f rawvideo -pix_fmt rgba pipe:1` → stdout lido em
  frames de tamanho fixo → `FrameSink` (`Arc<dyn Fn(&[u8]) + Send + Sync>`,
  injetado por quem chama `start()`, para não acoplar `stream_manager.rs` a
  um `VirtualCameraBackend` concreto). 14 testes de unidade cobrem o
  parsing (vetores de bytes conferidos à mão, não só round-trip contra o
  próprio encoder). **Não coberto**: conexão TCP real e decode ffmpeg
  fim-a-fim — sem Android conectado nesta máquina; falha aqui é não-fatal
  (a sessão continua rodando, a câmera fica em standby), mas o caminho
  precisa de hardware real para validação completa (mesma ressalva de R4
  sobre a ponte de memória compartilhada do DirectShow).
