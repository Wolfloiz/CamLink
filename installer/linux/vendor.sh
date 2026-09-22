#!/usr/bin/env bash
# CamLink — prepara os binários de terceiros do pacote Linux (T066).
# Espelha o installer/windows/vendor.ps1 do T067.
#
# Roda ANTES do `pnpm tauri build` — não faz parte do runtime do app.
# Idempotente: pode rodar de novo a qualquer momento.
#
#   ./vendor.sh                    jar do fork + adb + ffmpeg em vendor/bin/
#   ./vendor.sh --jar-only         só o jar do fork (é o que o .deb precisa)
#   ./vendor.sh --scrcpy --prefix /opt/camlink
#                                  instala o scrcpy oficial num prefixo
#                                  (usado pelo install.sh --with-scrcpy)
#   ./vendor.sh --stub             placeholders vazios, sem baixar nada —
#                                  o build.rs do tauri-build exige que todo
#                                  path de bundle.resources exista em
#                                  QUALQUER cargo check/clippy/test, e
#                                  vendor/ é gitignored (espelha o -Stub do
#                                  vendor.ps1). Não serve pra empacotar.
#
# Por que adb/ffmpeg ficam fora do .deb: no Debian/Ubuntu/Arch eles vêm da
# distro (`Depends:`), que é a convenção da plataforma e evita duplicar
# binário GPL no pacote. O AppImage, que precisa ser autocontido, leva a
# cópia vendorizada daqui — ver `bundle.linux.appimage.files` em
# src-tauri/tauri.linux.conf.json.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VENDOR_BIN="$SCRIPT_DIR/vendor/bin"
TMP_DIR="$SCRIPT_DIR/vendor/.tmp"

# Mesma linha pinada no Windows (n8.1) em vez de "master": o tag "latest" do
# BtbN é reconstruído periodicamente por branch, então não é um artefato
# imutável — para reprodutibilidade total, baixe uma vez e vendorize o zip.
FFMPEG_ASSET="${FFMPEG_ASSET:-ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz}"
FFMPEG_URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$FFMPEG_ASSET"
PLATFORM_TOOLS_URL="https://dl.google.com/android/repository/platform-tools-latest-linux.zip"
SCRCPY_API="https://api.github.com/repos/Genymobile/scrcpy/releases/latest"

MODE=all
PREFIX=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --jar-only) MODE=jar ;;
    --scrcpy)   MODE=scrcpy ;;
    --stub)     MODE=stub ;;
    --prefix)   PREFIX="$2"; shift ;;
    -h|--help)  sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "opção desconhecida: $1" >&2; exit 1 ;;
  esac
  shift
done

# `--stub` cria os arquivos vazios nos mesmos caminhos e sai. O build.rs do
# tauri-build valida que todo path de `bundle.resources`
# (src-tauri/tauri.linux.conf.json) EXISTE em qualquer
# `cargo build/check/clippy/test`, não só ao empacotar o instalador de
# verdade — e `installer/linux/vendor/` é gitignored, então um checkout
# limpo (CI incluso) não compila sem isto. Espelha o `-Stub` do vendor.ps1
# (T067). Placeholder vazio NÃO serve pra `tauri build`: rode sem `--stub`
# antes de gerar pacote pra valer.
if [[ "$MODE" == "stub" ]]; then
  mkdir -p "$VENDOR_BIN"
  for f in scrcpy-server-camlink adb ffmpeg; do
    : >"$VENDOR_BIN/$f"
    echo "  -> $f (stub)"
  done
  echo -e "\nStub criado em $VENDOR_BIN (placeholders vazios, não são binários reais)"
  exit 0
fi

need() { command -v "$1" >/dev/null 2>&1 || { echo "faltando: $1" >&2; exit 1; }; }
need curl

fetch() { # url destino
  echo "Baixando $1 ..."
  curl -fsSL --retry 3 -o "$2" "$1"
}

# --- jar do fork (buildado localmente, não baixado) -------------------------
vendor_jar() {
  local src="$REPO_ROOT/scrcpy/dist/scrcpy-server-camlink"
  local sha="$src.sha256"
  echo "== scrcpy-server-camlink (fork) =="
  if [[ ! -f "$src" ]]; then
    echo "scrcpy/dist/scrcpy-server-camlink não existe." >&2
    echo "Rode scrcpy/build-camlink.sh antes (precisa de JDK 17 + Android SDK)." >&2
    exit 1
  fi
  if [[ -f "$sha" ]]; then
    local expected actual
    expected="$(cut -d' ' -f1 <"$sha" | tr -d '[:space:]')"
    actual="$(sha256sum "$src" | cut -d' ' -f1)"
    if [[ -n "$expected" && "$expected" != "$actual" ]]; then
      echo "sha256 de scrcpy-server-camlink não bate com $sha — rebuilde com scrcpy/build-camlink.sh." >&2
      exit 1
    fi
  fi
  install -Dm644 "$src" "$VENDOR_BIN/scrcpy-server-camlink"
  echo "  -> scrcpy-server-camlink"
}

