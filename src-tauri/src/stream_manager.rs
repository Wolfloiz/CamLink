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
    Rotation, SessionEvent, SessionSource, SessionState, SessionStats, StreamConfig, StreamSession,
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
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(3);
/// Se uma tentativa não fica de pé por pelo menos isso, o próximo retry
/// dobra o backoff em vez de repetir o mesmo intervalo curto — visto em
/// hardware real (Samsung SM-G781B): reabrir a câmera rápido demais depois
/// de um crash esbarra em "system-wide limit for number of open cameras"
/// porque o Android ainda não liberou o handle anterior, e retry a 200ms
/// só faz o loop se perpetuar mais rápido.
const HEALTHY_STREAK: Duration = Duration::from_secs(2);
const STOP_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Circuit breaker (achado em bancada 2026-07-30, Samsung SM-G781B): sem
/// isso, uma sessão que nunca fica de pé por `HEALTHY_STREAK` martela
/// reconectando pra sempre (visto em hardware: 50+ segundos, várias dezenas
/// de tentativas, ciclando entre "Address already in use", "Demuxer error",
/// "Server connection failed" e `CAMERA_DISCONNECTED`, sem nunca se
/// recuperar sozinho — só um restart do app inteiro parava). Passado esse
/// número de falhas SEGUIDAS (sem nenhum `HEALTHY_STREAK` no meio), desiste
/// e vira erro acionável em vez de continuar martelando o device.
const MAX_CONSECUTIVE_FAILURES: u32 = 6;

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
    // Cada retentativa spawnava um novo run_video_pipeline sem nunca
    // encerrar o anterior — vários pipelines (e vários processos ffmpeg)
    // podiam ficar rodando em paralelo depois de reconexões, competindo
    // pelo mesmo backend e poluindo o diagnóstico. Guarda o handle da
    // instância atual para cancelar a anterior antes de subir uma nova.
    video_pipeline: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SessionControl {
    fn new() -> Self {
        Self {
            stop_requested: AtomicBool::new(false),
            notify: Notify::new(),
            video_pipeline: Mutex::new(None),
        }
    }

    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    fn is_stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::SeqCst)
    }

    async fn replace_video_pipeline(&self, handle: tokio::task::JoinHandle<()>) {
        let mut slot = self.video_pipeline.lock().await;
        if let Some(previous) = slot.replace(handle) {
            previous.abort();
        }
    }

    /// O monitor encerra por vários caminhos (stop, erro fatal, retry
    /// esgotado) e nenhum deles derrubava o pipeline da sessão — a task (e o
    /// ffmpeg dela, `kill_on_drop`) sobrevivia à sessão.
    async fn abort_video_pipeline(&self) {
        let mut slot = self.video_pipeline.lock().await;
        if let Some(handle) = slot.take() {
            handle.abort();
        }
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

/// Valor de `--capture-orientation` (cliente) / `capture_orientation`
/// (server) do scrcpy para a orientação pedida (FR-016a): `flip` = espelho
/// horizontal aplicado antes da rotação — mesma ordem do
/// `frame_transform::apply`. `None` para a identidade (flag omitida).
pub fn capture_orientation_value(rotation: Rotation, mirror: bool) -> Option<String> {
    let degrees = match rotation {
        Rotation::Deg0 => "0",
        Rotation::Deg90 => "90",
        Rotation::Deg180 => "180",
        Rotation::Deg270 => "270",
    };
    match (rotation, mirror) {
        (Rotation::Deg0, false) => None,
        (_, false) => Some(degrees.to_string()),
        (_, true) => Some(format!("flip{degrees}")),
    }
}

/// Argumentos do cliente `scrcpy` stock (Linux): captura a câmera do
/// Android e escreve direto no device v4l2loopback já criado por
/// `virtualcam::v4l2` (research.md R3).
pub fn build_scrcpy_client_args(
    config: &StreamConfig,
    v4l2_device: &str,
    serial: &str,
) -> Vec<String> {
    build_scrcpy_client_args_oriented(config, v4l2_device, Rotation::Deg0, false, serial)
}

/// Variante com orientação (FR-016a): no Linux girar/espelhar é aplicado
/// pelo próprio scrcpy (`--capture-orientation`, GPU do celular) — os frames
/// não passam pelo CamLink (vão direto ao v4l2loopback), então a mudança
/// exige restart do cliente (device v4l2 persiste; consumidores seguem).
///
/// `serial` é obrigatório mesmo com um único device plugado: com 2+
/// aparelhos Android conectados ao mesmo tempo (US6), o `scrcpy` recusa
/// escolher sozinho ("Multiple ADB devices ... Select a device via -s") e a
/// sessão entra num loop infinito de reconexão — achado em bancada
/// 2026-08-08 testando 3 celulares simultâneos (T065).
pub fn build_scrcpy_client_args_oriented(
    config: &StreamConfig,
    v4l2_device: &str,
    rotation: Rotation,
    mirror: bool,
    serial: &str,
) -> Vec<String> {
    let mut args = vec![
        format!("--serial={serial}"),
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
    ];
    if let Some(orientation) = capture_orientation_value(rotation, mirror) {
        args.push(format!("--capture-orientation={orientation}"));
    }
    args
}

/// Argumentos do ffmpeg que tira UM snapshot RGBA do lado de captura do
/// device v4l2loopback (preview no Linux — o scrcpy entrega os frames
/// direto ao device via `--v4l2-sink`, então a única fonte de verdade do
/// que a câmera virtual está exibindo é o próprio device). `-frames:v 1` é
/// essencial: o v4l2loopback (0.15, exclusive_caps=1) só admite UM leitor
/// streamando por vez — confirmado em hardware, um segundo leitor recebe
/// EBUSY — então o preview NÃO pode segurar o device aberto, senão OBS/Meet
/// veem "câmera indisponível". O `scale` reduz ao tamanho da miniatura
/// (`preview_dimensions` — mesma função usada por quem consome o frame) e
/// absorve o writer negociar outra geometria (ex.: rotação do device).
pub fn build_v4l2_preview_args(device: &str, resolution: (u32, u32)) -> Vec<String> {
    let (pw, ph) = crate::preview::preview_dimensions(resolution);
    vec![
        "-loglevel".to_string(),
        "error".to_string(),
        "-f".to_string(),
        "v4l2".to_string(),
        "-i".to_string(),
        device.to_string(),
        "-vf".to_string(),
        format!("scale={pw}:{ph}"),
        "-frames:v".to_string(),
        "1".to_string(),
        "-f".to_string(),
        "rawvideo".to_string(),
        "-pix_fmt".to_string(),
        "rgba".to_string(),
        "pipe:1".to_string(),
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

/// `<SCID>` é documentado como "a 31-bit random number" (`doc/develop.md`
/// §Connection) — o servidor real faz `Integer.parseInt(scidStr, 16)`
/// (`Options.java`), que é **assinado**: qualquer valor com o bit 31 ligado
/// estoura `NumberFormatException` e mata o processo na hora. Confirmado
/// em hardware real (Samsung SM-G781B): sem a máscara, o scid derivado de
/// bytes de UUID tinha ~50% de chance de ter esse bit ligado, crashando o
/// servidor sempre e disparando um loop de reconexão em ~600ms
/// (research.md R12).
///
/// Só o bootstrap Windows usa (o Linux vai pelo cliente scrcpy), mas os
/// testes de regressão rodam em qualquer plataforma — daí o `allow` em vez
/// de `#[cfg]`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn scid_from_session(id: Uuid) -> u32 {
    let bytes = id.as_bytes();
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) & 0x7FFF_FFFF
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
    orientation: (Rotation, bool),
    serial: &str,
) -> Result<(Child, Option<u16>), AppError> {
    #[cfg(target_os = "linux")]
    {
        let _ = session_id;
        // Mata qualquer scrcpy-server remanescente no device ANTES de subir
        // um novo: o `child.start_kill()` usado em stop()/reconexão é SIGKILL
        // no cliente LOCAL, que nunca chega a avisar o servidor no celular —
        // o `app_process` remoto sobrevive até notar sozinho a conexão
        // quebrada (tempo variável). Se o próximo start/restart for rápido
        // demais, o `CamLinkControlServer` novo colide no bind do socket
        // `localabstract:camlink` com o antigo ainda vivo (`Address already
        // in use` — encontrado em hardware ao trocar de câmera/girar,
        // 2026-07-24; câmera é exclusiva por device mesmo, então matar
        // qualquer scrcpy-server pré-existente aqui é seguro). Best-effort:
        // sem processo pra matar (`pkill` sem match) é o caso comum, não erro.
        let _ = crate::procutil::hide_console(Command::new(&paths.adb))
            .args([
                "-s",
                serial,
                "shell",
                "pkill",
                "-f",
                "com.genymobile.scrcpy.Server",
            ])
            .envs(paths.extra_env.iter().cloned())
            .output()
            .await;
        // `pkill` só entrega o sinal — não espera o processo morrer de fato.
        // Sob troca de câmera/rotação MUITO rápida e repetida (hardware,
        // 2026-07-27), o intervalo entre "sinal enviado" e "processo/socket
        // realmente liberado" ainda bastava pra colidir com o bind do
        // próximo `CamLinkControlServer` ("Address already in use") e até
        // pra dois `app_process` disputarem a câmera na Camera2 HAL
        // (`CameraAccessException ... Function not implemented (-38)`).
        // Poll curto e limitado (pgrep vazio = já morreu) fecha essa janela
        // sem arriscar travar o restart caso o processo já tenha saído
        // sozinho (`pgrep` sem match é o caso comum).
        for _ in 0..10 {
            let still_alive = crate::procutil::hide_console(Command::new(&paths.adb))
                .args([
                    "-s",
                    serial,
                    "shell",
                    "pgrep",
                    "-f",
                    "com.genymobile.scrcpy.Server",
                ])
                .envs(paths.extra_env.iter().cloned())
                .output()
                .await
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(false);
            if !still_alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let args = build_scrcpy_client_args_oriented(
            config,
            virtual_camera_target,
            orientation.0,
            orientation.1,
            serial,
        );
        let mut child = Command::new(&paths.scrcpy)
            .args(&args)
            .envs(paths.extra_env.iter().cloned())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AppError::new("spawn_failed", format!("Falha ao iniciar scrcpy: {e}")))?;
        // O cliente scrcpy manda TUDO para o stdout — INFO, WARN e até os
        // `[server] ERROR` re-ecoados; o stderr dele fica literalmente vazio
        // (0 bytes, medido em hardware 2026-07-15). Descartar o stdout
        // deixava o app cego a qualquer falha do backend (ex.: evicção da
        // câmera com a tela apagada).
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::info!(stdout = %line, "backend stdout");
                }
            });
        }
        Ok((child, None))
    }
    #[cfg(target_os = "windows")]
    {
        // Windows: orientação é aplicada no desktop (frame_transform no
        // sink, ver lib.rs) — o servidor roda sem transform pra permitir
        // mirror/180° ao vivo, sem restart (FR-016a/SC-004).
        let _ = (virtual_camera_target, orientation);
        bootstrap_windows_server(paths, config, session_id, serial).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (
            paths,
            config,
            virtual_camera_target,
            session_id,
            orientation,
            serial,
        );
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
    serial: &str,
) -> Result<(Child, Option<u16>), AppError> {
    let push_status = crate::procutil::hide_console(Command::new(&paths.adb))
        .args(["-s", serial, "push"])
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
    let forward_output = crate::procutil::hide_console(Command::new(&paths.adb))
        .args([
            "-s",
            serial,
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
    let mut child = crate::procutil::hide_console(Command::new(&paths.adb))
        .args(["-s", serial])
        .args(&args)
        .envs(paths.extra_env.iter().cloned())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            AppError::new(
                "spawn_failed",
                format!("Falha ao iniciar scrcpy-server: {e}"),
            )
        })?;
    // scrcpy-server manda mensagens de nível info/debug pro stdout (só
    // WARN/ERROR vão pro stderr, que já era capturado) — descartar o
    // stdout inteiro escondia exatamente o tipo de pista que precisávamos
    // pra entender por que a câmera do device cai sozinha.
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(stdout = %line, "backend stdout");
            }
        });
    }
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

