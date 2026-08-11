//! T038 — Cliente do protocolo de controle CamLink (US2).
//!
//! Fala NDJSON com o fork do scrcpy-server pelo túnel `adb forward
//! tcp:<porta> localabstract:camlink`: uma linha de request, uma de response
//! (mesma ordem), com eventos assíncronos (`af_state`/`faces`) intercalados —
//! demultiplexados pela chave `event` para um canal próprio (contrato §5).
//! Handshake `hello` valida a versão de protocolo do fork; formato validado
//! contra os golden files em `tests/protocol_contract_test.rs`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::model::{FocusMode, SmartMode, WbMode};

/// Versão de protocolo que este cliente entende (contrato: Versionamento).
pub const SUPPORTED_PROTOCOL: u32 = 1;

// ---------------------------------------------------------------------------
// Requests (formato de fio validado pelos golden files)
// ---------------------------------------------------------------------------

/// Comando tipado do protocolo §2–3 (escopo US2).
#[derive(Debug, Clone, PartialEq)]
pub enum ControlRequest {
    Hello,
    GetCapabilities,
    SetMode { mode: SmartMode },
    SetZoom { ratio: f32 },
    SetFocus { focus: FocusMode },
    SetExposure { compensation: i32 },
    SetIso { value: u32 },
    SetWb { mode: WbMode },
    SetEis { enabled: bool },
    SetTorch { enabled: bool },
    RawSnapshot,
    RawSequenceStart { fps: f32 },
    RawSequenceStop,
}

/// Espelho serde do formato de fio: tag `cmd`, campos achatados.
#[derive(Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum WireRequest {
    Hello,
    GetCapabilities,
    SetMode {
        mode: SmartMode,
    },
    SetZoom {
        ratio: f32,
    },
    SetFocus {
        mode: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        x: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        y: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        distance: Option<f32>,
    },
    SetExposure {
        compensation: i32,
    },
    SetIso {
        value: u32,
    },
    SetWb {
        mode: WbMode,
    },
    SetEis {
        enabled: bool,
    },
    SetTorch {
        enabled: bool,
    },
    RawSnapshot,
    RawSequenceStart {
        fps: f32,
    },
    RawSequenceStop,
}

impl ControlRequest {
    fn wire(&self) -> WireRequest {
        match self {
            ControlRequest::Hello => WireRequest::Hello,
            ControlRequest::GetCapabilities => WireRequest::GetCapabilities,
            ControlRequest::SetMode { mode } => WireRequest::SetMode { mode: *mode },
            ControlRequest::SetZoom { ratio } => WireRequest::SetZoom { ratio: *ratio },
            ControlRequest::SetFocus { focus } => match focus {
                FocusMode::ContinuousAuto => WireRequest::SetFocus {
                    mode: "continuous",
                    x: None,
                    y: None,
                    distance: None,
                },
                FocusMode::Tap { x, y } => WireRequest::SetFocus {
                    mode: "tap",
                    x: Some(*x),
                    y: Some(*y),
                    distance: None,
                },
                FocusMode::Manual { distance } => WireRequest::SetFocus {
                    mode: "manual",
                    x: None,
                    y: None,
                    distance: Some(*distance),
                },
            },
            ControlRequest::SetExposure { compensation } => WireRequest::SetExposure {
                compensation: *compensation,
            },
            ControlRequest::SetIso { value } => WireRequest::SetIso { value: *value },
            ControlRequest::SetWb { mode } => WireRequest::SetWb { mode: *mode },
            ControlRequest::SetEis { enabled } => WireRequest::SetEis { enabled: *enabled },
            ControlRequest::SetTorch { enabled } => WireRequest::SetTorch { enabled: *enabled },
            ControlRequest::RawSnapshot => WireRequest::RawSnapshot,
            ControlRequest::RawSequenceStart { fps } => WireRequest::RawSequenceStart { fps: *fps },
            ControlRequest::RawSequenceStop => WireRequest::RawSequenceStop,
        }
    }

