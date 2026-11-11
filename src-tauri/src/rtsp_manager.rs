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

/// Flags de input low-delay (research.md R5). `analyzeduration`/`probesize`
/// foram relaxados de 0/32 (mínimo absoluto do ffmpeg) para 100ms/64KB
/// (2026-08-19): câmeras que não anunciam `sprop-parameter-sets` no SDP
/// (achado em bancada com um app RTSP Android) exigem farejar SPS/PPS no
/// próprio stream, e 32 bytes nunca é suficiente — o ffmpeg nem identifica
/// a faixa de vídeo (`Could not find codec parameters`). O valor mínimo
/// absoluto só funciona com fontes que exportam os parâmetros via SDP.
pub fn ffmpeg_input_args(url: &str) -> Vec<String> {
    vec![
        "-fflags".into(),
        "nobuffer".into(),
        "-flags".into(),
        "low_delay".into(),
        "-analyzeduration".into(),
        "100000".into(),
        "-probesize".into(),
        "65536".into(),
        "-rtsp_transport".into(),
        "tcp".into(),
        "-timeout".into(),
        SOCKET_IO_TIMEOUT_US.to_string(),
        "-i".into(),
        url.into(),
    ]
}

/// Timeout de I/O do socket RTSP (microssegundos) — sem isso o default do
/// ffmpeg é 0 (espera infinita): se a fonte cair sem fechar a conexão TCP
/// de forma limpa, o processo nunca sai sozinho e o supervisor de
/// reconexão (`run_ffmpeg_once`) fica bloqueado pra sempre num
/// `read_exact` que também nunca retorna (achado em bancada 2026-08-19,
/// T054 — queda no meio do stream não se recuperava sem parar/reiniciar
/// a sessão manualmente).
const SOCKET_IO_TIMEOUT_US: u64 = 10_000_000;

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

/// Caracteres que podem ir CRUS na userinfo de uma URL — RFC 3986:
/// unreserved + sub-delims. Qualquer outro byte sai percent-encoded.
const USERINFO_SAFE: &[u8] = b"-._~!$&'()*+,;=";

/// Percent-encode de uma credencial que vai dentro da URL.
///
/// A lógica é invertida de propósito: passa cru só o que a RFC permite.
/// Enumerar os caracteres "perigosos" já falhou duas vezes — o espaço e os
/// controles só entraram numa correção posterior, e os não-ASCII (um
/// acento, um espaço não separável vindo de copiar-e-colar) continuavam
/// indo crus para o ffmpeg, que respondia "Invalid argument" sem nenhuma
/// pista de que a culpa era da senha.
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || USERINFO_SAFE.contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Fim da autoridade (`host[:porta]`) no que vem depois de `://`: a
/// primeira `/`, `?` ou `#` (RFC 3986). Só até aí um `@` separa credencial
/// de host; depois dele o `@` já é parte do caminho.
fn authority_end(rest: &str) -> usize {
    rest.find(['/', '?', '#']).unwrap_or(rest.len())
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
    // O `@` só é separador dentro da autoridade: em `rtsp://host/live@2`
    // não há usuário nenhum, e tratá-lo como separador montaria uma URL
    // sem sentido (`rtsp://host/live:senha@2`).
    match rest[..authority_end(rest)].rfind('@') {
        Some(at) => {
            let (user, host) = (&rest[..at], &rest[at + 1..]);
            // O usuário também é codificado: um espaço aqui quebra a URL
            // no ffmpeg exatamente como um espaço na senha.
            format!(
                "{scheme}://{}:{}@{host}",
                percent_encode(user),
                percent_encode(secret)
            )
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
            // O 401 não significa só "senha errada". Servidores respondem
            // 401 quando o CAMINHO do stream está ausente ou errado — foi o
            // que aconteceu em bancada (2026-10-14) com `rtsp://host:8080`
            // sem caminho no IP Webcam, que não tinha senha alguma
            // configurada. A dica antiga mandava procurar uma credencial
            // que não existia; esta cobre as duas causas, na ordem em que
            // compensa investigar.
            RtspFailure::AuthFailed => {
                "A câmera recusou o acesso (401/403). Confira o CAMINHO do stream na URL — \
                 muitos servidores respondem 401 quando ele falta (ex.: \
                 rtsp://host:8080/h264_ulaw.sdp em vez de rtsp://host:8080) — e, \
                 se a câmera tiver senha, o usuário e a senha."
                    .to_string()
            }
            RtspFailure::Unreachable => {
                "Câmera inacessível: confira o endereço, a porta e se ela está na mesma rede."
                    .to_string()
            }
            RtspFailure::Other(msg) => format!("Falha na fonte RTSP: {msg}"),
        }
    }
}

