//! T051 — Fontes IP/RTSP (US4): pipeline ffmpeg low-delay até a webcam
//! virtual, ≤ 300 ms (SC-002), com credenciais injetadas SOMENTE em runtime
//! (FR-018a) e erros acionáveis distinguindo auth de host inacessível.
//!
//! Linux: o ffmpeg escreve direto no device v4l2loopback (`-f v4l2`).
//! Windows: rawvideo RGBA no stdout → `FrameSink` (mesmo contrato do
//! pipeline Android em `stream_manager`). Reconexão: o supervisor coloca a
//! câmera em standby e re-tenta com backoff até `stop`.

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::AppError;
use crate::stream_manager::FrameSink;

/// Timeout da validação de URL (contrato do T049/quickstart Cenário 4).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Backoff entre tentativas de reconexão do supervisor.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Montagem de argumentos e classificação (puro, testado em rtsp_test.rs)
// ---------------------------------------------------------------------------

/// Flags de input low-delay — exatamente as de research.md R5.
pub fn ffmpeg_input_args(url: &str) -> Vec<String> {
    vec![
        "-fflags".into(),
        "nobuffer".into(),
        "-flags".into(),
        "low_delay".into(),
        "-analyzeduration".into(),
        "0".into(),
        "-probesize".into(),
        "32".into(),
        "-rtsp_transport".into(),
        "tcp".into(),
        "-i".into(),
        url.into(),
    ]
}

/// Saída Linux: direto no device v4l2loopback (YUYV — mesmo formato que o
/// backend `virtualcam::v4l2` negocia para consumidores reais).
pub fn output_args_v4l2(device: &str) -> Vec<String> {
    vec![
        "-pix_fmt".into(),
        "yuyv422".into(),
        "-f".into(),
        "v4l2".into(),
        device.into(),
    ]
}

/// Saída Windows: rawvideo RGBA no stdout, consumido frame a frame pelo
/// supervisor e entregue ao `FrameSink`.
pub fn output_args_rawvideo() -> Vec<String> {
    vec![
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "pipe:1".into(),
    ]
}

/// Percent-encode dos caracteres que quebrariam o parsing de URL do ffmpeg
/// dentro de credenciais (`%` primeiro para não re-escapar).
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '@' => out.push_str("%40"),
            ':' => out.push_str("%3A"),
            '/' => out.push_str("%2F"),
            '?' => out.push_str("%3F"),
            '#' => out.push_str("%23"),
            '[' => out.push_str("%5B"),
            ']' => out.push_str("%5D"),
            other => out.push(other),
        }
    }
    out
}

/// Injeta a credencial na URL SOMENTE em runtime (a URL persistida nunca a
/// contém — FR-018a). Duas formas:
/// - URL já tem usuário (`rtsp://user@host/...`): o segredo é a senha.
/// - URL sem usuário: o segredo pode ser o par `user:senha` completo.
pub fn inject_credentials(url: &str, secret: Option<&str>) -> String {
    let Some(secret) = secret else {
        return url.to_string();
    };
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    match rest.split_once('@') {
        Some((user, host)) => {
            format!("{scheme}://{user}:{}@{host}", percent_encode(secret))
        }
        None => {
            let encoded = match secret.split_once(':') {
                Some((user, pass)) => {
                    format!("{}:{}", percent_encode(user), percent_encode(pass))
                }
                None => percent_encode(secret),
            };
            format!("{scheme}://{encoded}@{rest}")
        }
    }
}

/// Valida o formato da URL antes de qualquer subprocess (barato e imediato).
pub fn validate_rtsp_url(url: &str) -> Result<(), AppError> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(AppError::new("rtsp_url_invalida", "URL RTSP inválida")
            .with_hint("Use o formato rtsp://endereço-da-câmera/caminho."));
    };
    if scheme != "rtsp" && scheme != "rtsps" {
        return Err(AppError::new(
            "rtsp_url_invalida",
            format!("Esquema '{scheme}' não é RTSP"),
        )
        .with_hint("A URL precisa começar com rtsp:// (ou rtsps://)."));
    }
    let host = rest.split(['/', '?']).next().unwrap_or("");
    if host.is_empty() {
        return Err(AppError::new("rtsp_url_invalida", "URL RTSP sem host")
            .with_hint("Inclua o endereço da câmera, ex.: rtsp://192.168.0.42/stream."));
    }
    Ok(())
}

/// Falha do ffmpeg classificada a partir do stderr — auth ≠ rede (FR-010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtspFailure {
    AuthFailed,
    Unreachable,
    Other(String),
}

impl RtspFailure {
    /// Dica acionável exibida na UI junto do erro.
    pub fn action_hint(&self) -> String {
        match self {
            RtspFailure::AuthFailed => {
                "Verifique o usuário e a senha da câmera (credenciais recusadas).".to_string()
            }
            RtspFailure::Unreachable => {
                "Câmera inacessível: confira o endereço, a porta e se ela está na mesma rede."
                    .to_string()
            }
            RtspFailure::Other(msg) => format!("Falha na fonte RTSP: {msg}"),
        }
    }
}

