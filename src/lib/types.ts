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
