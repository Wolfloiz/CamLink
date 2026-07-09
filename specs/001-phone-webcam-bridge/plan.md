# Implementation Plan: CamLink — Câmeras Android e IP como webcams virtuais

**Branch**: `001-phone-webcam-bridge` | **Date**: 2026-07-09 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-phone-webcam-bridge/spec.md`

**Note**: Este plano adapta o plano de desenvolvimento legado (PLAN.md do usuário) à
spec e à constituição vigentes. A diferença estrutural mais importante: a spec exige
**paridade Linux + Windows 10/11 na v1** (Clarifications 2026-07-09), então o
"Windows como Fase 10 futura" do plano legado foi absorvido como trilha de primeira
classe, atrás de uma abstração de câmera virtual por plataforma.

## Summary

CamLink conecta câmeras Android (via USB/ADB, sem instalar nada no celular) e
fontes IP/RTSP a dispositivos de webcam virtual do sistema, reconhecidos por OBS,
Chrome, Firefox, Discord e qualquer app de câmera, em Linux e Windows. A abordagem
técnica central: **fork do scrcpy-server** (Java, roda no Android com UID shell via
ADB) com uma thread de controle extra que aceita comandos JSON num socket
`localabstract`, permitindo controles Camera2 em runtime (zoom, foco, exposição,
ISO, WB, EIS, torch), modos inteligentes (Auto/Night/Sport/Pro) e captura RAW
(DNG) — tudo sobre a `CameraCaptureSession` que o próprio servidor possui. O vídeo
flui pela pipeline padrão do scrcpy; no Linux via `--v4l2-sink` direto para
v4l2loopback, no Windows via stream H.264 decodificado e empurrado para uma câmera
virtual DirectShow (akvirtualcamera). Fontes RTSP usam pipeline ffmpeg low-delay.
App desktop em Tauri 2.x (backend Rust) com frontend Svelte.

## Technical Context

**Language/Version**: Rust stable (pinada via `rust-toolchain.toml`, edition 2021+)
para todo o backend/desktop; Java 17 exclusivamente no fork do scrcpy-server (roda
no Android, fora do alcance do Rust); TypeScript/Svelte no frontend Tauri.

**Primary Dependencies**:
- Tauri 2.x (shell do app, bundling, IPC frontend↔backend) — **fixa o
  TODO(GUI_FRAMEWORK) da constituição**
- scrcpy ≥ 4.0 (cliente C, dependência de runtime) + fork do `scrcpy-server`
  (submodule, branch `camlink` sobre a tag do cliente)
- adb (android-tools) — detecção, túnel `adb forward`
- ffmpeg — pipeline RTSP low-delay e decode no caminho Windows
- v4l2loopback ≥ 0.13 (Linux) / akvirtualcamera (Windows, GPL-3.0, FFI C)
- Crates: `serde`/`serde_json`, `tokio` (subprocess + sockets), `tracing`,
  `keyring` (cofre de segredos: Secret Service/Credential Manager), `thiserror`

**Storage**: arquivos de configuração locais (formato TOML/JSON em dir de config da
plataforma); credenciais RTSP somente no cofre do SO (FR-018a); DNGs em diretório
escolhido pelo usuário.

**Testing**: `cargo test` (unit + integration com binários fake de adb/scrcpy),
testes de contrato do protocolo de controle (golden files JSON), JUnit no fork Java
para parsing/validação de comandos; CI em Linux e Windows; validação
hardware-in-the-loop guiada por `quickstart.md`.

**Target Platform**: Linux desktop (X11/Wayland; pacotes .deb, AppImage, AUR) e
Windows 10/11 (instalador gráfico via Tauri bundler NSIS/MSI). Android 12+ no lado
do celular (FR-002a).

**Project Type**: desktop app (Tauri) + componente Android embarcado (jar do fork,
pré-buildado e distribuído no pacote).

**Performance Goals**: latência Android ≤ 70 ms nas duas plataformas, tipicamente
35–70 ms (SC-002); RTSP ≤ 300 ms; controles
refletem em < 1 s (SC-004); troca frontal/traseira ≤ 2 s; preview 1 fps com < 5% de
CPU adicional; sessões de 2 h sem vazamento (SC-005).

**Constraints**: sem app no celular (premissa de produto); GPL-3.0 (FR-025);
nenhum dado sai da máquina exceto consumo RTSP configurado (FR-026); RAW limitado
pela banda ADB (~20–80 MB/s) → sequência 1–3 fps dinâmico, stream principal sempre
H.264/H.265 com prioridade (FR-020); instalação sem terminal (FR-024).

**Scale/Scope**: uso local, mono-usuário; múltiplas fontes simultâneas (limite
prático inicial: 4 dispositivos virtuais); 6 user stories; ~7 módulos Rust + fork
Java + frontend.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Princípio | Avaliação |
|---|---|
| I. Spec-First | ✅ spec.md validada + clarify concluído antes deste plano |
| II. Entrega incremental | ✅ user stories independentes; US1 (stream básico) é MVP; fases de implementação mapeiam stories |
| III. Test-First (NON-NEGOTIABLE) | ✅ estratégia definida em Technical Context; tasks incluirão testes antes da implementação; partes hardware-dependentes têm contratos testáveis via fakes + validação manual roteirizada no quickstart |
| IV. Paridade Linux+Windows | ✅ abstração `VirtualCameraBackend` por plataforma desde o início; CI nas duas plataformas; **risco**: câmera virtual Windows é o maior desconhecido — mitigado em research.md e num spike dedicado |
| V. Simplicidade/YAGNI | ⚠️ 3 desvios justificados na Complexity Tracking (fork Java, frontend Svelte, submodule) |
| VI. Quality gates & observabilidade | ✅ `tracing` estruturado em todos os subprocessos/sockets; erros `Result`-based; fmt/clippy/test como gates |

**Gate: PASS** (com justificativas registradas abaixo).

**Re-check pós-Fase 1**: PASS — o design (data-model, contratos, quickstart) não
introduziu desvios novos; os três desvios da tabela permanecem os únicos.

## Project Structure

### Documentation (this feature)

```text
specs/001-phone-webcam-bridge/
├── plan.md              # Este arquivo
├── research.md          # Fase 0 — decisões técnicas e alternativas
├── data-model.md        # Fase 1 — entidades e estados
├── quickstart.md        # Fase 1 — guia de validação ponta a ponta
├── contracts/
│   ├── control-protocol.md   # Protocolo JSON desktop ↔ fork scrcpy-server
│   └── tauri-commands.md     # Comandos/eventos IPC frontend ↔ backend Rust
└── tasks.md             # Fase 2 — gerado por /speckit-tasks
```

### Source Code (repository root)

```text
src-tauri/                      # Backend Rust (crate principal)
├── src/
│   ├── main.rs / lib.rs
│   ├── device_manager.rs       # adb devices + hotplug (udev/poll; Win: poll)
│   ├── stream_manager.rs       # lifecycle scrcpy (SCRCPY_SERVER_PATH → fork)
│   ├── camera_controller.rs    # socket JSON → fork (adb forward)
│   ├── rtsp_manager.rs         # pipeline ffmpeg low-delay
│   ├── raw_manager.rs          # recepção DNG (framing binário) + storage
│   ├── secrets.rs              # keyring (Secret Service / Credential Manager)
│   └── virtualcam/
│       ├── mod.rs              # trait VirtualCameraBackend (create/destroy/feed)
│       ├── v4l2.rs             # Linux: v4l2loopback-ctl + pkexec/polkit
│       └── akvcam.rs           # Windows: akvirtualcamera via FFI + frame push
├── tests/                      # integração: fakes de adb/scrcpy, contratos JSON
├── Cargo.toml
└── tauri.conf.json
src/                            # Frontend Svelte (SvelteKit)
├── routes/ (+page.svelte, +layout.ts)
└── lib/ (DeviceList, Preview, ModeSelector, CameraControls, RawPanel, RtspPanel)
scrcpy/                         # Submodule: fork scrcpy, branch camlink sobre v4.x
└── server/src/main/java/com/genymobile/scrcpy/camlink/
    ├── CamLinkControlServer.java   # socket localabstract + JSON
    ├── ModePresets.java            # tabela Camera2 por modo
    └── RawCapture.java             # ImageReader RAW_SENSOR + DngCreator