    /// Linha NDJSON do request (sem `\n` — o transporte acrescenta).
    pub fn to_line(&self) -> String {
        serde_json::to_string(&self.wire()).unwrap_or_else(|e| {
            // Serialização de tipos próprios sem estado externo não falha;
            // ainda assim, nunca panicar no caminho de controle.
            tracing::error!(error = %e, "serialização de ControlRequest falhou");
            String::from("{\"cmd\":\"hello\"}")
        })
    }
}

// ---------------------------------------------------------------------------
// Responses e eventos
// ---------------------------------------------------------------------------

/// Código de erro do envelope (contrato §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlErrorCode {
    OutOfRange,
    Unsupported,
    BadRequest,
    CameraError,
    Busy,
    /// Código desconhecido (fork mais novo que o cliente) — preservado.
    Other(String),
}

impl ControlErrorCode {
    pub fn as_str(&self) -> &str {
        match self {
            ControlErrorCode::OutOfRange => "OUT_OF_RANGE",
            ControlErrorCode::Unsupported => "UNSUPPORTED",
            ControlErrorCode::BadRequest => "BAD_REQUEST",
            ControlErrorCode::CameraError => "CAMERA_ERROR",
            ControlErrorCode::Busy => "BUSY",
            ControlErrorCode::Other(code) => code,
        }
    }
}

impl std::str::FromStr for ControlErrorCode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "OUT_OF_RANGE" => ControlErrorCode::OutOfRange,
            "UNSUPPORTED" => ControlErrorCode::Unsupported,
            "BAD_REQUEST" => ControlErrorCode::BadRequest,
            "CAMERA_ERROR" => ControlErrorCode::CameraError,
            "BUSY" => ControlErrorCode::Busy,
            other => ControlErrorCode::Other(other.to_string()),
        })
    }
}

/// Resposta do servidor a um request.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlReply {
    /// Resposta do `hello` (fora do envelope padrão).
    Hello { protocol: u32, server: String },
    /// Envelope `{"ok":true,"data":...}`.
    Ok(serde_json::Value),
    /// Envelope `{"ok":false,"error":{...}}`.
    Err { code: ControlErrorCode, msg: String },
}

/// Retângulo de rosto do evento `faces` (coordenadas normalizadas).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Evento assíncrono servidor → desktop (contrato §5).
#[derive(Debug, Clone, PartialEq)]
pub enum ControlEvent {
    AfState {
        state: String,
    },
    Faces {
        rects: Vec<FaceRect>,
    },
    RawFrameDropped {
        reason: String,
    },
    /// Frame RAW decodificado — NUNCA chega como linha NDJSON (contrato §4);
    /// `spawn_reader` monta isso a partir do framing binário
    /// (`raw_manager::parse_frame`), não de `parse_server_line`.
    RawFrame(crate::raw_manager::RawFrame),
}

/// Uma linha vinda do servidor: resposta ou evento.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Reply(ControlReply),
    Event(ControlEvent),
}

