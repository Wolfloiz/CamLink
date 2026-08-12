//! Comandos e eventos Tauri (T025/T039/T052/T080) — conforme
//! `specs/001-phone-webcam-bridge/contracts/tauri-commands.md`: liga
//! `device_manager` (polling + eventos de hotplug), `virtualcam` (câmera
//! virtual por plataforma), `stream_manager` (lifecycle scrcpy/adb),
//! `camera_controller` (controles em runtime via fork — US2) e
//! `rtsp_manager` (fontes IP — US4) ao frontend. Camada fina de
//! orquestração — a lógica testada vive nos módulos que ela chama; esta
//! camada só é validada manualmente (research.md R11, camada 3).

pub mod camera_controller;
pub mod config;
pub mod device_manager;
pub mod error;
pub mod frame_transform;
pub mod model;
pub mod preview;
pub mod procutil;
pub mod raw_manager;
pub mod rtsp_manager;
pub mod secrets;
pub mod stream_manager;
pub mod virtualcam;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

use camera_controller::{
    ControlClient, ControlErrorCode, ControlEvent, ControlReply, ControlRequest,
};
use device_manager::DeviceEvent;
use error::AppError;
use model::{
    AndroidDevice, ControlState, DeviceCapabilities, FocusMode, ManualExposure, Rotation,
    RtspSource, RtspState, SessionSource, SessionState, SessionStats, SmartMode, StreamConfig,
    WbMode,
};
use stream_manager::{ExternalPaths, FrameSink, StreamManager};
use virtualcam::VirtualCameraBackend;

#[cfg(target_os = "windows")]
use virtualcam::dshow::DShowBackend;
#[cfg(target_os = "linux")]
use virtualcam::v4l2::V4l2Backend;

const DEVICE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SESSION_STATE_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Cadência da miniatura de preview (~5 fps): suficiente para enquadrar a
/// câmera sem competir com o stream principal — o frame é pequeno
/// (`preview_dimensions`, ≤640 px) e o encode roda fora do hot path
/// (`spawn_preview_encoder`).
const PREVIEW_INTERVAL: Duration = Duration::from_millis(200);
// O prefixo comum dos labels vive em `virtualcam::LABEL_PREFIX` (a limpeza
// de devices órfãos precisa saber quais labels são deste app); o resto do
// label é montado por `virtualcam::vcam_label` a partir de (nome, sufixo).
/// Resolução/fps de saída das sessões RTSP (v1): o ffmpeg redimensiona o
/// stream da câmera IP para casar com o device virtual.
const RTSP_RESOLUTION: (u32, u32) = (1280, 720);
const RTSP_FPS: u32 = 30;
/// EMA aplicada em `SessionStats.fps` — janela efetiva ~= POLL_INTERVAL /
/// (1 - FPS_SMOOTHING) ≈ 1,25s, escolhida pra cobrir o período de rajada
/// (~1s) observado em hardware real (ver `spawn_session_state_emitter`).
const FPS_SMOOTHING: f32 = 0.8;
/// Timeout de conexão/handshake e de cada request do socket de controle.
const CONTROL_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Janela total de retry do handshake inicial de controle: o `adb forward`
/// fica pronto quase instantaneamente, mas o `CamLinkControlServer` no
/// celular só liga depois do push do jar + boot do `app_process` — a
/// primeira tentativa de conectar quase sempre perde essa corrida (medido em
/// hardware: ~600ms–1s até "control server listening"). `adb forward`
/// aceita a conexão TCP local mesmo sem ninguém ouvindo o socket abstrato no
/// device e a fecha em seguida — daí `ControlClientError::Closed`, não
/// `Timeout`, no primeiro request.
const CONTROL_CONNECT_RETRY_WINDOW: Duration = Duration::from_secs(8);
const CONTROL_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(300);

