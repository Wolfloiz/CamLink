//! Lifecycle do backend scrcpy/adb: monta os argumentos (T020), sobe o
//! processo, monitora crash/stderr e reconecta automaticamente
//! (FR-005/006). Linux: cliente `scrcpy` stock com `--v4l2-sink` direto —
//! o próprio cliente entrega os frames ao v4l2loopback, nada passa pelo
//! nosso código. Windows: sem o cliente `scrcpy` — bootstrap direto do
//! scrcpy-server (`adb push` + `adb forward` + `app_process`, replicando
//! `scrcpy/app/src/server.c`) porque `--record` não tem saída para stdout
//! na v4.0 real (research.md R12), e leitura do socket de vídeo (protocolo
//! documentado em `scrcpy/doc/develop.md` §Protocol, verificado byte-a-byte
//! contra `DesktopConnection.java`/`Streamer.java` do submodule) → decode
//! via subprocesso `ffmpeg` → `feed_frame` (via `FrameSink`, injetado por
//! quem chama `start()` — T025, que cria a câmera virtual primeiro).
//!
//! Jar **stock** nesta fase (US1); o jar forkado com o socket de controle
//! entra em T037/US2.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::error::AppError;
use crate::model::{
    SessionEvent, SessionSource, SessionState, SessionStats, StreamConfig, StreamSession,
    VideoCodec,
};

/// Path fixo no device onde o jar do servidor é empurrado (mesma convenção
/// do cliente scrcpy real — `SC_DEVICE_SERVER_PATH` em `server.c`).
pub const SCRCPY_DEVICE_SERVER_PATH: &str = "/data/local/tmp/scrcpy-server.jar";

/// Precisa acompanhar a tag do submodule `scrcpy/` (fixada em R1) — cliente
/// e servidor devem casar de versão; o protocolo de bootstrap do servidor é
/// "interno" e pode mudar entre releases (scrcpy/doc/develop.md).
pub const SCRCPY_VERSION: &str = "4.0";

const RETRY_BACKOFF: Duration = Duration::from_millis(200);
const STOP_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Sinal de parada compartilhado entre `stop()` e a task de monitor de uma
/// sessão. Existe porque o child do backend passa a maior parte do tempo
/// "emprestado" para dentro de `wait_for_exit_or_fatal_stderr` (fora do
/// `RunningSession.child`, que fica `None` enquanto isso) — sem esse sinal,
/// `stop()` só encontraria `child: None` e devolveria sucesso sem matar o
/// processo de verdade (bug encontrado ao revisar T024 para o frontend:
/// `stop()` "funcionava" nos testes só por sorte de timing).
struct SessionControl {
    stop_requested: AtomicBool,
    notify: Notify,
}

impl SessionControl {
    fn new() -> Self {
        Self {
            stop_requested: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    fn is_stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::SeqCst)
    }
}

/// Recebe frames RGBA decodificados (contrato de
/// `VirtualCameraBackend::feed_frame`) para entregar à câmera virtual.
/// Injetado por quem orquestra `start()` (T025): a criação da câmera
/// virtual acontece fora deste módulo, então este módulo nunca precisa
/// conhecer o backend concreto (v4l2/DirectShow) nem seu `Uuid`.
pub type FrameSink = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Caminhos dos executáveis externos usados pelo pipeline — injetáveis para
/// testes com binários fake (research.md R11) via `extra_env`.
#[derive(Debug, Clone)]
pub struct ExternalPaths {
    pub adb: PathBuf,
    pub scrcpy: PathBuf,
    pub ffmpeg: PathBuf,
    pub server_jar: PathBuf,
    /// Variáveis de ambiente extras aplicadas a todo subprocesso spawnado
    /// por este módulo. Em produção fica vazio; testes usam para controlar
    /// o comportamento do binário fake (`fake_backend`).
    pub extra_env: Vec<(String, String)>,
}

