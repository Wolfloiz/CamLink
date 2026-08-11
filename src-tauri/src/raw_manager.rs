//! Captura RAW (DNG): recepção com framing binário e armazenamento em disco.
//!
//! Protocolo (contracts/control-protocol.md §4), sempre big-endian, nunca
//! base64 (desperdiçaria ~33% do gargalo do túnel ADB — research.md R6):
//!
//! ```text
//! [u8 tag=0xD1][u32be metadata_len][metadata JSON][u64be dng_len][bytes DNG]
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{RawCaptureJob, RawJobKind, RawJobState};

// ---------------------------------------------------------------------------
// Framing binário (T055, FR-019)
// ---------------------------------------------------------------------------

/// Byte que identifica o início de um frame RAW no socket de controle.
pub const RAW_FRAME_TAG: u8 = 0xD1;

/// Metadata que acompanha cada frame (contracts/control-protocol.md §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawFrameMetadata {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub width: u32,
    pub height: u32,
}

/// Um frame RAW já decodificado do framing binário.
#[derive(Debug, Clone, PartialEq)]
pub struct RawFrame {
    pub metadata: RawFrameMetadata,
    pub dng: Vec<u8>,
}

/// Falha ao interpretar o framing binário — nunca por dado insuficiente
/// ainda chegando (isso é `Ok(None)` em `parse_frame`), só por bytes que já
/// comprovam corrupção.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameError {
    BadTag(u8),
    InvalidMetadata(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::BadTag(tag) => write!(
                f,
                "tag de frame RAW inválida: 0x{tag:02X} (esperado 0x{RAW_FRAME_TAG:02X})"
            ),
            FrameError::InvalidMetadata(msg) => {
                write!(f, "metadata do frame RAW corrompida: {msg}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// 1 (tag) + 4 (u32be metadata_len) — bytes fixos antes do JSON de metadata.
const HEADER_PREFIX_LEN: usize = 5;
/// Tamanho do campo de comprimento do DNG (u64be).
const DNG_LEN_FIELD: usize = 8;

/// Tenta extrair UM frame do início de `buf` (uso streaming: `buf` pode
/// conter só um pedaço do frame, ou vários frames concatenados — só o
/// primeiro é consumido por chamada).
///
/// - `Ok(Some((frame, consumed)))`: frame completo; o chamador deve
///   descartar os primeiros `consumed` bytes de `buf` antes da próxima
///   chamada.
/// - `Ok(None)`: ainda não há bytes suficientes para decidir — aguardar mais
///   dados do socket sem descartar nada (frame parcial, não é erro).
/// - `Err`: os bytes já recebidos comprovam que o frame está corrompido (tag
///   errada ou JSON de metadata inválido) — esperar mais dados não resolve,
///   a conexão deve ser encerrada.
pub fn parse_frame(buf: &[u8]) -> Result<Option<(RawFrame, usize)>, FrameError> {
    if buf.len() < HEADER_PREFIX_LEN {
        return Ok(None);
    }
    if buf[0] != RAW_FRAME_TAG {
        return Err(FrameError::BadTag(buf[0]));
    }
    let metadata_len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    let metadata_end = HEADER_PREFIX_LEN + metadata_len;
    if buf.len() < metadata_end {
        return Ok(None);
    }
    let metadata: RawFrameMetadata = serde_json::from_slice(&buf[HEADER_PREFIX_LEN..metadata_end])
        .map_err(|e| FrameError::InvalidMetadata(e.to_string()))?;

    let dng_len_end = metadata_end + DNG_LEN_FIELD;
    if buf.len() < dng_len_end {
        return Ok(None);
    }
    let dng_len_bytes: [u8; 8] = buf[metadata_end..dng_len_end]
        .try_into()
        .expect("slice de 8 bytes");
    let dng_len = u64::from_be_bytes(dng_len_bytes) as usize;

    let dng_end = dng_len_end + dng_len;
    if buf.len() < dng_end {
        return Ok(None);
    }

    let frame = RawFrame {
        metadata,
        dng: buf[dng_len_end..dng_end].to_vec(),
    };
    Ok(Some((frame, dng_end)))
}

/// Serializa um frame no framing binário do protocolo — usado nos testes
/// pra simular o que o fork manda pelo socket.
pub fn encode_frame(metadata: &RawFrameMetadata, dng: &[u8]) -> Vec<u8> {
    let metadata_json = serde_json::to_vec(metadata).expect("RawFrameMetadata sempre serializa");
    let mut out = Vec::with_capacity(1 + 4 + metadata_json.len() + 8 + dng.len());
    out.push(RAW_FRAME_TAG);
    out.extend_from_slice(&(metadata_json.len() as u32).to_be_bytes());
    out.extend_from_slice(&metadata_json);
    out.extend_from_slice(&(dng.len() as u64).to_be_bytes());
    out.extend_from_slice(dng);
    out
}

/// Nome do arquivo DNG a partir do metadata: sequência zero-padded (ordena
/// naturalmente por nome) + timestamp do aparelho (rastreabilidade de quando
/// cada frame foi capturado).
pub fn dng_filename(metadata: &RawFrameMetadata) -> String {
    format!("raw_{:06}_{}.dng", metadata.seq, metadata.timestamp_ms)
}

/// Grava um frame já decodificado no diretório de saída; retorna o caminho
/// final. `output_dir` precisa existir.
pub fn write_frame(output_dir: &Path, frame: &RawFrame) -> std::io::Result<PathBuf> {
    let path = output_dir.join(dng_filename(&frame.metadata));
    std::fs::write(&path, &frame.dng)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Cadência dinâmica (T056, FR-019/020)
// ---------------------------------------------------------------------------

/// Faixa permitida de fps da Sequência RAW (FR-019).
pub const RAW_SEQUENCE_MIN_FPS: f32 = 1.0;
pub const RAW_SEQUENCE_MAX_FPS: f32 = 3.0;

/// Banda disponível pra RAW depois de reservar o stream principal (FR-020:
/// vídeo tem prioridade na disputa por banda) — nunca negativa, mesmo que o
/// stream principal esteja momentaneamente usando mais do que o throughput
/// total medido (pico transitório).
pub fn throughput_for_raw(measured_total_bps: f64, main_stream_bps: f64) -> f64 {
    (measured_total_bps - main_stream_bps).max(0.0)
}

/// fps sustentável dado o tamanho de um frame DNG e a banda disponível pra
/// RAW (`throughput ÷ frame_bytes`), sempre dentro de [1,3] fps (FR-019).
/// `frame_bytes == 0` (não deveria acontecer — o sensor sempre produz algo)
/// devolve o teto em vez de dividir por zero.
pub fn effective_raw_fps(frame_bytes: u64, throughput_for_raw_bytes_per_sec: f64) -> f32 {
    if frame_bytes == 0 {
        return RAW_SEQUENCE_MAX_FPS;
    }
    let sustainable = throughput_for_raw_bytes_per_sec.max(0.0) / frame_bytes as f64;
    (sustainable as f32).clamp(RAW_SEQUENCE_MIN_FPS, RAW_SEQUENCE_MAX_FPS)
}

/// fps concedida em resposta a `raw_sequence_start`
/// (contracts/control-protocol.md §4, campo `granted_fps`): nunca acima do
/// sustentável nem do que o cliente pediu — não há motivo pra gravar mais
/// rápido do que o solicitado só porque a banda permite.
pub fn granted_fps(
    requested_fps: f32,
    frame_bytes: u64,
    throughput_for_raw_bytes_per_sec: f64,
) -> f32 {
    let requested = requested_fps.clamp(RAW_SEQUENCE_MIN_FPS, RAW_SEQUENCE_MAX_FPS);
    effective_raw_fps(frame_bytes, throughput_for_raw_bytes_per_sec).min(requested)
}

// ---------------------------------------------------------------------------
// Job da sessão (T059): roteia frames recebidos pro consumidor certo
// ---------------------------------------------------------------------------

/// Job RAW ativo de uma sessão (no máximo 1 — invariante do
/// `data-model.md`). Vive num slot `Option<RawJobRuntime>` compartilhado
/// entre o comando que o inicia e o loop de eventos da sessão (`lib.rs`) que
/// entrega os frames conforme chegam do fork.
pub struct RawJobRuntime {
    pub job: RawCaptureJob,
    /// `Some` só durante um Snapshot pendente: o loop de eventos entrega o
    /// frame aqui (em vez de gravar ele mesmo) e o job se encerra sozinho.
    /// Sempre `None` numa Sequência — essa grava direto em
    /// `handle_incoming_frame`.
    pub snapshot_tx: Option<tokio::sync::oneshot::Sender<RawFrame>>,
}

impl RawJobRuntime {
    pub fn snapshot(output_dir: PathBuf, tx: tokio::sync::oneshot::Sender<RawFrame>) -> Self {
        RawJobRuntime {
            job: RawCaptureJob {
                kind: RawJobKind::Snapshot,
                output_dir,
                state: RawJobState::Running {
                    frames: 0,
                    bytes: 0,
                    effective_fps: 0.0,
                },
            },
            snapshot_tx: Some(tx),
        }
    }

    pub fn sequence(output_dir: PathBuf, granted_fps: f32) -> Self {
        RawJobRuntime {
            job: RawCaptureJob {
                kind: RawJobKind::Sequence {
                    target_fps: granted_fps,
                },
                output_dir,
                state: RawJobState::Running {
                    frames: 0,
                    bytes: 0,
                    effective_fps: granted_fps,
                },
            },
            snapshot_tx: None,
        }
    }
}

/// Processa um `RawFrame` recebido contra o job ativo da sessão (`slot`).
/// Sem job ativo (corrida rara entre `raw_sequence_stop`/fim do snapshot e
/// um frame que já estava a caminho): descarta, devolve `None` — nunca
/// panica por frame "órfão".
///
/// - Snapshot: entrega o frame a quem está esperando (`snapshot_tx`) e limpa
///   o slot — o job dura exatamente um frame.
/// - Sequência: grava em disco (`write_frame`) e acumula `frames`/`bytes`
///   em `job.state`; falha de gravação encerra o job com `RawJobState::Failed`.
///
/// Devolve uma cópia do job (pra quem chamou emitir `raw_progress`) sempre
/// que o estado mudou — inclusive quando o job acabou de ser encerrado (o
/// consumidor do evento vê o estado final `Done`/`Failed` antes do slot
/// virar `None`).
pub fn handle_incoming_frame(
    slot: &mut Option<RawJobRuntime>,
    frame: RawFrame,
) -> Option<RawCaptureJob> {
    let runtime = slot.as_mut()?;
    match runtime.job.kind {
        RawJobKind::Snapshot => {
            if let Some(tx) = runtime.snapshot_tx.take() {
                let _ = tx.send(frame);
            }
            runtime.job.state = RawJobState::Done;
            let job = runtime.job.clone();
            *slot = None;
            Some(job)
        }
        RawJobKind::Sequence { .. } => {
            let dng_bytes = frame.dng.len() as u64;
            match write_frame(&runtime.job.output_dir, &frame) {
                Ok(_) => {
                    if let RawJobState::Running { frames, bytes, .. } = &mut runtime.job.state {
                        *frames += 1;
                        *bytes += dng_bytes;
                    }
                    Some(runtime.job.clone())
                }
                Err(e) => {
                    runtime.job.state = RawJobState::Failed(e.to_string());
                    let job = runtime.job.clone();
                    *slot = None;
                    Some(job)
                }
            }
        }
    }
}

/// Encerra uma Sequência em andamento (`raw_sequence_stop`): marca `Done` e
/// limpa o slot. No-op (devolve `None`) se não houver job ativo.
pub fn stop_sequence(slot: &mut Option<RawJobRuntime>) -> Option<RawCaptureJob> {
    let mut runtime = slot.take()?;
    runtime.job.state = RawJobState::Done;
    Some(runtime.job)
}
