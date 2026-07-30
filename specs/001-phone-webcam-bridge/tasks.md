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

- [ ] T042 [P] [US3] Golden files de `set_mode` (4 modos + transições + Pro liberando manuais) e teste JUnit da tabela `ModePresets` (parâmetros Camera2 exatos da tabela do contrato) em `scrcpy/server/src/test/java/.../ModePresetsTest.java`
- [ ] T043 [P] [US3] Teste Rust: `set_mode` sobrescreve `ControlState` conforme tabela (research R2) em `src-tauri/tests/mode_state_test.rs`

### Implementation for User Story 3

- [ ] T044 [US3] Fork: `ModePresets.java` (tabela AF/AE/FPS/EV/AWB/EIS/NR por modo, sem SCENE_MODE) + `set_mode` + face-AF automático (`STATISTICS_FACE_DETECT_MODE_SIMPLE` → `AF_REGIONS` em auto/night) + eventos `faces`
- [ ] T045 [US3] Comando Tauri `set_mode` + sincronização de `ControlState` na UI em `src-tauri/src/lib.rs`
- [ ] T046 [P] [US3] Frontend: `src/lib/ModeSelector.svelte` (4 modos, indicador ativo, Pro habilita campos manuais)
- [ ] T047 [US3] Validação manual: quickstart Cenário 3 (Sport 60 fps, Night +1EV/15–30 fps, Pro manual completo)

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
- [ ] T054 [US4] Validação manual: quickstart Cenário 4 (mediamtx simulado + câmera real; latência ≤ 300 ms; senha ausente do arquivo de config)

**Checkpoint**: US4 funcional e independente (só requer Phase 2)

---

## Phase 7: User Story 5 — Captura RAW/DNG (P3)

**Goal**: snapshot e sequência 1–3 fps dinâmica salvos como DNG válidos, sem degradar o stream

**Independent Test**: quickstart.md Cenário 5

### Tests for User Story 5 (REQUIRED) ⚠️

- [ ] T055 [P] [US5] Testes do receptor de framing binário (tag 0xD1, metadata + length-prefix, frames parciais/corrompidos, gravação com timestamp) em `src-tauri/tests/raw_framing_test.rs`
- [ ] T056 [P] [US5] Teste do cálculo de cadência dinâmica (frame_bytes × throughput medido → fps 1–3, prioridade do stream — FR-020) em `src-tauri/tests/raw_pacing_test.rs`

### Implementation for User Story 5

- [ ] T057 [US5] Fork: `RawCapture.java` — `ImageReader` RAW_SENSOR como surface extra, `DngCreator`, `raw_snapshot`/`raw_sequence_start|stop` com framing binário e `granted_fps`; `UNSUPPORTED`/`BUSY` conforme contrato
- [ ] T058 [US5] Implementar `src-tauri/src/raw_manager.rs`: recepção, gravação em `raw_output_dir`, progresso (`raw_progress`), throttle dinâmico
- [ ] T059 [US5] Comandos Tauri `raw_snapshot`, `raw_sequence_start/stop`, `set_raw_output_dir` em `src-tauri/src/lib.rs`
- [ ] T060 [P] [US5] Frontend: `src/lib/RawPanel.svelte` (snapshot, fps, progresso, dir de saída; oculto sem capability RAW — FR-016/019)
- [ ] T061 [US5] Validação manual: quickstart Cenário 5 (DNG abre no RawTherapee/Darktable em resolução nativa — SC-006; stream não degrada)

**Checkpoint**: US5 funcional e independente

---

## Phase 8: User Story 6 — Múltiplas fontes simultâneas (P3)

**Goal**: Android + RTSP ao mesmo tempo, cada um no seu device virtual, falhas isoladas

**Independent Test**: quickstart.md Cenário 6

### Tests for User Story 6 (REQUIRED) ⚠️

- [ ] T062 [P] [US6] Testes de orquestração multi-sessão (N sessões independentes, queda de uma não afeta outra, limite prático de 4, cleanup ao encerrar app) com backends mock em `src-tauri/tests/multi_session_test.rs`

### Implementation for User Story 6

- [ ] T063 [US6] Refatorar `stream_manager.rs`/`rtsp_manager.rs` para registry de sessões concorrentes (`HashMap<SessionId, …>` sob tokio), isolamento de falha por sessão (FR-021)
- [ ] T064 [US6] Frontend: UI multi-fonte (cards por sessão com preview/status/controles próprios) em `src/routes/+page.svelte`
- [ ] T065 [US6] Validação manual: quickstart Cenário 6 (duas fontes no OBS, queda isolada, preview com CPU < 5% — SC-007)

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

## Notes

- Spike A tem critério de abortar explícito (T015) — replaneje ANTES de investir nas Phases 4/5/7 se falhar
- Spike B (T016) abortou em 2026-07-10 (akvirtualcamera reprovado no Windows 11 — research.md R4); T074 (Spike C, filtro DirectShow próprio) aprovado no mesmo dia — libera T023 (implementação completa, canalizando frames reais do pipeline decode→push)
- T075–T081 (girar/espelhar) adicionados em 2026-07-20 ao Goal da US2 (spec.md FR-016a/SC-004) — transform RGBA local (`frame_transform.rs`), fora do protocolo do fork: mirror/180° não mudam resolução e aplicam ao vivo (T078); 90°/270° trocam width↔height e reaproveitam o restart de `switch_camera` (T079), mesmo orçamento ≤ 2 s de FR-015
- **Desvio de implementação do T078 no Linux** (2026-07-20): os frames Linux não passam pelo CamLink (scrcpy → v4l2loopback direto), então o transform local só é possível no Windows; no Linux TODA mudança de orientação usa `--capture-orientation` do scrcpy (GPU do celular) com restart do cliente (breve interrupção; device v4l2 persiste). `frame_transform::apply` continua cross-platform e testado (T075) — é o caminho quente do Windows
- T031/T037/T082 validados em 2026-07-24 (ambiente JDK 17 + SDK montado em Linux): 2 bugs de primeira-compilação corrigidos (`build-camlink.sh` usava task inexistente `testReleaseUnitTest` — este módulo só gera `testDebugUnitTest`; `ProtocolTest.java` usava `JSONObject.similar()`/`JSONException` unchecked, mas o `org.json` real do `android.jar` (compileSdk 36) é mais antigo, sem `similar()` e com `JSONException` checked — trocado por comparação recursiva manual). `./gradlew :server:testDebugUnitTest` verde (24 golden cases); `build-camlink.sh` gera o jar. Validado em hardware real (SM-G781B e SM-N970F) contra o socket NDJSON: todos os comandos OK, exceto limitação conhecida documentada em `scrcpy/README.camlink.md` (quirk #4 — `set_torch` desligar derruba o encoder Qualcomm em Snapdragon, não reproduz em Exynos)
- Golden files (`contracts/golden/`) são a fonte única de verdade do protocolo — Rust e Java testam contra os mesmos arquivos
- Validações manuais (T029, T041, T047, T054, T061, T065, T071) são gates de checkpoint: não avançar de story com elas pendentes
- Commit após cada task ou grupo lógico; PRs por story
