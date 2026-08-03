//! Tipos de domínio (data-model.md): AndroidDevice, DeviceCapabilities,
//! RtspSource, VirtualCamera, StreamSession (máquina de estados),
//! ControlState, RawCaptureJob, AppConfig.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// AndroidDevice
// ---------------------------------------------------------------------------

/// Estado de autorização ADB do dispositivo (FR-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Authorized,
    Unauthorized,
    Offline,
}

/// Fonte Android detectada via adb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AndroidDevice {
    /// Identidade única (vem do adb).
    pub serial: String,
    pub model: String,
    pub auth_state: AuthState,
    /// Android < 12 → incompatível, com explicação (FR-002a).
    pub compatible: bool,
    pub incompat_reason: Option<String>,
    /// Preenchido via `get_capabilities` quando o stream sobe.
    pub capabilities: Option<DeviceCapabilities>,
}

// ---------------------------------------------------------------------------
// DeviceCapabilities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraFacing {
    Front,
    Back,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraInfo {
    pub id: String,
    pub facing: CameraFacing,
    pub max_resolution: (u32, u32),
    pub fps_ranges: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbMode {
    Auto,
    Daylight,
    Cloudy,
    Fluorescent,
    Incandescent,
}

/// Capacidade RAW do sensor; ausente → UI oculta RAW (FR-016/019).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawInfo {
    pub sensor_size: (u32, u32),
    pub frame_bytes: u64,
}

/// Capacidades reportadas pelo fork; todo controle da UI é habilitado
/// estritamente a partir deste struct (FR-016).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub cameras: Vec<CameraInfo>,
    pub zoom_range: (f32, f32),
    /// Ausente = ISO manual não suportado.
    pub iso_range: Option<(u32, u32)>,
    /// Em passos de EV.
    pub exposure_comp_range: (i32, i32),
    pub wb_modes: Vec<WbMode>,
    pub supports_eis: bool,
    pub supports_torch: bool,
    pub raw: Option<RawInfo>,
}

// ---------------------------------------------------------------------------
// RtspSource
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtspState {
    Idle,
    Connecting,
    Streaming,
    Error(String),
    Reconnecting,
}

/// Fonte IP/RTSP. A URL nunca contém credenciais; a senha vive no cofre do
/// SO, referenciada por `secret_ref` (FR-018a).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtspSource {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub secret_ref: Option<String>,
    pub state: RtspState,
}

// ---------------------------------------------------------------------------
// VirtualCamera
// ---------------------------------------------------------------------------

/// `Standby` = exibindo imagem de espera (FR-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualCameraState {
    Live,
    Standby,
}

/// Dispositivo de webcam virtual. Invariante: 1 fonte ativa ↔ 1 device
/// (FR-021).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VirtualCamera {
    pub id: Uuid,
    /// Visível nos apps consumidores (ex.: "CamLink Android").
    pub label: String,
    /// Linux: `/dev/videoN`; Windows: id DirectShow.
    pub backend_path: String,
    pub state: VirtualCameraState,
}

// ---------------------------------------------------------------------------
// StreamSession
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    H265,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamConfig {
    pub resolution: (u32, u32),
    pub fps: u32,
    pub bitrate: u32,
    pub codec: VideoCodec,
    pub camera_id: String,
}

/// Origem da sessão: serial do Android ou id da fonte RTSP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    Android(String),
    Rtsp(Uuid),
}

/// Estados da sessão (data-model.md — Transições):
///
/// ```text
/// Idle → Starting → Streaming → Stopping → Idle
///                 Streaming → SourceLost → Reconnecting → Streaming
///                 Reconnecting → (timeout/cancel) → Idle
/// qualquer → Error(msg acionável) → Idle
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Starting,
    Streaming,
    Stopping,
    SourceLost,
    Reconnecting,
    Error(String),
}

/// Eventos que dirigem a máquina de estados de `StreamSession`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEvent {
    Start,
    Started,
    Stop,
    Stopped,
    SourceLost,
    ReconnectStarted,
    Recovered,
    ReconnectTimeout,
    Cancel,
    Fail(String),
    Reset,
}

/// Transição rejeitada: o par (estado, evento) não consta do diagrama.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("transição de estado inválida: {from:?} + {event:?}")]
pub struct InvalidTransition {
    pub from: SessionState,
    pub event: SessionEvent,
}

impl SessionState {
    /// Aplica `event` e devolve o próximo estado, ou `InvalidTransition` se o
    /// par não consta do diagrama de transições do data-model.md.
    pub fn transition(self, event: SessionEvent) -> Result<SessionState, InvalidTransition> {
        use SessionEvent as E;
        use SessionState as S;
        match (self, event) {
            (S::Idle, E::Start) => Ok(S::Starting),
            (S::Starting, E::Started) => Ok(S::Streaming),
            (S::Streaming, E::Stop) => Ok(S::Stopping),
            (S::Stopping, E::Stopped) => Ok(S::Idle),
            (S::Streaming, E::SourceLost) => Ok(S::SourceLost),
            (S::SourceLost, E::ReconnectStarted) => Ok(S::Reconnecting),
            (S::Reconnecting, E::Recovered) => Ok(S::Streaming),
            (S::Reconnecting, E::ReconnectTimeout | E::Cancel) => Ok(S::Idle),
            (S::Error(_), E::Reset) => Ok(S::Idle),
            // Qualquer estado pode falhar com mensagem acionável (FR-010).
            (_, E::Fail(msg)) => Ok(S::Error(msg)),
            (from, event) => Err(InvalidTransition { from, event }),
        }
    }
}

