#!/usr/bin/env bash
# CamLink — instalador de pré-requisitos do sistema (Linux, T066).
#
# Cobre o que um pacote (.deb/AppImage/AUR) não consegue fazer sozinho de
# forma portátil: dependências de runtime pela distro, regra udev do
# /dev/v4l2loopback, carga do módulo no boot e o usuário no grupo `video`.
#
#   sudo ./install.sh              instala tudo
#   ./install.sh --check           só diagnostica (não precisa de root)
#   sudo ./install.sh --with-scrcpy  também instala o scrcpy oficial se a
#                                  versão da distro for antiga demais
#   sudo ./install.sh --uninstall  remove os arquivos de sistema do CamLink
#
# Nada aqui deixa o app privilegiado: o CamLink roda como usuário comum, o
# acesso ao v4l2loopback vem do grupo `video` (ver
# 99-camlink-v4l2loopback.rules).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

UDEV_RULE_SRC="$SCRIPT_DIR/99-camlink-v4l2loopback.rules"
UDEV_RULE_DST="/etc/udev/rules.d/99-camlink-v4l2loopback.rules"
MODULES_LOAD_SRC="$SCRIPT_DIR/camlink-v4l2loopback.conf"
MODULES_LOAD_DST="/etc/modules-load.d/camlink-v4l2loopback.conf"
MODPROBE_SRC="$SCRIPT_DIR/modprobe-camlink.conf"
MODPROBE_DST="/etc/modprobe.d/camlink-v4l2loopback.conf"

# scrcpy ≥ 4.0 é requisito do fork (protocolo de controle do CamLink) —
# várias distros ainda empacotam 1.x/2.x, daí a checagem de versão em vez de
# só "está instalado?".
SCRCPY_MIN_MAJOR=4
SCRCPY_MIN_MINOR=0
# v4l2loopback ≥ 0.13 é o piso da alocação dinâmica (`v4l2loopback-ctl add`).
V4L2_MIN_MAJOR=0
V4L2_MIN_MINOR=13

WITH_SCRCPY=0
MODE=install

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

usage() {
  sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

for arg in "$@"; do
  case "$arg" in
    --check)       MODE=check ;;
    --uninstall)   MODE=uninstall ;;
    --with-scrcpy) WITH_SCRCPY=1 ;;
    -h|--help)     usage 0 ;;
    *) red "opção desconhecida: $arg"; usage 1 ;;
  esac
done

require_root() {
  if [[ "$(id -u)" -ne 0 ]]; then
    red "Esta operação precisa de root."
    echo "Rode: sudo $0 $*"
    exit 1
  fi
}

# Usuário real por trás do sudo/pkexec — é ele que precisa entrar no grupo
# `video`, não o root.
target_user() {
  if [[ -n "${SUDO_USER:-}" && "$SUDO_USER" != "root" ]]; then
    echo "$SUDO_USER"
  elif [[ -n "${PKEXEC_UID:-}" ]]; then
    getent passwd "$PKEXEC_UID" | cut -d: -f1
  else
    logname 2>/dev/null || echo ""
  fi
}

# ---------------------------------------------------------------------------
# Detecção de distro / pacotes
# ---------------------------------------------------------------------------

detect_pm() {
  for pm in apt-get pacman dnf zypper; do
    command -v "$pm" >/dev/null 2>&1 && { echo "$pm"; return; }
  done
  echo ""
}

# Dependências de runtime por gerenciador. `scrcpy` fica de fora de
# propósito: a versão empacotada pela maioria das distros baseadas em Debian
# é antiga demais (Ubuntu 24.04 = 1.25) e instalá-la mascararia o requisito
# real — ver `ensure_scrcpy`.
pm_packages() {
  case "$1" in
    apt-get) echo "adb ffmpeg v4l-utils v4l2loopback-dkms" ;;
    pacman)  echo "android-tools ffmpeg v4l-utils v4l2loopback-dkms scrcpy" ;;
    dnf)     echo "android-tools ffmpeg v4l-utils v4l2loopback" ;;
    zypper)  echo "android-tools ffmpeg v4l-utils v4l2loopback-kmp-default" ;;
  esac
}