fn new_vcam_backend() -> Box<dyn VirtualCameraBackend + Send> {
    #[cfg(target_os = "linux")]
    {
        Box::new(V4l2Backend::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(DShowBackend::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        compile_error!("plataforma não suportada (Princípio IV: só Linux e Windows)");
    }
}

/// Procura `scrcpy-server` (sem extensão) no mesmo diretório do binário
/// `scrcpy`/`scrcpy.exe` resolvido via PATH — é assim que a distribuição
/// oficial do scrcpy empacota os dois juntos (confirmado nesta máquina: o
/// WinGet coloca o diretório real do pacote no PATH, não um shim). Mesma
/// convenção que o cliente real usa para o path default do servidor
/// (`SC_SERVER_PATH_DEFAULT`, relativo ao próprio binário), só que via
/// busca no PATH em vez de relativo ao instalador do CamLink (ainda não
/// existe — bundling fica para o instalador real).
fn find_server_jar_next_to_scrcpy_binary() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let binary_name = if cfg!(windows) {
        "scrcpy.exe"
    } else {
        "scrcpy"
    };
    std::env::split_paths(&path_var).find_map(|dir| {
        if !dir.join(binary_name).is_file() {
            return None;
        }
        let candidate = dir.join("scrcpy-server");
        candidate.is_file().then_some(candidate)
    })
}

/// Caminhos dos executáveis externos. v1: resolvidos via PATH (assume
/// adb/scrcpy/ffmpeg instalados — quickstart.md pré-requisitos). O jar do
/// scrcpy-server vem, em ordem: `SCRCPY_SERVER_PATH` se definida (é assim
/// que o jar do fork `scrcpy-server-camlink` entra — T037) → procurado ao
/// lado do binário `scrcpy` no PATH → `scrcpy-server` no diretório de
/// trabalho como último recurso. Empacotamento real (instalador resolvendo
/// paths bundled) fica para uma iteração futura.
fn resolve_external_paths() -> ExternalPaths {
    let server_jar = std::env::var("SCRCPY_SERVER_PATH")
        .ok()
        .map(PathBuf::from)
        .or_else(find_server_jar_next_to_scrcpy_binary)
        .unwrap_or_else(|| PathBuf::from("scrcpy-server"));
    ExternalPaths {
        adb: PathBuf::from("adb"),
        scrcpy: PathBuf::from("scrcpy"),
        ffmpeg: PathBuf::from("ffmpeg"),
        server_jar,
        extra_env: Vec::new(),
    }
}

/// Contexto por sessão Android (US2): liga o `session_id` do
/// `stream_manager` ao device, à câmera virtual e ao canal de controle do
/// fork (criado sob demanda no primeiro comando de controle).
struct SessionCtx {
    serial: String,
    config: StreamConfig,
    vcam_id: Uuid,
    /// Orientação corrente (FR-016a): lida pelo sink a cada frame no
    /// Windows; no Linux vira `--capture-orientation` no (re)start.
    orientation: Arc<StdMutex<(Rotation, bool)>>,
    control_state: ControlState,
    control: Option<ControlClient>,
    /// Porta local do `adb forward tcp:<porta> localabstract:camlink`
    /// corrente (achado em bancada 2026-07-30: sem rastrear isso pra dar
    /// `adb forward --remove` depois, cada reconexão de controle deixava
    /// mais uma entrada órfã — 15 acumuladas num único aparelho de teste).
    forward_port: Option<u16>,
    /// Job RAW ativo da sessão (US5) — no máximo 1 por vez (data-model.md).
    /// `Arc` porque o loop de eventos de `ensure_control` (spawnado
    /// separado do `SessionCtx`) precisa do mesmo slot que os comandos
    /// `raw_snapshot`/`raw_sequence_start|stop` leem/escrevem.
    raw_job: Arc<TokioMutex<Option<raw_manager::RawJobRuntime>>>,
    /// `None` = usa o default (`dirs::picture_dir()/CamLink`) — configurável
    /// via `set_raw_output_dir` (data-model.md, `RawCaptureJob.output_dir`).
    raw_output_dir: Option<PathBuf>,
}

/// Remove o `adb forward` de uma sessão de controle encerrada. Best-effort:
/// erro aqui (device desconectado, etc.) não deve impedir o resto do
/// teardown.
async fn release_control_forward(ctx: &mut SessionCtx) {
    if let Some(port) = ctx.forward_port.take() {
        let paths = resolve_external_paths();
        let _ = procutil::hide_console(tokio::process::Command::new(&paths.adb))
            .args([
                "-s",
                &ctx.serial,
                "forward",
                "--remove",
                &format!("tcp:{port}"),
            ])
            .output()
            .await;
    }
    ctx.control = None;
}

/// Limpa `adb forward` órfãos de sessões anteriores (achado em bancada
/// 2026-07-30: 15 entradas `localabstract:camlink` acumuladas num aparelho —
/// nunca eram removidas). Best-effort, roda uma vez na inicialização do app,
/// mesmo padrão do `cleanup_stale()` do v4l2.
async fn cleanup_stale_forwards() {
    let paths = resolve_external_paths();
    let Ok(output) = tokio::process::Command::new(&paths.adb)
        .args(["forward", "--list"])
        .output()
        .await
    else {
        return;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(serial), Some(local), Some(remote)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if remote == "localabstract:camlink" {
            let _ = procutil::hide_console(tokio::process::Command::new(&paths.adb))
                .args(["-s", serial, "forward", "--remove", local])
                .output()
                .await;
        }
    }
}

/// Sessão RTSP ativa (US4).
struct RtspRuntime {
    session: rtsp_manager::RtspSession,
    vcam_id: Uuid,
    /// Linux: task de snapshots de preview do device v4l2 (FR-023).
    preview_task: Option<tokio::task::JoinHandle<()>>,
}

struct AppState {
    stream_manager: StreamManager,
    vcam: Arc<StdMutex<Box<dyn VirtualCameraBackend + Send>>>,
    devices: Arc<StdMutex<Vec<AndroidDevice>>>,
    sessions: TokioMutex<HashMap<Uuid, SessionCtx>>,
    /// `Arc` (não só `TokioMutex`) pra dar aos hooks de shutdown (sinal OS +
    /// `RunEvent::ExitRequested`) um handle independente do `AppState`
    /// gerenciado pelo Tauri — mesmo motivo do `StreamManager` ser `Clone`
    /// (ver `run()`): sem isso o `ffmpeg` de uma sessão RTSP ativa ficava
    /// órfão ao fechar o app, igual ao bug do `scrcpy` (2026-07-28), só que
    /// não coberto porque `kill_all_backends` só conhece o `StreamManager`.
    rtsp: Arc<TokioMutex<HashMap<Uuid, RtspRuntime>>>,
    /// Serializa restarts (`switch_camera`/rotação) por device (FR-015):
    /// `session_id` muda a cada restart, então o lock não pode viver dentro
    /// de `SessionCtx` — é chaveado pelo `serial`, estável entre trocas.
    /// Cada restart mata o `scrcpy-server` remanescente no celular antes de
    /// subir o novo (`spawn_backend`); dois restarts concorrentes para o
    /// mesmo device (cliques rápidos em girar/trocar câmera) intercalavam
    /// esse kill+bind e ainda batiam em "Address already in use" (1 em ~35
    /// trocas em hardware, 2026-07-24) — o lock garante que cada restart
    /// termina (stop + spawn) antes do próximo começar.
    restart_locks: TokioMutex<HashMap<String, Arc<TokioMutex<()>>>>,
}

/// Conta fontes ativas nos dois registries (Android + RTSP) para o gate de
/// capacidade (`virtualcam::check_capacity`, FR-021) — chamado ANTES de
/// alocar qualquer recurso pra nova fonte.
async fn active_source_count(state: &State<'_, AppState>) -> usize {
    state.sessions.lock().await.len() + state.rtsp.lock().await.len()
}

/// Lock de restart do device (ver doc de `AppState::restart_locks`):
/// cria sob demanda, nunca remove (poucos devices distintos por sessão de
/// uso — não vale a complexidade de recolher entradas).
async fn restart_lock_for(state: &State<'_, AppState>, serial: &str) -> Arc<TokioMutex<()>> {
    let mut locks = state.restart_locks.lock().await;
    Arc::clone(
        locks
            .entry(serial.to_string())
            .or_insert_with(|| Arc::new(TokioMutex::new(()))),
    )
}

fn config_path() -> Result<PathBuf, AppError> {
    config::default_config_path().ok_or_else(|| {
        AppError::new(
            "config_dir",
            "Não encontrei o diretório de configuração da plataforma",
        )
    })
}

// ---------------------------------------------------------------------------
// Comandos (contracts/tauri-commands.md)
// ---------------------------------------------------------------------------

/// FR-001: lista o cache mantido pelo polling em background — nunca faz um
/// `adb devices -l` síncrono aqui (regra de não bloquear > 200 ms).
#[tauri::command]
async fn list_devices(state: State<'_, AppState>) -> Result<Vec<AndroidDevice>, AppError> {
    Ok(state.devices.lock().unwrap().clone())
}

/// Resposta de `start_stream`/`switch_camera`: `StreamSession` não carrega o
/// próprio `session_id` (só é conhecido fora do `stream_manager`, como
/// retorno de `start()`), mas o frontend precisa dele para `stop_stream` e
/// para filtrar `session_state`/`preview_frame`.
#[derive(Clone, serde::Serialize)]
struct StartStreamResponse {
    session_id: Uuid,
    virtual_camera: model::VirtualCamera,
    config: StreamConfig,
    state: SessionState,
    stats: SessionStats,
}

/// Monta o sink de frames + preview e inicia a sessão no `stream_manager`.
/// Compartilhado por `start_stream`, `switch_camera` e o restart de rotação
/// 90°/270° (T079) — a câmera virtual já deve existir.
async fn wire_android_session(
    app: AppHandle,
    stream_manager: &StreamManager,
    vcam_handle: Arc<StdMutex<Box<dyn VirtualCameraBackend + Send>>>,
    serial: String,
    config: StreamConfig,
    virtual_camera: model::VirtualCamera,
    orientation: Arc<StdMutex<(Rotation, bool)>>,
) -> Result<StartStreamResponse, AppError> {
    let vcam_id = virtual_camera.id;
    // Dimensões do decode (o que chega do celular) e da câmera virtual (o
    // que sai depois do transform de orientação — trocadas em 90°/270°).
    let source_dims = config.resolution;
    let built_orientation = *orientation.lock().unwrap();
    let output_dims = if built_orientation.0.swaps_dimensions() {
        (source_dims.1, source_dims.0)
    } else {
        source_dims
    };
    let preview_dims = preview::preview_dimensions(output_dims);

    let session_id_cell: Arc<StdMutex<Option<Uuid>>> = Arc::new(StdMutex::new(None));
    let preview_last = Arc::new(StdMutex::new(Instant::now() - PREVIEW_INTERVAL));
    let frame_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let frame_count_for_sink = Arc::clone(&frame_count);

    let (preview_tx, preview_rx) = tokio::sync::watch::channel(None);
    let app_for_preview = app.clone();
    let session_id_for_preview = Arc::clone(&session_id_cell);
    spawn_preview_encoder(preview_rx, move |jpeg| {
        let Some(session_id) = *session_id_for_preview.lock().unwrap() else {
            return;
        };
        let payload = PreviewFrameEvent {
            session_id,
            jpeg_base64: BASE64.encode(jpeg),
        };
        let _ = app_for_preview.emit("preview_frame", payload);
    });

    let orientation_for_sink = Arc::clone(&orientation);
    let sink: FrameSink = Arc::new(move |frame: &[u8]| {
        frame_count_for_sink.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Windows: o sink é o caminho de ENTREGA — os frames vêm do socket
        // scrcpy e precisam ser empurrados à câmera virtual, com a
        // orientação corrente aplicada no desktop (T078: mirror/180° ao
        // vivo, sem restart; 90°/270° chegam aqui só depois do restart que
        // recriou a câmera nas dimensões trocadas). Linux: os frames vêm do
        // LEITOR do próprio device v4l2 (o scrcpy já escreveu lá via
        // --v4l2-sink, orientação aplicada no celular via
        // --capture-orientation); realimentá-los criaria um loop.
        #[cfg(target_os = "windows")]
        {
            let (rotation, mirror) = *orientation_for_sink.lock().unwrap();
            let identity = rotation == Rotation::Deg0 && !mirror;
            let (delivered, dw, dh): (std::borrow::Cow<[u8]>, u32, u32) = if identity {
                (
                    std::borrow::Cow::Borrowed(frame),
                    source_dims.0,
                    source_dims.1,
                )
            } else {
                let (out, w, h) =
                    frame_transform::apply(frame, source_dims.0, source_dims.1, rotation, mirror);
                (std::borrow::Cow::Owned(out), w, h)
            };
            if (dw, dh) != output_dims {
                // Transição de orientação em andamento (o restart vai
                // reconstruir o sink com as dimensões novas) — não corromper
                // o device com um frame de geometria errada.
                return;
            }
            if let Ok(mut backend) = vcam_handle.lock() {
                let _ = backend.feed_frame(&vcam_id, &delivered);
            }
            publish_preview(
                &preview_last,
                &preview_tx,
                &delivered,
                (dw, dh),
                preview_dims,
                true,
            );
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (&vcam_handle, &vcam_id, &orientation_for_sink);
            publish_preview(
                &preview_last,
                &preview_tx,
                frame,
                source_dims,
                preview_dims,
                false,
            );
        }
    });

    let start_result = stream_manager
        .start_with_orientation(
            SessionSource::Android(serial),
            config,
            &virtual_camera.backend_path,
            Some(sink),
            built_orientation,
        )
        .await;

    let session_id = start_result?;
    *session_id_cell.lock().unwrap() = Some(session_id);

    let session = stream_manager
        .session(session_id)
        .await
        .ok_or_else(|| AppError::new("session_not_found", "Sessão sumiu logo após ser criada"))?;

    spawn_session_state_emitter(app, session_id, frame_count);

    Ok(StartStreamResponse {
        session_id,
        virtual_camera,
        config: session.config,
        state: session.state,
        stats: session.stats,
    })
}

/// Publica um frame no canal de preview respeitando o throttle. `downsample`
/// indica que o frame está em resolução cheia e precisa ser reduzido; caso
/// contrário ele já chega no tamanho da miniatura (leitor Linux).
fn publish_preview(
    last: &StdMutex<Instant>,
    tx: &tokio::sync::watch::Sender<Option<(Vec<u8>, u32, u32)>>,
    frame: &[u8],
    frame_dims: (u32, u32),
    preview_dims: (u32, u32),
    downsample: bool,
) {
    let due = {
        let mut last = last.lock().unwrap();
        let due = last.elapsed() >= PREVIEW_INTERVAL;
        if due {
            *last = Instant::now();
        }
        due
    };
    if !due {
        return;
    }
    let (pw, ph) = preview_dims;
    let published: Result<Vec<u8>, String> = if downsample {
        preview::downsample_rgba(frame, frame_dims.0, frame_dims.1, pw, ph)
    } else {
        Ok(frame.to_vec())
    };
    match published {
        Ok(rgba) => {
            let _ = tx.send(Some((rgba, pw, ph)));
        }
        // Preview é descartável (FR-023), mas a falha precisa aparecer.
        Err(e) => tracing::warn!(error = %e, "downsample do preview falhou"),
    }
}

/// FR-003: cria a câmera virtual, inicia o backend scrcpy/adb com o sink de
/// frames ligado a ela e devolve a sessão (com a câmera virtual embutida).
#[tauri::command]
async fn start_stream(
    app: AppHandle,
    state: State<'_, AppState>,
    serial: String,
    config: StreamConfig,
) -> Result<StartStreamResponse, AppError> {
    virtualcam::check_capacity(active_source_count(&state).await)?;

    // T065c/T065e: nome amigável no seletor do OBS — apelido do usuário se
    // houver ("CamLink câmera teto P11P"), senão modelo do aparelho
    // ("CamLink SM_S921B P11P"), senão só o serial se o device ainda não
    // estiver no cache de descoberta.
    let nickname = device_nickname_for(&serial);
    let (name, suffix) = virtualcam::android_label_parts(
        &state.devices.lock().unwrap(),
        &serial,
        nickname.as_deref(),
    );
    let label = virtualcam::vcam_label(&name, &suffix);
    let virtual_camera = {
        let mut vcam = state.vcam.lock().unwrap();
        vcam.create(&label, config.resolution, config.fps)
            .map_err(|e| AppError::new("vcam_create_failed", e.to_string()))?
    };
    let vcam_id = virtual_camera.id;
    let orientation = Arc::new(StdMutex::new((Rotation::Deg0, false)));

    let response = wire_android_session(
        app,
        &state.stream_manager,
        Arc::clone(&state.vcam),
        serial.clone(),
        config.clone(),
        virtual_camera,
        Arc::clone(&orientation),
    )
    .await;

    match response {
        Ok(response) => {
            state.sessions.lock().await.insert(
                response.session_id,
                SessionCtx {
                    serial,
                    config,
                    vcam_id,
                    orientation,
                    control_state: ControlState::default(),
                    control: None,
                    forward_port: None,
                    raw_job: Arc::new(TokioMutex::new(None)),
                    raw_output_dir: None,
                },
            );
            Ok(response)
        }
        Err(err) => {
            if let Ok(mut vcam) = state.vcam.lock() {
                let _ = vcam.destroy(&vcam_id);
            }
            Err(err)
        }
    }
}

/// Encerra a sessão e libera a câmera virtual associada (FR-021: nunca
/// deixar um device virtual órfão).
#[tauri::command]
async fn stop_stream(state: State<'_, AppState>, session_id: Uuid) -> Result<(), AppError> {
    state.stream_manager.stop(session_id).await?;
    if let Some(mut ctx) = state.sessions.lock().await.remove(&session_id) {
        release_control_forward(&mut ctx).await;
        if let Ok(mut vcam) = state.vcam.lock() {
            let _ = vcam.destroy(&ctx.vcam_id);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// US2 — controles em runtime (T039/T080)
// ---------------------------------------------------------------------------

/// Mudança de controle (contracts/tauri-commands.md + FR-016a).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ControlChange {
    Mode(SmartMode),
    Zoom(f32),
    Focus(FocusMode),
    ExposureComp(i32),
    Iso(u32),
    Wb(WbMode),
    Eis(bool),
    Torch(bool),
    Rotation(Rotation),
    Mirror(bool),
}

#[derive(Clone, serde::Serialize)]
struct AfStateEvent {
    session_id: Uuid,
    state: String,
}

#[derive(Clone, serde::Serialize)]
struct FacesEvent {
    session_id: Uuid,
    rects: Vec<camera_controller::FaceRect>,
}

/// Tenta o handshake de controle repetidamente dentro de
/// `CONTROL_CONNECT_RETRY_WINDOW`: cobre a corrida entre o `adb forward`
/// (pronto quase na hora) e o `CamLinkControlServer` no celular (só liga
/// depois do push do jar + boot do `app_process`). Erros de protocolo
/// (`UnsupportedProtocol`/`Parse`) são persistentes — não adianta retentar —
/// então saem no primeiro erro; só `Closed`/`Io`/`Timeout` (sintomas de
/// "servidor ainda não subiu") são retentados.
async fn connect_control_with_retry(
    port: u16,
) -> Result<
    (
        camera_controller::ControlClient,
        tokio::sync::mpsc::Receiver<ControlEvent>,
    ),
    camera_controller::ControlClientError,
> {
    let deadline = tokio::time::Instant::now() + CONTROL_CONNECT_RETRY_WINDOW;
    loop {
        match ControlClient::connect(("127.0.0.1", port), CONTROL_CONNECT_TIMEOUT).await {
            Ok(pair) => return Ok(pair),
            Err(camera_controller::ControlClientError::UnsupportedProtocol { got }) => {
                return Err(camera_controller::ControlClientError::UnsupportedProtocol { got })
            }
            Err(camera_controller::ControlClientError::Parse(msg)) => {
                return Err(camera_controller::ControlClientError::Parse(msg))
            }
            Err(e) if tokio::time::Instant::now() >= deadline => return Err(e),
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "handshake de controle falhou, tentando de novo (servidor pode ainda estar subindo)"
                );
                tokio::time::sleep(CONTROL_CONNECT_RETRY_DELAY).await;
            }
        }
    }
}

/// Garante o canal de controle da sessão: `adb forward tcp:0` (porta
/// efêmera alocada pelo adb) + handshake `hello`. Reaproveitado entre
/// comandos; morre junto com o subprocess (restart limpa `ctx.control`).
async fn ensure_control(
    app: &AppHandle,
    session_id: Uuid,
    ctx: &mut SessionCtx,
) -> Result<(), AppError> {
    if ctx.control.is_some() {
        return Ok(());
    }
    // Reaproveita o forward já alocado (ex.: retry de handshake após
    // `connect_control_with_retry` falhar) em vez de criar outro — criar
    // sempre um novo aqui é o que gerava as entradas órfãs (achado em
    // bancada 2026-07-30, ver `release_control_forward`).
    let port = match ctx.forward_port {
        Some(port) => port,
        None => {
            let paths = resolve_external_paths();
            let mut args = camera_controller::adb_forward_args(&ctx.serial, 0);
            // adb imprime a porta alocada no stdout quando pedimos tcp:0.
            args[3] = "tcp:0".to_string();
            let output = procutil::hide_console(tokio::process::Command::new(&paths.adb))
                .args(&args)
                .output()
                .await
                .map_err(|e| {
                    AppError::new("adb_forward_failed", format!("Falha no adb forward: {e}"))
                })?;
            if !output.status.success() {
                return Err(AppError::new(
                    "adb_forward_failed",
                    format!(
                        "adb forward falhou: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                )
                .with_hint("Verifique se o dispositivo continua conectado (adb devices)."));
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let port = stream_manager::parse_forward_port(&stdout).ok_or_else(|| {
                AppError::new(
                    "adb_forward_failed",
                    format!("adb forward não devolveu porta: {stdout:?}"),
                )
            })?;
            ctx.forward_port = Some(port);
            port
        }
    };

    let (client, mut events) = connect_control_with_retry(port).await.map_err(|e| {
        AppError::new("control_connect_failed", e.to_string()).with_hint(
            "O stream precisa estar ativo com o scrcpy-server-camlink (fork) — \
                 confira SCRCPY_SERVER_PATH.",
        )
    })?;

    // Encaminha eventos assíncronos do fork para o frontend (contrato §5).
    let app_events = app.clone();
    let raw_job = Arc::clone(&ctx.raw_job);
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                ControlEvent::AfState { state } => {
                    let _ = app_events.emit("af_state", AfStateEvent { session_id, state });
                }
                ControlEvent::Faces { rects } => {
                    let _ = app_events.emit("faces", FacesEvent { session_id, rects });
                }
                ControlEvent::RawFrameDropped { reason } => {
                    tracing::info!(%reason, "frame RAW descartado pelo fork");
                }
                ControlEvent::RawFrame(frame) => {
                    let mut slot = raw_job.lock().await;
                    if let Some(job) = raw_manager::handle_incoming_frame(&mut slot, frame) {
                        let _ =
                            app_events.emit("raw_progress", RawProgressEvent { session_id, job });
                    }
                }
            }
        }
    });

    ctx.control = Some(client);
    Ok(())
}