installer/
├── linux/ (install.sh, polkit policy, udev rules, modules-load.d, PKGBUILD)
└── windows/ (config NSIS/MSI, instalação do driver akvirtualcamera)
```

**Structure Decision**: app desktop Tauri (opção desktop-app) com backend Rust
modular e abstração de plataforma isolada em `src-tauri/src/virtualcam/`
(Princípio IV: lógica de negócio neutra, `#[cfg(target_os)]` só nos backends). O
fork Java vive em submodule para rebase controlado contra o upstream do scrcpy.

## Fases de Implementação (mapeamento stories → fases)

Ordem herdada do plano legado, com Windows integrado (não mais "Fase 10"):

| Fase | Conteúdo | Story | Plataforma |
|---|---|---|---|
| 0 | Scaffold Tauri + stubs + CI (fmt/clippy/test, Linux+Windows) | — | ambas |
| 0.5 | **Spike A**: fork scrcpy-server — `set_zoom` runtime headless (critério de abortar: pivotar p/ pipeline própria e replanejar) | — | Android |
| 0.6 | **Spike B**: câmera virtual Windows — frame de teste visível no OBS via akvirtualcamera | — | Windows |
| 1 | `virtualcam/` (trait + v4l2 + akvcam) | US1 | ambas |
| 2 | Detecção ADB + orientação de autorização | US1 | ambas |
| 3 | Stream Android → câmera virtual (scrcpy stock) | US1 (MVP) | ambas |
| 4 | Fork em produção: protocolo base + capabilities | US2 | Android |
| 5 | Controles completos + UI | US2 | ambas |
| 5.5 | Modos inteligentes | US3 | ambas |
| 6 | RTSP/IP + cofre de credenciais | US4 | ambas |
| 7 | Captura RAW (DNG) | US5 | ambas |
| 8 | Multi-fonte + preview 1 fps | US6 | ambas |
| 9 | Empacotamento (.deb/AppImage/AUR + NSIS/MSI) e instaladores | — | ambas |

