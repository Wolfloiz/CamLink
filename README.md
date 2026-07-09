# CamLink

Transforme seu celular Android e câmeras IP em webcams virtuais — sem instalar
nada no celular.

O CamLink é um aplicativo desktop (Linux e Windows 10/11) que detecta
smartphones Android conectados por cabo USB e fontes de vídeo IP/RTSP, e os
expõe como webcams virtuais reconhecidas por OBS Studio, navegadores (Chrome,
Firefox — WebRTC), Discord e qualquer aplicativo que use câmera.

## Visão

- **Zero instalação no celular**: a comunicação com o Android acontece via
  USB/ADB; nenhum aplicativo é instalado no aparelho (Android 12+).
- **Controles de câmera em tempo real**: zoom, foco (tap-to-focus), exposição,
  balanço de branco, torch e estabilização, direto do desktop.
- **Fontes IP/RTSP**: câmeras de rede como webcams virtuais, com credenciais
  guardadas apenas no cofre de segredos do sistema operacional.
- **Múltiplos dispositivos simultâneos**: cada fonte vira um dispositivo de
  webcam independente.
- **Captura RAW (DNG)**: fotos em RAW a partir do sensor do celular.
- **Paridade Linux + Windows**: funcionalidades equivalentes nas duas
  plataformas desde a v1.

## Status

🚧 Em desenvolvimento — ainda não há release. Acompanhe o progresso em
[`specs/001-phone-webcam-bridge/`](specs/001-phone-webcam-bridge/).

## Stack

- **Backend**: Rust (Tauri 2.x) — `src-tauri/`
- **UI**: SvelteKit — `src/`
- **Android**: fork do [scrcpy](https://github.com/Genymobile/scrcpy)-server
  (Java 17, submodule em `scrcpy/`, branch `camlink`)
- **Runtime**: adb, scrcpy ≥ 4.0, ffmpeg, v4l2loopback ≥ 0.13 (Linux),
  akvirtualcamera (Windows)

## Desenvolvimento

```bash
pnpm install
pnpm tauri dev
```

Gates de qualidade (obrigatórios, ver `.specify/memory/constitution.md`):

```bash
cd src-tauri
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

## Licença

[GPL-3.0](LICENSE). O fork do scrcpy-server permanece sob Apache-2.0 (licença
do upstream).