/// Classifica o stderr acumulado de um ffmpeg que morreu.
pub fn classify_ffmpeg_failure(stderr: &str) -> RtspFailure {
    let lower = stderr.to_lowercase();
    if lower.contains("401") || lower.contains("unauthorized") || lower.contains("403") {
        return RtspFailure::AuthFailed;
    }
    if lower.contains("connection refused")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("no route to host")
        || lower.contains("network is unreachable")
        || lower.contains("name or service not known")
        || lower.contains("failed to resolve")
    {
        return RtspFailure::Unreachable;
    }
    RtspFailure::Other(
        stderr
            .lines()
            .last()
            .unwrap_or("erro desconhecido")
            .to_string(),
    )
}

// ---------------------------------------------------------------------------
// Probe de URL (validação com timeout de 3 s)
// ---------------------------------------------------------------------------

/// Valida que a URL responde, com timeout de 3 s (quickstart Cenário 4):
/// decodifica 1 frame e sai. `url` já deve vir com credenciais injetadas.
pub async fn probe_url(
    ffmpeg: &std::path::Path,
    url: &str,
    extra_env: &[(String, String)],
) -> Result<(), AppError> {
    validate_rtsp_url(url.split_once('@').map_or(url, |_| url))?;

    let mut args = ffmpeg_input_args(url);
    args.extend([
        "-frames:v".into(),
        "1".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);

    let mut cmd = Command::new(ffmpeg);
    cmd.args(&args)
        .envs(extra_env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = crate::procutil::hide_console(cmd).spawn().map_err(|e| {
        AppError::new(
            "ffmpeg_ausente",
            format!("Não consegui executar o ffmpeg: {e}"),
        )
        .with_hint("Instale o ffmpeg e garanta que está no PATH.")
    })?;

    let mut stderr = child.stderr.take();
    let wait = async {
        let mut buf = String::new();
        if let Some(err) = stderr.as_mut() {
            let _ = err.read_to_string(&mut buf).await;
        }
        let status = child.wait().await;
        (status, buf)
    };

    match tokio::time::timeout(PROBE_TIMEOUT, wait).await {
        Err(_) => Err(
            AppError::new("rtsp_timeout", "Fonte RTSP não respondeu em 3 s")
                .with_hint(RtspFailure::Unreachable.action_hint()),
        ),
        Ok((Ok(status), _)) if status.success() => Ok(()),
        Ok((_, stderr_text)) => {
            let failure = classify_ffmpeg_failure(&stderr_text);
            let code = match &failure {
                RtspFailure::AuthFailed => "rtsp_auth",
                RtspFailure::Unreachable => "rtsp_inacessivel",
                RtspFailure::Other(_) => "rtsp_falha",
            };
            Err(AppError::new(code, "Falha ao conectar na fonte RTSP")
                .with_hint(failure.action_hint()))
        }
    }
}

/// Tentativas do probe inicial antes de desistir (T054): uma câmera IP recém
/// ligada, ou o publicador de uma fonte de teste recém-iniciado, pode não
/// estar pronta pra entregar o 1º frame dentro dos 3s de UMA tentativa —
/// achado em bancada 2026-08-11, a fonte conectava e reconectava
/// normalmente DEPOIS de iniciada, mas a 1ª tentativa falhava se `start_rtsp`
/// fosse chamado cedo demais.
pub const PROBE_MAX_ATTEMPTS: u32 = 3;

/// Intervalo entre tentativas do probe inicial.
pub const PROBE_RETRY_DELAY: Duration = Duration::from_millis(1500);

/// Repete `probe_url` até `max_attempts` vezes (com `retry_delay` entre
/// elas), parando na primeira que suceder. Devolve o erro da ÚLTIMA
/// tentativa se todas falharem — ainda um erro acionável (auth vs.
/// inacessível), só que só depois de dar à fonte uma chance de "esquentar".
pub async fn probe_url_with_retry(
    ffmpeg: &std::path::Path,
    url: &str,
    extra_env: &[(String, String)],
    max_attempts: u32,
    retry_delay: Duration,
) -> Result<(), AppError> {
    let mut last_err = None;
    for attempt in 0..max_attempts.max(1) {
        if attempt > 0 {
            tokio::time::sleep(retry_delay).await;
        }
        match probe_url(ffmpeg, url, extra_env).await {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.expect("max_attempts.max(1) garante ao menos 1 iteração"))
}

// ---------------------------------------------------------------------------
// Sessão RTSP (supervisor com reconexão)
// ---------------------------------------------------------------------------

/// Configuração de uma sessão RTSP ativa.
pub struct RtspSessionConfig {
    pub ffmpeg: std::path::PathBuf,
    /// URL já com credenciais injetadas (nunca persistida nesta forma).
    pub url: String,
    /// Resolução de saída (a câmera virtual é criada com ela; o ffmpeg
    /// redimensiona o stream para casar).
    pub resolution: (u32, u32),
    pub fps: u32,
    /// Linux: caminho do device v4l2 (`/dev/videoN`) onde o ffmpeg escreve
    /// direto. Windows: ignorado (frames vão pelo `FrameSink`).
    pub v4l2_device: Option<String>,
}

/// Handle da sessão: derrubar = parar (aborta o supervisor e mata o ffmpeg
/// via `kill_on_drop`).
pub struct RtspSession {
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl RtspSession {
    pub async fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        self.task.abort();
        let _ = self.task.await;
    }
}

/// Argumentos completos do ffmpeg da sessão (input low-delay + scale + saída
/// da plataforma).
fn session_args(config: &RtspSessionConfig) -> Vec<String> {
    let (w, h) = config.resolution;
    let mut args = vec!["-loglevel".into(), "warning".into()];
    args.extend(ffmpeg_input_args(&config.url));
    args.extend([
        "-vf".into(),
        format!("scale={w}:{h}"),
        "-r".into(),
        config.fps.to_string(),
    ]);
    match &config.v4l2_device {
        Some(device) => args.extend(output_args_v4l2(device)),
        None => args.extend(output_args_rawvideo()),
    }
    args
}

/// Inicia a sessão: supervisor que spawna o ffmpeg, entrega frames (Windows)
/// ou monitora o processo (Linux), e reconecta com backoff em caso de queda.
/// `on_standby` é chamado ao perder a fonte (imagem de espera — FR-006) com
/// a mensagem de estado; `sink` recebe frames RGBA no Windows.
pub fn start_session(
    config: RtspSessionConfig,
    sink: Option<FrameSink>,
    on_standby: Arc<dyn Fn(&str) + Send + Sync>,
) -> RtspSession {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let task = tokio::spawn(async move {
        while !stop_flag.load(Ordering::SeqCst) {
            match run_ffmpeg_once(&config, sink.as_ref()).await {
                Ok(()) => {
                    tracing::info!(url_host = %redact_url(&config.url), "sessão RTSP encerrada pelo ffmpeg");
                }
                Err(failure) => {
                    tracing::warn!(
                        url_host = %redact_url(&config.url),
                        ?failure,
                        "ffmpeg da sessão RTSP caiu"
                    );
                    if matches!(failure, RtspFailure::AuthFailed) {
                        // Reconectar não resolve credencial errada.
                        on_standby(&failure.action_hint());
                        return;
                    }
                }
            }
            if stop_flag.load(Ordering::SeqCst) {
                return;
            }
            on_standby("Reconectando à fonte RTSP...");
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    });

    RtspSession { stop, task }
}

/// Remove credenciais da URL para logging (`tracing` nunca vê a senha).
fn redact_url(url: &str) -> String {
    match (url.split_once("://"), url.split_once('@')) {
        (Some((scheme, _)), Some((_, host))) => format!("{scheme}://***@{host}"),
        _ => url.to_string(),
    }
}

/// Uma execução do ffmpeg: retorna `Ok` em EOF limpo (fonte encerrou) ou a
/// falha classificada do stderr.
async fn run_ffmpeg_once(
    config: &RtspSessionConfig,
    sink: Option<&FrameSink>,
) -> Result<(), RtspFailure> {
    let args = session_args(config);
    let mut cmd = Command::new(&config.ffmpeg);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(if sink.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = crate::procutil::hide_console(cmd)
        .spawn()
        .map_err(|e| RtspFailure::Other(format!("spawn do ffmpeg falhou: {e}")))?;

    // Acumula o stderr em paralelo para classificar a falha no fim.
    let stderr_task = child.stderr.take().map(|mut err| {
        tokio::spawn(async move {
            let mut buf = String::new();
            let _ = err.read_to_string(&mut buf).await;
            buf
        })
    });

    if let (Some(sink), Some(mut stdout)) = (sink, child.stdout.take()) {
        let frame_bytes = config.resolution.0 as usize * config.resolution.1 as usize * 4;
        let mut frame = vec![0u8; frame_bytes];
        let mut delivered: u64 = 0;
        loop {
            if let Err(e) = stdout.read_exact(&mut frame).await {
                tracing::debug!(error = %e, delivered, "stdout do ffmpeg RTSP encerrou");
                break;
            }
            sink(&frame);
            delivered += 1;
            if delivered == 1 || delivered.is_multiple_of(300) {
                tracing::info!(delivered, "frames RTSP entregues à câmera virtual");
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| RtspFailure::Other(format!("wait do ffmpeg falhou: {e}")))?;
    let stderr_text = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    };
    if status.success() {
        Ok(())
    } else {
        Err(classify_ffmpeg_failure(&stderr_text))
    }
}