/// Converte a resposta do fork em `Result`, preservando o código do
/// protocolo como código do `AppError` (a UI diferencia OUT_OF_RANGE de
/// UNSUPPORTED — FR-016).
fn reply_to_result(reply: ControlReply) -> Result<serde_json::Value, AppError> {
    match reply {
        ControlReply::Ok(data) => Ok(data),
        ControlReply::Hello { .. } => Ok(serde_json::Value::Null),
        ControlReply::Err { code, msg } => Err(AppError::new(code.as_str(), msg)),
    }
}

/// `get_capabilities` logo após `ensure_control` pode alcançar o fork antes
/// da câmera terminar de abrir no celular: `CameraCapture.camlinkGetCameraId()`
/// é `null` até `init()` completar, e nesse meio-tempo o fork responde
/// `CAMERA_ERROR` ("Expected cameraId to be numeric, but it was: null") em
/// vez do BUSY que o caso pediria — mesma corrida do handshake de conexão
/// (`connect_control_with_retry`), só que uma camada acima. Retenta dentro
/// da mesma janela; qualquer outro código de erro (ou sucesso) volta na
/// hora.
async fn get_capabilities_with_retry(
    client: &mut ControlClient,
) -> Result<ControlReply, camera_controller::ControlClientError> {
    let deadline = tokio::time::Instant::now() + CONTROL_CONNECT_RETRY_WINDOW;
    loop {
        let reply = client.request(ControlRequest::GetCapabilities).await?;
        match &reply {
            ControlReply::Err {
                code: ControlErrorCode::CameraError,
                msg,
            } if tokio::time::Instant::now() < deadline => {
                tracing::debug!(
                    msg,
                    "get_capabilities falhou com CAMERA_ERROR, tentando de novo (câmera pode ainda estar abrindo)"
                );
                tokio::time::sleep(CONTROL_CONNECT_RETRY_DELAY).await;
            }
            _ => return Ok(reply),
        }
    }
}

