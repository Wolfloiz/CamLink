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
