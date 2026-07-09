# Contract: Protocolo de Controle CamLink (desktop ↔ fork scrcpy-server)

Transporte: socket `localabstract:camlink` no Android, exposto ao PC via
`adb forward tcp:<PORT> localabstract:camlink`. Mensagens JSON, uma por linha
(NDJSON), UTF-8. Respostas na mesma ordem das requisições. Frames RAW são a
única exceção: binário length-prefixed (ver §4).

Versionamento: `{"cmd":"hello"}` → `{"ok":true,"protocol":1,"server":"camlink-<scrcpy_tag>"}`.
Cliente Rust rejeita `protocol` desconhecido com erro descritivo (versão de fork
≠ versão de cliente).

## 1. Envelope de resposta

```json
{ "ok": true,  "data": { ... } }
{ "ok": false, "error": { "code": "OUT_OF_RANGE", "msg": "iso 12800 fora de [100, 6400]" } }
```

Códigos: `OUT_OF_RANGE`, `UNSUPPORTED` (capability ausente), `BAD_REQUEST`
(JSON/campos inválidos), `CAMERA_ERROR` (falha Camera2), `BUSY` (job RAW já
ativo).

## 2. Descoberta

```json
{ "cmd": "get_capabilities" }
```

→ `data` = objeto `DeviceCapabilities` (ver data-model.md): `cameras[]`,
`zoom_range`, `iso_range?`, `exposure_comp_range`, `wb_modes[]`,
`supports_eis`, `supports_torch`, `raw?{sensor_size, frame_bytes}`.

## 3. Controles (aplicados via setRepeatingRequest, sem reabrir a câmera)

```json
{ "cmd": "set_mode",     "mode": "auto" | "night" | "sport" | "pro" }
{ "cmd": "set_zoom",     "ratio": 2.5 }
{ "cmd": "set_focus",    "mode": "continuous" }
{ "cmd": "set_focus",    "mode": "tap", "x": 0.5, "y": 0.3 }        // normalizado [0,1]
{ "cmd": "set_focus",    "mode": "manual", "distance": 0.5 }        // dioptrias norm.
{ "cmd": "set_exposure", "compensation": -2 }                       // passos EV
{ "cmd": "set_iso",      "value": 400 }                             // exige modo pro
{ "cmd": "set_wb",       "mode": "cloudy" }
{ "cmd": "set_eis",      "enabled": true }
{ "cmd": "set_torch",    "enabled": true }
```

Regras contratuais:
- Servidor valida todo valor contra as capabilities **antes** de aplicar
  (`OUT_OF_RANGE`/`UNSUPPORTED`).
- `set_iso` fora do modo `pro` → `BAD_REQUEST` (AE precisa estar OFF).
- Efeito visível no stream em < 1 s (SC-004) — sem reinício de sessão.
- **Troca frontal/traseira NÃO é comando de socket**: `--camera-id` é fixado na
  inicialização do scrcpy; o desktop reinicia o subprocess (interrupção ≤ 2 s,
  FR-015).

### Tabela de modos (o que set_mode aplica)

| Parâmetro Camera2 | auto | night | sport | pro |
|---|---|---|---|---|
| CONTROL_AF_MODE | CONTINUOUS_VIDEO | CONTINUOUS_VIDEO | CONTINUOUS_VIDEO | OFF |
| CONTROL_AE_MODE | ON | ON | ON | OFF |
| AE_TARGET_FPS_RANGE | [30,30] | [15,30] | [60,60] | livre |
| AE_EXPOSURE_COMPENSATION | 0 | +1 | 0 | livre |
| CONTROL_AWB_MODE | AUTO | AUTO | AUTO | livre |
| VIDEO_STABILIZATION | ON | ON | OFF | OFF |
| NOISE_REDUCTION_MODE | FAST | HIGH_QUALITY | FAST | OFF |
| Face-AF automático | sim | sim | não | não |

## 4. Captura RAW

```json
{ "cmd": "raw_snapshot" }
{ "cmd": "raw_sequence_start", "fps": 3 }
{ "cmd": "raw_sequence_stop" }
```

Após `ok`, cada frame chega como:

```
[u8 tag=0xD1][u32be metadata_len][metadata JSON][u64be dng_len][bytes DNG]
```

`metadata` = `{ "seq": n, "timestamp_ms": ..., "width": ..., "height": ... }`.
Nunca base64. `raw_sequence_start` com fps acima do sustentável → servidor
responde `ok` com `{"granted_fps": <ajustado>}` e recalcula dinamicamente
(prioridade do stream principal — FR-020). Sem capability RAW → `UNSUPPORTED`.

## 5. Eventos assíncronos (servidor → desktop)

```json
{ "event": "af_state",  "state": "focused" | "searching" | "failed" }
{ "event": "faces",     "rects": [{ "x":0.4,"y":0.2,"w":0.1,"h":0.15 }] }
{ "event": "raw_frame_dropped", "reason": "bandwidth" }
```

Eventos são linhas JSON com chave `event` (nunca `ok`), intercaladas com
respostas; o cliente Rust demultiplexa por chave.

## 6. Golden files

`contracts/golden/` (criado nas tasks) conterá pares request/response canônicos
usados pelos testes de contrato dos DOIS lados (Rust `cargo test` e JUnit no
fork) — mesma fonte de verdade, sem divergência de protocolo.