/// FR-016: capabilities reais do aparelho, consultadas via fork (exige um
/// stream ativo — a sessão de câmera pertence ao scrcpy-server).
#[tauri::command]
async fn get_capabilities(
    app: AppHandle,
    state: State<'_, AppState>,
    serial: String,
) -> Result<DeviceCapabilities, AppError> {
    let mut sessions = state.sessions.lock().await;
    let (session_id, ctx) = sessions
        .iter_mut()
        .find(|(_, ctx)| ctx.serial == serial)
        .ok_or_else(|| {
            AppError::new(
                "no_active_session",
                "Nenhum stream ativo para este dispositivo",
            )
            .with_hint("Inicie a transmissão antes de consultar os controles.")
        })?;
    ensure_control(&app, *session_id, ctx).await?;
    let client = ctx.control.as_mut().expect("ensure_control garantiu");
    let reply = get_capabilities_with_retry(client).await;
    let reply = match reply {
        Ok(reply) => reply,
        Err(err) => {
            // Mesma invalidação de `set_control`: sem isso, uma conexão
            // morta (ex.: subprocess reiniciou) fica presa em `ctx.control`
            // pra sempre — toda chamada seguinte repete o mesmo erro
            // ("conexão de controle encerrada pelo servidor") até fechar e
            // reabrir o app (achado em bancada 2026-07-30, Samsung S20 FE).
            release_control_forward(ctx).await;
            return Err(AppError::new("control_request_failed", err.to_string()));
        }
    };
    let data = reply_to_result(reply)?;
    serde_json::from_value(data).map_err(|e| {
        AppError::new(
            "capabilities_parse",
            format!("Capabilities do fork não parseiam: {e}"),
        )
    })
}

#[derive(Clone, serde::Serialize)]
struct ControlStateEvent {
    session_id: Uuid,
    control_state: ControlState,
}

/// Progresso de um job RAW (US5, FR-019/020): emitido a cada frame gravado
/// numa Sequência, e ao fim de um Snapshot.
#[derive(Clone, serde::Serialize)]
struct RawProgressEvent {
    session_id: Uuid,
    job: model::RawCaptureJob,
}

/// FR-008..016a: aplica um controle na sessão. Controles de câmera vão pelo
/// fork (validação server-side contra capabilities); `rotation`/`mirror`
/// são transformação local (T080) — ao vivo quando as dimensões não mudam,
/// via restart (mesmo caminho do `switch_camera`) quando 90°/270° trocam
/// largura↔altura.
#[tauri::command]
async fn set_control(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    change: ControlChange,
) -> Result<ControlState, AppError> {
    // Rotação/espelho: caminho local, sem fork.
    match &change {
        ControlChange::Rotation(_) | ControlChange::Mirror(_) => {
            return set_orientation(app, state, session_id, change).await;
        }
        _ => {}
    }

    let mut sessions = state.sessions.lock().await;
    let ctx = sessions
        .get_mut(&session_id)
        .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
    ensure_control(&app, session_id, ctx).await?;

    let request = match &change {
        ControlChange::Mode(mode) => ControlRequest::SetMode { mode: *mode },
        ControlChange::Zoom(ratio) => ControlRequest::SetZoom { ratio: *ratio },
        ControlChange::Focus(focus) => ControlRequest::SetFocus { focus: *focus },
        ControlChange::ExposureComp(ev) => ControlRequest::SetExposure { compensation: *ev },
        ControlChange::Iso(value) => ControlRequest::SetIso { value: *value },
        ControlChange::Wb(mode) => ControlRequest::SetWb { mode: *mode },
        ControlChange::Eis(enabled) => ControlRequest::SetEis { enabled: *enabled },
        ControlChange::Torch(enabled) => ControlRequest::SetTorch { enabled: *enabled },
        ControlChange::Rotation(_) | ControlChange::Mirror(_) => unreachable!(),
    };
    let client = ctx.control.as_mut().expect("ensure_control garantiu");
    let reply = client.request(request).await.map_err(|e| {
        // Canal morto (ex.: subprocess reiniciou): derruba o cliente pra
        // reconectar no próximo comando.
        AppError::new("control_request_failed", e.to_string())
    });
    let reply = match reply {
        Ok(reply) => reply,
        Err(err) => {
            release_control_forward(ctx).await;
            return Err(err);
        }
    };
    reply_to_result(reply)?;

    match change {
        ControlChange::Mode(mode) => ctx.control_state.apply_smart_mode(mode),
        ControlChange::Zoom(ratio) => ctx.control_state.zoom_ratio = ratio,
        ControlChange::Focus(focus) => ctx.control_state.focus = focus,
        ControlChange::ExposureComp(ev) => ctx.control_state.exposure_comp = ev,
        ControlChange::Iso(value) => {
            ctx.control_state.manual_exposure = Some(ManualExposure {
                iso: value,
                // Espelha o default do fork (1/30 s) até exposição manual
                // completa ganhar comando próprio.
                exposure_time_ns: 33_333_333,
            });
        }
        ControlChange::Wb(mode) => ctx.control_state.wb_mode = mode,
        ControlChange::Eis(enabled) => ctx.control_state.eis = enabled,
        ControlChange::Torch(enabled) => ctx.control_state.torch = enabled,
        ControlChange::Rotation(_) | ControlChange::Mirror(_) => unreachable!(),
    }

    let control_state = ctx.control_state.clone();
    let _ = app.emit(
        "control_state",
        ControlStateEvent {
            session_id,
            control_state: control_state.clone(),
        },
    );
    Ok(control_state)
}

/// FR-016a (T080): aplica rotação/espelho. Windows: ao vivo quando o "lado"
/// (paisagem↔retrato) não muda — o sink lê a célula compartilhada no próximo
/// frame; troca de lado (0/180 ↔ 90/270) recria a câmera virtual nas
/// dimensões trocadas via restart (T079). Linux: sempre restart — o
/// transform roda no celular (`--capture-orientation`) e os frames não
/// passam pelo desktop.
async fn set_orientation(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    change: ControlChange,
) -> Result<ControlState, AppError> {
    let (serial, config, vcam_id, orientation_cell, mut control_state, current, raw_output_dir) = {
        let sessions = state.sessions.lock().await;
        let ctx = sessions
            .get(&session_id)
            .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
        let current = *ctx.orientation.lock().unwrap();
        (
            ctx.serial.clone(),
            ctx.config.clone(),
            ctx.vcam_id,
            Arc::clone(&ctx.orientation),
            ctx.control_state.clone(),
            current,
            ctx.raw_output_dir.clone(),
        )
    };

    let target = match change {
        ControlChange::Rotation(rotation) => (rotation, current.1),
        ControlChange::Mirror(mirror) => (current.0, mirror),
        _ => unreachable!("set_orientation só recebe Rotation/Mirror"),
    };
    control_state.rotation = target.0;
    control_state.mirror = target.1;

    let live_capable =
        cfg!(target_os = "windows") && target.0.swaps_dimensions() == current.0.swaps_dimensions();

    if live_capable {
        *orientation_cell.lock().unwrap() = target;
        let mut sessions = state.sessions.lock().await;
        if let Some(ctx) = sessions.get_mut(&session_id) {
            ctx.control_state = control_state.clone();
        }
        let _ = app.emit(
            "control_state",
            ControlStateEvent {
                session_id,
                control_state: control_state.clone(),
            },
        );
        return Ok(control_state);
    }

    // Caminho de restart (Linux sempre; Windows quando muda o lado):
    // reaproveita o fluxo do switch_camera (FR-015, ≤ 2 s). Device v4l2
    // novo só quando o giro realmente troca largura×altura (90°/270°) —
    // espelho puro e giro 0°/180° não mudam dimensão, então o device atual
    // continua servindo sem custo de reseleção pro consumidor.
    let dims_changed = target.0.swaps_dimensions() != current.0.swaps_dimensions();
    *orientation_cell.lock().unwrap() = target;
    let response = restart_android_session(
        app.clone(),
        &state,
        session_id,
        serial,
        config,
        vcam_id,
        orientation_cell,
        control_state.clone(),
        raw_output_dir,
        dims_changed,
    )
    .await?;
    // `set_control` devolve `ControlState` (contrato) — o novo session_id do
    // restart vai por evento, senão o frontend perde a referência da sessão.
    let _ = app.emit(
        "session_replaced",
        SessionReplacedEvent {
            old_session_id: session_id,
            response: response.clone(),
        },
    );
    let _ = app.emit(
        "control_state",
        ControlStateEvent {
            session_id: response.session_id,
            control_state: control_state.clone(),
        },
    );
    Ok(control_state)
}

