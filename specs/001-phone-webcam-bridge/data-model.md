# Data Model — CamLink (001-phone-webcam-bridge)

Entidades derivadas da spec (Key Entities) e refinadas pelo desenho do plan.md.
Todas vivem no backend Rust (`src-tauri`); o frontend recebe projeções via IPC.

## AndroidDevice (Fonte Android)

| Campo | Tipo | Notas |
|---|---|---|
| `serial` | String | Identidade única (chave; vem do adb) |
| `model` | String | Nome/modelo exibido |
| `auth_state` | enum `Authorized \| Unauthorized \| Offline` | `Unauthorized` dispara fluxo guiado (FR-002) |
| `compatible` | bool + `incompat_reason: Option<String>` | Android < 12 → incompatível com explicação (FR-002a) |
| `capabilities` | `DeviceCapabilities?` | Preenchido via `get_capabilities` quando o stream sobe |

### DeviceCapabilities

| Campo | Tipo |
|---|---|
| `cameras` | lista de `{ id, facing: Front\|Back, max_resolution, fps_ranges }` |
| `zoom_range` | `(f32, f32)` |
| `iso_range` | `Option<(u32, u32)>` (ausente = ISO manual não suportado) |
| `exposure_comp_range` | `(i32, i32)` em passos de EV |
| `wb_modes` | lista de enum (`Auto, Daylight, Cloudy, Fluorescent, Incandescent, ...`) |
| `supports_eis` | bool |
| `supports_torch` | bool |
| `raw` | `Option<{ sensor_size: (u32,u32), frame_bytes: u64 }>` (ausente → UI oculta RAW, FR-016/019) |

Regra: todo controle da UI é habilitado/exibido estritamente a partir deste
struct — nunca por suposição de modelo de aparelho.

## RtspSource (Fonte IP/RTSP)

| Campo | Tipo | Notas |
|---|---|---|
| `id` | UUID | Chave local |
| `name` | String | Rótulo do usuário |
| `url` | String **sem credenciais** | Persistida em config |
| `secret_ref` | Option<String> | Chave no cofre do SO (keyring) — FR-018a; nunca senha em claro |
| `state` | enum `Idle \| Connecting \| Streaming \| Error(String) \| Reconnecting` | |

## VirtualCamera (Dispositivo virtual)

| Campo | Tipo | Notas |
|---|---|---|
| `id` | UUID | |
| `label` | String | ex.: "CamLink Android" (visível nos apps consumidores) |
| `backend_path` | String | Linux: `/dev/videoN`; Windows: id DirectShow |
| `state` | enum `Live \| Standby` | `Standby` = exibindo imagem de espera (FR-006) |

Invariante: 1 fonte ativa ↔ 1 dispositivo virtual (FR-021). Criado ao ativar a
fonte, destruído (ou posto em `Standby`) ao encerrar; nunca deixado em estado que
congele apps consumidores.

## StreamSession (Sessão de transmissão)

| Campo | Tipo | Notas |
|---|---|---|
| `source` | `AndroidDevice.serial \| RtspSource.id` | |
| `virtual_camera` | `VirtualCamera.id` | |
| `config` | `StreamConfig` | resolução, fps, bitrate, codec (H264\|H265), camera_id |
| `state` | enum (ver transições) | |
| `stats` | `{ fps_atual, uptime, reconexões }` | alimenta indicadores de status (FR-010 análogo) |

### Transições de estado

```
Idle → Starting → Streaming → Stopping → Idle
                Streaming → SourceLost → Reconnecting → Streaming
                Reconnecting → (timeout/cancel) → Idle
qualquer → Error(msg acionável) → Idle
```

`SourceLost`/`Reconnecting` mantêm a VirtualCamera em `Standby` com imagem de
espera (FR-006); retomada automática ao religar cabo/stream.

## ControlState (Estado de controles — por sessão Android)

| Campo | Tipo | Default |
|---|---|---|
| `mode` | enum `Auto \| Night \| Sport \| Pro` | `Auto` |
| `zoom_ratio` | f32 | 1.0 |
| `focus` | enum `ContinuousAuto \| Tap{x,y} \| Manual{distance}` | `ContinuousAuto` |
| `exposure_comp` | i32 (validado contra range) | 0 |
| `manual_exposure` | Option<{iso, exposure_time}> | None (só modo Pro) |
| `wb_mode` | enum | `Auto` |
| `eis` | bool | conforme modo |
| `torch` | bool | false |

Regras: valores sempre validados contra `DeviceCapabilities` **no servidor**
(rejeição com erro claro) e refletidos na UI; trocar de `mode` sobrescreve os
campos que o modo define (tabela em research.md R2); `Pro` libera tudo.

## SmartMode (perfil pré-configurado)

Estático (não persistido): tabela Camera2 por modo — ver research.md R2 e
`contracts/control-protocol.md`. Não é editável pelo usuário na v1.

## RawCaptureJob (Captura RAW)

| Campo | Tipo | Notas |
|---|---|---|
| `kind` | enum `Snapshot \| Sequence{target_fps}` | fps alvo 1–3 |
| `output_dir` | PathBuf | configurável; default no dir de imagens do usuário |
| `state` | `Running{frames, bytes, effective_fps} \| Done \| Failed(msg)` | progresso na UI |
| `effective_fps` | f32 | recalculado por throughput medido (FR-019/020) |

Invariante: no máximo 1 job por sessão; o stream principal tem prioridade de
banda (FR-020).

## AppConfig (persistência local)

| Campo | Notas |
|---|---|
| `rtsp_sources` | lista de RtspSource (sem segredos) |
| `last_stream_config` | por serial de dispositivo |
| `raw_output_dir` | |
| `auto_connect` | serials com auto-start ao reconectar |

Formato TOML no diretório de config da plataforma. Nenhuma credencial neste
arquivo (FR-018a); nenhum dado sai da máquina (FR-026).
