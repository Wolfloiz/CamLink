// Wrappers tipados sobre os comandos/eventos Tauri de US1
// (contracts/tauri-commands.md, src-tauri/src/lib.rs).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AfStateEvent,
  AndroidDevice,
  ControlChange,
  ControlState,
  ControlStateEvent,
  DeviceCapabilities,
  PreviewFrameEvent,
  RawProgressEvent,
  RtspSource,
  SessionReplacedEvent,
  SessionStateEvent,
  StartStreamResponse,
  StreamConfig,
} from "./types";

export function listDevices(): Promise<AndroidDevice[]> {
  return invoke("list_devices");
}

export function startStream(
  serial: string,
  config: StreamConfig,
): Promise<StartStreamResponse> {
  return invoke("start_stream", { serial, config });
}

export function stopStream(sessionId: string): Promise<void> {
  return invoke("stop_stream", { sessionId });
}

// --- T065e: apelido de dispositivo Android (o "model" do adb costuma ser só
// o nome de código comercial, ex. "SM-S921B", não "Galaxy S24") ---

export function setDeviceNickname(
  serial: string,
  nickname: string,
): Promise<void> {
  return invoke("set_device_nickname", { serial, nickname });
}

export function listDeviceNicknames(): Promise<Record<string, string>> {
  return invoke("list_device_nicknames");
}

export function onDeviceConnected(
  handler: (device: AndroidDevice) => void,
): Promise<UnlistenFn> {
  return listen<AndroidDevice>("device_connected", (e) => handler(e.payload));
}

export function onDeviceDisconnected(
  handler: (serial: string) => void,
): Promise<UnlistenFn> {
  return listen<string>("device_disconnected", (e) => handler(e.payload));
}

export function onDeviceUnauthorized(
  handler: (serial: string) => void,
): Promise<UnlistenFn> {
  return listen<string>("device_unauthorized", (e) => handler(e.payload));
}

export function onSessionState(
  handler: (event: SessionStateEvent) => void,
): Promise<UnlistenFn> {
  return listen<SessionStateEvent>("session_state", (e) => handler(e.payload));
}

export function onPreviewFrame(
  handler: (event: PreviewFrameEvent) => void,
): Promise<UnlistenFn> {
  return listen<PreviewFrameEvent>("preview_frame", (e) => handler(e.payload));
}

// --- US2: controles de câmera ---

export function getCapabilities(serial: string): Promise<DeviceCapabilities> {
  return invoke("get_capabilities", { serial });
}

export function setControl(
  sessionId: string,
  change: ControlChange,
): Promise<ControlState> {
  return invoke("set_control", { sessionId, change });
}

export function switchCamera(
  sessionId: string,
  cameraId: string,
): Promise<StartStreamResponse> {
  return invoke("switch_camera", { sessionId, cameraId });
}

export function onControlState(
  handler: (event: ControlStateEvent) => void,
): Promise<UnlistenFn> {
  return listen<ControlStateEvent>("control_state", (e) => handler(e.payload));
}

export function onAfState(
  handler: (event: AfStateEvent) => void,
): Promise<UnlistenFn> {
  return listen<AfStateEvent>("af_state", (e) => handler(e.payload));
}

export function onSessionReplaced(
  handler: (event: SessionReplacedEvent) => void,
): Promise<UnlistenFn> {
  return listen<SessionReplacedEvent>("session_replaced", (e) =>
    handler(e.payload),
  );
}

// --- US4: fontes RTSP ---

export function listRtspSources(): Promise<RtspSource[]> {
  return invoke("list_rtsp_sources");
}

export function addRtspSource(
  name: string,
  url: string,
  password: string | null,
): Promise<RtspSource> {
  return invoke("add_rtsp_source", { name, url, password });
}

export function removeRtspSource(id: string): Promise<void> {
  return invoke("remove_rtsp_source", { id });
}

export function startRtsp(id: string): Promise<StartStreamResponse> {
  return invoke("start_rtsp", { id });
}

export function stopRtsp(id: string): Promise<void> {
  return invoke("stop_rtsp", { id });
}

// --- US5: Captura RAW ---

export function rawSnapshot(sessionId: string): Promise<string> {
  return invoke("raw_snapshot", { sessionId });
}

export function rawSequenceStart(
  sessionId: string,
  fps: number,
): Promise<number> {
  return invoke("raw_sequence_start", { sessionId, fps });
}

export function rawSequenceStop(sessionId: string): Promise<void> {
  return invoke("raw_sequence_stop", { sessionId });
}

export function setRawOutputDir(
  sessionId: string,
  dir: string,
): Promise<void> {
  return invoke("set_raw_output_dir", { sessionId, dir });
}

export function onRawProgress(
  handler: (event: RawProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<RawProgressEvent>("raw_progress", (e) => handler(e.payload));
}