/// Procura um código de status solto no texto SEM casar com os endereços
/// hexadecimais que o ffmpeg carimba em toda linha: `[rtsp @ 0x55d4403a]`
/// contém "403", e a comparação por substring crua fazia QUALQUER erro
/// virar "a câmera recusou o acesso" (achado na revisão da v0.1.0).
fn has_status_code(lower: &str, code: &str) -> bool {
    lower.match_indices(code).any(|(i, _)| {
        let antes = lower[..i].chars().next_back();
        let depois = lower[i + code.len()..].chars().next();
        !antes.is_some_and(|c| c.is_ascii_alphanumeric())
            && !depois.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// Classifica o stderr acumulado de um ffmpeg que morreu.
pub fn classify_ffmpeg_failure(stderr: &str) -> RtspFailure {
    let lower = stderr.to_lowercase();
    if has_status_code(&lower, "401")
        || has_status_code(&lower, "403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("authorization failed")
    {
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
    // Censurado aqui, no ponto de entrada: daqui o texto vai tanto para o
    // log (`?failure`) quanto para a interface (`action_hint`), e proteger
    // só um dos dois deixaria o outro vazando.
    RtspFailure::Other(redact_credentials(
        stderr.lines().last().unwrap_or("erro desconhecido"),
    ))
}

// ---------------------------------------------------------------------------
// Probe de URL (validação com timeout de 3 s)
// ---------------------------------------------------------------------------

/// Falha de um probe: o erro acionável e o stderr já censurado e cortado,
/// para quem chama decidir o nível do log.
struct ProbeFailure {
    error: AppError,
    stderr: String,
}

/// Últimas `n` linhas não vazias, numa linha só. O ffmpeg repete o mesmo
/// erro em vários níveis e o começo do stderr é sempre o menos informativo.
fn tail_lines(text: &str, n: usize) -> String {
    let linhas: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    linhas[linhas.len().saturating_sub(n)..].join(" | ")
}

/// Valida que a URL responde, com timeout de 3 s (quickstart Cenário 4):
/// decodifica 1 frame e sai. `url` já deve vir com credenciais injetadas.
pub async fn probe_url(
    ffmpeg: &std::path::Path,
    url: &str,
    extra_env: &[(String, String)],
) -> Result<(), AppError> {
    probe_once(ffmpeg, url, extra_env).await.map_err(|falha| {
        if !falha.stderr.is_empty() {
            tracing::debug!(stderr = %falha.stderr, "probe RTSP: stderr do ffmpeg");
        }
        falha.error
    })
}

/// Uma tentativa de probe. Devolve o stderr junto da falha em vez de
/// logá-lo: só quem sabe se esta era a última tentativa sabe se isso é um
/// problema do usuário ou o aquecimento normal de uma câmera.
async fn probe_once(
    ffmpeg: &std::path::Path,
    url: &str,
    extra_env: &[(String, String)],
) -> Result<(), ProbeFailure> {
    let sem_stderr = |error| ProbeFailure {
        error,
        stderr: String::new(),
    };
    validate_rtsp_url(url).map_err(sem_stderr)?;

    // `-hide_banner`: sem ele 1,6 KB dos ~1,9 KB de stderr são a versão e a
    // configuração de build do ffmpeg, que enterram no log a única linha
    // que interessa (medido em bancada: 1937 → 256 bytes). O nível de log
    // fica no padrão de propósito — é dele que sai o texto classificado.
    let mut args = vec!["-hide_banner".to_string()];
    args.extend(ffmpeg_input_args(url));
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
        sem_stderr(
            AppError::new(
                "ffmpeg_ausente",
                format!("Não consegui executar o ffmpeg: {e}"),
            )
            .with_hint("Instale o ffmpeg e garanta que está no PATH."),
        )
    })?;

    // O stderr é acumulado num buffer COMPARTILHADO, fora do future do
    // timeout: lê-lo dentro dele fazia o texto ser cancelado junto quando o
    // timeout vencia — e é justamente aí (câmera desligada, o caso mais
    // comum) que o log não tinha nada além de `code=rtsp_timeout`.
    let coletado = Arc::new(std::sync::Mutex::new(String::new()));
    let leitor = child.stderr.take().map(|mut err| {
        let destino = Arc::clone(&coletado);
        tokio::spawn(async move {
            let mut pedaco = [0u8; 2048];
            loop {
                match err.read(&mut pedaco).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut buf) = destino.lock() {
                            buf.push_str(&String::from_utf8_lossy(&pedaco[..n]));
                        }
                    }
                }
            }
        })
    });

    let esperou = tokio::time::timeout(PROBE_TIMEOUT, child.wait()).await;
    if esperou.is_ok() {
        // O processo saiu: dar um instante ao leitor para drenar o pipe,
        // senão o stderr chega truncado justamente no fim, que é a parte
        // que classifica a falha.
        if let Some(task) = leitor {
            let _ = tokio::time::timeout(Duration::from_millis(500), task).await;
        }
    }
    let stderr_text = coletado.lock().map(|buf| buf.clone()).unwrap_or_default();
    let stderr = tail_lines(&redact_credentials(&stderr_text), 4);

    match esperou {
        Err(_) => Err(ProbeFailure {
            error: AppError::new("rtsp_timeout", "Fonte RTSP não respondeu em 3 s")
                .with_hint(RtspFailure::Unreachable.action_hint()),
            stderr,
        }),
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(_) => {
            let failure = classify_ffmpeg_failure(&stderr_text);
            let code = match &failure {
                RtspFailure::AuthFailed => "rtsp_auth",
                RtspFailure::Unreachable => "rtsp_inacessivel",
                RtspFailure::Other(_) => "rtsp_falha",
            };
            Err(ProbeFailure {
                error: AppError::new(code, "Falha ao conectar na fonte RTSP")
                    .with_hint(failure.action_hint()),
                stderr,
            })
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
    let tentativas = max_attempts.max(1);
    for attempt in 0..tentativas {
        if attempt > 0 {
            tokio::time::sleep(retry_delay).await;
        }
        let started = std::time::Instant::now();
        match probe_once(ffmpeg, url, extra_env).await {
            Ok(()) => {
                tracing::debug!(attempt = attempt + 1, elapsed = ?started.elapsed(), "probe RTSP ok");
                return Ok(());
            }
            Err(falha) => {
                // Só a tentativa que DESISTE vira WARN. Falhar nas
                // primeiras é o comportamento esperado de uma câmera
                // recém-ligada (é para isso que este laço existe), e
                // alarmar nelas é o mesmo erro que o T099 corrigiu no
                // pipeline Android: três cliques normais do usuário
                // viraram três WARN e uma caçada a um defeito inexistente.
                if attempt + 1 == tentativas {
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = tentativas,
                        elapsed = ?started.elapsed(),
                        code = %falha.error.code,
                        stderr = %falha.stderr,
                        "probe RTSP falhou"
                    );
                } else {
                    tracing::debug!(
                        attempt = attempt + 1,
                        max_attempts = tentativas,
                        elapsed = ?started.elapsed(),
                        code = %falha.error.code,
                        stderr = %falha.stderr,
                        "probe RTSP falhou, vai repetir"
                    );
                }
                last_err = Some(falha.error);
            }
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
        // Quantas quedas seguidas já houve. Uma fonte que cai de verdade
        // (câmera sem energia) reconecta a cada RECONNECT_DELAY: logar o
        // stderr inteiro em toda volta encheria o log do dia — a noite
        // inteira daria ~86 mil linhas idênticas num arquivo que o T098
        // guarda por 7 dias.
        let mut seguidas: u32 = 0;
        while !stop_flag.load(Ordering::SeqCst) {
            match run_ffmpeg_once(&config, sink.as_ref()).await {
                Ok(()) => {
                    seguidas = 0;
                    tracing::info!(url_host = %redact_credentials(&config.url), "sessão RTSP encerrada pelo ffmpeg");
                }
                Err(queda) => {
                    seguidas += 1;
                    if seguidas == 1 || seguidas.is_multiple_of(FAILURE_LOG_EVERY) {
                        tracing::warn!(
                            url_host = %redact_credentials(&config.url),
                            failure = ?queda.failure,
                            stderr = %queda.stderr,
                            seguidas,
                            "ffmpeg da sessão RTSP caiu"
                        );
                    } else {
                        tracing::debug!(
                            url_host = %redact_credentials(&config.url),
                            failure = ?queda.failure,
                            seguidas,
                            "ffmpeg da sessão RTSP caiu de novo"
                        );
                    }
                    if matches!(queda.failure, RtspFailure::AuthFailed) {
                        // Reconectar não resolve credencial errada.
                        on_standby(&queda.failure.action_hint());
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

/// Censura credenciais embutidas em QUALQUER posição de um texto livre.
///
/// Existe porque o ffmpeg ecoa a URL de entrada no stderr, e esse texto vai
/// tanto para o log em arquivo (T098, que guarda 7 dias em disco) quanto
/// para a interface, via `RtspFailure::Other`/`action_hint`.
///
/// As duas versões anteriores escolheram um caractere para terminar a
/// autoridade e erraram em lados opostos: com `/`, uma senha contendo `/`
/// deixava o `@` fora da janela e passava inteira (`rtsp://cam:a/b@host`);
/// sem limite nenhum, a etiqueta `[rtsp @ 0x55...]` que o ffmpeg carimba
/// em toda linha virava "credencial" e apagava a mensagem de erro junto.
/// A regra aqui olha para as duas:
///
/// - a janela de uma autoridade acaba na quebra de linha ou no começo da
///   PRÓXIMA URL — daí em diante o texto não é mais dela;
/// - dentro dela vale o ÚLTIMO `@`, menos quando ele está cercado de
///   espaço dos dois lados, que é a etiqueta do ffmpeg e nunca uma URL.
///
/// Quando a dúvida sobra (um `@` no caminho, por exemplo), censura a mais:
/// perder diagnóstico se recupera pedindo o log de novo; vazar senha não.
pub fn redact_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("://") {
        let after = pos + "://".len();
        out.push_str(&rest[..after]);
        rest = &rest[after..];
        let janela = rest
            .find(['\n', '\r'])
            .into_iter()
            .chain(rest.find("://"))
            .min()
            .unwrap_or(rest.len());
        if let Some(at) = last_userinfo_at(&rest[..janela]) {
            out.push_str("***@");
            rest = &rest[at + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// Último `@` da janela que ainda pode separar credencial de host.
fn last_userinfo_at(janela: &str) -> Option<usize> {
    janela
        .char_indices()
        .filter(|(i, ch)| {
            *ch == '@' && {
                let antes = janela[..*i].chars().next_back();
                let depois = janela[i + 1..].chars().next();
                !(antes.is_some_and(|c| c.is_whitespace())
                    && depois.is_some_and(|c| c.is_whitespace()))
            }
        })
        .map(|(i, _)| i)
        .next_back()
}

/// A cada quantas quedas CONSECUTIVAS o log volta a gritar. Com
/// `RECONNECT_DELAY` de 2 s, 300 é uma linha a cada 10 min: a fonte
/// continua visível no log sem afogá-lo.
const FAILURE_LOG_EVERY: u32 = 300;

/// Falha de uma execução da sessão: a classificação e o stderr censurado,
/// que o supervisor loga só na PRIMEIRA de uma sequência.
struct RunFailure {
    failure: RtspFailure,
    stderr: String,
}

/// Uma execução do ffmpeg: retorna `Ok` em EOF limpo (fonte encerrou) ou a
/// falha classificada do stderr.
async fn run_ffmpeg_once(
    config: &RtspSessionConfig,
    sink: Option<&FrameSink>,
) -> Result<(), RunFailure> {
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
        .map_err(|e| RunFailure {
            failure: RtspFailure::Other(format!("spawn do ffmpeg falhou: {e}")),
            stderr: String::new(),
        })?;

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

    let status = child.wait().await.map_err(|e| RunFailure {
        failure: RtspFailure::Other(format!("wait do ffmpeg falhou: {e}")),
        stderr: String::new(),
    })?;
    let stderr_text = match stderr_task {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    };
    if status.success() {
        Ok(())
    } else {
        // O stderr sobe junto com a classificação em vez de virar uma
        // linha de log aqui: a classificação sozinha não diz a causa, mas
        // quem decide se vale gritar é o supervisor, que sabe se esta é a
        // primeira queda ou a milésima da mesma sequência.
        Err(RunFailure {
            failure: classify_ffmpeg_failure(&stderr_text),
            stderr: tail_lines(&redact_credentials(stderr_text.trim()), 4),
        })
    }
}