/// Emitido quando um restart interno (rotação 90°/270°) substitui a sessão:
/// o frontend troca o `session_id` que acompanha sem chamar comando nenhum.
#[derive(Clone, serde::Serialize)]
struct SessionReplacedEvent {
    old_session_id: Uuid,
    response: StartStreamResponse,
}

/// FR-015: troca frontal/traseira reiniciando o subprocess com o novo
/// `--camera-id` (`--camera-id` é fixado na inicialização — contrato §3);
/// a câmera virtual sobrevive quando as dimensões não mudam, então o app
/// consumidor não precisa reselecionar o device.
#[tauri::command]
async fn switch_camera(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    camera_id: String,
) -> Result<StartStreamResponse, AppError> {
    let (serial, mut config, vcam_id, orientation_cell, control_state, raw_output_dir) = {
        let sessions = state.sessions.lock().await;
        let ctx = sessions
            .get(&session_id)
            .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
        (
            ctx.serial.clone(),
            ctx.config.clone(),
            ctx.vcam_id,
            Arc::clone(&ctx.orientation),
            ctx.control_state.clone(),
            ctx.raw_output_dir.clone(),
        )
    };
    config.camera_id = camera_id;
    // `config.resolution` é a mesma pedida pras duas câmeras (só o
    // `camera_id` muda) — FR-015 exige que o device v4l2 sobreviva à troca
    // pra não pedir reseleção no Meet/OBS a cada clique em
    // Frontal/Traseira. Um device novo aqui (`force_fresh_vcam=true`) foi
    // tentado como blindagem extra contra uma câmera física entregar
    // resolução diferente, mas o custo (reseleção obrigatória a CADA troca,
    // mesmo quando a resolução não muda — o caso comum) não compensou:
    // usuário confirmou em hardware que quebrava a persistência esperada.
    restart_android_session(
        app,
        &state,
        session_id,
        serial,
        config,
        vcam_id,
        orientation_cell,
        control_state,
        raw_output_dir,
        false,
    )
    .await
}

/// Restart compartilhado (T039/T079): para a sessão antiga, recria a câmera
/// virtual se as dimensões de saída mudaram (rotação 90°/270°) e religa o
/// pipeline. O canal de controle antigo morre com o subprocess — o próximo
/// comando reconecta.
///
/// `force_fresh_vcam`: quando a resolução de saída MUDA de fato (só o giro
/// 90°/270°, que inverte largura×altura — `switch_camera` mantém
/// `config.resolution` igual pras duas câmeras, FR-015), o device v4l2
/// precisa ser um NOVO `/dev/videoN` (`create_fresh`) em vez de reaproveitar
/// o atual — o v4l2loopback trava o formato do device enquanto um
/// consumidor (Chrome/Meet) ainda o mantém aberto, e um writer novo com
/// resolução diferente é silenciosamente ignorado, corrompendo os frames
/// até o consumidor reselecionar a câmera manualmente (achado em bancada
/// 2026-07-27). Fora desse caso certo (giro 0°/180°, espelho, troca de
/// câmera) o device é sempre reaproveitado — forçar novo a cada troca de
/// câmera "pra garantir" foi tentado e piorou a experiência comum (exigia
/// reseleção a cada clique em Frontal/Traseira, mesmo sem a resolução ter
/// mudado — confirmado em hardware 2026-07-27).
#[allow(clippy::too_many_arguments)]
async fn restart_android_session(
    app: AppHandle,
    state: &State<'_, AppState>,
    old_session_id: Uuid,
    serial: String,
    config: StreamConfig,
    vcam_id: Uuid,
    orientation_cell: Arc<StdMutex<(Rotation, bool)>>,
    control_state: ControlState,
    raw_output_dir: Option<PathBuf>,
    force_fresh_vcam: bool,
) -> Result<StartStreamResponse, AppError> {
    let lock = restart_lock_for(state, &serial).await;
    let _restart_guard = lock.lock().await;

    state.stream_manager.stop(old_session_id).await?;
    if let Some(mut old_ctx) = state.sessions.lock().await.remove(&old_session_id) {
        release_control_forward(&mut old_ctx).await;
    }

    let orientation = *orientation_cell.lock().unwrap();
    let output_dims = if orientation.0.swaps_dimensions() {
        (config.resolution.1, config.resolution.0)
    } else {
        config.resolution
    };

    let can_reuse = cfg!(target_os = "linux") && !force_fresh_vcam;
    // T065c/T065e: mesmo nome amigável de `start_stream` — precisa ser
    // resolvido fora do lock de `vcam` (mutex distinto de `devices`).
    let nickname = device_nickname_for(&serial);
    let (name, suffix) = virtualcam::android_label_parts(
        &state.devices.lock().unwrap(),
        &serial,
        nickname.as_deref(),
    );
    let virtual_camera = {
        let mut vcam = state.vcam.lock().unwrap();
        let existing = vcam.camera(&vcam_id).cloned();
        match existing {
            // Windows negocia o media type da câmera nas dimensões de
            // criação; 90°/270° exigem recriar (FR-016a). No Linux, quando
            // a resolução não muda, o device pode ficar.
            Some(cam) if can_reuse => cam,
            _ => {
                let _ = vcam.destroy(&vcam_id);
                let label = virtualcam::vcam_label(&name, &suffix);
                if cfg!(target_os = "linux") && force_fresh_vcam {
                    vcam.create_fresh(&label, output_dims, config.fps)
                } else {
                    vcam.create(&label, output_dims, config.fps)
                }
                .map_err(|e| AppError::new("vcam_create_failed", e.to_string()))?
            }
        }
    };
    let new_vcam_id = virtual_camera.id;

    let response = wire_android_session(
        app,
        &state.stream_manager,
        Arc::clone(&state.vcam),
        serial.clone(),
        config.clone(),
        virtual_camera,
        Arc::clone(&orientation_cell),
    )
    .await?;

    state.sessions.lock().await.insert(
        response.session_id,
        SessionCtx {
            serial,
            config,
            vcam_id: new_vcam_id,
            orientation: orientation_cell,
            control_state,
            control: None,
            forward_port: None,
            raw_job: Arc::new(TokioMutex::new(None)),
            raw_output_dir,
        },
    );
    Ok(response)
}

// ---------------------------------------------------------------------------
// US5 — Captura RAW (T059)
// ---------------------------------------------------------------------------

/// Timeout esperando o frame chegar depois do `ok` de `raw_snapshot`
/// (captura na resolução nativa do sensor + geração do DNG no aparelho pode
/// levar um instante).
const RAW_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);

fn default_raw_output_dir() -> Result<PathBuf, AppError> {
    dirs::picture_dir()
        .map(|dir| dir.join("CamLink"))
        .ok_or_else(|| {
            AppError::new(
                "raw_output_dir",
                "Não encontrei o diretório de imagens da plataforma",
            )
            .with_hint("Defina manualmente com set_raw_output_dir.")
        })
}

fn raw_output_dir_for(ctx: &SessionCtx) -> Result<PathBuf, AppError> {
    match &ctx.raw_output_dir {
        Some(dir) => Ok(dir.clone()),
        None => default_raw_output_dir(),
    }
}

/// Define o diretório de saída dos DNGs desta sessão (data-model.md,
/// `RawCaptureJob.output_dir`) — não persiste entre sessões, só a URL/nome
/// de fontes RTSP viram `AppConfig` hoje.
#[tauri::command]
async fn set_raw_output_dir(
    state: State<'_, AppState>,
    session_id: Uuid,
    dir: PathBuf,
) -> Result<(), AppError> {
    let mut sessions = state.sessions.lock().await;
    let ctx = sessions
        .get_mut(&session_id)
        .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
    ctx.raw_output_dir = Some(dir);
    Ok(())
}

/// FR-019: Snapshot RAW — um frame DNG na resolução nativa do sensor salvo
/// em disco. Bloqueia até o frame chegar (ou timeout). O fork responde
/// `UNSUPPORTED` se o aparelho não expõe RAW e `BUSY` se já há uma captura
/// em andamento (contrato §4) — ambos chegam aqui via `reply_to_result`.
#[tauri::command]
async fn raw_snapshot(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
) -> Result<PathBuf, AppError> {
    let (raw_job, output_dir) = {
        let mut sessions = state.sessions.lock().await;
        let ctx = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
        ensure_control(&app, session_id, ctx).await?;
        (Arc::clone(&ctx.raw_job), raw_output_dir_for(ctx)?)
    };
    std::fs::create_dir_all(&output_dir).map_err(|e| {
        AppError::new(
            "raw_output_dir",
            format!("Não consegui criar o diretório de saída: {e}"),
        )
    })?;

    // Registra o job ANTES de mandar o comando: o frame pode chegar pelo
    // loop de eventos antes desta função retomar depois do `.await` do
    // request, e sem o slot já pronto ele seria descartado em silêncio
    // (`handle_incoming_frame` com `slot == None`).
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut slot = raw_job.lock().await;
        if slot.is_some() {
            return Err(AppError::new(
                "raw_job_busy",
                "Já existe uma captura RAW em andamento nesta sessão",
            )
            .with_hint("Espere terminar ou pare a captura atual antes de um novo snapshot."));
        }
        *slot = Some(raw_manager::RawJobRuntime::snapshot(output_dir.clone(), tx));
    }

    let sent = {
        let mut sessions = state.sessions.lock().await;
        let ctx = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
        let client = ctx.control.as_mut().expect("ensure_control garantiu");
        client
            .request(ControlRequest::RawSnapshot)
            .await
            .map_err(|e| AppError::new("control_request_failed", e.to_string()))
            .and_then(reply_to_result)
    };
    if let Err(err) = sent {
        *raw_job.lock().await = None;
        return Err(err);
    }

    let frame = match tokio::time::timeout(RAW_SNAPSHOT_TIMEOUT, rx).await {
        Ok(Ok(frame)) => frame,
        Ok(Err(_)) => {
            return Err(AppError::new(
                "raw_snapshot_failed",
                "A captura RAW terminou sem entregar o frame",
            ))
        }
        Err(_) => {
            *raw_job.lock().await = None;
            return Err(AppError::new(
                "raw_snapshot_timeout",
                "O aparelho não entregou o snapshot RAW a tempo",
            ));
        }
    };

    raw_manager::write_frame(&output_dir, &frame).map_err(|e| {
        AppError::new(
            "raw_write_failed",
            format!("Falha ao gravar o DNG em disco: {e}"),
        )
    })
}

