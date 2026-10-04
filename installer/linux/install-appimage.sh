#!/usr/bin/env bash
# CamLink — integra o AppImage ao sistema (T090).
#
# Um AppImage não tem passo de instalação: ele fica inerte na pasta de
# downloads, sem entrada no menu, sem ícone, e — como gerenciadores de
# arquivos modernos não executam binários por duplo clique (o Nautilus
# removeu isso por segurança) — sem forma óbvia de abrir. Pior: o bit de
# execução não sobrevive ao download HTTP, então o sistema reclama que "não
# há aplicativo instalado para AppImage", mensagem que aponta para o lado
# errado do problema.
#
# Este script faz o que um pacote faria:
#
#   ./install-appimage.sh [caminho/do/CamLink.AppImage]
#   ./install-appimage.sh --uninstall
#
# A parte do aplicativo vai para ~/.local e NÃO precisa de root. Só os
# pré-requisitos de sistema (módulo v4l2loopback e a regra udev) pedem
# sudo, e apenas se estiverem faltando.
set -euo pipefail

APP_DIR="$HOME/.local/bin"
APP_PATH="$APP_DIR/CamLink.AppImage"
DESKTOP="$HOME/.local/share/applications/camlink.desktop"
ICON_BASE="$HOME/.local/share/icons/hicolor"

UDEV_DST=/etc/udev/rules.d/99-camlink-v4l2loopback.rules
MODLOAD_DST=/etc/modules-load.d/camlink-v4l2loopback.conf
MODPROBE_DST=/etc/modprobe.d/camlink-v4l2loopback.conf

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

