#!/usr/bin/env bash
# CamLink — prepara os binários de terceiros do pacote Linux (T066).
# Espelha o installer/windows/vendor.ps1 do T067.
#
# Roda ANTES do `pnpm tauri build` — não faz parte do runtime do app.
# Idempotente: pode rodar de novo a qualquer momento.
#
#   ./vendor.sh                    jar do fork + adb + ffmpeg + scrcpy
#   ./vendor.sh --jar-only         só o jar do fork
#   ./vendor.sh --vendor-scrcpy    só o cliente scrcpy na versão pinada
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
#
# O scrcpy é a exceção: vai embutido nos DOIS (T092), porque a exigência é
# de versão exata e não de um piso — ver SCRCPY_PINNED abaixo. O prebuilt do
# upstream serve para isso: linka SDL2 e libav* estaticamente (daí os 33 MB)
# e em runtime só precisa de libc, libm, libgcc_s e libudev, presentes em
# qualquer distro com systemd — verificado com `ldd` na 4.1.
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

# Versão do cliente scrcpy embutida no pacote. NÃO é "a mais recente" de
# propósito: o scrcpy exige que cliente e servidor tenham EXATAMENTE a mesma
# versão (`The server version (X) does not match the client (Y)`), e o nosso
# fork é compilado sobre esta tag. Baixar "latest" quebraria o app toda vez
# que o upstream publicasse — foi assim que o scrcpy 4.1 da distro quebrou a
# instalação com o fork 4.0 (bancada 2026-10-04).
#
# Ao rebasear o fork para uma versão nova do scrcpy, atualize AQUI também.
SCRCPY_PINNED="v4.1"
# sha256 do tarball de $SCRCPY_PINNED. Pinar a versão sem pinar o conteúdo
# deixaria o build dependendo de um asset mutável; é o mesmo hash que o
# PKGBUILD declara em sha256sums. Atualize junto com SCRCPY_PINNED.
SCRCPY_PINNED_SHA256="ad56ae8bfeedf41e824945c11dbf55fcb092b3e615b9b486f48a50e30d389635"

MODE=all

while [[ $# -gt 0 ]]; do
  case "$1" in
    --jar-only) MODE=jar ;;
    --vendor-scrcpy) MODE=vendor-scrcpy ;;
    --stub)     MODE=stub ;;
    -h|--help)  sed -n '2,28p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
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
  for f in scrcpy-server-camlink adb ffmpeg scrcpy; do
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
  assert_jar_matches_pin "$src"
  install -Dm644 "$src" "$VENDOR_BIN/scrcpy-server-camlink"
  echo "  -> scrcpy-server-camlink ($(pinned_version))"
}

# `SCRCPY_PINNED` sem o "v" — é o formato que o scrcpy usa internamente.
pinned_version() { echo "${SCRCPY_PINNED#v}"; }