/// Tempo máximo esperando o processo do servidor no device terminar de
/// subir (`app_process` → carregar classes → `DesktopConnection.open()` →
/// `accept()`) antes de considerar o socket de vídeo indisponível. Sem
/// isso, a primeira tentativa de conexão (imediatamente após spawnar o
/// processo) corre contra o boot da JVM no device: `adb forward` aceita a
/// conexão TCP do lado do host mesmo sem nada ouvindo ainda do lado do
/// device, e a leitura subsequente recebe EOF assim que o adb desfaz o
/// relay — o retry cobre conectar de novo, não só reler.
#[cfg(target_os = "windows")]
const VIDEO_SOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(target_os = "windows")]
const VIDEO_SOCKET_RETRY_INTERVAL: Duration = Duration::from_millis(150);

#[cfg(target_os = "windows")]
async fn connect_video_socket_with_retry(
    forward_port: u16,
) -> Result<tokio::net::TcpStream, AppError> {
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpStream;

    let deadline = tokio::time::Instant::now() + VIDEO_SOCKET_CONNECT_TIMEOUT;
    loop {
        let attempt: Result<TcpStream, AppError> = async {
            let mut socket = TcpStream::connect(("127.0.0.1", forward_port))
                .await
                .map_err(|e| {
                    AppError::new(
                        "video_socket_connect_failed",
                        format!("Falha ao conectar no socket de vídeo: {e}"),
                    )
                })?;
            // O dummy byte só é enviado depois que o device chama
            // `accept()` — lê-lo aqui (em vez de só conectar) é o que de
            // fato confirma que o servidor já está pronto do outro lado.
            let mut dummy = [0u8; 1];
            socket.read_exact(&mut dummy).await.map_err(io_err)?;
            Ok(socket)
        }
        .await;

        match attempt {
            Ok(socket) => return Ok(socket),
            Err(err) if tokio::time::Instant::now() >= deadline => return Err(err),
            Err(_) => tokio::time::sleep(VIDEO_SOCKET_RETRY_INTERVAL).await,
        }
    }
}