uninstall() {
  rm -f "$APP_PATH" "$DESKTOP"
  rm -f "$ICON_BASE"/*/apps/camlink.png
  update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
  gtk-update-icon-cache -f -t "$ICON_BASE" 2>/dev/null || true
  green "CamLink removido de ~/.local."
  echo "Os pré-requisitos de sistema (v4l2loopback, regra udev) NÃO foram"
  echo "removidos: outros programas podem depender deles."
  exit 0
}

[[ "${1:-}" == "--uninstall" ]] && uninstall

# --- localizar o AppImage ---------------------------------------------------
SRC="${1:-}"
if [[ -z "$SRC" ]]; then
  # Procura no diretório atual e em Downloads, do mais recente para o mais
  # antigo (o usuário pode ter baixado várias vezes — "(1)", "(2)"...).
  SRC="$(find . "$HOME/Downloads" -maxdepth 1 -name 'CamLink*.AppImage' -printf '%T@ %p\n' 2>/dev/null \
         | sort -rn | head -1 | cut -d' ' -f2-)"
fi
if [[ -z "$SRC" || ! -f "$SRC" ]]; then
  red "Não encontrei o AppImage."
  echo "Passe o caminho: ./install-appimage.sh ~/Downloads/CamLink_0.1.0_amd64.AppImage"
  exit 1
fi
bold "CamLink — instalando a partir de $SRC"

# --- integração do aplicativo (sem root) ------------------------------------
mkdir -p "$APP_DIR" "$(dirname "$DESKTOP")"
install -Dm755 "$SRC" "$APP_PATH"
green "  binário: $APP_PATH"

# O .desktop e os ícones já viajam DENTRO do AppImage; extraímos em vez de
# duplicar aqui, para não divergirem quando o design mudar.
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
( cd "$TMP" && "$APP_PATH" --appimage-extract >/dev/null 2>&1 ) || true

icons_found=0
if [[ -d "$TMP/squashfs-root/usr/share/icons/hicolor" ]]; then
  while IFS= read -r icon; do
    size_dir="$(basename "$(dirname "$(dirname "$icon")")")"
    # O Tauri gera diretórios como "256x256@2", que NÃO é nome válido no
    # padrão hicolor (o freedesktop usa "<N>x<N>"; escala vai em
    # subdiretório próprio). Instalar com esse nome cria uma pasta que
    # nenhum tema lê — o ícone some silenciosamente.
    [[ "$size_dir" =~ ^[0-9]+x[0-9]+$ ]] || continue
    install -Dm644 "$icon" "$ICON_BASE/$size_dir/apps/camlink.png"
    icons_found=$((icons_found + 1))
  done < <(find "$TMP/squashfs-root/usr/share/icons/hicolor" -name 'camlink.png' 2>/dev/null)
fi
if [[ "$icons_found" -eq 0 && -f "$TMP/squashfs-root/camlink.png" ]]; then
  install -Dm644 "$TMP/squashfs-root/camlink.png" "$ICON_BASE/256x256/apps/camlink.png"
  icons_found=1
fi
green "  ícones: $icons_found instalado(s)"

# Exec aponta para o caminho absoluto: o .desktop interno usa `Exec=camlink`,
# que só funciona de dentro do bundle.
cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=CamLink
Comment=Câmeras Android e IP como webcams virtuais
Exec=$APP_PATH
Icon=camlink
Terminal=false
Categories=AudioVideo;Video;
StartupWMClass=camlink
EOF
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
gtk-update-icon-cache -f -t "$ICON_BASE" 2>/dev/null || true
green "  menu: $DESKTOP"

# --- conferência do que o AppImage deveria trazer ---------------------------
# adb, ffmpeg e o cliente scrcpy vão DENTRO do AppImage (T066/T092). Se o
# cliente não estiver lá, o app cai no scrcpy do PATH — que é de outra versão
# e faz o fork abortar com "The server version (X) does not match the client
# (Y)" só quando o usuário tenta transmitir, longe daqui. Melhor dizer agora.
for b in adb ffmpeg scrcpy; do
  if [[ ! -x "$TMP/squashfs-root/usr/lib/CamLink/bin/$b" ]]; then
    yellow "  aviso: este AppImage não traz o $b embutido"
    echo "     O app tentará usar o $b do sistema, que pode não servir."
    echo "     Baixe o AppImage de novo da release se isso se repetir."
  fi
done

# --- pré-requisitos de sistema (só aqui entra o sudo) -----------------------
need_system=0
command -v v4l2loopback-ctl >/dev/null 2>&1 || need_system=1
[[ -f "$UDEV_DST" ]] || need_system=1
grep -q '^v4l2loopback ' /proc/modules 2>/dev/null || need_system=1

if [[ "$need_system" -eq 0 ]]; then
  green ""
  green "Tudo pronto. Procure por CamLink no menu de aplicativos."
  exit 0
fi

echo
bold "Faltam pré-requisitos do sistema (precisam de sudo uma única vez):"
command -v v4l2loopback-ctl >/dev/null 2>&1 || echo "  - v4l2loopback-ctl (pacote v4l2loopback-utils)"
[[ -f "$UDEV_DST" ]] || echo "  - regra udev do /dev/v4l2loopback"
grep -q '^v4l2loopback ' /proc/modules 2>/dev/null || echo "  - módulo v4l2loopback carregado"
echo
read -r -p "Instalar agora? [S/n] " resp
[[ "${resp:-S}" =~ ^[Nn] ]] && { echo "Pulado. Rode este script de novo quando quiser."; exit 0; }

# Os arquivos de configuração viajam dentro do AppImage (bundle.linux.appimage
# em tauri.linux.conf.json), então não é preciso clonar o repositório.
CFG="$TMP/squashfs-root/usr/share/camlink"
if [[ ! -d "$CFG" ]]; then
  red "Este AppImage não traz os arquivos de configuração."
  echo "Use uma versão mais recente, ou instale manualmente: v4l2loopback-utils +"
  echo "v4l2loopback-dkms, e a regra udev do repositório."
  exit 1
fi

if command -v pacman >/dev/null 2>&1;      then PKGS=(v4l2loopback-utils v4l2loopback-dkms); PM=(pacman -S --needed --noconfirm)
elif command -v apt-get >/dev/null 2>&1;   then PKGS=(v4l2loopback-utils v4l2loopback-dkms); PM=(apt-get install -y)
elif command -v dnf >/dev/null 2>&1;       then PKGS=(v4l2loopback); PM=(dnf install -y)
else PKGS=(); PM=()
fi

sudo bash -s -- "$CFG" "$UDEV_DST" "$MODLOAD_DST" "$MODPROBE_DST" "${PM[@]:-}" <<'ROOT'
set -euo pipefail
CFG="$1"; UDEV="$2"; MODLOAD="$3"; MODPROBE="$4"; shift 4
if [[ $# -gt 0 && -n "${1:-}" ]]; then "$@" v4l2loopback-utils v4l2loopback-dkms 2>/dev/null || true; fi
install -Dm644 "$CFG/99-camlink-v4l2loopback.rules" "$UDEV"
install -Dm644 "$CFG/camlink-v4l2loopback.conf" "$MODLOAD"
install -Dm644 "$CFG/modprobe-camlink.conf" "$MODPROBE"
udevadm control --reload-rules 2>/dev/null || true
# /dev/v4l2loopback é SUBSYSTEM=misc, não video4linux.
udevadm trigger --subsystem-match=misc --subsystem-match=video4linux 2>/dev/null || true
modprobe v4l2loopback 2>/dev/null || true
ROOT

echo
if grep -q '^v4l2loopback ' /proc/modules 2>/dev/null; then
  green "Tudo pronto. Procure por CamLink no menu de aplicativos."
else
  red "O módulo v4l2loopback não carregou. Verifique se o v4l2loopback-dkms"
  red "compilou para o seu kernel (precisa dos headers)."
fi