# O par cliente+servidor TEM que ser da mesma versão: o scrcpy aborta com
# "The server version (X) does not match the client (Y)" e nenhum celular
# conecta. Até a 0.1.0 isso não era checado em lugar nenhum, e a divergência
# só aparecia no celular do usuário — o jar saiu na 4.0 e o cliente da distro
# foi para a 4.1. As duas checagens abaixo cobrem os dois jeitos de errar:
#
#   1. fork não rebaseado (ou pin não atualizado): `versionName` do
#      server/build.gradle diverge do SCRCPY_PINNED;
#   2. jar velho em dist/: o fork está certo mas ninguém rodou
#      build-camlink.sh depois do rebase — é o caso mais traiçoeiro, porque
#      o repositório inteiro parece correto.
assert_jar_matches_pin() { # $1 = caminho do jar
  need unzip
  local want gradle_version
  want="$(pinned_version)"

  local gradle="$REPO_ROOT/scrcpy/server/build.gradle"
  if [[ -f "$gradle" ]]; then
    gradle_version="$(sed -n 's/.*versionName *"\([^"]*\)".*/\1/p' "$gradle" | head -1)"
    if [[ -n "$gradle_version" && "$gradle_version" != "$want" ]]; then
      echo "O fork em scrcpy/ está na versão $gradle_version, mas SCRCPY_PINNED é $SCRCPY_PINNED." >&2
      echo "Rebaseie o fork na tag v$want OU atualize SCRCPY_PINNED — as duas têm que casar." >&2
      exit 1
    fi
  fi

  # A versão fica no classes.dex como string: byte de tamanho + MUTF-8.
  # Procurar só "$want" casaria com qualquer número parecido no binário.
  local len prefix
  len="$(printf '%s' "$want" | wc -c)"
  # Byte de tamanho de verdade, e não o texto "\x03": o printf de fora
  # precisa receber a sequência já montada pelo de dentro.
  prefix="$(printf "\\x$(printf '%02x' "$len")")"
  # `grep -c` e comparação do NÚMERO, não `grep -q` + status do pipe: o
  # `-q` sai no primeiro match e mata o `unzip` com SIGPIPE (141), que o
  # `set -o pipefail` do topo propaga — o `if !` então concluía "não é a
  # versão certa" para QUALQUER jar, inclusive o correto. É o mesmo bug que
  # o `install.sh` tinha na checagem do módulo v4l2loopback; `-c` consome a
  # entrada toda, então o produtor termina normalmente.
  local matches
  matches="$(unzip -p "$1" classes.dex 2>/dev/null | grep -acF "$prefix$want" || true)"
  if [[ "${matches:-0}" -eq 0 ]]; then
    echo "O jar em scrcpy/dist/ não é da versão $want." >&2
    echo "Rode scrcpy/build-camlink.sh depois do rebase (precisa de JDK 17 + Android SDK)." >&2
    exit 1
  fi
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

# --- cliente scrcpy, versão pinada, para DENTRO do pacote ----------------
# Embutir em vez de depender da distro porque a exigência real é de versão
# EXATA, não ">= 4.0" como o projeto declarava: qualquer atualização da
# distro quebraria o app sem nada que pudéssemos fazer.
vendor_scrcpy() {
  need tar
  echo "== scrcpy $SCRCPY_PINNED (cliente, embutido) =="
  local url="https://github.com/Genymobile/scrcpy/releases/download/$SCRCPY_PINNED/scrcpy-linux-x86_64-$SCRCPY_PINNED.tar.gz"
  fetch "$url" "$TMP_DIR/scrcpy-pin.tar.gz"

  local actual
  actual="$(sha256sum "$TMP_DIR/scrcpy-pin.tar.gz" | cut -d' ' -f1)"
  if [[ "$actual" != "$SCRCPY_PINNED_SHA256" ]]; then
    echo "sha256 do scrcpy $SCRCPY_PINNED não bate:" >&2
    echo "  esperado $SCRCPY_PINNED_SHA256" >&2
    echo "  obtido   $actual" >&2
    exit 1
  fi

  rm -rf "$TMP_DIR/scrcpy-pin" && mkdir -p "$TMP_DIR/scrcpy-pin"
  tar -xzf "$TMP_DIR/scrcpy-pin.tar.gz" -C "$TMP_DIR/scrcpy-pin" --strip-components=1
  install -Dm755 "$TMP_DIR/scrcpy-pin/scrcpy" "$VENDOR_BIN/scrcpy"
  echo "  -> scrcpy ($SCRCPY_PINNED)"
}

mkdir -p "$VENDOR_BIN" "$TMP_DIR"
case "$MODE" in
  jar)           vendor_jar ;;
  vendor-scrcpy) vendor_scrcpy ;;
  all)           vendor_jar; vendor_adb; vendor_ffmpeg; vendor_scrcpy ;;
esac
rm -rf "$TMP_DIR"
echo -e "\nVendoring concluído em $VENDOR_BIN"