/// Demultiplexa uma linha NDJSON do servidor pela chave (`event` vs `ok`).
pub fn parse_server_line(line: &str) -> Result<ServerMessage, ControlClientError> {
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| ControlClientError::Parse(format!("linha não é JSON: {e}")))?;

    if let Some(event) = value.get("event").and_then(|v| v.as_str()) {
        let event = match event {
            "af_state" => ControlEvent::AfState {
                state: value
                    .get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            "faces" => {
                let rects = value
                    .get("rects")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|e| ControlClientError::Parse(format!("rects inválidos: {e}")))?
                    .unwrap_or_default();
                ControlEvent::Faces { rects }
            }
            "raw_frame_dropped" => ControlEvent::RawFrameDropped {
                reason: value
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            other => {
                return Err(ControlClientError::Parse(format!(
                    "evento desconhecido: {other}"
                )))
            }
        };
        return Ok(ServerMessage::Event(event));
    }

    match value.get("ok").and_then(|v| v.as_bool()) {
        Some(true) => {
            if let Some(protocol) = value.get("protocol").and_then(|v| v.as_u64()) {
                Ok(ServerMessage::Reply(ControlReply::Hello {
                    protocol: protocol as u32,
                    server: value
                        .get("server")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                }))
            } else {
                Ok(ServerMessage::Reply(ControlReply::Ok(
                    value
                        .get("data")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )))
            }
        }
        Some(false) => {
            let error = value.get("error").cloned().unwrap_or_default();
            let code = error
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAMERA_ERROR")
                .parse()
                .unwrap_or(ControlErrorCode::CameraError);
            let msg = error
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Ok(ServerMessage::Reply(ControlReply::Err { code, msg }))
        }
        None => Err(ControlClientError::Parse(
            "linha sem chave 'ok' nem 'event'".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Túnel adb
// ---------------------------------------------------------------------------

/// Argumentos de `adb -s <serial> forward tcp:<porta> localabstract:camlink`.
pub fn adb_forward_args(serial: &str, port: u16) -> Vec<String> {
    vec![
        "-s".to_string(),
        serial.to_string(),
        "forward".to_string(),
        format!("tcp:{port}"),
        "localabstract:camlink".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Cliente
// ---------------------------------------------------------------------------

/// Falha do cliente de controle (FR-010: mensagens acionáveis).
#[derive(Debug, thiserror::Error)]
pub enum ControlClientError {
    #[error("timeout esperando resposta do servidor de controle")]
    Timeout,
    #[error(
        "versão de protocolo do fork desconhecida: {got} (este cliente suporta \
         {SUPPORTED_PROTOCOL}) — atualize o CamLink ou o scrcpy-server-camlink"
    )]
    UnsupportedProtocol { got: u32 },
    #[error("conexão de controle falhou: {0}")]
    Io(#[from] std::io::Error),
    #[error("resposta inválida do servidor de controle: {0}")]
    Parse(String),
    #[error("conexão de controle encerrada pelo servidor")]
    Closed,
}

/// Cliente NDJSON: um request por vez (respostas chegam na ordem — contrato);
/// eventos saem pelo canal devolvido por `connect`.
#[derive(Debug)]
pub struct ControlClient {
    write: OwnedWriteHalf,
    replies: mpsc::Receiver<ControlReply>,
    timeout: Duration,
}

/// Resultado de uma tentativa de extrair UM item do início do buffer
/// acumulado do socket.
enum DrainOutcome {
    /// Item completo (resposta ou evento); os bytes já foram consumidos do
    /// buffer.
    Item(ServerMessage),
    /// Linha vazia ou JSON inválido — ignorada (com log), bytes já
    /// consumidos; não é motivo pra encerrar a conexão nem pra esperar mais
    /// bytes, só tentar extrair o próximo item do que já está no buffer.
    Skip,
    /// Não há bytes suficientes no buffer pra decidir nada ainda — precisa
    /// ler mais do socket antes de tentar de novo.
    NeedMoreData,
    /// Frame RAW corrompido (tag ou metadata inválidos) — framing binário
    /// não tem como "ressincronizar" no meio de um frame, a conexão precisa
    /// encerrar (contrato §4).
    Fatal,
}

/// Extrai UM item (linha NDJSON ou frame RAW binário, contrato §4) do
/// início de `buf`, consumindo os bytes usados. Puro (sem I/O) — testável
/// isoladamente sem socket real. O byte `0xD1` nunca é o início de uma
/// linha JSON válida (que sempre começa com `{`), então checar esse byte
/// primeiro é suficiente pra decidir qual dos dois framings usar.
fn drain_one(buf: &mut Vec<u8>) -> DrainOutcome {
    if buf.first() == Some(&crate::raw_manager::RAW_FRAME_TAG) {
        return match crate::raw_manager::parse_frame(buf) {
            Ok(Some((frame, consumed))) => {
                buf.drain(..consumed);
                DrainOutcome::Item(ServerMessage::Event(ControlEvent::RawFrame(frame)))
            }
            Ok(None) => DrainOutcome::NeedMoreData,
            Err(e) => {
                tracing::warn!(error = %e, "frame RAW corrompido no socket de controle");
                DrainOutcome::Fatal
            }
        };
    }

    let Some(pos) = buf.iter().position(|&b| b == b'\n') else {
        return DrainOutcome::NeedMoreData;
    };
    let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
    let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
    let line = line.trim_end_matches('\r');
    if line.trim().is_empty() {
        return DrainOutcome::Skip;
    }
    match parse_server_line(line) {
        Ok(msg) => DrainOutcome::Item(msg),
        Err(e) => {
            let line = line.to_string();
            tracing::warn!(error = %e, line, "linha inválida do servidor de controle");
            DrainOutcome::Skip
        }
    }
}

fn spawn_reader(
    read: OwnedReadHalf,
) -> (mpsc::Receiver<ControlReply>, mpsc::Receiver<ControlEvent>) {
    use tokio::io::AsyncReadExt;

    let (reply_tx, reply_rx) = mpsc::channel(16);
    let (event_tx, event_rx) = mpsc::channel(64);
    tokio::spawn(async move {
        let mut read = read;
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 8192];
        loop {
            loop {
                match drain_one(&mut buf) {
                    DrainOutcome::Item(ServerMessage::Reply(reply)) => {
                        if reply_tx.send(reply).await.is_err() {
                            return;
                        }
                    }
                    DrainOutcome::Item(ServerMessage::Event(event)) => {
                        // Evento é descartável se ninguém escuta (preview da
                        // UI fechado, por exemplo) — nunca trava o leitor.
                        let _ = event_tx.try_send(event);
                    }
                    DrainOutcome::Skip => continue,
                    DrainOutcome::NeedMoreData => break,
                    DrainOutcome::Fatal => return,
                }
            }
            match read.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    });
    (reply_rx, event_rx)
}

impl ControlClient {
    /// Conecta no túnel e faz o handshake `hello`; rejeita versão de
    /// protocolo desconhecida com erro descritivo.
    pub async fn connect(
        addr: impl tokio::net::ToSocketAddrs,
        timeout: Duration,
    ) -> Result<(Self, mpsc::Receiver<ControlEvent>), ControlClientError> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| ControlClientError::Timeout)??;
        stream.set_nodelay(true)?;
        let (read, write) = stream.into_split();
        let (replies, events) = spawn_reader(read);
        let mut client = Self {
            write,
            replies,
            timeout,
        };

        match client.request(ControlRequest::Hello).await? {
            ControlReply::Hello { protocol, server } => {
                if protocol != SUPPORTED_PROTOCOL {
                    return Err(ControlClientError::UnsupportedProtocol { got: protocol });
                }
                tracing::info!(protocol, server, "servidor de controle conectado");
                Ok((client, events))
            }
            other => Err(ControlClientError::Parse(format!(
                "hello respondeu com {other:?}"
            ))),
        }
    }

    /// Envia um request e espera a resposta correspondente (respostas chegam
    /// na mesma ordem dos requests — contrato), com timeout.
    pub async fn request(
        &mut self,
        request: ControlRequest,
    ) -> Result<ControlReply, ControlClientError> {
        let line = format!("{}\n", request.to_line());
        self.write.write_all(line.as_bytes()).await?;
        self.write.flush().await?;

        match tokio::time::timeout(self.timeout, self.replies.recv()).await {
            Err(_) => Err(ControlClientError::Timeout),
            Ok(None) => Err(ControlClientError::Closed),
            Ok(Some(reply)) => Ok(reply),
        }
    }
}