install_packages() {
  local pm; pm="$(detect_pm)"
  local pkgs; pkgs="$(pm_packages "$pm")"
  if [[ -z "$pm" ]]; then
    yellow "Gerenciador de pacotes não reconhecido — instale manualmente:"
    echo "  adb, ffmpeg, v4l-utils, v4l2loopback (dkms) e scrcpy >= ${SCRCPY_MIN_MAJOR}.${SCRCPY_MIN_MINOR}"
    return
  fi
  bold "Instalando dependências ($pm): $pkgs"
  # shellcheck disable=SC2086
  case "$pm" in
    apt-get) apt-get update && apt-get install -y $pkgs ;;
    pacman)  pacman -Sy --needed --noconfirm $pkgs ;;
    dnf)     dnf install -y $pkgs ;;
    zypper)  zypper --non-interactive install $pkgs ;;
  esac
}

# ---------------------------------------------------------------------------
# Versões
# ---------------------------------------------------------------------------

# Primeiro "X.Y" que aparecer na saída — cobre `scrcpy 4.0` e
# `v4l2loopback-ctl v0.15.4-1-g9ef83fb`.
parse_version() {
  grep -oE '[0-9]+\.[0-9]+' <<<"$1" | head -1
}

version_at_least() { # $1=versão "X.Y"  $2=major mínimo  $3=minor mínimo
  local major minor
  major="${1%%.*}"; minor="${1#*.}"; minor="${minor%%.*}"
  [[ -z "$major" || -z "$minor" ]] && return 1
  (( major > $2 )) || { (( major == $2 )) && (( minor >= $3 )); }
}

scrcpy_version() { command -v scrcpy >/dev/null 2>&1 && parse_version "$(scrcpy --version 2>/dev/null | head -1)"; }
v4l2_version()   { command -v v4l2loopback-ctl >/dev/null 2>&1 && parse_version "$(v4l2loopback-ctl --version 2>/dev/null | head -1)"; }

ensure_scrcpy() {
  local ver; ver="$(scrcpy_version || true)"
  if [[ -n "$ver" ]] && version_at_least "$ver" "$SCRCPY_MIN_MAJOR" "$SCRCPY_MIN_MINOR"; then
    green "scrcpy $ver OK"
    return
  fi
  if [[ "$WITH_SCRCPY" -eq 1 ]]; then
    "$SCRIPT_DIR/vendor.sh" --scrcpy --prefix /opt/camlink
    ln -sf /opt/camlink/bin/scrcpy /usr/local/bin/scrcpy
    green "scrcpy oficial instalado em /opt/camlink (link em /usr/local/bin/scrcpy)"
    return
  fi
  yellow "scrcpy ${ver:-ausente} — o CamLink precisa de >= ${SCRCPY_MIN_MAJOR}.${SCRCPY_MIN_MINOR} para fontes Android."
  echo "  Sua distro empacota uma versão antiga demais. Opções:"
  echo "    sudo $0 --with-scrcpy    (baixa o build oficial do projeto scrcpy)"
  echo "    ou instale manualmente:  https://github.com/Genymobile/scrcpy/releases"
  echo "  (fontes RTSP funcionam sem scrcpy.)"
}

# ---------------------------------------------------------------------------
# Arquivos de sistema
# ---------------------------------------------------------------------------

install_system_files() {
  bold "Instalando regra udev e configuração do módulo"
  install -Dm644 "$UDEV_RULE_SRC"    "$UDEV_RULE_DST"
  install -Dm644 "$MODULES_LOAD_SRC" "$MODULES_LOAD_DST"
  install -Dm644 "$MODPROBE_SRC"     "$MODPROBE_DST"
  udevadm control --reload-rules 2>/dev/null || true
  udevadm trigger --subsystem-match=video4linux 2>/dev/null || true
  green "  $UDEV_RULE_DST"
  green "  $MODULES_LOAD_DST"
  green "  $MODPROBE_DST"
}

load_module() {
  if modprobe v4l2loopback 2>/tmp/camlink-modprobe.err; then
    # A regra udev só se aplica a um device criado depois dela; se o módulo
    # já estava carregado, o node existente continua com o dono antigo.
    [[ -e /dev/v4l2loopback ]] && chgrp video /dev/v4l2loopback && chmod 0660 /dev/v4l2loopback
    green "Módulo v4l2loopback carregado"
  else
    local err; err="$(cat /tmp/camlink-modprobe.err)"
    if grep -qi "key was rejected by service" <<<"$err"; then
      red "Secure Boot bloqueou o v4l2loopback (módulo não assinado)."
      echo "  Assine o módulo (mokutil + DKMS) ou desative o Secure Boot no BIOS/UEFI."
    else
      red "Falha ao carregar o v4l2loopback: $err"
    fi
  fi
  rm -f /tmp/camlink-modprobe.err
}

