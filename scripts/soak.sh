#!/usr/bin/env bash
# CamLink — soak de sessão longa (T070 / SC-005).
#
# O critério SC-005 é "2 h sem interrupção perceptível ou vazamento de
# recursos". Isso não se verifica olhando a tela: precisa de RSS amostrado ao
# longo do tempo e de um registro de fps/reconexões. Este script faz a
# primeira parte e lê a segunda do log do app (que passou a registrar as
# stats periodicamente — ver `STATS_LOG_EVERY_N_TICKS` em lib.rs).
#
#   ./scripts/soak.sh              2 h (padrão)
#   ./scripts/soak.sh 600          10 min, para validar o próprio roteiro
#
# Variáveis: SOAK_INTERVAL (s entre amostras, padrão 30), SOAK_OUT (CSV).
#
# Antes de rodar: abra o CamLink, inicie UMA fonte e deixe transmitindo. O
# script não inicia nada — ele observa, para não interferir no que mede.
set -euo pipefail

DURATION="${1:-7200}"
INTERVAL="${SOAK_INTERVAL:-30}"
OUT="${SOAK_OUT:-soak-$(date +%Y%m%d-%H%M%S).csv}"
LOG_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/CamLink/logs"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

# O binário do AppImage e o do pacote se chamam `camlink`; o de dev também.
find_pid() { pgrep -x camlink | head -1; }

# RSS do processo E dos descendentes: o scrcpy e o ffmpeg fazem o trabalho
# pesado, e um vazamento neles não apareceria no RSS do app sozinho.
tree_pids() { # $1 = pid raiz
  local pid="$1" kids
  echo "$pid"
  kids="$(pgrep -P "$pid" 2>/dev/null || true)"
  for k in $kids; do tree_pids "$k"; done
}

rss_kb_total() { # $1 = pid raiz
  local total=0 rss
  for p in $(tree_pids "$1"); do
    rss="$(awk '/^VmRSS:/ {print $2}' "/proc/$p/status" 2>/dev/null || true)"
    [[ -n "$rss" ]] && total=$((total + rss))
  done
  echo "$total"
}

PID="$(find_pid || true)"
if [[ -z "$PID" ]]; then
  red "Nenhum processo 'camlink' rodando."
  echo "  Abra o CamLink, inicie uma fonte e deixe transmitindo antes de rodar isto."
  exit 1
fi

if ((DURATION >= 60)); then
  bold "CamLink — soak de $((DURATION / 60)) min (amostra a cada ${INTERVAL}s)"
else
  bold "CamLink — soak de ${DURATION}s (amostra a cada ${INTERVAL}s)"
fi
echo "  pid=$PID  saída=$OUT"
echo "  processos observados: $(tree_pids "$PID" | tr '\n' ' ')"
echo

echo "epoch,elapsed_s,rss_kb,procs" >"$OUT"

START="$(date +%s)"
FIRST_RSS=""
MAX_RSS=0
SAMPLES=0

while :; do
  NOW="$(date +%s)"
  ELAPSED=$((NOW - START))
  ((ELAPSED >= DURATION)) && break

  # Se o app morreu no meio, isso é o resultado — não um erro do script.
  if ! kill -0 "$PID" 2>/dev/null; then
    red "O processo camlink (pid $PID) MORREU após ${ELAPSED}s."
    echo "  Isso reprova o SC-005 por si só. Veja o log em $LOG_DIR."
    break
  fi

  RSS="$(rss_kb_total "$PID")"
  PROCS="$(tree_pids "$PID" | wc -l)"
  echo "$NOW,$ELAPSED,$RSS,$PROCS" >>"$OUT"
  [[ -z "$FIRST_RSS" ]] && FIRST_RSS="$RSS"
  ((RSS > MAX_RSS)) && MAX_RSS="$RSS"
  SAMPLES=$((SAMPLES + 1))

  printf '\r  %5ds  RSS %6d MiB  procs %d  ' \
    "$ELAPSED" "$((RSS / 1024))" "$PROCS"
  sleep "$INTERVAL"
done
echo

# ---------------------------------------------------------------------------
# Veredito
# ---------------------------------------------------------------------------
LAST_RSS="$(tail -1 "$OUT" | cut -d, -f3)"
bold "Resultado"
echo "  amostras:   $SAMPLES"
echo "  RSS inicial: $((FIRST_RSS / 1024)) MiB"
echo "  RSS final:   $((LAST_RSS / 1024)) MiB"
echo "  RSS máximo:  $((MAX_RSS / 1024)) MiB"

# 15% de crescimento é o limiar: abaixo disso cabe em variação de alocador e
# cache de frames; acima, merece investigação antes de chamar de estável.
if ((FIRST_RSS > 0)); then
  GROWTH=$(( (LAST_RSS - FIRST_RSS) * 100 / FIRST_RSS ))
  echo "  crescimento: ${GROWTH}%"
  if ((GROWTH > 15)); then
    red "  RSS cresceu mais de 15% — investigar antes de declarar SC-005 OK."
  else
    green "  RSS estável."
  fi
fi

# As stats vêm do log do app, não daqui: fps e reconnects são do ponto de
# vista dele. `grep || true` de propósito — sem match não é erro, e com
# `set -o pipefail` um `grep -q` num pipe mataria o produtor por SIGPIPE
# (foi um bug real deste projeto, duas vezes).
LOG="$(ls -t "$LOG_DIR"/camlink.*.log 2>/dev/null | head -1 || true)"
if [[ -n "$LOG" && -f "$LOG" ]]; then
  echo
  bold "Do log do app ($LOG)"
  STATS="$(grep -F 'stats da sessão' "$LOG" || true)"
  if [[ -n "$STATS" ]]; then
    echo "  linhas de stats: $(wc -l <<<"$STATS")"
    echo "  fps (min/máx):   $(sed -n 's/.*fps="\([0-9.]*\)".*/\1/p' <<<"$STATS" | sort -g | sed -n '1p;$p' | paste -sd'/')"
    echo "  reconexões:      $(sed -n 's/.*reconnects=\([0-9]*\).*/\1/p' <<<"$STATS" | sort -g | tail -1)"
  else
    yellow "  nenhuma linha de stats da sessão neste log."
    echo "     Significa que NENHUMA fonte transmitiu durante o soak (abra o app,"
    echo "     inicie uma fonte e só então rode isto) — ou que o app é anterior"
    echo "     ao registro periódico de stats."
  fi
  WARNS="$(grep -cE ' WARN | ERROR ' "$LOG" || true)"
  echo "  WARN/ERROR:      ${WARNS:-0}"
  ((${WARNS:-0} > 0)) && echo "     Veja-os com: grep -E ' WARN | ERROR ' '$LOG'"
else
  yellow "Nenhum log encontrado em $LOG_DIR"
fi

echo
echo "CSV completo em $OUT (epoch,elapsed_s,rss_kb,procs)"
