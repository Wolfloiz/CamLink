---

description: "Task list for CamLink — Câmeras Android e IP como webcams virtuais"
---

# Tasks: CamLink — Câmeras Android e IP como webcams virtuais

**Input**: Design documents from `/specs/001-phone-webcam-bridge/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: OBRIGATÓRIOS (Constituição, Princípio III — Test-First). Todo teste é
escrito antes da implementação e deve FALHAR antes de implementar. Gates por
checkpoint: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`
(Linux e Windows).

**Organization**: por user story, para implementação e validação independentes.
Validação manual (hardware) por cenário do quickstart.md.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: paralelizável (arquivos diferentes, sem dependência pendente)
- **[Story]**: US1–US6 (mapeia spec.md)

## Path Conventions

Estrutura do plan.md: `src-tauri/` (Rust), `src/` (Svelte), `scrcpy/` (submodule
do fork, Java), `installer/{linux,windows}/`.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: projeto compilando nas duas plataformas com gates de qualidade ativos

- [X] T001 Criar scaffold Tauri 2.x + SvelteKit (pnpm) na raiz: `src-tauri/`, `src/`, `tauri.conf.json` conforme plan.md; `pnpm tauri dev` abre janela vazia
- [X] T002 Pinar toolchain e deps: `rust-toolchain.toml` (stable), `src-tauri/Cargo.toml` (tokio, serde, serde_json, tracing, tracing-subscriber, thiserror, keyring, uuid), rustfmt/clippy sem warnings
- [X] T003 [P] Adicionar submodule do fork scrcpy em `scrcpy/` (branch `camlink` sobre a tag estável mais recente ≥ 4.0) + doc de build do jar em `scrcpy/README.camlink.md`
- [X] T004 [P] CI em `.github/workflows/ci.yml`: fmt --check, clippy -D warnings, cargo test em `ubuntu-latest` e `windows-latest` (Princípio IV)
- [X] T005 [P] `LICENSE` GPL-3.0 (FR-025) + `README.md` esqueleto com visão do produto
- [X] T006 Stubs de módulos em `src-tauri/src/`: `device_manager.rs`, `stream_manager.rs`, `camera_controller.rs`, `rtsp_manager.rs`, `raw_manager.rs`, `secrets.rs`, `error.rs`, `config.rs`, `model.rs`, `virtualcam/{mod,v4l2,akvcam}.rs`; registrar em `lib.rs`; init do `tracing` em `main.rs`
- [X] T007 [P] Configurar `tauri.conf.json`: bundle id, targets (.deb, AppImage, NSIS/MSI), permissões IPC mínimas

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: tipos de domínio, infraestrutura de erro/config e os DOIS spikes de risco que gateiam a arquitetura

**⚠️ CRITICAL**: nenhuma user story começa antes desta fase completa

### Tests (escrever primeiro, ver FALHAR)

- [X] T008 [P] Testes de serialização/roundtrip dos tipos de domínio e transições de estado de `StreamSession` (Idle→Starting→Streaming→SourceLost→Reconnecting…, conforme data-model.md) em `src-tauri/tests/model_test.rs`
- [X] T009 [P] Testes de persistência de `AppConfig` (TOML, sem credenciais no arquivo — FR-018a) em `src-tauri/tests/config_test.rs`
- [X] T010 [P] Testes do trait `VirtualCameraBackend` com backend mock (create/feed/standby/destroy; invariante 1 fonte ↔ 1 device) em `src-tauri/tests/virtualcam_test.rs`

### Implementation

- [X] T011 Implementar tipos de domínio de data-model.md em `src-tauri/src/model.rs` (AndroidDevice, DeviceCapabilities, RtspSource, VirtualCamera, StreamSession + máquina de estados, ControlState, RawCaptureJob, AppConfig)
- [X] T012 [P] Implementar `src-tauri/src/error.rs`: `AppError { code, msg, action_hint }` + integração `tracing` (Princípio VI)
- [X] T013 [P] Implementar `src-tauri/src/config.rs`: load/save TOML no dir de config da plataforma
- [X] T014 Implementar `src-tauri/src/virtualcam/mod.rs`: trait `VirtualCameraBackend` (create/destroy/feed_frame/set_standby) + gerador de imagem de espera (imagem estática com logo + mensagem de estado, ex.: "Aguardando dispositivo…" — FR-006); mock backend para testes

### Spikes de risco (gates de arquitetura — critérios em research.md R1/R4)