/// FR-019/020: Sequência RAW (1-3 fps, cadência ajustada dinamicamente pelo
/// fork conforme a banda do túnel ADB — o vídeo principal tem prioridade).
/// Devolve a fps efetivamente concedida (`granted_fps`, contrato §4), que
/// pode ser menor que a pedida.
#[tauri::command]
async fn raw_sequence_start(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
    fps: f32,
) -> Result<f32, AppError> {
    let (raw_job, output_dir) = {
        let mut sessions = state.sessions.lock().await;
        let ctx = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
        ensure_control(&app, session_id, ctx).await?;
        (Arc::clone(&ctx.raw_job), raw_output_dir_for(ctx)?)
    };
    std::fs::create_dir_all(&output_dir).map_err(|e| {
        AppError::new(
            "raw_output_dir",
            format!("Não consegui criar o diretório de saída: {e}"),
        )
    })?;

    {
        let mut slot = raw_job.lock().await;
        if slot.is_some() {
            return Err(AppError::new(
                "raw_job_busy",
                "Já existe uma captura RAW em andamento nesta sessão",
            )
            .with_hint("Pare a captura atual antes de iniciar outra."));
        }
        // Registrado com a fps pedida como palpite inicial; ajustado pra
        // `granted_fps` assim que a resposta chegar (mesma razão do
        // snapshot: o job precisa existir ANTES do request pra não perder
        // um frame que chegue rápido demais).
        *slot = Some(raw_manager::RawJobRuntime::sequence(
            output_dir.clone(),
            fps,
        ));
    }

    let result = {
        let mut sessions = state.sessions.lock().await;
        let ctx = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
        let client = ctx.control.as_mut().expect("ensure_control garantiu");
        client
            .request(ControlRequest::RawSequenceStart { fps })
            .await
            .map_err(|e| AppError::new("control_request_failed", e.to_string()))
            .and_then(reply_to_result)
    };

    match result {
        Ok(data) => {
            let granted_fps = data
                .get("granted_fps")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or(fps);
            let mut slot = raw_job.lock().await;
            if let Some(runtime) = slot.as_mut() {
                runtime.job.kind = model::RawJobKind::Sequence {
                    target_fps: granted_fps,
                };
                if let model::RawJobState::Running { effective_fps, .. } = &mut runtime.job.state {
                    *effective_fps = granted_fps;
                }
            }
            Ok(granted_fps)
        }
        Err(err) => {
            *raw_job.lock().await = None;
            Err(err)
        }
    }
}