add_to_video_group() {
  local user; user="$(target_user)"
  if [[ -z "$user" ]]; then
    yellow "Não consegui identificar o usuário — adicione manualmente: sudo usermod -aG video \$USER"
    return
  fi
  if id -nG "$user" | tr ' ' '\n' | grep -qx video; then
    green "Usuário '$user' já está no grupo video"
    return
  fi
  usermod -aG video "$user"
  green "Usuário '$user' adicionado ao grupo video"
  yellow "Faça logout/login (ou 'newgrp video') para o grupo valer na sessão atual."
}

uninstall() {
  bold "Removendo arquivos de sistema do CamLink"
  rm -fv "$UDEV_RULE_DST" "$MODULES_LOAD_DST" "$MODPROBE_DST"
  udevadm control --reload-rules 2>/dev/null || true
  yellow "Mantidos de propósito: pacotes da distro e a participação no grupo video"
  yellow "(outros programas podem depender deles)."
}

# ---------------------------------------------------------------------------
# Diagnóstico (--check) — mesma tabela do troubleshooting do quickstart
# ---------------------------------------------------------------------------

check() {
  local failures=0
  bold "CamLink — diagnóstico de pré-requisitos"

  for bin in adb ffmpeg v4l2loopback-ctl; do
    if command -v "$bin" >/dev/null 2>&1; then
      green "  $bin: $(command -v "$bin")"
    else
      red   "  $bin: AUSENTE"; failures=$((failures + 1))
    fi
  done

  local sver; sver="$(scrcpy_version || true)"
  if [[ -n "$sver" ]] && version_at_least "$sver" "$SCRCPY_MIN_MAJOR" "$SCRCPY_MIN_MINOR"; then
    green "  scrcpy: $sver"
  else
    red   "  scrcpy: ${sver:-ausente} (preciso >= ${SCRCPY_MIN_MAJOR}.${SCRCPY_MIN_MINOR}; só afeta fontes Android)"
    failures=$((failures + 1))
  fi

  local vver; vver="$(v4l2_version || true)"
  if [[ -n "$vver" ]] && version_at_least "$vver" "$V4L2_MIN_MAJOR" "$V4L2_MIN_MINOR"; then
    green "  v4l2loopback-ctl: $vver"
  else
    red   "  v4l2loopback-ctl: ${vver:-ausente} (preciso >= ${V4L2_MIN_MAJOR}.${V4L2_MIN_MINOR})"
    failures=$((failures + 1))
  fi

  [[ -f "$UDEV_RULE_DST" ]] && green "  udev rule: $UDEV_RULE_DST" \
    || { red "  udev rule: ausente"; failures=$((failures + 1)); }
  [[ -f "$MODULES_LOAD_DST" ]] && green "  modules-load.d: $MODULES_LOAD_DST" \
    || { red "  modules-load.d: ausente"; failures=$((failures + 1)); }

  if lsmod | grep -q '^v4l2loopback'; then
    green "  módulo: carregado"
  else
    red "  módulo: não carregado (sudo modprobe v4l2loopback)"; failures=$((failures + 1))
  fi

  if [[ -e /dev/v4l2loopback ]]; then
    if [[ -r /dev/v4l2loopback && -w /dev/v4l2loopback ]]; then
      green "  /dev/v4l2loopback: acessível por este usuário"
    else
      red "  /dev/v4l2loopback: sem permissão ($(stat -c '%U:%G %a' /dev/v4l2loopback))"
      echo "     Você está nos grupos: $(id -nG)"
      echo "     Se 'video' aparece aí mas o erro persiste, falta logout/login."
      failures=$((failures + 1))
    fi
  else
    red "  /dev/v4l2loopback: não existe (módulo não carregado)"; failures=$((failures + 1))
  fi

  if [[ "$failures" -eq 0 ]]; then
    green "Tudo pronto."
  else
    yellow "$failures item(ns) pendente(s) — rode: sudo $0"
  fi
  return 0
}

# ---------------------------------------------------------------------------

case "$MODE" in
  check)     check ;;
  uninstall) require_root "$@"; uninstall ;;
  install)
    require_root "$@"
    install_packages
    ensure_scrcpy
    install_system_files
    load_module
    add_to_video_group
    echo
    bold "Pronto. Verifique com: $0 --check"
    ;;
esac
