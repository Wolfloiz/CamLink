// Espelha os tipos serde de src-tauri/src/model.rs relevantes à US1
// (contracts/tauri-commands.md). Mantido manualmente em sincronia — sem
// geração automática de bindings nesta fase.

export type AuthState = "authorized" | "unauthorized" | "offline";

export interface AndroidDevice {
  serial: string;
  model: string;
  auth_state: AuthState;
  compatible: boolean;
  incompat_reason: string | null;
  capabilities: unknown | null;
}

export type VideoCodec = "h264" | "h265";

export interface StreamConfig {
  resolution: [number, number];
  fps: number;
  bitrate: number;
  codec: VideoCodec;
  camera_id: string;
}

export type SessionState =
  | "idle"
  | "starting"
  | "streaming"
  | "stopping"
  | "source_lost"
  | "reconnecting"
  | { error: string };

export interface SessionStats {
  fps: number;
  uptime_secs: number;
  reconnects: number;
}

export type SessionSource = { android: string } | { rtsp: string };

export interface VirtualCamera {
  id: string;
  label: string;
  backend_path: string;
  state: "live" | "standby";
}

export interface StreamSession {
  source: SessionSource;
  virtual_camera: string;
  config: StreamConfig;
  state: SessionState;
  stats: SessionStats;
}

// Resposta de `start_stream` (src-tauri/src/lib.rs `StartStreamResponse`):
// `StreamSession` não carrega o próprio id, então o backend devolve
// `session_id` e a `VirtualCamera` completa separadamente.
export interface StartStreamResponse {
  session_id: string;
  virtual_camera: VirtualCamera;
  config: StreamConfig;
  state: SessionState;
  stats: SessionStats;
}

export interface AppError {
  code: string;
  msg: string;
  action_hint: string | null;
}

// --- US2: controles de câmera (contracts/tauri-commands.md, model.rs) ---

export type CameraFacing = "front" | "back";

export interface CameraInfo {
  id: string;
  facing: CameraFacing;
  max_resolution: [number, number];
  fps_ranges: Array<[number, number]>;
}

export type WbMode =
  | "auto"
  | "daylight"
  | "cloudy"
  | "fluorescent"
  | "incandescent";

export interface DeviceCapabilities {
  cameras: CameraInfo[];
  zoom_range: [number, number];
  iso_range: [number, number] | null;
  exposure_comp_range: [number, number];
  wb_modes: WbMode[];
  supports_eis: boolean;
  supports_torch: boolean;
  raw: { sensor_size: [number, number]; frame_bytes: number } | null;
}

export type FocusMode =
  | "continuous_auto"
  | { tap: { x: number; y: number } }
  | { manual: { distance: number } };

export type Rotation = "deg0" | "deg90" | "deg180" | "deg270";

export type SmartMode = "auto" | "night" | "sport" | "pro";

export interface ControlState {
  mode: SmartMode;
  zoom_ratio: number;
  focus: FocusMode;
  exposure_comp: number;
  manual_exposure: { iso: number; exposure_time_ns: number } | null;
  wb_mode: WbMode;
  eis: boolean;
  torch: boolean;
  rotation: Rotation;
  mirror: boolean;
}

/** Enum serde externally-tagged de `ControlChange` (lib.rs). */
export type ControlChange =
  | { zoom: number }
  | { focus: FocusMode }
  | { exposure_comp: number }
  | { iso: number }
  | { wb: WbMode }
  | { eis: boolean }
  | { torch: boolean }
  | { rotation: Rotation }
  | { mirror: boolean };

export interface ControlStateEvent {
  session_id: string;
  control_state: ControlState;
}

export interface AfStateEvent {
  session_id: string;
  state: "focused" | "searching" | "failed" | string;
}

/** Restart interno (rotação 90°/270°) trocou a sessão de lugar. */
export interface SessionReplacedEvent {
  old_session_id: string;
  response: StartStreamResponse;
}

// --- US4: fontes RTSP (model.rs RtspSource) ---

export type RtspState =
  | "idle"
  | "connecting"
  | "streaming"
  | "reconnecting"
  | { error: string };

export interface RtspSource {
  id: string;
  name: string;
  url: string;
  secret_ref: string | null;
  state: RtspState;
}

export interface SessionStateEvent {
  session_id: string;
  state: SessionState;
  stats: SessionStats;
}

export interface PreviewFrameEvent {
  session_id: string;
  jpeg_base64: string;
}

export function isSessionError(
  state: SessionState,
): state is { error: string } {
  return typeof state === "object" && state !== null && "error" in state;
}

export function isSessionActive(state: SessionState): boolean {
  return (
    state === "streaming" ||
    state === "source_lost" ||
    state === "reconnecting"
  );
}

export const DEFAULT_STREAM_CONFIG: Omit<StreamConfig, "camera_id"> = {
  resolution: [1920, 1080],
  fps: 30,
  bitrate: 8_000_000,
  codec: "h264",
};