#[cfg(target_os = "windows")]
async fn run_video_pipeline(ffmpeg_path: PathBuf, forward_port: u16, fps: u32, sink: FrameSink) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let result: Result<(), AppError> = async {
        // H264 elementar puro não carrega PTS/DTS nenhum (só NAL units, sem
        // container) — sem dizer o fps, avformat_find_stream_info() tenta
        // *inferir* o frame rate observando o timing real de chegada dos
        // bytes. `-r` elimina a necessidade de inferir (já temos o fps
        // pedido em `config.fps`, não depende do handshake do socket).
        //
        // `-analyzeduration` é em MICROSSEGUNDOS: o valor antigo (1_000_000
        // = 1s inteiro) fazia o ffmpeg gastar até 1s analisando o stream
        // antes de soltar o primeiro frame decodificado — medido em
        // hardware real, `read_ms` do primeiro frame batia ~1.2-1.3s. Como
        // isso só acontece uma vez (não por frame), virava um atraso FIXO
        // pro resto da sessão inteira (não cresce, mas nunca recupera).
        // Reduzir probesize/analyzeduration cortou pra ~400ms; abaixo disso
        // o gargalo passa a ser o overhead do PROCESSO em si (carregar
        // DLLs, inicializar o decoder) — não dá pra cortar via flag do
        // ffmpeg, mas dá pra ESCONDER: spawnamos o ffmpeg AGORA, antes de
        // esperar o socket de vídeo conectar (`app_process` no device pode
        // levar até `VIDEO_SOCKET_CONNECT_TIMEOUT` = 5s pra terminar de
        // subir) — o processo já está de pé e o decoder inicializado
        // quando o primeiro pacote H264 de verdade chegar, em vez de somar
        // esse overhead DEPOIS que o handshake já terminou.
        //
        // `-fflags nobuffer` NÃO entra: isolado em hardware real (dump do
        // H264 bruto pra disco + replay offline com `ffprobe`/`ffmpeg`, ver
        // git log) — com esse flag o filtergraph auto-inserido
        // (yuv420p→rgba) nunca recebe frame nenhum do decoder
        // ("No filtered frames for output stream" / "Output file is empty"
        // mesmo lendo um arquivo local inteiro, sem nenhuma pressão de
        // tempo). Sem ele, o mesmíssimo dado decodifica normalmente.
        let fps_arg = fps.max(1).to_string();
        let mut ffmpeg = crate::procutil::hide_console(Command::new(&ffmpeg_path))
            .args([
                "-loglevel",
                "warning",
                "-flags",
                "low_delay",
                "-threads",
                "1",
                "-r",
                &fps_arg,
                "-probesize",
                "65536",
                "-analyzeduration",
                "50000",
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
            .stderr(Stdio::piped())
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
        // Sem isso não há como saber por que o ffmpeg parou de produzir
        // frames — antes ia pro void (Stdio::null()), igual ao gap que já
        // tinha existido com o stderr do próprio backend scrcpy.
        if let Some(stderr) = ffmpeg.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(stderr = %line, "ffmpeg stderr");
                }
            });
        }

        let mut socket = connect_video_socket_with_retry(forward_port).await?;

        // Handshake (research.md R12 / doc/develop.md §Connection): o dummy
        // byte já foi consumido por `connect_video_socket_with_retry`
        // (é o sinal de que o servidor está pronto); falta só o nome do
        // device (64 bytes).
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

        let frame_bytes = session.width as usize * session.height as usize * 4;
        // Read (decode) e sink (cópia para a memória compartilhada do
        // filtro) desacoplados por uma célula latest-frame: o loop de
        // leitura publica com `mem::swap` O(1) e volta imediatamente ao
        // stdout do ffmpeg; a task de entrega consome sempre o frame mais
        // recente. Se a entrega ficar para trás, frames intermediários são
        // descartados (drop-to-latest, contado em `dropped_count`) — nunca
        // enfileirados, que é o que virava latência crescente. Um
        // `watch<Vec<u8>>` faria o mesmo papel clonando ~8 MB por frame; o
        // swap devolve o buffer anterior para reuso. Consequência para
        // `SessionStats.fps`: o sink conta frames ENTREGUES à câmera
        // virtual (não decodificados) — é a taxa que o consumidor vê.
        let latest_cell: Arc<std::sync::Mutex<(Vec<u8>, bool)>> =
            Arc::new(std::sync::Mutex::new((Vec::new(), false)));
        let frame_ready = Arc::new(Notify::new());

        let delivery_cell = Arc::clone(&latest_cell);
        let delivery_ready = Arc::clone(&frame_ready);
        let delivery_task = tokio::spawn(async move {
            let mut out: Vec<u8> = Vec::new();
            let mut delivered_count: u64 = 0;
            loop {
                delivery_ready.notified().await;
                let fresh = {
                    let mut cell = delivery_cell.lock().unwrap_or_else(|p| p.into_inner());
                    let fresh = cell.1;
                    if fresh {
                        std::mem::swap(&mut out, &mut cell.0);
                        cell.1 = false;
                    }
                    fresh
                };
                if !fresh {
                    continue;
                }
                let sink_start = tokio::time::Instant::now();
                sink(&out);
                let sink_elapsed = sink_start.elapsed();
                delivered_count += 1;
                if delivered_count <= 5 || delivered_count.is_multiple_of(30) {
                    tracing::info!(
                        delivered_count,
                        sink_ms = sink_elapsed.as_millis(),
                        "frame entregue à câmera virtual"
                    );
                }
            }
        });

        let reader_cell = Arc::clone(&latest_cell);
        let reader_ready = Arc::clone(&frame_ready);
        let reader_task = tokio::spawn(async move {
            let mut buf = vec![0u8; frame_bytes];
            let mut decoded_count: u64 = 0;
            let mut dropped_count: u64 = 0;
            loop {
                let read_start = tokio::time::Instant::now();
                match ffmpeg_stdout.read_exact(&mut buf).await {
                    Ok(_) => {
                        let read_elapsed = read_start.elapsed();
                        decoded_count += 1;
                        {
                            let mut cell = reader_cell.lock().unwrap_or_else(|p| p.into_inner());
                            std::mem::swap(&mut buf, &mut cell.0);
                            if cell.1 {
                                // O frame anterior nunca foi entregue —
                                // descartado em favor deste.
                                dropped_count += 1;
                            }
                            cell.1 = true;
                        }
                        reader_ready.notify_one();
                        // O swap devolve o buffer já consumido (ou vazio,
                        // nas primeiras voltas) para o próximo read_exact.
                        if buf.len() != frame_bytes {
                            buf.resize(frame_bytes, 0);
                        }
                        if decoded_count <= 5 || decoded_count.is_multiple_of(30) {
                            tracing::info!(
                                decoded_count,
                                dropped_count,
                                read_ms = read_elapsed.as_millis(),
                                "frame decodificado pelo ffmpeg"
                            );
                        }
                    }
                    Err(e) => {
                        // Antes esse `while ... .is_ok()` saía em silêncio;
                        // não dava pra saber se o ffmpeg nunca chegou a
                        // produzir nada (0 frames) ou parou no meio.
                        tracing::warn!(decoded_count, dropped_count, error = %e, "leitura do stdout do ffmpeg encerrada");
                        break;
                    }
                }
            }
        });

        let mut header_buf = [0u8; 12];
        let mut packet_count: u64 = 0;
        let mut bytes_forwarded: u64 = 0;
        loop {
            if let Err(e) = socket.read_exact(&mut header_buf).await {
                tracing::warn!(packet_count, bytes_forwarded, error = %e, "socket de vídeo fechou lendo o cabeçalho de frame");
                break;
            }
            let Some(header) = parse_frame_header(&header_buf) else {
                // `parse_frame_header` só retorna `None` quando o bit 0x80
                // tá setado, ou seja: só pode ser pacote de sessão. O
                // servidor manda um novo sem fechar o socket sempre que
                // `SurfaceEncoder.prepareRetry()` recupera de um erro de
                // captura internamente (visto em hardware real: "Capture/
                // encoding error" + "Camera capture failed" no stderr do
                // backend, seguido de um novo pacote de sessão com a MESMA
                // resolução). Encerrar o pipeline aqui fechava o socket na
                // cara do servidor no meio desse retry — o
                // `IO.writeFully` seguinte vira broken pipe, que
                // `SurfaceEncoder` trata como fatal (não tenta de novo),
                // derrubando o processo inteiro e forçando nosso reconnect
                // completo (mata processo, adb push, novo spawn, ~5-7s)
                // por algo que o próprio servidor já resolvia sozinho em
                // ~50ms. Resolução igual → segue lendo pela mesma conexão;
                // resolução diferente (rotação de verdade) → aí sim precisa
                // reconfigurar o decoder, então encerra pro reconnect.
                match parse_session_packet(&header_buf) {
                    Some(new_session)
                        if new_session.width == session.width
                            && new_session.height == session.height =>
                    {
                        tracing::info!(
                            packet_count,
                            bytes_forwarded,
                            width = new_session.width,
                            height = new_session.height,
                            "nova sessão de captura no device (mesma resolução) — mantendo a conexão"
                        );
                        continue;
                    }
                    Some(new_session) => {
                        tracing::warn!(
                            packet_count,
                            bytes_forwarded,
                            old_width = session.width,
                            old_height = session.height,
                            new_width = new_session.width,
                            new_height = new_session.height,
                            "nova sessão de captura com resolução diferente — encerrando pipeline para reconectar"
                        );
                        break;
                    }
                    None => {
                        tracing::warn!(packet_count, bytes_forwarded, "pacote inesperado no meio do stream (nem frame nem sessão válida)");
                        break;
                    }
                }
            };
            let mut payload = vec![0u8; header.packet_size as usize];
            if header.packet_size > 0 {
                if let Err(e) = socket.read_exact(&mut payload).await {
                    tracing::warn!(packet_count, bytes_forwarded, packet_size = header.packet_size, error = %e, "socket de vídeo fechou lendo o payload");
                    break;
                }
            }
            if let Err(e) = ffmpeg_stdin.write_all(&payload).await {
                tracing::warn!(packet_count, bytes_forwarded, error = %e, "escrita no stdin do ffmpeg falhou");
                break;
            }
            packet_count += 1;
            bytes_forwarded += payload.len() as u64;
            if packet_count <= 5 || packet_count.is_multiple_of(60) {
                tracing::info!(
                    packet_count,
                    bytes_forwarded,
                    packet_size = header.packet_size,
                    config_packet = header.config_packet,
                    key_frame = header.key_frame,
                    pts = header.pts,
                    "pacote de vídeo encaminhado ao ffmpeg"
                );
            }
        }
        tracing::info!(packet_count, bytes_forwarded, "loop de leitura do socket de vídeo encerrado");

        drop(ffmpeg_stdin);
        let _ = ffmpeg.wait().await;
        reader_task.abort();
        delivery_task.abort();
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

