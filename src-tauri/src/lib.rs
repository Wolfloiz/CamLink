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
/// Cadência da miniatura de preview (~5 fps): suficiente para enquadrar a
/// câmera sem competir com o stream principal — o frame é pequeno
/// (`preview_dimensions`, ≤640 px) e o encode roda fora do hot path
/// (`spawn_preview_encoder`).
const PREVIEW_INTERVAL: Duration = Duration::from_millis(200);
const VIRTUAL_CAMERA_LABEL: &str = "CamLink Android";
/// EMA aplicada em `SessionStats.fps` — janela efetiva ~= POLL_INTERVAL /
/// (1 - FPS_SMOOTHING) ≈ 1,25s, escolhida pra cobrir o período de rajada
/// (~1s) observado em hardware real (ver `spawn_session_state_emitter`).
const FPS_SMOOTHING: f32 = 0.8;

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
/// scrcpy-server stock vem, em ordem: `SCRCPY_SERVER_PATH` se definida →
/// procurado ao lado do binário `scrcpy` no PATH → `scrcpy-server` no
/// diretório de trabalho como último recurso. Empacotamento real
/// (instalador resolvendo paths bundled) fica para uma iteração futura.
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
    // O preview trafega em miniatura (`preview_dimensions`): no Linux o
    // leitor ffmpeg já entrega nesse tamanho (mesma função gera o `scale=`);
    // no Windows os frames chegam full-res do decode do socket scrcpy e são
    // reduzidos no sink antes de publicar. As dims viajam junto com o frame
    // até o encoder: `encode_preview_jpeg` valida len×dims, e uma
    // divergência mataria o preview em silêncio.
    let source_dims = config.resolution;
    let preview_dims = preview::preview_dimensions(source_dims);

    // T026 (FR-023): a sessão ainda não existe no momento em que o sink é
    // montado (o id só nasce dentro de `stream_manager.start()`), então o
    // preview usa uma célula preenchida logo depois — frames que cheguem
    // antes disso (janela muito curta) simplesmente não geram preview,
    // aceitável por serem descartáveis por natureza.
    let session_id_cell: Arc<StdMutex<Option<Uuid>>> = Arc::new(StdMutex::new(None));
    let preview_last = Arc::new(StdMutex::new(Instant::now() - PREVIEW_INTERVAL));
    // `SessionStats.fps` nunca era atualizado (ficava travado no 0.0 do
    // valor inicial) — contamos aqui, fora do `stream_manager` (que não
    // sabe nada sobre frames, só sobre o processo do backend), e
    // computamos fps por delta no emissor de `session_state` abaixo.
    let frame_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let frame_count_for_sink = Arc::clone(&frame_count);

    // T1.3: o sink publica o frame cru (miniatura) e segue; o encode JPEG —
    // que em debug/1080p chegava a centenas de ms e virava backpressure no
    // pipe do ffmpeg — roda na task abaixo, sempre sobre o frame mais
    // recente. O sender morre junto com o sink (abort do pipeline no stop),
    // encerrando a task.
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

    let sink: FrameSink = Arc::new(move |frame: &[u8]| {
        frame_count_for_sink.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Windows: o sink é o caminho de ENTREGA — os frames vêm do socket
        // scrcpy e precisam ser empurrados à câmera virtual. Linux: os
        // frames vêm do LEITOR do próprio device v4l2 (o scrcpy já escreveu
        // lá via --v4l2-sink); realimentá-los com feed_frame criaria um
        // loop device→sink→device.
        #[cfg(target_os = "windows")]
        {
            if let Ok(mut backend) = vcam_handle.lock() {
                let _ = backend.feed_frame(&vcam_id, frame);
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (&vcam_handle, &vcam_id);
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
        let (pw, ph) = preview_dims;
        let published: Result<Vec<u8>, String>;
        #[cfg(target_os = "windows")]
        {
            published = preview::downsample_rgba(frame, source_dims.0, source_dims.1, pw, ph);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = source_dims;
            published = Ok(frame.to_vec());
        }
        match published {
            Ok(rgba) => {
                let _ = preview_tx.send(Some((rgba, pw, ph)));
            }
            // Preview é descartável (FR-023), mas a falha precisa aparecer:
            // um frame fora da geometria esperada (ex.: rotação) cai aqui.
            Err(e) => tracing::warn!(error = %e, "downsample do preview falhou"),
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

    spawn_session_state_emitter(app, session_id, frame_count);

    Ok(StartStreamResponse {
        session_id,
        virtual_camera,
        config: session.config,
        state: session.state,
        stats: session.stats,
    })
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