# --- adb (Google, oficial) --------------------------------------------------
vendor_adb() {
  need unzip
  echo "== platform-tools (adb) =="
  fetch "$PLATFORM_TOOLS_URL" "$TMP_DIR/platform-tools.zip"
  unzip -qo "$TMP_DIR/platform-tools.zip" -d "$TMP_DIR"
  local src="$TMP_DIR/platform-tools/adb"
  [[ -f "$src" ]] || { echo "adb não encontrado no zip — layout mudou?" >&2; exit 1; }
  install -Dm755 "$src" "$VENDOR_BIN/adb"
  echo "  -> adb"
}

# --- ffmpeg (BtbN, build estático GPL) --------------------------------------
vendor_ffmpeg() {
  need tar
  echo "== ffmpeg =="
  fetch "$FFMPEG_URL" "$TMP_DIR/$FFMPEG_ASSET"
  rm -rf "$TMP_DIR/ffmpeg" && mkdir -p "$TMP_DIR/ffmpeg"
  tar -xJf "$TMP_DIR/$FFMPEG_ASSET" -C "$TMP_DIR/ffmpeg" --strip-components=1
  local src="$TMP_DIR/ffmpeg/bin/ffmpeg"
  [[ -f "$src" ]] || { echo "ffmpeg não encontrado em $TMP_DIR/ffmpeg — layout mudou?" >&2; exit 1; }
  install -Dm755 "$src" "$VENDOR_BIN/ffmpeg"
  echo "  -> ffmpeg"
}

# --- scrcpy oficial (release prebuilt do upstream) --------------------------
# Só para quem não tem uma versão recente na distro (install.sh
# --with-scrcpy). Não entra no AppImage: o cliente scrcpy linka SDL2 +
# libav*, e arrastar essa árvore para dentro do AppImage é frágil — o
# AppImage funciona sozinho para RTSP e pede scrcpy para fontes Android.
install_scrcpy() {
  need tar
  [[ -n "$PREFIX" ]] || { echo "--scrcpy exige --prefix <dir>" >&2; exit 1; }
  echo "== scrcpy (release oficial) =="
  local url
  url="$(curl -fsSL "$SCRCPY_API" \
        | grep -oE '"browser_download_url": *"[^"]*scrcpy-linux-x86_64-[^"]*\.tar\.gz"' \
        | head -1 | cut -d'"' -f4)"
  [[ -n "$url" ]] || {
    echo "não achei o asset linux-x86_64 na release mais recente do scrcpy." >&2
    echo "Instale manualmente: https://github.com/Genymobile/scrcpy/releases" >&2
    exit 1
  }
  fetch "$url" "$TMP_DIR/scrcpy.tar.gz"
  rm -rf "$TMP_DIR/scrcpy" && mkdir -p "$TMP_DIR/scrcpy"
  tar -xzf "$TMP_DIR/scrcpy.tar.gz" -C "$TMP_DIR/scrcpy" --strip-components=1
  mkdir -p "$PREFIX/bin" "$PREFIX/share/scrcpy"
  install -Dm755 "$TMP_DIR/scrcpy/scrcpy" "$PREFIX/bin/scrcpy"
  # O cliente oficial procura o jar ao lado do binário; o CamLink sobrepõe
  # com o jar do fork em runtime (SCRCPY_SERVER_PATH / resource do pacote),
  # mas o jar de origem tem que existir para o scrcpy avulso funcionar.
  [[ -f "$TMP_DIR/scrcpy/scrcpy-server" ]] && install -Dm644 "$TMP_DIR/scrcpy/scrcpy-server" "$PREFIX/bin/scrcpy-server"
  echo "  -> $PREFIX/bin/scrcpy"
}

mkdir -p "$VENDOR_BIN" "$TMP_DIR"
case "$MODE" in
  jar)    vendor_jar ;;
  scrcpy) install_scrcpy ;;
  all)    vendor_jar; vendor_adb; vendor_ffmpeg ;;
esac
rm -rf "$TMP_DIR"
[[ "$MODE" == "scrcpy" ]] || echo -e "\nVendoring concluído em $VENDOR_BIN"
