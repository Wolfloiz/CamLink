# CamLink

App desktop (Linux + Windows 10/11) que transforma câmeras Android (USB/ADB, sem
app no celular) e fontes IP/RTSP em webcams virtuais (OBS, Chrome, Firefox,
Discord). GPL-3.0.

## Stack (pinada — constituição v2.0.x + plan da feature 001)

- **Rust stable** (rust-toolchain.toml) — backend em `src-tauri/`
- **Tauri 2.x + SvelteKit** — GUI; bundling .deb/AppImage/NSIS/MSI
- **Java 17** — somente no fork do scrcpy-server (submodule `scrcpy/`, branch
  `camlink`; roda no Android)
- Runtime: adb, scrcpy ≥ 4.0, ffmpeg, v4l2loopback ≥ 0.13 (Linux), filtro
  DirectShow próprio via `windows-rs` (Windows — akvirtualcamera reprovado no
  Spike B, ver research.md R4)
- Crates-chave: tokio, serde, tracing, keyring, thiserror

## Regras do projeto (constituição `.specify/memory/constitution.md`)

- TDD obrigatório; gates: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`
- Paridade Linux+Windows em toda feature; platform-specific só em
  `src-tauri/src/virtualcam/` (trait + `#[cfg(target_os)]`)
- Sem `unwrap()` em caminho falível; `tracing` estruturado; `unsafe` só FFI com
  `// SAFETY:`
- Credenciais apenas no cofre do SO (keyring); nada sai da máquina

## Feature ativa

`specs/001-phone-webcam-bridge/` — spec.md, plan.md, research.md,
data-model.md, contracts/, quickstart.md. Fluxo: /speckit-tasks →
/speckit-implement.
