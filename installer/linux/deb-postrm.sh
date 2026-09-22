#!/bin/sh
# CamLink — pós-remoção do .deb (T066).
#
# Os arquivos de /etc e /usr/lib são removidos pelo próprio dpkg; aqui só
# recarregamos o udev. Não removemos ninguém do grupo `video` nem
# descarregamos o módulo: outros programas (OBS, navegadores, outras
# webcams virtuais) podem depender dos dois.
set -e
udevadm control --reload-rules 2>/dev/null || true
exit 0