/// Indicadores de status da sessão (FR-010).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionStats {
    pub fps: f32,
    pub uptime_secs: u64,
    pub reconnects: u32,
}

/// Sessão de transmissão: 1 fonte → 1 câmera virtual.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamSession {
    pub source: SessionSource,
    pub virtual_camera: Uuid,
    pub config: StreamConfig,
    pub state: SessionState,
    pub stats: SessionStats,
}

impl StreamSession {
    /// Aplica `event` à sessão; em transição inválida o estado atual é
    /// preservado.
    pub fn apply(&mut self, event: SessionEvent) -> Result<(), InvalidTransition> {
        self.state = self.state.clone().transition(event)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ControlState
// ---------------------------------------------------------------------------

/// Modos inteligentes (research R2); tabela de presets vive no fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartMode {
    Auto,
    Night,
    Sport,
    Pro,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusMode {
    ContinuousAuto,
    /// Coordenadas normalizadas (0.0–1.0) do tap no preview.
    Tap {
        x: f32,
        y: f32,
    },
    /// Distância de foco em dioptrias (`LENS_FOCUS_DISTANCE`).
    Manual {
        distance: f32,
    },
}

/// Exposição manual (só modo Pro): AE off + `SENSOR_SENSITIVITY`/`EXPOSURE_TIME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualExposure {
    pub iso: u32,
    pub exposure_time_ns: u64,
}

/// Rotação do vídeo entregue (FR-016a), aplicada no desktop — não no celular.
/// 90°/270° trocam width↔height (exigem recriar a câmera virtual, caminho de
/// restart ≤ 2 s); 0°/180° preservam as dimensões (aplicáveis ao vivo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    /// `true` quando a rotação troca largura↔altura do frame.
    pub fn swaps_dimensions(self) -> bool {
        matches!(self, Rotation::Deg90 | Rotation::Deg270)
    }
}

/// Estado de controles por sessão Android; valores sempre validados contra
/// `DeviceCapabilities` no servidor (data-model.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlState {
    pub mode: SmartMode,
    pub zoom_ratio: f32,
    pub focus: FocusMode,
    pub exposure_comp: i32,
    pub manual_exposure: Option<ManualExposure>,
    pub wb_mode: WbMode,
    pub eis: bool,
    pub torch: bool,
    /// Orientação do vídeo entregue (FR-016a) — transformação local, fora do
    /// protocolo do fork. `#[serde(default)]` mantém configs antigas válidas.
    #[serde(default)]
    pub rotation: Rotation,
    #[serde(default)]
    pub mirror: bool,
}

impl ControlState {
    /// Aplica ao `ControlState` local os únicos 2 campos que a tabela de
    /// modos (research.md R2 / control-protocol.md §3) sobrescreve e que têm
    /// representação dedicada aqui (`eis`, `exposure_comp`) — o resto da
    /// tabela (AF/AE/FPS/AWB/NR) não tem campo próprio na UI
    /// (data-model.md). `pro` deixa `exposure_comp` livre (não fixado pela
    /// tabela) mas AINDA fixa `eis=false` (`VIDEO_STABILIZATION_MODE_OFF` não
    /// é "livre" pra pro, só fps/EV/AWB são).
    pub fn apply_smart_mode(&mut self, mode: SmartMode) {
        match mode {
            SmartMode::Auto => {
                self.eis = true;
                self.exposure_comp = 0;
            }
            SmartMode::Night => {
                self.eis = true;
                self.exposure_comp = 1;
            }
            SmartMode::Sport => {
                self.eis = false;
                self.exposure_comp = 0;
            }
            SmartMode::Pro => {
                self.eis = false;
            }
        }
        self.mode = mode;
    }
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            mode: SmartMode::Auto,
            zoom_ratio: 1.0,
            focus: FocusMode::ContinuousAuto,
            exposure_comp: 0,
            manual_exposure: None,
            wb_mode: WbMode::Auto,
            eis: true,
            torch: false,
            rotation: Rotation::Deg0,
            mirror: false,
        }
    }
}

// ---------------------------------------------------------------------------
// RawCaptureJob
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawJobKind {
    Snapshot,
    /// Sequência com fps alvo 1–3, ajustado por throughput medido (FR-020).
    Sequence {
        target_fps: f32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawJobState {
    Running {
        frames: u32,
        bytes: u64,
        effective_fps: f32,
    },
    Done,
    Failed(String),
}

/// Captura RAW/DNG; no máximo 1 job por sessão, stream principal tem
/// prioridade de banda (FR-020).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawCaptureJob {
    pub kind: RawJobKind,
    pub output_dir: PathBuf,
    pub state: RawJobState,
}

// ---------------------------------------------------------------------------
// AppConfig
// ---------------------------------------------------------------------------

/// Persistência local em TOML; nenhuma credencial neste arquivo (FR-018a) e
/// nenhum dado sai da máquina (FR-026).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub rtsp_sources: Vec<RtspSource>,
    /// Último `StreamConfig` usado, por serial de dispositivo.
    pub last_stream_config: HashMap<String, StreamConfig>,
    pub raw_output_dir: Option<PathBuf>,
    /// Serials com auto-start ao reconectar.
    pub auto_connect: Vec<String>,
}
