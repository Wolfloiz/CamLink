//! Comandos e eventos Tauri de US1 (T025) — conforme
//! `specs/001-phone-webcam-bridge/contracts/tauri-commands.md`: liga
//! `device_manager` (polling + eventos de hotplug), `virtualcam` (câmera
//! virtual por plataforma) e `stream_manager` (lifecycle scrcpy/adb) ao
//! frontend. Camada fina de orquestração — a lógica testada vive nos
//! módulos que ela chama; esta camada só é validada manualmente
//! (research.md R11, camada 3 — T029).

pub mod camera_controller;
pub mod config;
pub mod device_manager;
pub mod error;
pub mod model;
pub mod preview;
pub mod raw_manager;
pub mod rtsp_manager;
pub mod secrets;
pub mod stream_manager;
pub mod virtualcam;

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use device_manager::DeviceEvent;
use error::AppError;
use model::{AndroidDevice, SessionSource, SessionState, SessionStats, StreamConfig};
use stream_manager::{ExternalPaths, FrameSink, StreamManager};
use virtualcam::VirtualCameraBackend;

#[cfg(target_os = "windows")]
use virtualcam::dshow::DShowBackend;
#[cfg(target_os = "linux")]
use virtualcam::v4l2::V4l2Backend;

const DEVICE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SESSION_STATE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const PREVIEW_INTERVAL: Duration = Duration::from_secs(1);
const VIRTUAL_CAMERA_LABEL: &str = "CamLink Android";

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