- [X] T015 **Spike A** (gateia US2/US3/US5): buildar jar stock do submodule e validar via `SCRCPY_SERVER_PATH`; adicionar thread mínima `CamLinkControlServer` escutando `localabstract:camlink`; implementar `set_zoom` real e comprovar zoom mudando no stream headless (`--no-playback`) via `adb forward` — documentar hooks em `scrcpy/README.camlink.md`. **Critério de abortar**: sessão inacessível → pivotar para pipeline própria (APK) e replanejar
- [X] T016 **Spike B** (gateia US1 no Windows): instalar akvirtualcamera, empurrar frames de teste via FFI em `src-tauri/examples/win_vcam_spike.rs` e validar enumeração/vídeo em OBS, Chrome, Firefox e Discord no Windows 10 e 11 — registrar resultados em research.md R4. **Resultado: REPROVADO** (2026-07-10) — driver não registra o device no SO em Windows 11 (bug upstream do backend Media Foundation, akvirtualcamera/#95/#96); ver research.md R4 para a investigação completa. **Critério de abortar acionado**: pivotar para filtro DirectShow próprio → **T074 (Spike C)** gateia o novo T023
- [X] T074 **Spike C** (gateia T023, substitui a função de gate do T016 para o backend Windows): implementar filtro DirectShow push-source mínimo em Rust (`windows-rs`, feature `Win32_Media_DirectShow`) em `src-tauri/examples/win_dshow_spike.rs` — registro COM (CLSID próprio, sem copiar código GPL-2.0 da OBS), 1 formato RGB24/NV12, push de frames — validar enumeração/vídeo em OBS, Chrome, Firefox e Discord no Windows 10 e 11; registrar resultados em research.md R4. **Resultado: APROVADO** (2026-07-12, revalidado após bugs adicionais encontrados em teste manual) — validado via Chrome/Meet real (headless+CDP e não-headless), OBS real (padrão de cores exibido, sem travar) e cliente DirectShow genérico próprio (`win_dshow_connect_probe.rs`). 5 bugs de contrato COM encontrados e corrigidos no total (pFilter/pGraph nulos derrubavam o processo hospedeiro; pConnector nulo rejeitado por consumidores reais; property ID errado em IKsPropertySet; `IEnumMediaTypes::Skip` no-op causava sondagem infinita e travava a OBS de vez; `IMediaSample::SetTime` nunca chamado fazia a OBS descartar todo frame silenciosamente, tela preta — ver research.md R4 para detalhes de cada um). Falta validação manual em Firefox/Discord (não bloqueante — mesmo contrato COM já provado em dois consumidores reais distintos). **Critério de aceite adicional**: medir latência fim-a-fim do caminho decode→push; se exceder 70 ms (FR-004/SC-002), pausar e propor emenda à spec — pendente, sem processo real de push de frames decodificados ainda (fica para T023). **Critério de abortar**: não acionado.

**Checkpoint**: tipos+trait testados; spikes aprovados → user stories podem começar

---

## Phase 3: User Story 1 — Câmera Android via USB como webcam virtual (P1) 🎯 MVP

**Goal**: cabo conectado → webcam virtual funcionando em OBS/Chrome/Firefox/Discord, com preview, espera e reconexão

**Independent Test**: quickstart.md Cenário 1 (Linux e Windows)

### Tests for User Story 1 (REQUIRED) ⚠️

> Escrever PRIMEIRO, garantir que FALHAM antes de implementar

- [x] T017 [P] [US1] Testes do parser de `adb devices -l` (estados device/unauthorized/offline; Android < 12 → incompatível FR-002a) com fixtures em `src-tauri/tests/adb_parse_test.rs` — 16 testes, verde.
- [x] T018 [P] [US1] Testes de lifecycle do stream com binários fake de adb/scrcpy (Linux: subprocess fake do cliente; Windows: fake do bootstrap adb push/forward/app_process + socket TCP fake servindo o protocolo de frame do research.md R12) (start/stop/crash→Reconnecting→retomada; stderr → erro acionável) em `src-tauri/tests/stream_lifecycle_test.rs` — 8 testes com subprocess real (`tests/bin/fake_backend.rs`), verde no Windows (host desta sessão).
- [x] T019 [P] [US1] Testes do backend v4l2 (parsing de `v4l2loopback-ctl`, alocação dinâmica, fallback < 0.13, detecção Secure Boot) em `src-tauri/tests/v4l2_test.rs` — formatos verificados contra o código-fonte real do v4l2loopback-ctl upstream; arquivo é `#[cfg(target_os = "linux")]`, compila vazio no Windows — Red/Green real só verificável em CI/dev Linux.
- [x] T020 [P] [US1] Testes de montagem de linha de comando/argumentos scrcpy: Linux — flags do cliente (`--video-source=camera`, `--v4l2-sink`, codec/fps/bitrate por `StreamConfig`); Windows — argumentos de `Server.main()` para o bootstrap direto (`scid`, `tunnel_forward=true`, `video_source=camera`, `control=false`, `max_size`/`max_fps`/`video_bit_rate` por `StreamConfig`, research.md R12) em `src-tauri/tests/scrcpy_cmd_test.rs` — 8 testes, verde.

### Implementation for User Story 1

- [x] T021 [US1] Implementar `src-tauri/src/device_manager.rs`: exec `adb devices -l`, polling 500 ms, eventos `device_connected/disconnected/unauthorized` (≤ 3 s — FR-001), gate Android 12+ (FR-002a) — implementado, T017 verde.
- [x] T022 [P] [US1] Implementar `src-tauri/src/virtualcam/v4l2.rs`: add/delete via `v4l2loopback-ctl` + pkexec/polkit, `exclusive_caps`, fallback modprobe, diagnóstico Secure Boot (research R3) — implementado (entrega de frames via subprocesso ffmpeg, não ioctl manual — ver nota em R3); fallback modprobe automático ainda não acionado por `create()`; não compilável/testável nesta sessão (Windows) — precisa CI/dev Linux.
- [x] T023 [P] [US1] Implementar `src-tauri/src/virtualcam/dshow.rs`: filtro DirectShow próprio (registro COM, criar câmera, push de frames, standby) consolidando a Spike C (research R4; substitui o antigo `akvcam.rs`/FFI akvirtualcamera — ver T016) — implementado com ponte de memória compartilhada nova (produtor no processo do CamLink, leitor no processo consumidor — não existia na Spike C). Compila limpo (fmt+clippy) no Windows; **ainda não validado em OBS real** — precisa do mesmo ciclo de teste hands-on que achou os bugs #4/#5 da Spike C antes de ser considerado confiável. v1 suporta 1 câmera DirectShow por vez.
- [x] T024 [US1] Implementar `src-tauri/src/stream_manager.rs`: servidor stock (o jar forkado só entra em T037/US2) — Linux: subprocess cliente `scrcpy` com `--v4l2-sink` direto; Windows: sem cliente `scrcpy` (adb push do jar + adb forward + `app_process` replicando `Server.main()`, research.md R12) → parse do protocolo de frame do socket de vídeo → subprocesso ffmpeg (pipes) → `feed_frame`; diferença Linux/Windows isolada atrás de abstração dedicada (Princípio IV); lifecycle start/stop/auto-reconnect com standby (FR-005/006), captura de stderr → `AppError` — implementado e completo, incluindo a leitura do socket de vídeo Windows (handshake + session packet + frame headers, protocolo verificado byte-a-byte contra `DesktopConnection.java`/`doc/develop.md` do submodule) → decode ffmpeg → `FrameSink` (novo tipo, injetado por quem chama `start()`). T018/T020 verdes (lifecycle real testado via subprocess fake); 14 testes novos em `tests/scrcpy_protocol_test.rs` cobrindo o parsing do protocolo (vetores de bytes verificados à mão, não só round-trip). **Não testado**: a conexão TCP real + decode ffmpeg fim-a-fim (sem Android conectado nesta sessão) — falha nesse trecho degrada para standby, não derruba a sessão, mas o caminho em si só foi validado por compilação + testes de unidade dos parsers, não por hardware real. **Correção posterior (durante T028)**: `stop()` tinha uma race real — o processo do backend fica "emprestado" dentro da task de monitor a maior parte do tempo (`RunningSession.child` fica `None`), então `stop()` podia devolver sucesso sem matar o processo de verdade, e também falhava se chamado fora do estado `Streaming` (ex.: durante `Reconnecting`, a única transição da máquina de estados para `Stop` é `Streaming→Stopping`). Corrigido com `SessionControl` (sinal `Notify` + flag compartilhados com o monitor); adicionado teste de regressão `stop_while_reconnecting_reaches_idle` e stress test manual (8 execuções) confirmando via `tasklist` que nenhum `fake_backend.exe` fica órfão. **Validado em hardware real (2026-07-13, Samsung SM-G781B via `cargo tauri dev`)**: encontrados e corrigidos mais 4 bugs só visíveis com dispositivo/processo de verdade (binário ambíguo do `cargo run`, runtime Tokio ausente em `.setup()`, scid de 32 bits estourando o `Integer.parseInt` assinado do servidor real, conexão do socket de vídeo sem retry contra o boot da JVM) — detalhes em research.md R12. Pipeline de vídeo confirmado ponta a ponta até o handshake/frames chegando; câmera Samsung se desconecta sozinha após alguns segundos (quirk de brilho adaptativo já previsto em R1), reconexão automática funciona, backoff exponencial adicionado para não martelar o device. **Ainda não confirmado**: frame aparecendo de fato no OBS/Chrome via a câmera virtual DirectShow.
- [x] T025 [US1] Comandos/eventos Tauri de US1 conforme contracts/tauri-commands.md: `list_devices`, `start_stream`, `stop_stream`, `session_state`, `device_*` em `src-tauri/src/lib.rs` — implementado (`AppState` com `StreamManager` + `Box<dyn VirtualCameraBackend>` + cache de devices); compila limpo (fmt+clippy), binário completo builda. Camada fina de orquestração sem testes automatizados (Tauri commands não são unit-testáveis sem app rodando — R11 camada 3); caminhos externos (adb/scrcpy/ffmpeg/jar) resolvidos via PATH/env var, empacotamento bundled fica para depois.
- [x] T026 [US1] Preview 1 fps para fontes Android: leitura do device virtual (Linux: crate `v4l`; Windows: tap no pipe de decode) → evento `preview_frame` JPEG, frames descartáveis (FR-023) em `src-tauri/src/preview.rs` (novo arquivo — desvio deliberado de `stream_manager.rs`, ver nota) (preview de fontes RTSP: T051) — implementado: encode RGBA→JPEG e conversão YUYV422→RGBA são funções puras testadas (9 testes novos, incl. vetores conhecidos preto/branco). Windows: reaproveita os frames que já passam pelo `FrameSink` (sem ler o device de volta). Linux: lê de volta o device via crate `v4l` — **não compilável/testável nesta sessão** (mesma limitação Linux-only de T019/T022).
- [x] T027 [P] [US1] Frontend: `src/lib/DeviceList.svelte` (lista, estados, guia de autorização passo a passo — FR-002) + `src/lib/Preview.svelte` ("aguardando stream" quando inativo) — implementado (Svelte 5 runes) com `src/lib/types.ts`/`api.ts` (bindings tipados para os comandos/eventos Tauri). `svelte-check`: 0 erros, 0 avisos.
- [x] T028 [US1] Frontend: start/stop por dispositivo + `StreamConfig` (resolução/fps/bitrate/codec — FR-007) + indicadores de status/erro acionável em `src/routes/+page.svelte` — implementado. `svelte-check` e `vite build` (produção) limpos. Ao ligar o botão Parar descobri e corrigi um bug real em `stream_manager.rs::stop()` (ver nota em T024) — não visto antes porque nenhum teste chamava `stop()` fora do estado `Streaming`. **Não testado visualmente**: sem display neste ambiente, não rodei `cargo tauri dev`/abri a janela real — só verificação por compilação/type-check, nunca visual. Fica para T029.
- [x] T029 [US1] Validação manual: quickstart Cenário 1 completo em Linux **e** Windows (SC-001/002/003); registrar resultados no PR

**Checkpoint**: MVP entregável — US1 funcional nas duas plataformas

---

## Phase 4: User Story 2 — Controles de câmera em tempo real (P2)

**Goal**: zoom/foco/exposição/ISO/WB/EIS/torch/girar e espelhar a camera, escolher a camera traseira ou frontal, em runtime via fork, sem interromper o stream; troca frente/trás ≤ 2 s

**Independent Test**: quickstart.md Cenário 2

### Tests for User Story 2 (REQUIRED) ⚠️

- [x] T030 [P] [US2] Golden files do protocolo (request/response de todos os comandos + erros OUT_OF_RANGE/UNSUPPORTED/BAD_REQUEST) em `specs/001-phone-webcam-bridge/contracts/golden/` + teste de contrato Rust em `src-tauri/tests/protocol_contract_test.rs`
- [x] T031 [P] [US2] JUnit no fork validando os MESMOS golden files (parsing, validação contra capabilities, envelope ok/error) em `scrcpy/server/src/test/java/com/genymobile/scrcpy/camlink/ProtocolTest.java`
- [x] T032 [P] [US2] Testes do cliente de controle (adb forward, timeout, demux respostas × eventos `af_state`/`faces`, protocolo desconhecido → erro de versão) com servidor fake TCP em `src-tauri/tests/camera_controller_test.rs`
- [x] T075 [P] [US2] Teste do transform RGBA (mirror horizontal + rotação 180°, sem troca de dimensão; rotação 90°/270°, com troca width↔height) em `src-tauri/tests/frame_transform_test.rs`

### Implementation for User Story 2

- [x] T033 [US2] Fork: consolidar `CamLinkControlServer.java` (NDJSON robusto, envelope ok/error, `hello` com versão de protocolo) conforme contracts/control-protocol.md
- [x] T034 [US2] Fork: `get_capabilities` (cameras, zoom/iso/EV ranges, wb_modes, EIS, torch, RAW via `REQUEST_AVAILABLE_CAPABILITIES_RAW` + `getOutputSizes(RAW_SENSOR)`)
- [x] T035 [US2] Fork: comandos `set_zoom`, `set_torch`, `set_exposure`, `set_wb`, `set_eis` via rebuild da repeating request (sem reabrir câmera); validação server-side contra capabilities
- [x] T036 [US2] Fork: `set_focus` (continuous / tap→`AF_TRIGGER_START`+`CONTROL_AF_REGIONS`+cancel / manual→`LENS_FOCUS_DISTANCE`) + `set_iso` (AE off + `SENSOR_SENSITIVITY`/`EXPOSURE_TIME`, exige modo pro)
- [x] T037 [US2] Build reproduzível do jar `scrcpy-server-camlink` (script `scrcpy/build-camlink.sh`) + `stream_manager.rs` passa a usar `SCRCPY_SERVER_PATH`
- [x] T038 [US2] Implementar `src-tauri/src/camera_controller.rs`: túnel `adb forward`, cliente NDJSON com timeout, demux eventos, handshake de versão
- [x] T039 [US2] Comandos Tauri `get_capabilities`, `set_control`, `switch_camera` (restart do subprocess com `--camera-id`, indicador "trocando câmera", ≤ 2 s — FR-015) em `src-tauri/src/lib.rs`
- [x] T076 [US2] Implementar `src-tauri/src/frame_transform.rs`: `apply(frame: &[u8], width, height, rotation, mirror) -> (Vec<u8>, u32, u32)` — puro, sem I/O; registrar módulo em `lib.rs`
- [x] T077 [US2] Adicionar `rotation: Rotation` (enum `Deg0`/`Deg90`/`Deg180`/`Deg270`, default `Deg0`) e `mirror: bool` (default `false`) a `ControlState` em `model.rs`; cobrir no roundtrip do T008
- [x] T078 [US2] Pipeline de decode→push (`stream_manager.rs`, Linux **e** Windows): aplicar mirror + rotação 180° via `frame_transform::apply` antes do `FrameSink`, sem interromper o stream (resolução não muda — FR-016a)
- [x] T079 [US2] Rotação 90°/270° (troca width↔height): reaproveitar o caminho de restart de `switch_camera` (T039) pra recriar a câmera virtual (v4l2/dshow) já nas dimensões trocadas, mesmo orçamento ≤ 2 s (FR-015/FR-016a)
- [x] T080 [US2] Comando Tauri `set_control` (T039) passa a aceitar `rotation`/`mirror`; despacha pro caminho ao vivo (T078, 180°/mirror) ou pro restart (T079, 90°/270°) conforme o valor
- [x] T040 [P] [US2] Frontend: `src/lib/CameraControls.svelte` — sliders/toggles/select, tap-to-focus no preview (coords normalizadas), botão flip; habilitação estritamente por capabilities (FR-016)
- [x] T081 [P] [US2] Frontend: `CameraControls.svelte` — botão girar (cicla 0°→90°→180°→270°) + toggle espelhar horizontal (FR-016a)
- [x] T082 [US2] Build do jar do fork (`scrcpy-server-camlink`) numa máquina com JDK 17 + Android SDK (platform 36, build-tools 36.0.0). Concluído em 2026-07-24 em Linux via `scrcpy/build-camlink.sh` (2 bugs de primeira-compilação corrigidos, ver Notes) + validado em hardware real (SM-G781B, SM-N970F) contra o socket NDJSON — todos os comandos do contrato OK, exceto limitação conhecida documentada em `scrcpy/README.camlink.md` (quirk #4: `set_torch` desligar derruba o encoder em Snapdragon, não em Exynos)
- [x] T041 [US2] Validação manual: quickstart Cenário 2 em ≥ 2 fabricantes (quirks Camera2), incluindo girar/espelhar; SC-004; registrar no PR (depende de T082 — `SCRCPY_SERVER_PATH` apontando pro jar do fork). Concluído em 2026-07-30 — testado em hardware real em 2 fabricantes (Samsung SM-G781B/S20 FE e Motorola Moto G55). Zoom/foco/exposição/ISO/WB/EIS/torch e troca frontal/traseira OK (device v4l2 preservado, sem reseleção — FR-015). **Limitações conhecidas documentadas no README** (SC-004 não cumprida à risca no Linux): espelhar/girar 180° reiniciam a sessão no Linux (não só 90°/270° como previsto — pipeline `--v4l2-sink` não passa pelo `frame_transform::apply`); reconexão pós-restart pode entrar em ciclo instável em ambos os fabricantes testados (bug conhecido e aberto do próprio scrcpy com Camera2 HAL — Genymobile/scrcpy#6514/#5977/#5311), mitigado por circuit breaker mas sem causa raiz corrigida. Decisão final sobre essas limitações adiada para antes do release.

**Checkpoint**: US1 + US2 independentes e funcionais

---

## Phase 5: User Story 3 — Modos inteligentes (P2)

**Goal**: Auto/Night/Sport/Pro aplicados em runtime sem interromper o stream

**Independent Test**: quickstart.md Cenário 3

### Tests for User Story 3 (REQUIRED) ⚠️

- [x] T042 [P] [US3] Golden files de `set_mode` (4 modos + transições + Pro liberando manuais) e teste JUnit da tabela `ModePresets` (parâmetros Camera2 exatos da tabela do contrato) em `scrcpy/server/src/test/java/.../ModePresetsTest.java`
- [x] T043 [P] [US3] Teste Rust: `set_mode` sobrescreve `ControlState` conforme tabela (research R2) em `src-tauri/tests/mode_state_test.rs`

### Implementation for User Story 3

- [x] T044 [US3] Fork: `ModePresets.java` (tabela AF/AE/FPS/EV/AWB/EIS/NR por modo, sem SCENE_MODE) + `set_mode` + face-AF automático (`STATISTICS_FACE_DETECT_MODE_SIMPLE` → `AF_REGIONS` em auto/night) + eventos `faces`
- [x] T045 [US3] Comando Tauri `set_mode` + sincronização de `ControlState` na UI em `src-tauri/src/lib.rs`
- [x] T046 [P] [US3] Frontend: `src/lib/ModeSelector.svelte` (4 modos, indicador ativo, Pro habilita campos manuais)
- [x] T047 [US3] Validação manual: quickstart Cenário 3 (Sport 60 fps, Night +1EV/15–30 fps, Pro manual completo) — validado em hardware (SM-G781B): troca de modo não derruba o stream (SC-004), Night mais claro com fps 15–30, Pro libera ISO manual na UI. Sport medido em 30 fps estável (via `ffmpeg -f v4l2 -i /dev/videoX -f null -`) — confirmado via `dumpsys media.camera` que o aparelho não declara nenhum range de AE com 60 fps (só até `[30,30]`), então 30 fps é o teto real do hardware, coberto pela cláusula "quando o aparelho suporta" (spec.md:141). UI (`ModeSelector.svelte`) agora mostra a fps real ao lado do modo quando diverge do ideal, em vez de falhar silenciosamente (spec.md:239).

**Checkpoint**: US1–US3 independentes e funcionais

---

## Phase 6: User Story 4 — Câmeras IP/RTSP (P3)

**Goal**: fontes RTSP como webcams virtuais independentes, ≤ 300 ms, credenciais no cofre do SO

**Independent Test**: quickstart.md Cenário 4

### Tests for User Story 4 (REQUIRED) ⚠️

- [x] T048 [P] [US4] Testes de `secrets.rs` (store/retrieve/delete via keyring; config nunca contém senha — FR-018a) em `src-tauri/tests/secrets_test.rs`
- [x] T049 [P] [US4] Testes de montagem da pipeline ffmpeg low-delay (flags exatas de research R5, URL com credencial injetada só em runtime) e validação de URL (timeout 3 s, erro de auth distinto) em `src-tauri/tests/rtsp_test.rs`

### Implementation for User Story 4

- [x] T050 [US4] Implementar `src-tauri/src/secrets.rs` com crate `keyring` (Secret Service/Credential Manager)
- [x] T051 [US4] Implementar `src-tauri/src/rtsp_manager.rs`: subprocess ffmpeg (Linux `-f v4l2`; Windows rawvideo→`feed_frame`), lifecycle com standby/reconexão, erros de auth/host acionáveis; emitir `preview_frame` 1 fps também para sessões RTSP nas duas plataformas (Linux: leitura do device virtual; Windows: tap no pipe rawvideo — FR-023)
- [x] T052 [US4] Comandos Tauri `add_rtsp_source`/`remove_rtsp_source` (senha→keyring; remoção limpa o segredo), `start_rtsp`/`stop_rtsp` em `src-tauri/src/lib.rs`
- [x] T053 [P] [US4] Frontend: `src/lib/RtspPanel.svelte` (nome/URL/senha, status, remover)
- [ ] T054 [US4] Validação manual: quickstart Cenário 4 (mediamtx simulado + câmera real; latência ≤ 300 ms; senha ausente do arquivo de config) — **parcialmente validado (2026-08-03)**: fonte conecta (após corrigir a convenção de credencial na URL), latência ok, senha nunca aparece no `config.toml`, senha errada dá erro claro. **Bloqueado**: reconexão automática após queda da fonte não se autorrecupera de forma confiável — ver bug em "Débito técnico" abaixo. Fechar só depois desse bug resolvido.

**Checkpoint**: US4 funcional e independente (só requer Phase 2)

---

## Phase 7: User Story 5 — Captura RAW/DNG (P3)

**Goal**: snapshot e sequência 1–3 fps dinâmica salvos como DNG válidos, sem degradar o stream

**Independent Test**: quickstart.md Cenário 5

### Tests for User Story 5 (REQUIRED) ⚠️

- [x] T055 [P] [US5] Testes do receptor de framing binário (tag 0xD1, metadata + length-prefix, frames parciais/corrompidos, gravação com timestamp) em `src-tauri/tests/raw_framing_test.rs` — implementado junto o parser puro (`parse_frame`/`encode_frame`/`write_frame`) em `raw_manager.rs` (frame completo, streaming byte-a-byte, tag/metadata corrompidos, nome de arquivo por seq+timestamp)
- [x] T056 [P] [US5] Teste do cálculo de cadência dinâmica (frame_bytes × throughput medido → fps 1–3, prioridade do stream — FR-020) em `src-tauri/tests/raw_pacing_test.rs` — `throughput_for_raw`/`effective_raw_fps`/`granted_fps` em `raw_manager.rs`; fórmula é divisão (throughput ÷ frame_bytes), não multiplicação — o "×" do título da task é linguagem informal, não a operação real

### Implementation for User Story 5

- [x] T057 [US5] Fork: `RawCapture.java` — `ImageReader` RAW_SENSOR como surface extra, `DngCreator`, `raw_snapshot`/`raw_sequence_start|stop` com framing binário e `granted_fps`; `UNSUPPORTED`/`BUSY` conforme contrato — superfície RAW aditiva/condicional em `CameraCapture.java` (só quando `REQUEST_AVAILABLE_CAPABILITIES_RAW`, nunca em sessão high-speed), cadência recalculada a cada frame pelo throughput medido de escrita no socket (mesma fórmula do Rust). `./gradlew :server:testDebugUnitTest` e `build-camlink.sh` verdes (6 casos em `ProtocolTest`, incluindo BUSY e clamp de `granted_fps`; jar gerado). **Ainda sem validação em hardware real** (câmera/DngCreator/ImageReader não são testáveis em JVM unit test) — fica pro T061
- [x] T058 [US5] Implementar `src-tauri/src/raw_manager.rs`: recepção, gravação em `raw_output_dir`, progresso (`raw_progress`), throttle dinâmico — inclui a reescrita do `spawn_reader` em `camera_controller.rs` pra demultiplexar linha NDJSON e frame binário (`0xD1`) no mesmo socket (testado com `tokio::io::duplex`/servidor TCP fake em `camera_controller_test.rs`); `handle_incoming_frame`/`stop_sequence`/`RawJobRuntime` roteiam Snapshot vs. Sequência (`raw_job_test.rs`, 8 casos)
- [x] T059 [US5] Comandos Tauri `raw_snapshot`, `raw_sequence_start/stop`, `set_raw_output_dir` em `src-tauri/src/lib.rs` — job registrado ANTES do request pro fork (evita perder o primeiro frame numa corrida), `raw_output_dir` persiste entre `switch_camera`/rotação (`restart_android_session`). **Ainda não testável ponta a ponta**: depende do T057 (fork) pra existir alguém do outro lado do socket respondendo `raw_snapshot`/`raw_sequence_start` e mandando frames de verdade
- [x] T060 [P] [US5] Frontend: `src/lib/RawPanel.svelte` (snapshot, fps, progresso, dir de saída; oculto sem capability RAW — FR-016/019) — `pnpm check` limpo (0 erros/warnings)
- [x] T061 [US5] Validação manual: quickstart Cenário 5 (DNG abre no RawTherapee/Darktable em resolução nativa — SC-006; stream não degrada) — **validado (2026-08-06)**: Snapshot e duas Sequências RAW capturadas com sucesso num Galaxy S24 (SM-S921B), 16 DNGs de 23,8MB (4080×3060, 16bpp, sem compressão) confirmados íntegros — abrem corretamente no Darktable. Stream principal testado no OBS durante captura RAW: latência da câmera aumentou levemente, mas sem impacto na transmissão em si — SC-006 considerado atendido

**Checkpoint**: US5 funcional e independente

---

## Phase 8: User Story 6 — Múltiplas fontes simultâneas (P3)

**Goal**: Android + RTSP ao mesmo tempo, cada um no seu device virtual, falhas isoladas

**Independent Test**: quickstart.md Cenário 6

### Tests for User Story 6 (REQUIRED) ⚠️

- [x] T062 [P] [US6] Testes de orquestração multi-sessão (N sessões independentes, queda de uma não afeta outra, limite prático de 4, cleanup ao encerrar app) com backends mock em `src-tauri/tests/multi_session_test.rs` — 5 testes cobrindo 2 celulares Android + 2 fontes RTSP concorrentes (`four_mixed_sources_run_independently`, `one_session_crash_does_not_affect_others`, `stopping_one_leaves_others_running`, `fifth_concurrent_session_is_rejected`, `stopping_all_four_sources_completes_without_orphans`). Investigação revelou que `AppState.sessions`/`AppState.rtsp` (lib.rs) e `StreamManager`/`rtsp_manager::start_session` **já eram** registries multi-sessão (T063 em grande parte já feito estruturalmente) — o gap real era (1) `VIRTUAL_CAMERA_LABEL`/`RTSP_CAMERA_LABEL` globais fazendo uma 2ª fonte roubar o device v4l2 da 1ª, corrigido com `virtualcam::vcam_label(base, discriminator)` (serial/id da fonte) nos 3 call sites de `lib.rs`; (2) nenhum cap de 4 fontes, corrigido com `virtualcam::{MAX_CONCURRENT_SOURCES, check_capacity}` chamado em `start_stream`/`start_rtsp` antes de alocar qualquer recurso. Testes puros novos em `tests/v4l2_test.rs` (label). **Achado colateral importante**: `tests/bin/fake_backend.rs` não tratava `adb shell pkill/pgrep` (chamada de `spawn_backend` que mata um `scrcpy-server` remanescente antes de subir um novo) como setup rápido — caía no `stay_alive()` de 3600s, travando QUALQUER teste baseado em `StreamManager` nesta engine (inclusive os pré-existentes de `stream_lifecycle_test.rs`, que pareciam "lentos" mas na verdade estavam travados). Corrigido adicionando `pkill`/`pgrep` ao fast-path do fake — toda a suíte roda em segundos agora. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` e todos os arquivos de teste (17 arquivos, 100% verde) confirmados.

### Implementation for User Story 6

- [ ] T063 [US6] Refatorar `stream_manager.rs`/`rtsp_manager.rs` para registry de sessões concorrentes (`HashMap<SessionId, …>` sob tokio), isolamento de falha por sessão (FR-021) — **maior parte já estava feita** (ver nota do T062: registries e tasks por sessão já existiam); a fatia que realmente faltava (label único por fonte + cap de 4) foi implementada junto com T062. O que resta aqui, se algo: comportamento multi-instância do backend DShow no Windows (não verificado, follow-up)
- [x] T064 [US6] Frontend: UI multi-fonte (cards por sessão com preview/status/controles próprios) em `src/routes/+page.svelte` — design prototipado no Penpot (board "CamLink — Multi Source / Dark") antes de implementar, reaproveitando os componentes existentes (PreviewCard/ModeSelector/ControlsCard). Implementação: `ActiveSource`/`MAX_CONCURRENT_SOURCES` (`types.ts`, espelha `virtualcam::MAX_CONCURRENT_SOURCES`), `SourceCard.svelte` (card compacto: miniatura ao vivo via `Preview`, pill de status, nome/fps/meta, parar) e `SourceGrid.svelte` (grade + card fantasma "Adicionar fonte") novos; `+page.svelte` reescrito pra tratar `sources: ActiveSource[]` (Android + RTSP misturados) em vez de uma única sessão — `onSessionState`/`onSessionReplaced`/`onControlState` agora atualizam a fonte certa no array por `sessionId`; clique no card expande um painel abaixo da grade com Preview grande + Modo + Controles + RAW (só Android) reaproveitando os componentes existentes sem duplicação. `RtspPanel.svelte` refatorado: estado "rodando" por fonte deixou de ser interno (`running` local) e passou a vir de `activeIds` (prop derivada de `sources` no pai) — fonte única de verdade entre o card na grade e o painel lateral, evitando dessincronia. `pnpm check` limpo (0 erros); fluxo completo (selecionar → iniciar → aparece na grade → expande com controles reais → parar → remove e fecha o painel) validado via Playwright com Tauri mockado (sem hardware).
- [x] T065 [US6] Validação manual: quickstart Cenário 6 (duas fontes no OBS, queda isolada, preview com CPU < 5% — SC-007) — testado em hardware real (2026-08-08) com escopo ampliado: 3 celulares Android (SM-N970F, SM-G781B, SM-S921B) + 1 câmera RTSP simultâneos (as 4 fontes do cap de `MAX_CONCURRENT_SOURCES`). **Bug real encontrado e corrigido**: `scrcpy`/`adb` eram invocados sem `--serial`/`-s <serial>` no pipeline Linux (cliente scrcpy, cleanup `pkill`/`pgrep` pré-spawn) e no bootstrap do servidor Windows (`push`/`forward`/`app_process`) — com 1 device plugado o adb escolhia sozinho, mascarando o bug; com 2+ simultâneos o scrcpy recusava escolher ("Multiple ADB devices ... Select a device via -s") e a sessão entrava em loop infinito de reconexão, nunca chegando a rodar. Corrigido threading `serial: &str` por todo `spawn_backend`/`build_scrcpy_client_args(_oriented)`/`bootstrap_windows_server`/`monitor_session` em `stream_manager.rs`; teste de regressão `client_args_include_serial_to_disambiguate_multiple_devices` em `scrcpy_cmd_test.rs`. Depois do fix: as 4 fontes streimam de forma independente no OBS (latência subjetivamente maior com a carga de 4, mas sem impacto perceptível na transmissão — como já relatado em T061/SC-006), e a queda de 1 fonte (quirk de brilho adaptativo do SM-S921B reconectando sozinho, já documentado) não afetou as outras 3 — isolamento de falha (FR-021) confirmado em hardware com fontes reais, não só nos testes de T062. **Achado colateral, corrigido**: o preview *interno do app* (não o output do OBS) piscava entre a imagem e "Aguardando primeiro frame" nas 4 fontes sob essa carga — causa raiz: no Linux, todo source (Android via `--v4l2-sink` do scrcpy, RTSP via `-f v4l2` do ffmpeg) escrevia direto no device v4l2loopback, e como o módulo só admite 1 LEITOR por vez (`exclusive_caps=1`), o preview (que lê de volta do mesmo device) entrava em disputa constante com o OBS pelo slot assim que havia um consumidor real — indiferente de qual fonte, por isso todas piscavam junto. RTSP corrigido nesta sessão: Linux passou a usar o mesmo caminho sem read-back que o Windows já usa (ffmpeg decodifica rawvideo no stdout → `FrameSink` → `feed_frame` na câmera virtual + preview do mesmo buffer, `start_rtsp` em `lib.rs`), eliminando a disputa pra RTSP. **Android/Linux NÃO corrigido nesta sessão** (decisão consciente do usuário, ver Débito técnico abaixo) — exigiria portar o pipeline Android do Linux pro mesmo mecanismo de socket de vídeo + decode próprio que o Windows já usa (T024), escopo do tamanho da implementação original, feito como task separada com TDD completo em vez de ao vivo com hardware plugado.

**Checkpoint**: todas as user stories funcionais

---

## Phase 9: Polish & Cross-Cutting Concerns
- [ ] T066 [P] Instalador Linux: `installer/linux/install.sh` (deps, policy polkit, udev rules, modules-load.d) + bundle `.deb`/AppImage + `installer/linux/PKGBUILD` (AUR), jar do fork embutido (FR-024)
- [ ] T067 [P] Instalador Windows: NSIS/MSI via Tauri bundler com adb/scrcpy/ffmpeg embutidos + registro do filtro DirectShow próprio (`regsvr32` equivalente no instalador, sem driver de terceiros) em `installer/windows/` (FR-024)
- [ ] T068 [P] System tray + auto-connect de dispositivos lembrados (`AppConfig.auto_connect`) em `src-tauri/src/main.rs`
- [ ] T069 Launcher Firefox com `v4l2compat.so` (LD_PRELOAD) quando necessário, em `src-tauri/src/virtualcam/v4l2.rs` (edge case da spec)
- [ ] T070 Soak test 2 h com monitoramento de RSS/fps (SC-005) — roteiro no quickstart Cenário 7; corrigir vazamentos encontrados
- [ ] T071 Instalação limpa validada: Ubuntu 22.04/24.04, Arch, Windows 10 e 11 — primeiro vídeo sem terminal (SC-008, SC-010)
- [ ] T072 [P] Documentação de usuário final (`README.md` completo: instalação, autorização USB, troubleshooting/diagnósticos da tabela do quickstart)
- [ ] T073 Passe final de gates + quickstart completo nas duas plataformas; auditoria: sem `unwrap()` em caminho falível, `// SAFETY:` em todo unsafe FFI (Constituição VI); auditoria de privacidade (FR-026): confirmar que nenhuma dependência/código faz chamada de rede além das fontes RTSP configuradas, e documentar no README

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: sem dependências
- **Phase 2 (Foundational)**: depende de Phase 1 — **BLOQUEIA todas as stories**; T015 (Spike A) gateia Phases 4/5/7; T016 (Spike B) reprovou akvirtualcamera, T074 (Spike C, filtro DirectShow próprio) aprovado em seu lugar — Phase 3/T023 liberada para o lado Windows
- **Phase 3 (US1/MVP)**: depende de Phase 2
- **Phase 4 (US2)**: depende de Phase 3 (stream ativo) + T015
- **Phase 5 (US3)**: depende de Phase 4 (protocolo base)
- **Phase 6 (US4)**: depende só de Phase 2 — **pode rodar em paralelo** com Phases 3–5
- **Phase 7 (US5)**: depende de Phase 4 (socket de controle)
- **Phase 8 (US6)**: depende de Phases 3 e 6
- **Phase 9 (Polish)**: depende de todas as stories desejadas

### Within Each User Story

- Testes escritos e FALHANDO antes da implementação (NON-NEGOTIABLE)
- Fork (Java) antes do cliente Rust que o consome; backend antes de UI
- Story fecha com validação manual do cenário do quickstart nas DUAS plataformas

### Parallel Opportunities

- Setup: T003, T004, T005, T007 em paralelo após T001–T002
- Foundational: T008–T010 em paralelo; T012–T013 em paralelo; T015 ∥ T016 (máquinas diferentes)
- Todos os testes [P] de uma story em paralelo; frontend [P] em paralelo com validação de backend
- Trilha RTSP (Phase 6) inteira em paralelo com a trilha Android (Phases 3–5) — duas pessoas/agentes

---

## Parallel Example: User Story 1

```bash
# Testes primeiro, em paralelo:
Task: "T017 parser adb em src-tauri/tests/adb_parse_test.rs"
Task: "T018 lifecycle fake-scrcpy em src-tauri/tests/stream_lifecycle_test.rs"
Task: "T019 backend v4l2 em src-tauri/tests/v4l2_test.rs"
Task: "T020 cmdline scrcpy em src-tauri/tests/scrcpy_cmd_test.rs"

# Depois, implementação com paralelismo parcial:
Task: "T022 virtualcam/v4l2.rs"   # ∥ T023 (arquivos distintos)
Task: "T023 virtualcam/dshow.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Phases 1–2 (setup + foundational + spikes)
2. Phase 3 completa → **PARAR e VALIDAR**: quickstart Cenário 1 em Linux e Windows
3. MVP demonstrável: celular como webcam no OBS, sem app no telefone

### Incremental Delivery

1. MVP (US1) → release 0.1
2. +US2 (controles) → 0.2 · +US3 (modos) → 0.3
3. +US4 (RTSP, paralelizável desde cedo) → 0.4
4. +US5 (RAW) → 0.5 · +US6 (multi) → 0.6
5. Polish + instaladores → 1.0

### Parallel Team Strategy

Após Phase 2: Dev A → trilha Android (US1→US2→US3→US5); Dev B → trilha RTSP
(US4) + instaladores; qualquer um → US6 quando as duas trilhas fecharem.

---

## Débito técnico / melhorias futuras

- **Bloqueia T054**: bug de concorrência no device v4l2 entre o writer da
  sessão RTSP (Linux) e o leitor de snapshot do preview — reconexão
  automática após queda da fonte RTSP pode nunca se recuperar sozinha
  (documentado em `README.md` § Limitações conhecidas, achado em bancada
  2026-08-03). Evidência: `fuser -v` mostrou dois processos `ffmpeg`
  simultâneos no mesmo device (um vazado, nunca encerrado); mesmo depois de
  matar o processo vazado e liberar o device, o supervisor de reconexão não
  retomou sozinho — suspeita de deadlock envolvendo o leitor de preview
  bloqueado em `read_exact` sem writer ativo. **Possivelmente resolvido como
  efeito colateral do fix de T065** (2026-08-08): RTSP no Linux deixou de
  fazer read-back do device pra gerar preview (o leitor de snapshot que
  suspeitávamos travado nem existe mais nesse caminho) — precisa reabrir
  T054 e reconfirmar em bancada antes de fechar, mas a causa suspeita
  (leitor de preview vs. writer da sessão disputando o mesmo device) não se
  aplica mais à arquitetura atual.

- **Preview interno do app pisca sob carga com Android/Linux** (achado em
  bancada 2026-08-08, T065, com 3 celulares + 1 RTSP simultâneos + OBS
  consumindo as 4 fontes): mesma causa raiz do item acima (v4l2loopback só
  admite 1 leitor por vez — `exclusive_caps=1` — e o preview lê de volta do
  device disputando o slot com o OBS), mas só corrigido pro RTSP nesta
  sessão (ver nota do T065). Pra fontes Android no Linux o bug persiste: o
  cliente `scrcpy` ainda escreve direto no device via `--v4l2-sink`
  (`stream_manager.rs::spawn_backend`), então o preview continua fazendo
  read-back e piscando sob disputa real com um consumidor. **Não afeta o
  output real (OBS)** — só a miniatura de conveniência dentro do app.
  Correção completa exige portar o pipeline Android do Linux pro mesmo
  mecanismo que o Windows já usa (bootstrap direto do servidor via socket
  de vídeo — `bootstrap_windows_server`/`run_video_pipeline` em
  `stream_manager.rs`, hoje `#[cfg(target_os = "windows")]` — + decode
  próprio em vez do cliente `scrcpy`/`--v4l2-sink`), eliminando o read-back
  de vez, igual foi feito pro RTSP. Escopo do tamanho da implementação
  original do pipeline Android (T024): requer TDD completo (constituição
  Princípio III) e revalida toda a trilha MVP já validada em hardware
  (T029) — tratar como task própria, não ajuste pontual. Efeito colateral
  esperado se/quando corrigido: também resolveria o bug de giro/espelho
  precisando de F5 no Linux (item abaixo), já que passaria a aplicar
  `frame_transform` ao vivo como o Windows faz, em vez de reiniciar o
  cliente scrcpy via `--capture-orientation`.
  - **Diagnóstico D1 em bancada (2026-08-11)** — a premissa do read-back foi
    testada de verdade, fora do app, com writer sintético + leitores
    concorrentes nos devices ociosos (`/dev/video2-4`), porque duas coisas
    sugeriam que o limite podia não ser real:
    `/sys/module/v4l2loopback/parameters/max_openers = 10` e o fato de
    `exclusive_caps` controlar quais *capabilities* o device anuncia
    (research.md R3), não a contagem de leitores. **Resultado: o limite é
    real e a conclusão original está certa.** Com um leitor streamando, o
    segundo recebe `Error opening input: Device or resource busy` — falha no
    `open()`, não no `VIDIOC_S_FMT`, então fixar `-input_format`/`-video_size`
    (hipótese de conflito de formato) **não** ajuda: testado, mesmo EBUSY.
    Controle em `/dev/video3` (`exclusive_caps=0`) dá o MESMO EBUSY, ou seja
    a atribuição a `exclusive_caps` em `stream_manager.rs:252` é imprecisa —
    é limitação do v4l2loopback 0.15.3 em si (um leitor streamando por vez),
    independente desse parâmetro. **Conclusão: não existe fix barato do lado
    do backend; enquanto o Android/Linux usar `--v4l2-sink`, o preview não
    tem como coexistir com um consumidor real. O port continua sendo o único
    caminho pro read-back.**
  - **Diagnóstico D2 (2026-08-12) — o "piscar" eram DOIS defeitos somados, e
    o de frontend foi corrigido.** Ponto de partida: o backend NUNCA volta o
    preview pro placeholder — quando o snapshot falha, `run_preview_pipeline`
    só pula a rodada (`stream_manager.rs:1156-1161`) e nenhum evento é
    emitido, então o frame anterior continua na tela congelado. Logo o
    sintoma relatado ("volta pra *Aguardando primeiro frame*") só podia vir
    do frontend, e por exatamente dois caminhos: (a) `Preview.svelte` zerava
    `frameSrc` no `$effect`, cuja única dependência é `sessionId`; (b) o
    componente ser destruído e recriado, o que exige a chave do `{#each}`
    (`SourceGrid.svelte:23`, `source.id`) mudar. **Os dois caminhos tinham a
    MESMA origem**: `adoptSession`/`onSessionReplaced` reescreviam
    `ActiveSource.id` com o `session_id` novo, e no Linux TODO giro/espelho
    passa pelo restart (`lib.rs::set_orientation`, "Caminho de restart:
    Linux sempre"). Resultado: a cada giro o card inteiro era destruído e
    recriado com preview zerado — e aí o D1 entra como agravante, porque a
    sessão nova só consegue o primeiro frame quando o OBS soltar o device,
    deixando o placeholder pendurado por segundos em vez de ~200 ms.
    - Corrigido: `ActiveSource.id` virou identidade ESTÁVEL da fonte
      (`android:<serial>`; o RTSP já fazia assim com o id da fonte
      configurada) e só `sessionId` muda no restart. Some junto o remendo
      `if (expandedId === oldId) expandedId = newId` dos dois handlers.
    - Corrigido: `Preview.svelte` só descarta o frame quando `sessionId`
      vira `null` (sessão realmente encerrada). Numa troca de sessão o
      último frame fica na tela — é a mesma câmera física, e quem sinaliza
      problema é o status do card, não o sumiço da imagem.
    - As duas partes são necessárias: a identidade estável evita o remount
      (que zeraria o estado de qualquer jeito) e o `$effect` conservador
      evita o blank na troca de prop.
    - **Validado em bancada pelo usuário (2026-08-12): o preview parou de
      voltar ao placeholder.** Sem runner de teste no frontend (o projeto
      nunca teve — `pnpm check` só faz type-check), bancada é a única
      verificação possível, igual foi feito no T065c/T065e. Se o piscar
      reaparecer SEM giro/troca de câmera/parar-iniciar, é um terceiro
      gatilho ainda não identificado — nesse caso instrumentar o `$effect`
      do `Preview` e o handler de `onSessionState` é o próximo passo.
  - **Segundo achado do D2 (bancada 2026-08-12): os PAINÉIS de controle
    (Modo/Controles/RAW) piscavam e ficavam inclicáveis.** Bug irmão do
    anterior, mesma família (estado zerado antes de recarregar), mas nos três
    componentes que consomem `get_capabilities`. Cada um fazia
    `caps = null; getCapabilities(serial).then(...)` num `$effect` que
    depende de `sessionId` — e no Linux TODO giro reinicia a sessão. Zerar
    `caps` some com o painel inteiro (`{#if caps}` no markup) durante um
    round-trip até o aparelho que tem janela de retry
    (`get_capabilities_with_retry`), então cada giro apagava os controles por
    centenas de ms. Agravante: os três painéis pediam em paralelo o MESMO
    serial, e do lado Rust `get_capabilities` segura o lock de
    `state.sessions` durante o round-trip — as três chamadas serializavam.
    - Corrigido: os três param de zerar `caps` (os valores anteriores ficam
      na tela até chegar o novo) e ganharam guard `capsLoadedFor` — `let`
      cru, não `$state`, senão viraria dependência do efeito que a escreve.
    - Corrigido: `api.ts::getCapabilities` compartilha a chamada EM VOO por
      serial (3 round-trips viram 1). Só o pedido pendente é compartilhado,
      nada é cacheado entre chamadas — `switch_camera` troca a câmera aberta
      e com ela `iso_range`/`wb_modes`, que são características dela.
    - **Validado em bancada pelo usuário (2026-08-12): os painéis pararam de
      sumir e os controles voltaram a responder.**
    - Débito relacionado (não corrigido): recarregar capabilities a cada
      restart é conservador demais. Só `switch_camera` pode mudar o
      resultado; giro/espelho não. Dá pra recarregar só nesse caso, mas
      exige distinguir os dois no frontend — hoje ambos chegam como
      `session_replaced`/`onSessionChanged` indistinguíveis.

- **`run_preview_pipeline` não tem timeout no snapshot** (achado durante o
  D1, 2026-08-11 — bug independente do item acima, e barato de corrigir).
  `stream_manager.rs:1078-1095` faz `stdout.read_exact(&mut buf).await`
  seguido de `ffmpeg.wait().await` sem nenhum timeout: se o ffmpeg abre o
  device mas o frame nunca chega, a task fica pendurada pra sempre — o
  preview daquela sessão morre em definitivo (não volta mais) E o processo
  segue segurando o slot do device. **Evidência direta**: um `ffmpeg
  -frames:v 1` órfão de uma sessão anterior foi encontrado travado em
  `/dev/video8` por **1h55m** (confirmado com `ps -o etime`), sem writer
  nenhum no device. Piora dois sintomas: (a) explica o preview ficar preso
  no placeholder em vez de só engasgar; (b) um device segurado assim faz o
  `v4l2loopback-ctl delete` do `purge_all` (T065d) falhar com EBUSY, deixando
  device fantasma no OBS. Agrava também que esses `ffmpeg` **ignoram
  SIGTERM** quando bloqueados no v4l2 (reproduzido 3x no D1: só morrem com
  SIGKILL) — `kill_on_drop`/`child.kill()` do tokio manda SIGKILL, então o
  fix é envolver o snapshot num `tokio::time::timeout` e dropar o child.
  - **Corrigido (2026-08-11, TDD)**: snapshot extraído pra
    `stream_manager::capture_preview_snapshot`, com `PREVIEW_SNAPSHOT_TIMEOUT
    = 3s`; no caminho de timeout o futuro é dropado junto com o `Child`, o
    que dispara o SIGKILL do `kill_on_drop` (SIGTERM não serve, ver acima).
    `run_preview_pipeline` passou a só orquestrar o loop. Aproveitado pra
    corrigir uma inconsistência: era o ÚNICO subprocesso do módulo que não
    repassava `extra_env` (research.md R11) — agora repassa, o que também
    tornou o teste determinístico (sem `set_var` global, que dava corrida
    entre testes paralelos no mesmo processo).
  - Testes: `tests/preview_snapshot_test.rs` (2 casos — desiste no timeout,
    e MATA o processo ao desistir, provado lendo o PID que o fake grava e
    conferindo `/proc/<pid>`), com modo `hang` novo no `fake_backend`.

- **Devices fantasma reapareceram depois da sessão de teste do usuário**
  (2026-08-11, T065d não cobre este caminho): com o app e o OBS fechados,
  `v4l2loopback-ctl list` ainda mostrava `/dev/video5-8` (`CamLink IP
  (teste-1 #2469)`, `CamLink Android (SM_S921B (P11P`, `... (camera para o `,
  `... (camera teto (P`) — e `fuser` mostrava **ninguém** segurando video5/6/7,
  ou seja o `delete` teria funcionado: o `purge_all` simplesmente não rodou.
  Hipótese principal: a sessão foi encerrada por `Ctrl+C` no `pnpm tauri
  dev`, e o `pnpm`/`cargo run` não repassa o sinal pro binário filho — o
  mesmo cenário que o comentário de `watch_for_shutdown_signal`
  (`lib.rs:1762-1771`) já descreve pro bug do `scrcpy` órfão. O
  `RunEvent::ExitRequested` (fechar a janela) e o SIGTERM direto no binário
  já foram validados; falta cobrir "processo pai morre sem repassar sinal".
  Candidatos: limpar devices órfãos no START (o `cleanup_stale()` já roda
  na construção do backend mas só colapsa duplicatas do mesmo label — não
  remove label sem dono), ou não depender só de sinal.
  - **Corrigido (2026-08-11, TDD)** pelo caminho do START, que cobre também
    crash (nenhum hook de saída roda) e não só o sinal perdido:
    `cleanup_stale()` passou a apagar TODO device cujo label começa com
    `virtualcam::LABEL_PREFIX` (`v4l2::orphan_devices`, função pura). Roda
    na construção do backend, quando nenhuma sessão existe ainda, e é
    best-effort: se um consumidor estiver com o device aberto o `delete`
    falha com EBUSY e o device fica (e é reaproveitado). Não conflita com o
    reuso entre sessões porque desde o T065d o app já apaga os próprios
    devices ao encerrar — nada deveria sobreviver entre execuções.
  - Pra o prefixo ser confiável como "dono", os labels-base saíram de
    `lib.rs` e viraram `virtualcam::LABEL_PREFIX` (fonte única), com teste
    fechando o ciclo cria→reconhece
    (`labels_created_by_this_app_are_recognized_as_its_own_orphans`) e um
    caso explícito garantindo que `CamDroidLink A/B/C` (de outro app, reais
    nesta máquina) e `OBS Virtual Camera` NUNCA são tocados.
  - Devices `/dev/video5-8` da sessão do usuário removidos manualmente
    nesta sessão, depois de confirmar que estavam livres.

- **Truncamento do label corta no meio da estrutura** (visível nos devices
  acima): `MAX_LABEL_LEN = 31` corta cru, produzindo
  `CamLink Android (SM_S921B (P11P` e `CamLink Android (camera para o ` —
  parêntese aberto sem fechar e nome cortado no meio, exatamente no seletor
  do OBS que o T065c/T065e queriam deixar legível. O orçamento de 31 chars
  é gasto pelo prefixo fixo `CamLink Android (` (17 chars), sobrando 14 pro
  nome+sufixo. **Pior que feio: o corte podia comer o próprio sufixo de
  unicidade**, e o sufixo é a chave que impede duas fontes de roubarem o
  device uma da outra (`find_reusable_device`, bug do T062).
  - **Corrigido (2026-08-11, TDD)**, formato escolhido pelo usuário:
    `CamLink {nome} {sufixo}` (ex.: `CamLink camera teto P11P`,
    `CamLink SM_S921B P11P`, `CamLink teste-1 2469`). Tirar o
    `Android`/`IP` do meio libera 8-10 chars: o orçamento do nome foi de 8
    pra **18** chars, e o `CamLink` na frente mantém as fontes agrupadas na
    lista do OBS.
  - `vcam_label(name, suffix)` agora recebe as duas partes separadas e
    trunca SÓ o nome, nunca o sufixo — antes recebia a string já composta,
    o que tornava impossível proteger o sufixo. `android_label_discriminator`
    /`rtsp_label_discriminator` viraram `android_label_parts`/
    `rtsp_label_parts`, devolvendo `(nome, sufixo)`.
  - Teste dedicado pro risco de colisão
    (`vcam_label_keeps_the_whole_suffix_even_when_the_name_is_truncated`):
    dois nomes longos IGUAIS com sufixos diferentes têm que continuar
    gerando labels diferentes.

- Bug de giro/espelho no Linux exigindo F5 no Meet/Chrome (não bloqueia
  fases atuais, documentado em
  `README.md` § Limitações conhecidas) — reproduzido em 2 aparelhos
  diferentes (SM-G781B e Moto G55, 2026-08-03), então não é peculiaridade de
  fabricante. Decisão de investigar/corrigir adiada a pedido do usuário;
  candidato a investigação futura (possível causa: o Meet não relê o device
  v4l2 depois do restart do processo scrcpy).

- **T065c — Nome amigável das câmeras no seletor do OBS** (pedido do
  usuário, 2026-08-10, após validar T065 em bancada com 4 fontes: "confuso
  saber qual é qual"). Pesquisa feita nesta sessão:
  - O device v4l2/DirectShow já é único por fonte desde T062
    (`virtualcam::vcam_label(base, discriminator)`), mas o `discriminator`
    hoje é o dado técnico bruto: `serial` do adb pra Android
    (`lib.rs:482,1090`) e o `Uuid` da fonte pra RTSP (`lib.rs:1491`) — daí
    o nome que aparece no OBS ser algo como `CamLink Android (R58M12ABCDE)`
    ou `CamLink IP (a1b2c3d4-...)`, sem relação com o aparelho/câmera real.
  - **RTSP já tem o dado certo pronto pra usar**: `RtspSource.name`
    (`model.rs:109`) é o nome amigável que o próprio usuário digita no
    formulário (`RtspPanel.svelte`, ex.: "Câmera do portão") — só não está
    sendo usado como discriminador. Troca direta: `id.to_string()` →
    `source.name` em `start_rtsp` (`lib.rs:1491`).
  - **Android tem quase tudo pronto**: `AndroidDevice.model` (`model.rs:29`,
    ex.: `"SM-N970F"`) já vem do `adb devices -l` e já está cacheado em
    `AppState.devices` (`lib.rs:231`) — `start_stream`/`restart_android_session`
    só recebem `serial: String` do frontend, mas podem resolver o `model`
    olhando `state.devices.lock()` pelo `serial` sem mudar a API do
    frontend. Precisa de um discriminador estável mesmo com 2 aparelhos do
    MESMO modelo plugados ao mesmo tempo — usar
    `format!("{model} ({sufixo_do_serial})")` (ex.: 4 últimos chars do
    serial) em vez do model sozinho.
  - **Risco a tratar**: `card_label` do v4l2loopback tem limite de tamanho
    do kernel (struct de 32 bytes) — hoje não há truncamento/sanitização
    em `vcam_label`/`build_add_args`; nomes de RTSP digitados livremente
    pelo usuário (ou modelos Android + sufixo) podem estourar isso e falhar
    silenciosamente ou truncar de forma feia. Precisa de um limite
    explícito + teste (`tests/v4l2_test.rs`) antes de expor no OBS.
  - **Implementado em 2026-08-10** (mesma sessão do planejamento, TDD):
    1. `virtualcam::sanitize_label(raw, max_len)` pura (trim, colapsa
       espaços, corta por `char` — não por byte — em `MAX_LABEL_LEN = 31`,
       o limite do `card_label` do v4l2loopback) + `vcam_label` passou a
       sanitizar sempre o resultado composto.
    2. `virtualcam::android_label_discriminator(devices, serial)`: resolve
       `AndroidDevice.model` via `AppState.devices` (já cacheado) e monta
       `"{model} ({4 últimos chars do serial})"` (ex.: `"SM-N970F (BCDE)"`);
       cai pro serial puro se o device ainda não estiver no cache ou o
       model vier vazio. Usado em `start_stream` (`lib.rs`) e
       `restart_android_session`.
    3. `virtualcam::rtsp_label_discriminator(name, id)`: usa
       `RtspSource.name` + um sufixo curto do `Uuid` (ex.:
       `"Câmera do portão #a1b2"`) — o sufixo é necessário porque o
       discriminador também é a CHAVE de unicidade do device
       (`find_reusable_device`); sem ele, duas fontes RTSP com o mesmo
       nome roubariam o device uma da outra (mesmo bug do T062). Usado em
       `start_rtsp`.
    4. 11 testes novos (`virtualcam::tests`, inline em `mod.rs` — módulo
       cross-platform, não Linux-only como `v4l2_test.rs`): sanitização
       (trim/colapso/corte seguro em UTF-8), `vcam_label` nunca estoura
       `MAX_LABEL_LEN`, discriminador Android usa modelo+sufixo e
       desambigua 2 aparelhos do mesmo modelo, discriminador RTSP usa o
       nome e desambigua 2 fontes com o mesmo nome. Suíte completa (21
       arquivos), `cargo fmt --check` e `cargo clippy --all-targets -- -D
       warnings` confirmados limpos.
    5. **Não verificado ainda**: confirmação visual em bancada (nome novo
       aparecendo certo no seletor de câmera do OBS/Chrome com 2+ fontes
       do mesmo modelo Android plugadas) e no Windows/DirectShow (mesmo
       `label` é consumido, mas o comportamento real do filtro com esses
       nomes não foi testado nessa plataforma).

- **T065d — Device v4l2 "fantasma" no OBS depois de fechar o app** (achado
  pelo usuário, 2026-08-11: abriu o OBS sem o CamLink rodando e ainda via
  câmeras "CamLink ..." listadas, inacessíveis). Causa: `V4l2Backend::destroy`
  (`v4l2.rs:379`) deliberadamente NÃO apaga o device v4l2loopback — ele é
  reaproveitado entre restarts (troca de câmera, reconexão) enquanto o app
  continua rodando (comportamento correto, T062). O problema é que os dois
  hooks de encerramento do processo inteiro (`RunEvent::ExitRequested` em
  `lib.rs:1883` e `watch_for_shutdown_signal` em `lib.rs:1772`, ambos
  adicionados em 2026-07-28 pro bug do `scrcpy` órfão) só matavam os
  backends de CAPTURA (scrcpy/ffmpeg de origem) — nunca chamavam nada em
  `state.vcam`. O `ffmpeg` que escrevia no device v4l2 morria sozinho (stdin
  fecha quando o processo pai sai), mas o device continuava registrado no
  kernel sem ninguém escrevendo — daí "inacessível" no OBS até o próximo
  `cargo tauri dev`/app start rodar `cleanup_stale()` (que só reaproveita 1
  device por label, não some com ele).
  - **Corrigido**: novo método `VirtualCameraBackend::purge_all(&mut self)`
    (default no-op — Windows recria o filtro DirectShow do zero a cada
    `start_stream`, nada fica pendurado ao fechar). `V4l2Backend::purge_all`
    dreina `self.cameras`, mata cada `ffmpeg` (via `Drop` de `ManagedCamera`)
    e chama `v4l2loopback-ctl delete <path>` pra cada device — best-effort,
    igual ao `cleanup_duplicates` existente. Fiado nos dois hooks de
    shutdown (`lib.rs`): precisou trocar `Arc::new(StdMutex::new(new_vcam_backend()))`
    de dentro do `AppState` pra uma variável `vcam` construída antes,
    clonada tanto pro `app_state` quanto pros dois hooks
    (`vcam_for_exit`/parâmetro novo de `watch_for_shutdown_signal`).
  - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` e
    `cargo test` (227 testes, 21 suítes) confirmados limpos.
  - **Validado em bancada (2026-08-11)**: fonte RTSP ativa, `SIGTERM` no
    processo — log confirma `device v4l2 removido ao fechar o app` e o
    device some do `v4l2loopback-ctl list` (não fica só sem writer). Cobre
    o caminho de `watch_for_shutdown_signal`; o caminho de
    `RunEvent::ExitRequested` (fechar pela janela) usa o mesmo
    `purge_all()`, mas não foi clicado fisicamente — risco residual baixo,
    mesmo código.

- **T054 — reconexão RTSP travada, causa real encontrada e corrigida**
  (reaberto pelo usuário, 2026-08-11, testando com mediamtx+ffmpeg: "está
  conectando e reconectando" — ou seja, o supervisor de reconexão do
  `start_session` já funciona bem — "porém, ao iniciar o app a câmera só
  conecta quando eu dou esse comando: `ss -tnp | grep 8554`"). Causa: não é
  o `ss` em si (comando somente-leitura, sem efeito colateral em rede) —
  `rtsp_manager::probe_url` faz UMA tentativa só, com timeout de 3s
  (`PROBE_TIMEOUT`), ANTES de `start_rtsp` alocar qualquer recurso; uma
  fonte recém-iniciada (câmera IP ligando, ou o publicador ffmpeg do setup
  de teste subindo) pode não estar pronta pra entregar o 1º frame nesses
  3s — a falha aparece na hora como "Falha ao conectar / Câmera
  inacessível", e o tempo que o usuário levou pra digitar o comando de
  diagnóstico foi o que deu à fonte tempo de "esquentar" antes do próximo
  clique em iniciar, não o comando em si.
  - **Corrigido**: `rtsp_manager::probe_url_with_retry` repete o probe até
    `PROBE_MAX_ATTEMPTS = 3` vezes, com `PROBE_RETRY_DELAY = 1500ms` entre
    tentativas — devolve o erro da ÚLTIMA tentativa se todas falharem
    (continua acionável, auth vs. inacessível). `probe_url` ganhou um
    parâmetro `extra_env` (antes só usado no bootstrap Windows do adb) pra
    permitir injetar `FAKE_BACKEND_*` isolado por teste sem mexer no
    ambiente global do processo.
  - Novo modo `fail_then_succeed` em `tests/bin/fake_backend.rs` (conta
    tentativas num marker file, falha as N primeiras, sucede na seguinte —
    sai rápido em vez de ficar de pé, diferente do `crash_once` existente)
    + 2 testes novos em `rtsp_test.rs` cobrindo "sucede assim que a fonte
    fica pronta" e "desiste depois de `max_attempts`".
  - **Não verificado ainda**: bancada com câmera IP real (só testado com o
    setup de mediamtx+ffmpeg desta sessão) — uma câmera real pode levar bem
    mais que 3×3s pra ligar de vez.

- **T065e — Apelido de câmera Android** (pedido do usuário, 2026-08-11,
  depois de validar T065c em bancada: o nome amigável mostra o modelo bruto
  do adb, ex. `"SM-S921B (BCDE)"`, não o nome de marketing "Galaxy S24" —
  o app não tem como saber esse mapeamento sozinho). Usuário pode definir
  um apelido por serial (ex. "Câmera lateral"), editável direto no card da
  fonte (lápis ao lado do nome, só em fontes Android — RTSP já tem nome
  próprio no cadastro).
  - `AppConfig.device_nicknames: HashMap<String, String>` (chave = serial,
    mesmo padrão de `last_stream_config`), persistido em `config.toml`.
    Comandos `set_device_nickname(serial, nickname)` (nickname vazio
    remove) e `list_device_nicknames()`.
  - `virtualcam::android_label_discriminator` ganhou um parâmetro
    `nickname: Option<&str>`: apelido tem prioridade sobre o modelo, com o
    mesmo sufixo de serial pra desambiguar (o discriminador também é a
    chave de unicidade do device virtual — 2 aparelhos com o mesmo apelido
    não podem colidir). 3 testes novos cobrindo prioridade,
    desambiguação e fallback com apelido em branco.
  - Frontend: edição inline (lápis → input → Enter/blur salva, Esc cancela)
    em dois pontos — `SourceCard.svelte` (fonte já ativa) e
    `DeviceList.svelte` (dispositivo ainda não iniciado); `+page.svelte`
    carrega os apelidos uma vez (`onMount`) e é a fonte única de verdade
    passada como prop pros dois (`nicknames`/`onRename`), evitando
    dessincronia entre o seletor e o card ativo.
  - **Limitação real descoberta em bancada** (2026-08-11, testado pelo
    usuário): `v4l2loopback-ctl` não tem verbo de renomear um device já
    criado (só `add`/`delete`) — renomear com a fonte JÁ transmitindo só
    atualiza o nome dentro do próprio app (`SourceCard`); o OBS continua
    mostrando o nome antigo até a fonte ser parada e iniciada de novo (o
    device virtual, criado com o label antigo, precisaria ser recriado).
    **Decisão do usuário**: não vale a pena forçar um restart automático ao
    renomear ao vivo (trocaria o `/dev/videoN`, exigindo reselecionar a
    fonte no OBS) — o caminho recomendado é renomear ANTES de iniciar, via
    `DeviceList`, onde o device ainda nem existe.
  - `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
    (232 testes) e `pnpm check` (0 erros/warnings) confirmados limpos.
  - **Não verificado ainda**: bancada — apelido definido em `DeviceList`
    ANTES de iniciar e confirmar que aparece certo no OBS desde o 1º start.

## Notes

- Spike A tem critério de abortar explícito (T015) — replaneje ANTES de investir nas Phases 4/5/7 se falhar
- Spike B (T016) abortou em 2026-07-10 (akvirtualcamera reprovado no Windows 11 — research.md R4); T074 (Spike C, filtro DirectShow próprio) aprovado no mesmo dia — libera T023 (implementação completa, canalizando frames reais do pipeline decode→push)
- T075–T081 (girar/espelhar) adicionados em 2026-07-20 ao Goal da US2 (spec.md FR-016a/SC-004) — transform RGBA local (`frame_transform.rs`), fora do protocolo do fork: mirror/180° não mudam resolução e aplicam ao vivo (T078); 90°/270° trocam width↔height e reaproveitam o restart de `switch_camera` (T079), mesmo orçamento ≤ 2 s de FR-015
- **Desvio de implementação do T078 no Linux** (2026-07-20): os frames Linux não passam pelo CamLink (scrcpy → v4l2loopback direto), então o transform local só é possível no Windows; no Linux TODA mudança de orientação usa `--capture-orientation` do scrcpy (GPU do celular) com restart do cliente (breve interrupção; device v4l2 persiste). `frame_transform::apply` continua cross-platform e testado (T075) — é o caminho quente do Windows
- T031/T037/T082 validados em 2026-07-24 (ambiente JDK 17 + SDK montado em Linux): 2 bugs de primeira-compilação corrigidos (`build-camlink.sh` usava task inexistente `testReleaseUnitTest` — este módulo só gera `testDebugUnitTest`; `ProtocolTest.java` usava `JSONObject.similar()`/`JSONException` unchecked, mas o `org.json` real do `android.jar` (compileSdk 36) é mais antigo, sem `similar()` e com `JSONException` checked — trocado por comparação recursiva manual). `./gradlew :server:testDebugUnitTest` verde (24 golden cases); `build-camlink.sh` gera o jar. Validado em hardware real (SM-G781B e SM-N970F) contra o socket NDJSON: todos os comandos OK, exceto limitação conhecida documentada em `scrcpy/README.camlink.md` (quirk #4 — `set_torch` desligar derruba o encoder Qualcomm em Snapdragon, não reproduz em Exynos)
- Golden files (`contracts/golden/`) são a fonte única de verdade do protocolo — Rust e Java testam contra os mesmos arquivos
- Validações manuais (T029, T041, T047, T054, T061, T065, T071) são gates de checkpoint: não avançar de story com elas pendentes
- Commit após cada task ou grupo lógico; PRs por story