/// Encerra a Sequência RAW ativa; idempotente (sem job ativo, é um no-op).
#[tauri::command]
async fn raw_sequence_stop(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: Uuid,
) -> Result<(), AppError> {
    let (raw_job, client_reply) = {
        let mut sessions = state.sessions.lock().await;
        let ctx = sessions
            .get_mut(&session_id)
            .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
        let raw_job = Arc::clone(&ctx.raw_job);
        let reply = match ctx.control.as_mut() {
            Some(client) => Some(client.request(ControlRequest::RawSequenceStop).await),
            None => None,
        };
        (raw_job, reply)
    };
    if let Some(Err(e)) = client_reply {
        tracing::warn!(error = %e, "raw_sequence_stop: falha ao avisar o fork (limpando estado local mesmo assim)");
    }

    let mut slot = raw_job.lock().await;
    if let Some(job) = raw_manager::stop_sequence(&mut slot) {
        let _ = app.emit("raw_progress", RawProgressEvent { session_id, job });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// US4 — fontes RTSP (T052)
// ---------------------------------------------------------------------------

/// FR-018/018a: registra a fonte; a senha vai direto ao cofre do SO e a
/// config só guarda a URL (sem credencial) + a referência do segredo.
#[tauri::command]
async fn add_rtsp_source(
    name: String,
    url: String,
    password: Option<String>,
) -> Result<RtspSource, AppError> {
    rtsp_manager::validate_rtsp_url(&url)?;
    let id = Uuid::new_v4();
    let secret_ref = match password.as_deref() {
        Some(password) if !password.is_empty() => {
            let secret_ref = secrets::secret_ref_for(&id);
            secrets::store_secret(&secret_ref, password).map_err(|e| {
                AppError::new("keyring", e.to_string())
                    .with_hint("O cofre de segredos do sistema precisa estar disponível.")
            })?;
            Some(secret_ref)
        }
        _ => None,
    };
    let source = RtspSource {
        id,
        name,
        url,
        secret_ref,
        state: RtspState::Idle,
    };

    let path = config_path()?;
    let mut app_config =
        config::load_or_default(&path).map_err(|e| AppError::new("config_load", e.to_string()))?;
    app_config.rtsp_sources.push(source.clone());
    config::save_to(&path, &app_config).map_err(|e| AppError::new("config_save", e.to_string()))?;
    Ok(source)
}

/// FR-018a: remover a fonte limpa também o segredo do cofre.
#[tauri::command]
async fn remove_rtsp_source(state: State<'_, AppState>, id: Uuid) -> Result<(), AppError> {
    // Para a sessão se estiver ativa (idempotente).
    if let Some(runtime) = state.rtsp.lock().await.remove(&id) {
        teardown_rtsp_runtime(&state, runtime).await;
    }

    let path = config_path()?;
    let mut app_config =
        config::load_or_default(&path).map_err(|e| AppError::new("config_load", e.to_string()))?;
    let Some(index) = app_config.rtsp_sources.iter().position(|s| s.id == id) else {
        return Err(AppError::new("rtsp_not_found", "Fonte RTSP não encontrada"));
    };
    let source = app_config.rtsp_sources.remove(index);
    if let Some(secret_ref) = source.secret_ref.as_deref() {
        secrets::delete_secret(secret_ref).map_err(|e| AppError::new("keyring", e.to_string()))?;
    }
    config::save_to(&path, &app_config).map_err(|e| AppError::new("config_save", e.to_string()))?;
    Ok(())
}

/// Lista as fontes RTSP persistidas (a UI monta o painel a partir daqui).
#[tauri::command]
async fn list_rtsp_sources() -> Result<Vec<RtspSource>, AppError> {
    let path = config_path()?;
    let app_config =
        config::load_or_default(&path).map_err(|e| AppError::new("config_load", e.to_string()))?;
    Ok(app_config.rtsp_sources)
}

/// T065e: define (ou remove, com `nickname` vazio) o apelido de um
/// dispositivo Android por serial — o `model` do adb costuma ser só o nome
/// de código comercial ("SM-S921B"), não o nome de marketing ("Galaxy
/// S24"), e o app não tem como saber esse mapeamento sozinho.
#[tauri::command]
async fn set_device_nickname(serial: String, nickname: String) -> Result<(), AppError> {
    let path = config_path()?;
    let mut app_config =
        config::load_or_default(&path).map_err(|e| AppError::new("config_load", e.to_string()))?;
    let nickname = nickname.trim();
    if nickname.is_empty() {
        app_config.device_nicknames.remove(&serial);
    } else {
        app_config
            .device_nicknames
            .insert(serial, nickname.to_string());
    }
    config::save_to(&path, &app_config).map_err(|e| AppError::new("config_save", e.to_string()))?;
    Ok(())
}

/// Apelidos persistidos, por serial (a UI resolve o nome exibido a partir
/// daqui, sem precisar de uma chamada por dispositivo).
#[tauri::command]
async fn list_device_nicknames() -> Result<HashMap<String, String>, AppError> {
    let path = config_path()?;
    let app_config =
        config::load_or_default(&path).map_err(|e| AppError::new("config_load", e.to_string()))?;
    Ok(app_config.device_nicknames)
}

/// Apelido persistido de um serial, se houver — usado nos pontos de
/// criação/restart do device virtual (`start_stream`/
/// `restart_android_session`) pra resolver o discriminador amigável sem a
/// UI precisar repassar o apelido em toda chamada.
fn device_nickname_for(serial: &str) -> Option<String> {
    let path = config_path().ok()?;
    let app_config = config::load_or_default(&path).ok()?;
    app_config.device_nicknames.get(serial).cloned()
}

async fn teardown_rtsp_runtime(state: &State<'_, AppState>, runtime: RtspRuntime) {
    runtime.session.stop().await;
    if let Some(task) = runtime.preview_task {
        task.abort();
    }
    if let Ok(mut vcam) = state.vcam.lock() {
        let _ = vcam.destroy(&runtime.vcam_id);
    }
}

/// FR-018: inicia a fonte RTSP como webcam virtual independente (≤ 300 ms —
/// pipeline low-delay de research R5). A credencial sai do cofre e entra na
/// URL SOMENTE aqui, em runtime.
#[tauri::command]
async fn start_rtsp(
    app: AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<StartStreamResponse, AppError> {
    if state.rtsp.lock().await.contains_key(&id) {
        return Err(AppError::new("rtsp_already_running", "Fonte já está ativa"));
    }
    virtualcam::check_capacity(active_source_count(&state).await)?;

    let path = config_path()?;
    let app_config =
        config::load_or_default(&path).map_err(|e| AppError::new("config_load", e.to_string()))?;
    let source = app_config
        .rtsp_sources
        .iter()
        .find(|s| s.id == id)
        .cloned()
        .ok_or_else(|| AppError::new("rtsp_not_found", "Fonte RTSP não encontrada"))?;

    let secret = match source.secret_ref.as_deref() {
        Some(secret_ref) => {
            secrets::get_secret(secret_ref).map_err(|e| AppError::new("keyring", e.to_string()))?
        }
        None => None,
    };
    let runtime_url = rtsp_manager::inject_credentials(&source.url, secret.as_deref());

    let paths = resolve_external_paths();
    // Validação (auth ≠ host inacessível) antes de criar qualquer recurso —
    // com retry (T054): uma fonte recém-iniciada pode não estar pronta pra
    // entregar o 1º frame dentro dos 3s de UMA tentativa.
    rtsp_manager::probe_url_with_retry(
        &paths.ffmpeg,
        &runtime_url,
        &paths.extra_env,
        rtsp_manager::PROBE_MAX_ATTEMPTS,
        rtsp_manager::PROBE_RETRY_DELAY,
    )
    .await?;

    // T065c: nome amigável no seletor do OBS (o nome que o usuário deu à
    // fonte, ex. "CamLink câmera do portão a1b2") em vez do Uuid cru.
    let (name, suffix) = virtualcam::rtsp_label_parts(&source.name, &id);
    let label = virtualcam::vcam_label(&name, &suffix);
    let virtual_camera = {
        let mut vcam = state.vcam.lock().unwrap();
        vcam.create(&label, RTSP_RESOLUTION, RTSP_FPS)
            .map_err(|e| AppError::new("vcam_create_failed", e.to_string()))?
    };
    let vcam_id = virtual_camera.id;
    let session_id = Uuid::new_v4();

    // Preview 1-5 fps (FR-023) — mesma infra do preview Android.
    let preview_dims = preview::preview_dimensions(RTSP_RESOLUTION);
    let preview_last = Arc::new(StdMutex::new(Instant::now() - PREVIEW_INTERVAL));
    let (preview_tx, preview_rx) = tokio::sync::watch::channel(None);
    let app_for_preview = app.clone();
    spawn_preview_encoder(preview_rx, move |jpeg| {
        let payload = PreviewFrameEvent {
            session_id,
            jpeg_base64: BASE64.encode(jpeg),
        };
        let _ = app_for_preview.emit("preview_frame", payload);
    });

    // Frames RGBA decodificados pelo ffmpeg (rawvideo no stdout) chegam
    // pelo sink → câmera virtual (`feed_frame`, escrito pelo ffmpeg PRÓPRIO
    // do `V4l2Backend`) + preview, no Linux e no Windows igualmente. Antes o
    // Linux escrevia direto no device (`-f v4l2`) e o preview lia de volta
    // do mesmo `/dev/videoN` — como o v4l2loopback só admite um LEITOR por
    // vez (`exclusive_caps=1`), o preview entrava em disputa constante com
    // o OBS pelo slot de leitura assim que havia um consumidor real, e
    // piscava (achado em bancada com 3 celulares + 1 RTSP simultâneos,
    // 2026-08-08 — T065). Unificado com o caminho já usado pelo Windows:
    // sem read-back nenhum, preview e câmera virtual vêm do MESMO buffer
    // decodificado.
    #[allow(unused_variables)]
    let (sink, v4l2_device, preview_task): (
        Option<FrameSink>,
        Option<String>,
        Option<tokio::task::JoinHandle<()>>,
    );
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let vcam_handle = Arc::clone(&state.vcam);
        let preview_last = Arc::clone(&preview_last);
        let frame_sink: FrameSink = Arc::new(move |frame: &[u8]| {
            if let Ok(mut backend) = vcam_handle.lock() {
                let _ = backend.feed_frame(&vcam_id, frame);
            }
            publish_preview(
                &preview_last,
                &preview_tx,
                frame,
                RTSP_RESOLUTION,
                preview_dims,
                true,
            );
        });
        sink = Some(frame_sink);
        v4l2_device = None;
        preview_task = None;
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (preview_tx, preview_last, preview_dims);
        return Err(AppError::new(
            "unsupported_platform",
            "Plataforma não suportada",
        ));
    }

    let session_config = rtsp_manager::RtspSessionConfig {
        ffmpeg: paths.ffmpeg,
        url: runtime_url,
        resolution: RTSP_RESOLUTION,
        fps: RTSP_FPS,
        v4l2_device,
    };
    let vcam_for_standby = Arc::clone(&state.vcam);
    let on_standby: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |message: &str| {
        if let Ok(mut backend) = vcam_for_standby.lock() {
            let _ = backend.set_standby(&vcam_id, message);
        }
    });
    let session = rtsp_manager::start_session(session_config, sink, on_standby);

    state.rtsp.lock().await.insert(
        id,
        RtspRuntime {
            session,
            vcam_id,
            preview_task,
        },
    );

    Ok(StartStreamResponse {
        session_id,
        virtual_camera,
        config: StreamConfig {
            resolution: RTSP_RESOLUTION,
            fps: RTSP_FPS,
            bitrate: 0,
            codec: model::VideoCodec::H264,
            camera_id: String::new(),
        },
        state: SessionState::Streaming,
        stats: SessionStats::default(),
    })
}

/// Encerra a fonte RTSP e libera a câmera virtual (FR-021).
#[tauri::command]
async fn stop_rtsp(state: State<'_, AppState>, id: Uuid) -> Result<(), AppError> {
    let Some(runtime) = state.rtsp.lock().await.remove(&id) else {
        return Ok(()); // idempotente
    };
    teardown_rtsp_runtime(&state, runtime).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Eventos (emit)
// ---------------------------------------------------------------------------

#[derive(Clone, serde::Serialize)]
struct SessionStateEvent {
    session_id: Uuid,
    state: SessionState,
    stats: SessionStats,
}

/// `preview_frame` (contracts/tauri-commands.md, FR-023): descartável, o
/// stream principal nunca espera por ele.
#[derive(Clone, serde::Serialize)]
struct PreviewFrameEvent {
    session_id: Uuid,
    jpeg_base64: String,
}

/// Task de encode do preview (T1.3/FR-023): consome o frame RGBA mais
/// recente publicado pelo sink (canal `watch` — sem fila; frames que chegam
/// enquanto um encode roda substituem o anterior) e entrega o JPEG ao
/// `emit`. Termina sozinha quando o sender some — o sink é dropado junto com
/// o pipeline no stop da sessão.
fn spawn_preview_encoder(
    mut rx: tokio::sync::watch::Receiver<Option<(Vec<u8>, u32, u32)>>,
    emit: impl Fn(Vec<u8>) + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let frame = rx.borrow_and_update().clone();
            let Some((rgba, w, h)) = frame else { continue };
            match preview::encode_preview_jpeg(&rgba, w, h) {
                Ok(jpeg) => emit(jpeg),
                // Preview é descartável (FR-023): falha aqui nunca derruba o
                // stream, mas precisa aparecer no log — era o silêncio que
                // escondia o preview morto no Linux.
                Err(e) => tracing::warn!(error = %e, "encode de preview falhou"),
            }
        }
    })
}

/// Emite `session_state` a cada tick (`SESSION_STATE_POLL_INTERVAL`) até a
/// sessão voltar a `Idle` (FR-010) — inclui fps/reconnects atualizados, que
/// mudam sem necessariamente trocar de `SessionState`. Cada `start_stream`
/// bem-sucedido dispara uma instância deste emissor; ele mesmo termina
/// quando não há mais nada a reportar.
fn spawn_session_state_emitter(
    app: AppHandle,
    session_id: Uuid,
    frame_count: Arc<std::sync::atomic::AtomicU64>,
) {
    tauri::async_runtime::spawn(async move {
        let mut last_frame_count = 0u64;
        let mut last_tick = tokio::time::Instant::now();
        let mut smoothed_fps = 0.0f32;
        loop {
            let session = {
                let state = app.state::<AppState>();
                state.stream_manager.session(session_id).await
            };
            let Some(mut session) = session else {
                break;
            };

            let now = tokio::time::Instant::now();
            let current_count = frame_count.load(std::sync::atomic::Ordering::Relaxed);
            let elapsed = (now - last_tick).as_secs_f32();
            if elapsed > 0.0 {
                // A taxa instantânea por tick (250ms) tem alias com o
                // padrão real de chegada dos frames, que em hardware real
                // (Samsung SM-G781B) veio em rajadas de ~1s (provável
                // intervalo de GOP/keyframe) — o valor cru oscilava entre
                // ~4 e ~27 fps a cada tick mesmo com uma taxa sustentada
                // real de ~15 fps (confirmado contra os timestamps de
                // `decoded_count` do próprio ffmpeg). Suavizado por EMA
                // (janela efetiva ~1,25s) em vez de mostrar o valor cru.
                let instantaneous_fps = (current_count - last_frame_count) as f32 / elapsed;
                smoothed_fps =
                    smoothed_fps * FPS_SMOOTHING + instantaneous_fps * (1.0 - FPS_SMOOTHING);
            }
            session.stats.fps = smoothed_fps;
            last_frame_count = current_count;
            last_tick = now;

            // Emite sempre (não só em mudança de estado): fps/reconnects
            // mudam a cada tick durante o streaming normal, sem transição
            // de SessionState nenhuma — só emitir "toda transição" (leitura
            // literal do contrato) deixava o fps aparecer travado no
            // frontend o tempo todo.
            let payload = SessionStateEvent {
                session_id,
                state: session.state.clone(),
                stats: session.stats.clone(),
            };
            let _ = app.emit("session_state", payload);
            if session.state == SessionState::Idle {
                break;
            }
            tokio::time::sleep(SESSION_STATE_POLL_INTERVAL).await;
        }
    });
}