/// Caminhos dos executáveis externos. v1: resolvidos via PATH (assume
/// adb/scrcpy/ffmpeg instalados — quickstart.md pré-requisitos); o jar do
/// scrcpy-server stock vem de `SCRCPY_SERVER_PATH` se definida, senão
/// assume-se `scrcpy-server` no diretório de trabalho. Empacotamento real
/// (instalador resolvendo paths bundled) fica para uma iteração futura.
fn resolve_external_paths() -> ExternalPaths {
    let server_jar = std::env::var("SCRCPY_SERVER_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("scrcpy-server"));
    ExternalPaths {
        adb: PathBuf::from("adb"),
        scrcpy: PathBuf::from("scrcpy"),
        ffmpeg: PathBuf::from("ffmpeg"),
        server_jar,
        extra_env: Vec::new(),
    }
}

struct AppState {
    stream_manager: StreamManager,
    vcam: Arc<StdMutex<Box<dyn VirtualCameraBackend + Send>>>,
    devices: Arc<StdMutex<Vec<AndroidDevice>>>,
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

/// Resposta de `start_stream`: `StreamSession` não carrega o próprio
/// `session_id` (só é conhecido fora do `stream_manager`, como retorno de
/// `start()`), mas o frontend precisa dele para `stop_stream` e para
/// filtrar `session_state`/`preview_frame` — contracts/tauri-commands.md
/// diz "StreamSession (inclui VirtualCamera)"; devolvemos os dois
/// explicitamente em vez do `Uuid` cru de `StreamSession.virtual_camera`.
#[derive(Clone, serde::Serialize)]
struct StartStreamResponse {
    session_id: Uuid,
    virtual_camera: model::VirtualCamera,
    config: StreamConfig,
    state: SessionState,
    stats: SessionStats,
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
    let virtual_camera = {
        let mut vcam = state.vcam.lock().unwrap();
        vcam.create(VIRTUAL_CAMERA_LABEL, config.resolution, config.fps)
            .map_err(|e| AppError::new("vcam_create_failed", e.to_string()))?
    };

    let vcam_target = virtual_camera.backend_path.clone();
    let vcam_id = virtual_camera.id;
    let vcam_handle = Arc::clone(&state.vcam);
    let (width, height) = config.resolution;

    // T026 (FR-023): a sessão ainda não existe no momento em que o sink é
    // montado (o id só nasce dentro de `stream_manager.start()`), então o
    // preview usa uma célula preenchida logo depois — frames que cheguem
    // antes disso (janela muito curta) simplesmente não geram preview,
    // aceitável por serem descartáveis por natureza.
    let session_id_cell: Arc<StdMutex<Option<Uuid>>> = Arc::new(StdMutex::new(None));
    let session_id_for_sink = Arc::clone(&session_id_cell);
    let preview_last = Arc::new(StdMutex::new(Instant::now() - PREVIEW_INTERVAL));
    let app_for_preview = app.clone();

    let sink: FrameSink = Arc::new(move |frame: &[u8]| {
        if let Ok(mut backend) = vcam_handle.lock() {
            let _ = backend.feed_frame(&vcam_id, frame);
        }
        let Some(session_id) = *session_id_for_sink.lock().unwrap() else {
            return;
        };
        let due = {
            let mut last = preview_last.lock().unwrap();
            let due = last.elapsed() >= PREVIEW_INTERVAL;
            if due {
                *last = Instant::now();
            }
            due
        };
        if !due {
            return;
        }
        if let Ok(jpeg) = preview::encode_preview_jpeg(frame, width, height) {
            let payload = PreviewFrameEvent {
                session_id,
                jpeg_base64: BASE64.encode(jpeg),
            };
            let _ = app_for_preview.emit("preview_frame", payload);
        }
    });

    let start_result = state
        .stream_manager
        .start(
            SessionSource::Android(serial),
            config,
            &vcam_target,
            Some(sink),
        )
        .await;

    let session_id = match start_result {
        Ok(id) => id,
        Err(err) => {
            if let Ok(mut vcam) = state.vcam.lock() {
                let _ = vcam.destroy(&vcam_id);
            }
            return Err(err);
        }
    };
    *session_id_cell.lock().unwrap() = Some(session_id);

    let session = state
        .stream_manager
        .session(session_id)
        .await
        .ok_or_else(|| AppError::new("session_not_found", "Sessão sumiu logo após ser criada"))?;

    spawn_session_state_emitter(app.clone(), session_id);
    #[cfg(target_os = "linux")]
    spawn_preview_polling_linux(app, session_id, vcam_target);

    Ok(StartStreamResponse {
        session_id,
        virtual_camera,
        config: session.config,
        state: session.state,
        stats: session.stats,
    })
}

/// Linux: o cliente scrcpy escreve direto no device (`--v4l2-sink`), sem
/// passar pelo nosso processo — diferente do Windows, onde os frames já
/// fluem pelo `FrameSink`. Por isso o preview no Linux lê de volta o
/// device virtual a ~1 fps (T026). **Não validado nesta sessão** (host de
/// desenvolvimento é Windows) — mesma ressalva de `preview::
/// capture_one_frame_linux` e de T022.
#[cfg(target_os = "linux")]
fn spawn_preview_polling_linux(app: AppHandle, session_id: Uuid, device_path: String) {
    tokio::spawn(async move {
        loop {
            let state = app.state::<AppState>();
            let still_running = matches!(
                state.stream_manager.state(session_id).await,
                Some(
                    SessionState::Streaming | SessionState::Reconnecting | SessionState::SourceLost
                )
            );
            drop(state);
            if !still_running {
                break;
            }

            let path = device_path.clone();
            let frame =
                tokio::task::spawn_blocking(move || preview::capture_one_frame_linux(&path))
                    .await
                    .ok()
                    .and_then(|r| r.ok());
            if let Some((rgba, width, height)) = frame {
                if let Ok(jpeg) = preview::encode_preview_jpeg(&rgba, width, height) {
                    let payload = PreviewFrameEvent {
                        session_id,
                        jpeg_base64: BASE64.encode(jpeg),
                    };
                    let _ = app.emit("preview_frame", payload);
                }
            }
            tokio::time::sleep(PREVIEW_INTERVAL).await;
        }
    });
}

/// Encerra a sessão e libera a câmera virtual associada (FR-021: nunca
/// deixar um device virtual órfão).
#[tauri::command]
async fn stop_stream(state: State<'_, AppState>, session_id: Uuid) -> Result<(), AppError> {
    let session = state.stream_manager.session(session_id).await;
    state.stream_manager.stop(session_id).await?;
    if let Some(session) = session {
        if let Ok(mut vcam) = state.vcam.lock() {
            let _ = vcam.destroy(&session.virtual_camera);
        }
    }
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

/// Emite `session_state` a cada mudança de estado até a sessão voltar a
/// `Idle` (FR-010). Cada `start_stream` bem-sucedido dispara uma instância
/// deste emissor; ele mesmo termina quando não há mais nada a reportar.
fn spawn_session_state_emitter(app: AppHandle, session_id: Uuid) {
    tokio::spawn(async move {
        let mut last_state: Option<SessionState> = None;
        loop {
            let session = {
                let state = app.state::<AppState>();
                state.stream_manager.session(session_id).await
            };
            let Some(session) = session else {
                break;
            };
            if last_state.as_ref() != Some(&session.state) {
                let payload = SessionStateEvent {
                    session_id,
                    state: session.state.clone(),
                    stats: session.stats.clone(),
                };
                let _ = app.emit("session_state", payload);
                last_state = Some(session.state.clone());
            }
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
    tokio::spawn(async move {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let adb_path = PathBuf::from("adb");
    let app_state = AppState {
        stream_manager: StreamManager::new(resolve_external_paths()),
        vcam: Arc::new(StdMutex::new(new_vcam_backend())),
        devices: Arc::new(StdMutex::new(Vec::new())),
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
            stop_stream
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