/// Classifica uma linha de stderr do backend: `Some` quando o erro é
/// definitivo e acionável (não vale reconectar); `None` quando deve ser
/// tratado como perda transitória de sinal (→ SourceLost/Reconnecting).
pub fn classify_stderr(line: &str) -> Option<AppError> {
    let lower = line.to_lowercase();
    if lower.contains("unauthorized") {
        return Some(
            AppError::new("device_unauthorized", "Dispositivo Android não autorizado")
                .with_hint("Autorize a depuração USB no celular e tente novamente."),
        );
    }
    if lower.contains("no devices/emulators found") || lower.contains("device not found") {
        return Some(
            AppError::new("device_not_found", "Dispositivo não encontrado")
                .with_hint("Verifique o cabo USB e se o dispositivo aparece em `adb devices`."),
        );
    }
    if lower.contains("version") && (lower.contains("mismatch") || lower.contains("doesn't match"))
    {
        return Some(
            AppError::new(
                "scrcpy_version_mismatch",
                "Versão do scrcpy/adb incompatível",
            )
            .with_hint("Reinstale o CamLink ou atualize o scrcpy para a versão >= 4.0."),
        );
    }
    None
}

// ---------------------------------------------------------------------------
// Montagem de argumentos (T020) — puro, sem subprocess.
// ---------------------------------------------------------------------------

fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::H265 => "h265",
    }
}

/// Argumentos do cliente `scrcpy` stock (Linux): captura a câmera do
/// Android e escreve direto no device v4l2loopback já criado por
/// `virtualcam::v4l2` (research.md R3).
pub fn build_scrcpy_client_args(config: &StreamConfig, v4l2_device: &str) -> Vec<String> {
    vec![
        "--video-source=camera".to_string(),
        format!("--camera-id={}", config.camera_id),
        format!(
            "--camera-size={}x{}",
            config.resolution.0, config.resolution.1
        ),
        format!("--camera-fps={}", config.fps),
        format!("--video-bit-rate={}", config.bitrate),
        format!("--video-codec={}", codec_name(config.codec)),
        format!("--v4l2-sink={v4l2_device}"),
        "--no-audio".to_string(),
        "--no-window".to_string(),
        "--no-control".to_string(),
    ]
}

/// Argumentos de `adb ... shell CLASSPATH=... app_process ...
/// com.genymobile.scrcpy.Server` (Windows): bootstrap direto do servidor,
/// sem passar pelo cliente `scrcpy` (research.md R12), replicando
/// `scrcpy/app/src/server.c`.
pub fn build_server_launch_args(scid: u32, config: &StreamConfig) -> Vec<String> {
    vec![
        "shell".to_string(),
        format!("CLASSPATH={SCRCPY_DEVICE_SERVER_PATH}"),
        "app_process".to_string(),
        "/".to_string(),
        "com.genymobile.scrcpy.Server".to_string(),
        SCRCPY_VERSION.to_string(),
        format!("scid={scid:08x}"),
        "log_level=info".to_string(),
        "audio=false".to_string(),
        "video_source=camera".to_string(),
        format!("camera_id={}", config.camera_id),
        format!(
            "camera_size={}x{}",
            config.resolution.0, config.resolution.1
        ),
        format!("max_fps={}", config.fps),
        format!("video_bit_rate={}", config.bitrate),
        "tunnel_forward=true".to_string(),
        "control=false".to_string(),
    ]
}