/// Polling de hotplug ADB (FR-001/FR-002/FR-002a — research.md R9): mantém
/// o cache de `list_devices` e emite `device_connected`/`device_disconnected`
/// /`device_unauthorized`.
fn spawn_device_polling(app: AppHandle, adb_path: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let mut previous: Vec<AndroidDevice> = Vec::new();
        loop {
            match device_manager::poll_devices(&adb_path, &mut previous).await {
                Ok(events) => {
                    {
                        let state = app.state::<AppState>();
                        *state.devices.lock().unwrap() = previous.clone();
                    }
                    for event in events {
                        match event {
                            DeviceEvent::Connected(device) => {
                                let _ = app.emit("device_connected", device);
                            }
                            DeviceEvent::Disconnected(serial) => {
                                let _ = app.emit("device_disconnected", serial);
                            }
                            DeviceEvent::Unauthorized(serial) => {
                                let _ = app.emit("device_unauthorized", serial);
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::debug!(code = %err.code, msg = %err.msg, "poll_devices falhou");
                }
            }
            tokio::time::sleep(DEVICE_POLL_INTERVAL).await;
        }
    });
}

/// Espera SIGINT/Ctrl+C (e SIGTERM no Unix) e mata à força qualquer backend
/// ainda rodando antes de sair — cobre o processo inteiro sendo morto
/// (terminal fechado, Ctrl+C, `pnpm`/`cargo run` não repassando o sinal
/// pros filhos), caso em que `kill_on_drop(true)` nunca chega a rodar
/// porque não há shutdown gracioso do processo Rust (achado em bancada
/// 2026-07-28: `scrcpy` órfão continuava escrevendo no device v4l2 mesmo
/// com o app "fechado"). `std::process::exit` no fim é necessário porque
/// `tauri::Builder::run` bloqueia a thread principal no próprio loop de
/// eventos da janela — sem isso o processo nunca voltaria a ler o
/// resultado deste `select!` pra realmente encerrar.
async fn watch_for_shutdown_signal(
    stream_manager: StreamManager,
    rtsp: Arc<TokioMutex<HashMap<Uuid, RtspRuntime>>>,
    vcam: Arc<StdMutex<Box<dyn VirtualCameraBackend + Send>>>,
) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("sinal de encerramento recebido; matando backends ativos");
    stream_manager.kill_all_backends().await;
    kill_all_rtsp(&rtsp).await;
    if let Ok(mut vcam) = vcam.lock() {
        vcam.purge_all();
    }
    std::process::exit(0);
}

/// Mata os `ffmpeg` de todas as sessões RTSP ativas sem tocar no device
/// v4l2/dshow em si (persiste pro próximo start, mesma lógica de
/// `kill_all_backends` pro lado Android) — usado só nos dois hooks de
/// shutdown do processo inteiro, nunca no fluxo normal de `stop_rtsp`
/// (que já limpa certo via `teardown_rtsp_runtime`).
async fn kill_all_rtsp(rtsp: &Arc<TokioMutex<HashMap<Uuid, RtspRuntime>>>) {
    let mut sessions = rtsp.lock().await;
    for (_, runtime) in sessions.drain() {
        if let Some(preview) = runtime.preview_task {
            preview.abort();
        }
        runtime.session.stop().await;
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Tauri não entra sozinho num runtime Tokio (o loop de eventos da
    // janela não é tokio); sem isso, qualquer `tokio::spawn`/`.await` fora
    // do handler de um comando (ex.: dentro de `.setup()`) panica com
    // "there is no reactor running" — só apareceu rodando `cargo tauri dev`
    // de verdade, nenhum teste (todos `#[tokio::test]`, que já entram no
    // runtime sozinhos) pegava isso.
    let runtime = tokio::runtime::Runtime::new().expect("falha ao criar runtime tokio");
    tauri::async_runtime::set(runtime.handle().clone());
    let _runtime_guard = runtime.enter();

    let adb_path = PathBuf::from("adb");
    runtime.spawn(cleanup_stale_forwards());
    let stream_manager = StreamManager::new(resolve_external_paths());
    let rtsp_sessions: Arc<TokioMutex<HashMap<Uuid, RtspRuntime>>> =
        Arc::new(TokioMutex::new(HashMap::new()));
    let vcam: Arc<StdMutex<Box<dyn VirtualCameraBackend + Send>>> =
        Arc::new(StdMutex::new(new_vcam_backend()));
    runtime.spawn(watch_for_shutdown_signal(
        stream_manager.clone(),
        Arc::clone(&rtsp_sessions),
        Arc::clone(&vcam),
    ));
    let stream_manager_for_exit = stream_manager.clone();
    let rtsp_for_exit = Arc::clone(&rtsp_sessions);
    let vcam_for_exit = Arc::clone(&vcam);
    let app_state = AppState {
        stream_manager,
        vcam,
        devices: Arc::new(StdMutex::new(Vec::new())),
        sessions: TokioMutex::new(HashMap::new()),
        rtsp: rtsp_sessions,
        restart_locks: TokioMutex::new(HashMap::new()),
    };

    tauri::Builder::default()
        .manage(app_state)
        .setup(move |app| {
            spawn_device_polling(app.handle().clone(), adb_path.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            start_stream,
            stop_stream,
            get_capabilities,
            set_control,
            switch_camera,
            add_rtsp_source,
            remove_rtsp_source,
            list_rtsp_sources,
            start_rtsp,
            stop_rtsp,
            raw_snapshot,
            raw_sequence_start,
            raw_sequence_stop,
            set_raw_output_dir,
            set_device_nickname,
            list_device_nicknames
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |_app_handle, event| {
            // Fechar a janela normalmente (botão X, `app.exit()`) NÃO passa
            // por sinal nenhum do SO — o tauri chama `std::process::exit`
            // internamente no fim do próprio loop de eventos, pulando o
            // `Drop` de tudo que ainda estiver vivo na stack/heap (inclusive
            // `kill_on_drop` dos `Child` de backend). `watch_for_shutdown_signal`
            // só cobre o processo inteiro morrendo por sinal externo
            // (Ctrl+C, `kill`); este hook cobre o outro caminho de saída
            // (achado em bancada 2026-07-28: `scrcpy` órfão sobrevivia
            // mesmo fechando a janela do app normalmente).
            if let tauri::RunEvent::ExitRequested { .. } = event {
                tauri::async_runtime::block_on(async {
                    stream_manager_for_exit.kill_all_backends().await;
                    kill_all_rtsp(&rtsp_for_exit).await;
                });
                if let Ok(mut vcam) = vcam_for_exit.lock() {
                    vcam.purge_all();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T1.3 (FR-023): o encode de preview roda numa task própria alimentada
    /// por um canal `watch` (sempre o frame mais recente, sem fila) — o loop
    /// de leitura publica e segue, nunca espera pelo encode.
    #[tokio::test]
    async fn preview_encoder_emits_jpeg_for_published_frame() {
        let (tx, rx) = tokio::sync::watch::channel(None);
        let (jpeg_tx, mut jpeg_rx) = tokio::sync::mpsc::unbounded_channel();
        let _handle = spawn_preview_encoder(rx, move |jpeg| {
            let _ = jpeg_tx.send(jpeg);
        });
        let rgba = [10u8, 20, 30, 255].repeat(4 * 4);
        tx.send(Some((rgba, 4, 4))).expect("receiver vivo");
        let jpeg = tokio::time::timeout(Duration::from_secs(2), jpeg_rx.recv())
            .await
            .expect("encoder deve emitir dentro do timeout")
            .expect("canal de saída aberto");
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "JPEG começa com SOI");
    }

    /// Lifecycle (stop/start repetido): quando o pipeline é abortado no stop
    /// o sink some, o sender do watch é dropado e a task de encode termina
    /// sozinha — nada vaza entre sessões.
    #[tokio::test]
    async fn preview_encoder_exits_when_sink_is_dropped() {
        for _ in 0..3 {
            let (tx, rx) = tokio::sync::watch::channel(None);
            let handle = spawn_preview_encoder(rx, |_jpeg| {});
            drop(tx);
            tokio::time::timeout(Duration::from_secs(2), handle)
                .await
                .expect("task deve terminar quando o sender some")
                .expect("task não deve panicar");
        }
    }
}