Cada fase entrega testes antes da implementação (Princípio III) e fecha com o
checkpoint de validação do quickstart correspondente.

## Complexity Tracking

> Preenchido porque o Constitution Check registra desvios do Princípio V e do
> constraint "Rust em tudo".

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Componente Java (fork scrcpy-server) | Os controles Camera2 exigem posse da `CameraCaptureSession`, que vive no processo do scrcpy-server rodando no Android; Rust não roda ali e um APK separado é evictado pela arbitragem de câmera do Android 10+ | APK companion: inviável tecnicamente (evicção) e viola a premissa "nada instalado no celular"; scrcpy stock: sem foco/ISO/WB/EIS, e zoom/torch exigem janela (indisponível em headless `--no-playback`) |
| Frontend Svelte/TS (não-Rust) | UI rica (sliders, tap-to-focus sobre preview, painéis dinâmicos por capabilities) com Tauri 2.x, que é o framework GUI pinado; o backend permanece 100% Rust | GUI pure-Rust (egui/iced/Slint): ecossistema de componentes e velocidade de iteração inferiores para este tipo de painel; Tauri já entrega bundling .deb/AppImage/MSI exigido por FR-024 |
| Submodule do fork scrcpy | O jar do servidor deve casar com a versão do cliente scrcpy; submodule permite rebase por release e build reproduzível do jar distribuído no pacote | Vendorizar código copiado: perde histórico e torna rebase upstream custoso; depender do jar stock: não expõe controles (ver linha 1) |
