# Contract: Comandos e Eventos Tauri (frontend Svelte ↔ backend Rust)

IPC do Tauri 2.x: `invoke` (request/response) e `emit` (eventos). Payloads
serde-serializados; erros como `Result<T, AppError>` com `code` + `msg`
acionável (FR-010).

## Comandos (invoke)

| Comando | Args | Retorno | FR |
|---|---|---|---|
| `list_devices` | — | `AndroidDevice[]` | FR-001 |
| `start_stream` | `serial, StreamConfig` | `StreamSession` (inclui `VirtualCamera`) | FR-003 |
| `stop_stream` | `session_id` | `()` | |
| `switch_camera` | `session_id, camera_id` | `StreamSession` (reinício ≤ 2 s) | FR-015 |
| `get_capabilities` | `serial` | `DeviceCapabilities` | FR-016 |
| `set_mode` | `session_id, mode` | `ControlState` | FR-017 |
| `set_control` | `session_id, ControlChange` | `ControlState` | FR-008..014 |
| `add_rtsp_source` | `name, url, password?` | `RtspSource` (senha → keyring) | FR-018/018a |
| `remove_rtsp_source` | `id` | `()` (remove segredo do cofre) | FR-018a |
| `start_rtsp` / `stop_rtsp` | `id` | `StreamSession` / `()` | FR-018 |
| `raw_snapshot` | `session_id` | `path: String` | FR-019 |
| `raw_sequence_start` | `session_id, fps` | `RawCaptureJob` | FR-019 |
| `raw_sequence_stop` | `session_id` | `RawCaptureJob` (final) | |
| `set_raw_output_dir` | `path` | `()` | |
| `get_config` / `set_config` | — / `AppConfig` (sem segredos) | `AppConfig` | FR-026 |

`ControlChange` = enum serde: `Zoom(f32) | Focus(FocusMode) | ExposureComp(i32) |
Iso(u32) | Wb(WbMode) | Eis(bool) | Torch(bool)`.

## Eventos (emit, backend → frontend)

| Evento | Payload | Gatilho |
|---|---|---|
| `device_connected` / `device_disconnected` | `AndroidDevice` / `serial` | hotplug ADB (≤ 3 s, FR-001) |
| `device_unauthorized` | `serial` | guia de autorização (FR-002) |
| `session_state` | `{ session_id, state, stats }` | toda transição (FR-010) |
| `preview_frame` | `{ session_id, jpeg_base64 }` | 1 fps (FR-023) |
| `control_state` | `ControlState` | após aplicar controle/modo |
| `af_state` / `faces` | repassados do protocolo de controle | tap-to-focus, face-AF |
| `raw_progress` | `RawCaptureJob` | frames/bytes/fps efetivo |
| `error` | `AppError { code, msg, action_hint }` | falhas (firewall, secure boot, versão scrcpy, etc.) |

## Regras

- Nenhum comando bloqueia > 200 ms; operações longas retornam imediatamente e
  progridem por eventos (`session_state`).
- Controles indisponíveis: o frontend deriva visibilidade/enabled exclusivamente
  de `DeviceCapabilities` (FR-016) — nunca hardcode por modelo.
- `preview_frame` é descartável: se o frontend atrasar, frames são pulados (o
  stream principal nunca espera o preview — FR-023).
- Toda mensagem de erro carrega `action_hint` legível (ex.: Secure Boot → link
  do guia de assinatura do módulo).