fn scid_from_session(id: Uuid) -> u32 {
    let bytes = id.as_bytes();
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

// ---------------------------------------------------------------------------
// Protocolo do socket de vídeo scrcpy (Windows — research.md R12).
// Verificado byte-a-byte contra `scrcpy/doc/develop.md` §Protocol e
// `scrcpy/server/src/main/java/com/genymobile/scrcpy/device/
// DesktopConnection.java` (tag do submodule, v4.0), não deduzido/assumido.
// ---------------------------------------------------------------------------

/// `DesktopConnection.DEVICE_NAME_FIELD_LENGTH`.
pub const DEVICE_NAME_FIELD_LENGTH: usize = 64;

/// Ids de codec (ASCII de 4 letras como `u32` big-endian) — só H264 é
/// suportado nesta fase (`StreamConfig::codec` default do pipeline Android).
pub const CODEC_ID_H264: u32 = 0x6832_3634;
pub const CODEC_ID_H265: u32 = 0x6832_3635;

/// Extrai o nome do device do campo de metadata (64 bytes, UTF-8,
/// preenchido com zeros à direita).
pub fn parse_device_name(bytes: &[u8; DEVICE_NAME_FIELD_LENGTH]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionPacket {
    pub client_resized: bool,
    pub width: u32,
    pub height: u32,
}

/// Pacote de sessão (12 bytes, um por início/rotação de captura): bit mais
/// significativo do byte 0 = flag de sessão (distingue de frame header);
/// bit menos significativo do byte 3 = client-resized; bytes 4..8 = width
/// BE; bytes 8..12 = height BE.
pub fn parse_session_packet(bytes: &[u8; 12]) -> Option<SessionPacket> {
    if bytes[0] & 0x80 == 0 {
        return None; // não é pacote de sessão (é um frame header)
    }
    Some(SessionPacket {
        client_resized: bytes[3] & 0x01 != 0,
        width: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        height: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub config_packet: bool,
    pub key_frame: bool,
    pub pts: u64,
    pub packet_size: u32,
}

/// Cabeçalho de frame (12 bytes, antes de cada pacote de mídia): bit mais
/// significativo do byte 0 = 0 (media packet, distingue de session
/// packet); bit 6 = config packet; bit 5 = key frame; bits restantes (61)
/// = PTS; bytes 8..12 = tamanho do pacote BE.
pub fn parse_frame_header(bytes: &[u8; 12]) -> Option<FrameHeader> {
    if bytes[0] & 0x80 != 0 {
        return None; // é pacote de sessão, não frame header
    }
    let mut pts_bytes = [0u8; 8];
    pts_bytes.copy_from_slice(&bytes[0..8]);
    let raw = u64::from_be_bytes(pts_bytes);
    Some(FrameHeader {
        config_packet: bytes[0] & 0x40 != 0,
        key_frame: bytes[0] & 0x20 != 0,
        pts: raw & 0x1FFF_FFFF_FFFF_FFFF,
        packet_size: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
    })
}

/// Extrai a porta local alocada por `adb forward tcp:0 <remote>` (o adb
/// imprime só o número da porta no stdout quando a porta local é `0`).
pub fn parse_forward_port(stdout: &str) -> Option<u16> {
    stdout.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Spawn do backend (plataforma-específico, isolado atrás desta função —
// Princípio IV). Retorna também a porta encaminhada (só Windows: usada
// pelo pipeline de vídeo; `None` no Linux, onde o cliente scrcpy entrega
// os frames sozinho via `--v4l2-sink`).
// ---------------------------------------------------------------------------

async fn spawn_backend(
    paths: &ExternalPaths,
    config: &StreamConfig,
    virtual_camera_target: &str,
    session_id: Uuid,
) -> Result<(Child, Option<u16>), AppError> {
    #[cfg(target_os = "linux")]
    {
        let _ = session_id;
        let args = build_scrcpy_client_args(config, virtual_camera_target);
        let child = Command::new(&paths.scrcpy)
            .args(&args)
            .envs(paths.extra_env.iter().cloned())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AppError::new("spawn_failed", format!("Falha ao iniciar scrcpy: {e}")))?;
        Ok((child, None))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = virtual_camera_target;
        bootstrap_windows_server(paths, config, session_id).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (paths, config, virtual_camera_target, session_id);
        Err(AppError::new(
            "unsupported_platform",
            "Plataforma não suportada",
        ))
    }
}

#[cfg(target_os = "windows")]
async fn bootstrap_windows_server(
    paths: &ExternalPaths,
    config: &StreamConfig,
    session_id: Uuid,
) -> Result<(Child, Option<u16>), AppError> {
    let push_status = Command::new(&paths.adb)
        .arg("push")
        .arg(&paths.server_jar)
        .arg(SCRCPY_DEVICE_SERVER_PATH)
        .envs(paths.extra_env.iter().cloned())
        .status()
        .await
        .map_err(|e| AppError::new("adb_push_failed", format!("Falha no adb push: {e}")))?;
    if !push_status.success() {
        return Err(AppError::new("adb_push_failed", "adb push retornou erro")
            .with_hint("Verifique a conexão USB e a autorização de depuração."));
    }

    let scid = scid_from_session(session_id);
    let forward_output = Command::new(&paths.adb)
        .args([
            "forward",
            "tcp:0",
            &format!("localabstract:scrcpy_{scid:08x}"),
        ])
        .envs(paths.extra_env.iter().cloned())
        .output()
        .await
        .map_err(|e| AppError::new("adb_forward_failed", format!("Falha no adb forward: {e}")))?;
    if !forward_output.status.success() {
        return Err(AppError::new(
            "adb_forward_failed",
            "adb forward retornou erro",
        ));
    }
    // Binários fake de teste (research.md R11) não implementam o protocolo
    // real do adb forward e não imprimem porta nenhuma — nesse caso não há
    // pipeline de vídeo a conectar (os testes de lifecycle usam
    // `video_sink: None`), então a ausência de porta não é um erro aqui.
    let forward_port = parse_forward_port(&String::from_utf8_lossy(&forward_output.stdout));

    let args = build_server_launch_args(scid, config);
    let child = Command::new(&paths.adb)
        .args(&args)
        .envs(paths.extra_env.iter().cloned())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            AppError::new(
                "spawn_failed",
                format!("Falha ao iniciar scrcpy-server: {e}"),
            )
        })?;
    Ok((child, forward_port))
}

// ---------------------------------------------------------------------------
// Pipeline de vídeo Windows: conecta no socket encaminhado, decodifica via
// ffmpeg, entrega os frames RGBA ao `FrameSink`. Falhas aqui NÃO derrubam a
// sessão (o backend/scrcpy-server pode estar saudável mesmo se este
// pipeline falhar) — a câmera simplesmente permanece mostrando a imagem de
// espera (o leitor de memória compartilhada do filtro DirectShow já cobre
// isso via timeout de staleness, ver `virtualcam::dshow`).
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
async fn run_video_pipeline(ffmpeg_path: PathBuf, forward_port: u16, sink: FrameSink) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let result: Result<(), AppError> = async {
        let mut socket = TcpStream::connect(("127.0.0.1", forward_port))
            .await
            .map_err(|e| {
                AppError::new(
                    "video_socket_connect_failed",
                    format!("Falha ao conectar no socket de vídeo: {e}"),
                )
            })?;

        // Handshake (research.md R12 / doc/develop.md §Connection): 1 byte
        // dummy (detecta erro de conexão) + 64 bytes de nome do device.
        let mut dummy = [0u8; 1];
        socket.read_exact(&mut dummy).await.map_err(io_err)?;
        let mut name_buf = [0u8; DEVICE_NAME_FIELD_LENGTH];
        socket.read_exact(&mut name_buf).await.map_err(io_err)?;
        let device_name = parse_device_name(&name_buf);

        let mut codec_buf = [0u8; 4];
        socket.read_exact(&mut codec_buf).await.map_err(io_err)?;
        let codec_id = u32::from_be_bytes(codec_buf);
        if codec_id != CODEC_ID_H264 {
            return Err(AppError::new(
                "unsupported_codec",
                format!("codec de vídeo não suportado: {codec_id:#010x}"),
            ));
        }

        let mut session_buf = [0u8; 12];
        socket.read_exact(&mut session_buf).await.map_err(io_err)?;
        let session = parse_session_packet(&session_buf).ok_or_else(|| {
            AppError::new(
                "protocol_error",
                "pacote de sessão inválido no socket de vídeo",
            )
        })?;
        tracing::info!(
            %device_name,
            width = session.width,
            height = session.height,
            "pipeline de vídeo conectado"
        );

        let mut ffmpeg = Command::new(&ffmpeg_path)
            .args([
                "-loglevel",
                "error",
                "-f",
                "h264",
                "-i",
                "pipe:0",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                AppError::new(
                    "ffmpeg_spawn_failed",
                    format!("Falha ao iniciar ffmpeg: {e}"),
                )
            })?;
        let mut ffmpeg_stdin = ffmpeg
            .stdin
            .take()
            .ok_or_else(|| AppError::new("ffmpeg_io_error", "stdin do ffmpeg indisponível"))?;
        let mut ffmpeg_stdout = ffmpeg
            .stdout
            .take()
            .ok_or_else(|| AppError::new("ffmpeg_io_error", "stdout do ffmpeg indisponível"))?;

        let frame_bytes = session.width as usize * session.height as usize * 4;
        let reader_task = tokio::spawn(async move {
            let mut buf = vec![0u8; frame_bytes];
            while ffmpeg_stdout.read_exact(&mut buf).await.is_ok() {
                sink(&buf);
            }
        });

        let mut header_buf = [0u8; 12];
        loop {
            if socket.read_exact(&mut header_buf).await.is_err() {
                break;
            }
            let Some(header) = parse_frame_header(&header_buf) else {
                break; // pacote de sessão inesperado no meio do stream (rotação) — encerra este pipeline; o próximo spawn reconecta
            };
            let mut payload = vec![0u8; header.packet_size as usize];
            if header.packet_size > 0 && socket.read_exact(&mut payload).await.is_err() {
                break;
            }
            if ffmpeg_stdin.write_all(&payload).await.is_err() {
                break;
            }
        }

        drop(ffmpeg_stdin);
        let _ = ffmpeg.wait().await;
        reader_task.abort();
        Ok(())
    }
    .await;

    if let Err(err) = result {
        tracing::warn!(code = %err.code, msg = %err.msg, "pipeline de vídeo encerrado");
    }
}

#[cfg(target_os = "windows")]
fn io_err(e: std::io::Error) -> AppError {
    AppError::new(
        "video_socket_read_failed",
        format!("Falha ao ler o socket de vídeo: {e}"),
    )
}

/// Sobe o pipeline de vídeo (Windows) em background quando há porta e sink
/// disponíveis; no-op no Linux e quando `video_sink` é `None` (ex.: testes
/// de lifecycle, que não exercitam o protocolo real — research.md R11).
#[allow(unused_variables)]
fn maybe_spawn_video_pipeline(
    paths: &ExternalPaths,
    forward_port: Option<u16>,
    video_sink: Option<&FrameSink>,
) {
    #[cfg(target_os = "windows")]
    {
        if let (Some(port), Some(sink)) = (forward_port, video_sink) {
            let ffmpeg_path = paths.ffmpeg.clone();
            let sink = Arc::clone(sink);
            tokio::spawn(async move {
                run_video_pipeline(ffmpeg_path, port, sink).await;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

struct RunningSession {
    session: StreamSession,
    child: Option<Child>,
    control: Arc<SessionControl>,
}

/// Orquestra o lifecycle das sessões de stream: start/stop e reconexão
/// automática em caso de crash transitório do backend (FR-005/006).
pub struct StreamManager {
    paths: ExternalPaths,
    sessions: Arc<Mutex<HashMap<Uuid, RunningSession>>>,
}

impl StreamManager {
    pub fn new(paths: ExternalPaths) -> Self {
        Self {
            paths,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Inicia uma sessão.
    ///
    /// - `virtual_camera_target`: Linux — path do device v4l2loopback já
    ///   criado (`/dev/videoN`), alvo do `--v4l2-sink`; Windows — não usado
    ///   por este módulo.
    /// - `video_sink`: Windows — recebe os frames RGBA decodificados (quem
    ///   chama já deve ter criado a câmera virtual e ligado isso a
    ///   `VirtualCameraBackend::feed_frame`); Linux — não usado (o cliente
    ///   scrcpy entrega os frames sozinho).
    pub async fn start(
        &self,
        source: SessionSource,
        config: StreamConfig,
        virtual_camera_target: &str,
        video_sink: Option<FrameSink>,
    ) -> Result<Uuid, AppError> {
        let session_id = Uuid::new_v4();
        let mut session = StreamSession {
            source,
            virtual_camera: Uuid::new_v4(),
            config: config.clone(),
            state: SessionState::Idle,
            stats: SessionStats {
                fps: 0.0,
                uptime_secs: 0,
                reconnects: 0,
            },
        };
        session.apply(SessionEvent::Start)?;

        let (child, forward_port) =
            spawn_backend(&self.paths, &config, virtual_camera_target, session_id).await?;
        maybe_spawn_video_pipeline(&self.paths, forward_port, video_sink.as_ref());
        session.apply(SessionEvent::Started)?;

        let control = Arc::new(SessionControl::new());
        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(
                session_id,
                RunningSession {
                    session,
                    child: Some(child),
                    control: Arc::clone(&control),
                },
            );
        }

        self.spawn_monitor(
            session_id,
            config,
            virtual_camera_target.to_string(),
            video_sink,
            control,
        );
        Ok(session_id)
    }

    /// Para a sessão e garante que o processo do backend morre de verdade,
    /// mesmo que ele esteja "emprestado" para dentro da task de monitor no
    /// momento da chamada (ver doc de `SessionControl`) — sinaliza a task
    /// via `SessionControl` e espera ela confirmar (estado volta a `Idle`),
    /// em vez de tentar matar o processo diretamente aqui.
    pub async fn stop(&self, session_id: Uuid) -> Result<(), AppError> {
        let control = {
            let sessions = self.sessions.lock().await;
            let running = sessions
                .get(&session_id)
                .ok_or_else(|| AppError::new("session_not_found", "Sessão não encontrada"))?;
            if running.session.state == SessionState::Idle {
                return Ok(()); // idempotente
            }
            Arc::clone(&running.control)
        };
        control.request_stop();

        let deadline = tokio::time::Instant::now() + STOP_WAIT_TIMEOUT;
        loop {
            {
                let sessions = self.sessions.lock().await;
                match sessions.get(&session_id) {
                    Some(running) if running.session.state == SessionState::Idle => return Ok(()),
                    None => return Ok(()),
                    _ => {}
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AppError::new(
                    "stop_timeout",
                    "Tempo esgotado ao parar a sessão",
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn state(&self, session_id: Uuid) -> Option<SessionState> {
        let sessions = self.sessions.lock().await;
        sessions.get(&session_id).map(|r| r.session.state.clone())
    }

    /// Snapshot completo da sessão (contracts/tauri-commands.md:
    /// `start_stream` retorna `StreamSession`, que inclui `VirtualCamera`).
    pub async fn session(&self, session_id: Uuid) -> Option<StreamSession> {
        let sessions = self.sessions.lock().await;
        sessions.get(&session_id).map(|r| r.session.clone())
    }

    fn spawn_monitor(
        &self,
        session_id: Uuid,
        config: StreamConfig,
        virtual_camera_target: String,
        video_sink: Option<FrameSink>,
        control: Arc<SessionControl>,
    ) {
        let sessions = Arc::clone(&self.sessions);
        let paths = self.paths.clone();
        tokio::spawn(async move {
            monitor_session(
                sessions,
                paths,
                session_id,
                config,
                virtual_camera_target,
                video_sink,
                control,
            )
            .await;
        });
    }
}

enum MonitorOutcome {
    FatalStderr(AppError),
    Exited,
    StoppedByUser,
}

/// Espera o processo sair, uma linha de stderr fatal aparecer, ou um
/// pedido de parada (`SessionControl`) — o que vier primeiro. Linhas de
/// stderr não-fatais são ignoradas (o backend segue rodando normalmente).
async fn wait_for_exit_or_fatal_stderr(
    mut child: Child,
    control: &SessionControl,
) -> MonitorOutcome {
    if control.is_stop_requested() {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return MonitorOutcome::StoppedByUser;
    }

    let stderr = child.stderr.take();
    let mut lines = stderr.map(|s| BufReader::new(s).lines());

    loop {
        if let Some(l) = lines.as_mut() {
            tokio::select! {
                _ = control.notify.notified() => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return MonitorOutcome::StoppedByUser;
                }
                status = child.wait() => {
                    let _ = status;
                    return MonitorOutcome::Exited;
                }
                line = l.next_line() => {
                    match line {
                        Ok(Some(text)) => {
                            if let Some(err) = classify_stderr(&text) {
                                let _ = child.start_kill();
                                let _ = child.wait().await;
                                return MonitorOutcome::FatalStderr(err);
                            }
                        }
                        _ => {
                            // stderr fechado/erro de leitura: só resta
                            // esperar o exit do processo.
                            lines = None;
                        }
                    }
                }
            }
        } else {
            tokio::select! {
                _ = control.notify.notified() => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return MonitorOutcome::StoppedByUser;
                }
                _ = child.wait() => {
                    return MonitorOutcome::Exited;
                }
            }
        }
    }
}

/// Marca a sessão como `Idle` e limpa o child (usado tanto pela parada
/// pedida pelo usuário quanto por qualquer outro caminho de saída do
/// monitor que não passe pela transição gradual Streaming→Stopping→Idle
/// do diagrama de estados — ver `SessionControl`).
async fn finish_stop(sessions: &Arc<Mutex<HashMap<Uuid, RunningSession>>>, session_id: Uuid) {
    let mut guard = sessions.lock().await;
    if let Some(running) = guard.get_mut(&session_id) {
        running.session.state = SessionState::Idle;
        running.child = None;
    }
}

async fn monitor_session(
    sessions: Arc<Mutex<HashMap<Uuid, RunningSession>>>,
    paths: ExternalPaths,
    session_id: Uuid,
    config: StreamConfig,
    virtual_camera_target: String,
    video_sink: Option<FrameSink>,
    control: Arc<SessionControl>,
) {
    loop {
        let child = {
            let mut guard = sessions.lock().await;
            match guard.get_mut(&session_id).and_then(|r| r.child.take()) {
                Some(c) => c,
                None => return, // sessão parada ou inexistente
            }
        };

        match wait_for_exit_or_fatal_stderr(child, &control).await {
            MonitorOutcome::StoppedByUser => {
                finish_stop(&sessions, session_id).await;
                return;
            }
            MonitorOutcome::FatalStderr(err) => {
                let mut guard = sessions.lock().await;
                if let Some(running) = guard.get_mut(&session_id) {
                    let _ = running.session.apply(SessionEvent::Fail(err.msg));
                }
                return;
            }
            MonitorOutcome::Exited => {
                if control.is_stop_requested() {
                    finish_stop(&sessions, session_id).await;
                    return;
                }

                let should_retry = {
                    let mut guard = sessions.lock().await;
                    match guard.get_mut(&session_id) {
                        Some(running) if running.session.state == SessionState::Streaming => {
                            running.session.apply(SessionEvent::SourceLost).is_ok()
                                && running
                                    .session
                                    .apply(SessionEvent::ReconnectStarted)
                                    .is_ok()
                        }
                        _ => false,
                    }
                };
                if !should_retry {
                    return;
                }

                tokio::time::sleep(RETRY_BACKOFF).await;

                if control.is_stop_requested() {
                    finish_stop(&sessions, session_id).await;
                    return;
                }

                match spawn_backend(&paths, &config, &virtual_camera_target, session_id).await {
                    Ok((mut new_child, forward_port)) => {
                        if control.is_stop_requested() {
                            let _ = new_child.start_kill();
                            finish_stop(&sessions, session_id).await;
                            return;
                        }
                        let mut guard = sessions.lock().await;
                        let recovered = match guard.get_mut(&session_id) {
                            Some(running) => running.session.apply(SessionEvent::Recovered).is_ok(),
                            None => false,
                        };
                        if recovered {
                            maybe_spawn_video_pipeline(&paths, forward_port, video_sink.as_ref());
                            if let Some(running) = guard.get_mut(&session_id) {
                                running.child = Some(new_child);
                            }
                        } else {
                            drop(guard);
                            let _ = new_child.start_kill();
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        }
    }
}
