#!/bin/sh
# CamLink — pós-instalação do .deb (T066).
#
# O dpkg já colocou a regra udev, o modules-load.d e o modprobe.d no lugar
# (bundle.linux.deb.files). Aqui é só aplicar tudo sem exigir reboot e
# resolver o único passo que um pacote não consegue adivinhar: qual usuário
# da máquina é o dono da sessão que vai usar o app.
set -e

[ "$1" = "configure" ] || exit 0

# O repositório vive num filesystem que força 0777, então o dpkg empacota os
# arquivos de configuração com bit de execução; normaliza aqui.
for f in /usr/lib/udev/rules.d/99-camlink-v4l2loopback.rules \
         /usr/lib/modules-load.d/camlink-v4l2loopback.conf \
         /usr/lib/modprobe.d/camlink-v4l2loopback.conf; do
  [ -f "$f" ] && chmod 0644 "$f" || true
done

udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --subsystem-match=video4linux 2>/dev/null || true
modprobe v4l2loopback 2>/dev/null || true

# A regra udev só vale para um device criado depois dela; se o módulo já
# estava carregado, o node existente continua root:root.
if [ -e /dev/v4l2loopback ]; then
  chgrp video /dev/v4l2loopback 2>/dev/null || true
  chmod 0660 /dev/v4l2loopback 2>/dev/null || true
fi

# Usuário real por trás do sudo/pkexec (apt via terminal ou loja gráfica).
CAMLINK_USER="${SUDO_USER:-}"
if [ -z "$CAMLINK_USER" ] && [ -n "${PKEXEC_UID:-}" ]; then
  CAMLINK_USER="$(getent passwd "$PKEXEC_UID" | cut -d: -f1)"
fi

if [ -n "$CAMLINK_USER" ] && [ "$CAMLINK_USER" != "root" ]; then
  if id -nG "$CAMLINK_USER" | tr ' ' '\n' | grep -qx video; then
    echo "CamLink: '$CAMLINK_USER' já está no grupo video."
  else
    usermod -aG video "$CAMLINK_USER" || true
    echo "CamLink: '$CAMLINK_USER' adicionado ao grupo video — faça logout/login para valer."
  fi
else
  echo "CamLink: adicione seu usuário ao grupo video e faça logout/login:"
  echo "         sudo usermod -aG video \$USER"
fi

echo "CamLink: diagnóstico completo em /usr/share/camlink/install.sh --check"
exit 0