/// Sobe o pipeline de vídeo em background; no-op quando `video_sink` é
/// `None` (ex.: testes de lifecycle, que não exercitam o protocolo real —
/// research.md R11). Windows: conecta no socket encaminhado e decodifica
/// (precisa da porta). Linux: o scrcpy já entrega os frames ao device via
/// `--v4l2-sink`, então o pipeline aqui é um *leitor* do lado de captura do
/// device — é o que alimenta preview e fps (que nunca funcionavam no Linux:
/// o sink jamais era chamado). Cancela qualquer pipeline anterior da mesma
/// sessão antes de subir um novo (ver doc de `SessionControl::video_pipeline`).
///
/// `orientation` importa só no Linux: com rotação 90°/270°, o celular já
/// entrega o vídeo girado no device via `--capture-orientation`
/// (`spawn_backend`/`capture_orientation_value`) — as dimensões REAIS do
/// device são as trocadas, não `config.resolution` cru. Usar o valor cru
/// aqui (como antes) desalinha o `(pw,ph)` marcado no frame de preview
/// (calculado com as dimensões trocadas em `wire_android_session`, `lib.rs`)
/// do tamanho real do buffer lido pelo ffmpeg (calculado sem a troca aqui),
/// e `encode_preview_jpeg` rejeita todo frame com "tamanho do frame não
/// bate" (encontrado em hardware ao trocar de câmera/girar, 2026-07-24).
#[allow(unused_variables)]
async fn maybe_spawn_video_pipeline(
    paths: &ExternalPaths,
    forward_port: Option<u16>,
    config: &StreamConfig,
    orientation: (Rotation, bool),
    virtual_camera_target: &str,
    video_sink: Option<&FrameSink>,
    control: &SessionControl,
) {
    #[cfg(target_os = "windows")]
    {
        let _ = orientation;
        if let (Some(port), Some(sink)) = (forward_port, video_sink) {
            let ffmpeg_path = paths.ffmpeg.clone();
            let fps = config.fps;
            let sink = Arc::clone(sink);
            let handle = tokio::spawn(async move {
                run_video_pipeline(ffmpeg_path, port, fps, sink).await;
            });
            control.replace_video_pipeline(handle).await;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(sink) = video_sink {
            let ffmpeg_path = paths.ffmpeg.clone();
            let device = virtual_camera_target.to_string();
            let resolution = if orientation.0.swaps_dimensions() {
                (config.resolution.1, config.resolution.0)
            } else {
                config.resolution
            };
            let sink = Arc::clone(sink);
            let handle = tokio::spawn(async move {
                run_preview_pipeline(ffmpeg_path, device, resolution, sink).await;
            });
            control.replace_video_pipeline(handle).await;
        }
    }
}

/// Intervalo entre snapshots de preview no Linux. Entre um snapshot e outro
/// o device fica LIVRE — o slot único de leitura do v4l2loopback pertence
/// ao consumidor real (OBS/Meet); o preview é descartável (FR-023).
#[cfg(target_os = "linux")]
const PREVIEW_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(200);

/// Tira snapshots RGBA do lado de captura do device v4l2loopback e entrega
/// ao `FrameSink` (Linux). Cada snapshot abre o device, lê UM frame e fecha
/// (`build_v4l2_preview_args`): o v4l2loopback só admite um leitor
/// streamando por vez, então segurar o device aberto bloqueava OBS/Meet com
/// "câmera indisponível". Quando um consumidor está com o slot (EBUSY) ou o
/// writer ainda não conectou, o snapshot falha e é simplesmente pulado —
/// logado uma vez por transição, não por tentativa. A task só morre por
/// `abort()` (stop da sessão ou substituição por um pipeline novo).
#[cfg(target_os = "linux")]
pub(crate) async fn run_preview_pipeline(
    ffmpeg_path: PathBuf,
    device: String,
    resolution: (u32, u32),
    sink: FrameSink,
) {
    use tokio::io::AsyncReadExt;

    // Mesma fonte de verdade do `scale=` nos args do ffmpeg — se divergisse,
    // o `read_exact` abaixo desalinharia em silêncio.
    let (w, h) = crate::preview::preview_dimensions(resolution);
    let frame_bytes = w as usize * h as usize * 4;
    let mut buf = vec![0u8; frame_bytes];
    let mut had_frame = false;
    loop {
        let snapshot: Result<(), String> = async {
            let mut ffmpeg = Command::new(&ffmpeg_path)
                .args(build_v4l2_preview_args(&device, resolution))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("falha ao iniciar ffmpeg do preview: {e}"))?;
            let mut stdout = ffmpeg
                .stdout
                .take()
                .ok_or_else(|| "stdout do ffmpeg de preview indisponível".to_string())?;
            let read = stdout.read_exact(&mut buf).await;
            let _ = ffmpeg.wait().await;
            read.map(|_| ()).map_err(|e| format!("sem frame: {e}"))
        }
        .await;

        match snapshot {
            Ok(()) => {
                if !had_frame {
                    tracing::info!(%device, "preview capturando snapshots");
                    had_frame = true;
                }
                sink(&buf);
            }
            Err(reason) => {
                if had_frame {
                    tracing::info!(%device, %reason, "preview sem acesso ao device (consumidor ativo ou writer fora) — cedendo o slot");
                    had_frame = false;
                }
            }
        }
        tokio::time::sleep(PREVIEW_SNAPSHOT_INTERVAL).await;
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
///
/// `Clone` é barato (`Arc` + `ExternalPaths` que já é `Clone`) — usado pra
/// dar ao watcher de shutdown (`run()`, lib.rs) um handle independente do
/// que fica dentro do `AppState` gerenciado pelo Tauri.
#[derive(Clone)]
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
        self.start_with_orientation(
            source,
            config,
            virtual_camera_target,
            video_sink,
            (Rotation::Deg0, false),
        )
        .await
    }

    /// `start` com orientação (FR-016a). No Linux ela vira
    /// `--capture-orientation` do cliente scrcpy (transform na GPU do
    /// celular); no Windows é ignorada aqui — o sink em `lib.rs` aplica
    /// `frame_transform` no desktop.
    pub async fn start_with_orientation(
        &self,
        source: SessionSource,
        config: StreamConfig,
        virtual_camera_target: &str,
        video_sink: Option<FrameSink>,
        orientation: (Rotation, bool),
    ) -> Result<Uuid, AppError> {
        let session_id = Uuid::new_v4();
        // `StreamManager` só orquestra fontes Android (RTSP passa por
        // `rtsp_manager::start_session`, sem depender de `adb`/`scrcpy`) —
        // o serial identifica o device pro `adb -s`/`--serial` do scrcpy
        // quando 2+ aparelhos estão plugados ao mesmo tempo (US6/T065).
        let serial = match &source {
            SessionSource::Android(serial) => serial.clone(),
            SessionSource::Rtsp(id) => id.to_string(),
        };
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

        let control = Arc::new(SessionControl::new());

        let (child, forward_port) = spawn_backend(
            &self.paths,
            &config,
            virtual_camera_target,
            session_id,
            orientation,
            &serial,
        )
        .await?;
        maybe_spawn_video_pipeline(
            &self.paths,
            forward_port,
            &config,
            orientation,
            virtual_camera_target,
            video_sink.as_ref(),
            &control,
        )
        .await;
        session.apply(SessionEvent::Started)?;
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
            orientation,
            serial,
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

    /// Mata à força todos os processos-filho de sessões ativas, sem passar
    /// pelo fluxo graceful de `stop()` — usado só no shutdown do processo
    /// (SIGINT/SIGTERM/Ctrl+C chegando direto no `camlink`; ver
    /// `watch_for_shutdown_signal`, lib.rs). `kill_on_drop(true)` no spawn
    /// só mata o filho quando o `Child` é dropado durante um shutdown
    /// GRACIOSO do processo Rust — quando o processo inteiro morre por
    /// sinal (Ctrl+C no terminal, `pnpm`/`cargo run` não repassando o
    /// sinal adiante — comum nesse wrapper) esse `Drop` nunca roda, e o
    /// `scrcpy` local (e o `app_process` remoto atrás dele) ficava vivo
    /// escrevendo no device mesmo com o app "fechado" (achado em bancada
    /// 2026-07-28). Best-effort: o processo está saindo de qualquer forma.
    pub async fn kill_all_backends(&self) {
        let mut sessions = self.sessions.lock().await;
        for running in sessions.values_mut() {
            if let Some(child) = running.child.as_mut() {
                let _ = child.start_kill();
            }
        }
        drop(sessions);
        // Mesmo raciocínio do cleanup pré-emptivo em `spawn_backend`: o
        // kill local não garante que o `app_process` remoto já notou a
        // queda a tempo, então também derruba ele direto.
        let _ = crate::procutil::hide_console(Command::new(&self.paths.adb))
            .args(["shell", "pkill", "-f", "com.genymobile.scrcpy.Server"])
            .envs(self.paths.extra_env.iter().cloned())
            .output()
            .await;
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

    #[allow(clippy::too_many_arguments)]
    fn spawn_monitor(
        &self,
        session_id: Uuid,
        config: StreamConfig,
        virtual_camera_target: String,
        video_sink: Option<FrameSink>,
        control: Arc<SessionControl>,
        orientation: (Rotation, bool),
        serial: String,
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
                orientation,
                serial,
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
                            // Linhas não-fatais eram descartadas em nível
                            // debug (invisível no log padrão) — não dava pra
                            // ver por que o scrcpy-server real morria rápido
                            // no device (só "early eof" do lado do socket de
                            // vídeo, sem nenhum stderr visível do processo
                            // em si). warn! temporário até achar a causa raiz
                            // da conexão caindo a cada poucos segundos.
                            tracing::warn!(stderr = %text, "backend stderr");
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

// Plumbing interno com destino único (spawn_monitor): agrupar em struct só
// adicionaria indireção sem segundo call site.
#[allow(clippy::too_many_arguments)]
async fn monitor_session(
    sessions: Arc<Mutex<HashMap<Uuid, RunningSession>>>,
    paths: ExternalPaths,
    session_id: Uuid,
    config: StreamConfig,
    virtual_camera_target: String,
    video_sink: Option<FrameSink>,
    control: Arc<SessionControl>,
    orientation: (Rotation, bool),
    serial: String,
) {
    // Corpo num bloco async para que todos os `return` (stop, erro fatal,
    // retry esgotado) desemboquem num único ponto de limpeza do pipeline.
    let control_cleanup = Arc::clone(&control);
    async move {
        let mut backoff = RETRY_BACKOFF;
        let mut last_recovery = tokio::time::Instant::now();
        let mut consecutive_failures: u32 = 0;

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

                    let healthy = last_recovery.elapsed() >= HEALTHY_STREAK;
                    backoff = if healthy {
                        RETRY_BACKOFF
                    } else {
                        (backoff * 2).min(MAX_RETRY_BACKOFF)
                    };
                    consecutive_failures = if healthy { 0 } else { consecutive_failures + 1 };
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        let mut guard = sessions.lock().await;
                        if let Some(running) = guard.get_mut(&session_id) {
                            let _ = running.session.apply(SessionEvent::Fail(format!(
                                "Câmera não estabilizou após {MAX_CONSECUTIVE_FAILURES} tentativas de reconexão seguidas — desconecte e reconecte o cabo USB, ou reinicie o app."
                            )));
                        }
                        return;
                    }
                    // backoff pode chegar a MAX_RETRY_BACKOFF (3s); um
                    // sleep() simples aqui deixava stop() esperando o sleep
                    // inteiro terminar antes de sequer checar o pedido de
                    // parada de novo — corrida real contra o STOP_WAIT_TIMEOUT
                    // de stop() (5s), causada pelo próprio backoff exponencial
                    // adicionado depois de ver o crash-loop em hardware real.
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = control.notify.notified() => {
                            finish_stop(&sessions, session_id).await;
                            return;
                        }
                    }

                    if control.is_stop_requested() {
                        finish_stop(&sessions, session_id).await;
                        return;
                    }

                    match spawn_backend(
                        &paths,
                        &config,
                        &virtual_camera_target,
                        session_id,
                        orientation,
                        &serial,
                    )
                    .await
                    {
                        Ok((mut new_child, forward_port)) => {
                            if control.is_stop_requested() {
                                let _ = new_child.start_kill();
                                finish_stop(&sessions, session_id).await;
                                return;
                            }
                            let mut guard = sessions.lock().await;
                            let recovered = match guard.get_mut(&session_id) {
                                Some(running) => {
                                    running.session.apply(SessionEvent::Recovered).is_ok()
                                }
                                None => false,
                            };
                            if recovered {
                                last_recovery = tokio::time::Instant::now();
                                maybe_spawn_video_pipeline(
                                    &paths,
                                    forward_port,
                                    &config,
                                    orientation,
                                    &virtual_camera_target,
                                    video_sink.as_ref(),
                                    &control,
                                )
                                .await;
                                if let Some(running) = guard.get_mut(&session_id) {
                                    running.child = Some(new_child);
                                    // Nunca era incrementado antes — stats.reconnects
                                    // ficava travado em 0 pra sempre, mesmo com o
                                    // reconector funcionando de verdade.
                                    running.session.stats.reconnects += 1;
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
    .await;
    control_cleanup.abort_video_pipeline().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regressão: encontrada em hardware real (Samsung SM-G781B) —
    /// `scid_from_session` sem a máscara de 31 bits gerava valores como
    /// `0xce9cd994` que o servidor real rejeita com
    /// `NumberFormatException: For input string: "ce9cd994" under radix 16`
    /// (via `Integer.parseInt`, assinado), crashando o processo a cada
    /// tentativa.
    #[test]
    fn scid_never_sets_bit_31() {
        // UUID cujos 4 primeiros bytes têm o bit mais significativo ligado
        // (0xce...) — exatamente o padrão que crashava o servidor real.
        let id = Uuid::from_bytes([0xce, 0x9c, 0xd9, 0x94, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let scid = scid_from_session(id);
        assert!(
            scid <= 0x7FFF_FFFF,
            "scid {scid:#010x} tem o bit 31 ligado — Integer.parseInt (assinado) no servidor real rejeita"
        );
    }

    #[test]
    fn scid_stays_within_31_bits_for_many_uuids() {
        for _ in 0..1000 {
            let scid = scid_from_session(Uuid::new_v4());
            assert!(scid <= 0x7FFF_FFFF, "scid {scid:#010x} excede 31 bits");
        }
    }

    /// O leitor de preview devolve frames RGBA no tamanho da MINIATURA
    /// (`preview_dimensions`, não a resolução da config — banda ~10x menor),
    /// lendo do device v4l2 e escalando com `scale=PW:PH` explícito: quem
    /// define o formato do device é o writer (scrcpy), e o consumidor faz
    /// `read_exact(pw*ph*4)` — qualquer divergência de arredondamento
    /// desalinharia a leitura em silêncio.
    #[test]
    fn preview_args_read_device_and_emit_rgba_at_preview_size() {
        let args = build_v4l2_preview_args("/dev/video9", (1280, 720));
        let joined = args.join(" ");
        assert!(joined.contains("-f v4l2 -i /dev/video9"), "{joined}");
        assert!(joined.contains("-vf scale=640:360"), "{joined}");
        assert!(joined.contains("-pix_fmt rgba"), "{joined}");
        // Um frame por invocação: o preview não pode segurar o único slot
        // de leitura do v4l2loopback (EBUSY para OBS/Meet).
        assert!(joined.contains("-frames:v 1"), "{joined}");
        assert!(
            joined.ends_with("pipe:1"),
            "saída deve ser o stdout: {joined}"
        );
        // Entrada (device) antes da saída (pipe) — ffmpeg é posicional.
        let dev = args.iter().position(|a| a == "/dev/video9");
        let pipe = args.iter().position(|a| a == "pipe:1");
        assert!(dev < pipe, "{joined}");
    }

    /// `SessionStats.fps` é computado contando os frames que o sink recebe —
    /// um filtro `fps=` no leitor mascararia a taxa real do device.
    #[test]
    fn preview_args_never_resample_frame_rate() {
        let args = build_v4l2_preview_args("/dev/video9", (1920, 1080));
        assert!(
            !args.iter().any(|a| a.contains("fps=")),
            "leitor de preview não pode reamostrar a taxa: {args:?}"
        );
    }
}
