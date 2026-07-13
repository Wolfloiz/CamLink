// Wrappers tipados sobre os comandos/eventos Tauri de US1
// (contracts/tauri-commands.md, src-tauri/src/lib.rs).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AndroidDevice,
  PreviewFrameEvent,
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
