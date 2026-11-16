#!/usr/bin/env bash
# Headless boot test: wait for init marker, check eth0 was found, run one shell command, check output, power off.
set -euo pipefail
cd "$(dirname "$0")/.."; source ./env.sh
TIMEOUT=${SMOKE_TIMEOUT:-30}
log=$(mktemp -t netos-smoke); fifo=$(mktemp -u -t netos-fifo); mkfifo "$fifo"
cleanup() { kill "$qpid" 2>/dev/null || true; rm -f "$fifo"; }
"${QEMU[@]}" -no-reboot -display none -monitor none -serial stdio < "$fifo" > "$log" 2>&1 &
qpid=$!; trap cleanup EXIT
exec 3>"$fifo"                      # keep the writer open until poweroff

wait_for() {  # $1 = extended regex (CRs stripped), $2 = seconds
  local deadline=$((SECONDS + $2))
  until tr -d '\r' < "$log" | grep -qE "$1"; do
    if ! kill -0 "$qpid" 2>/dev/null || (( SECONDS >= deadline )); then
      echo "FAIL: did not see /$1/ within $2s"; echo "--- serial log ---"; cat "$log"; exit 1
    fi
    sleep 0.2
  done
}
wait_for '^netos: init ready$' "$TIMEOUT"
echo "boot-to-marker: ${SECONDS}s"
wait_for '^netos: iface eth0 ' 1
printf 'cat /proc/sys/kernel/hostname\n' >&3
wait_for '^netos$' 10
printf 'poweroff\n' >&3
for _ in $(seq 1 50); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.2; done
if kill -0 "$qpid" 2>/dev/null; then echo "FAIL: QEMU still running after poweroff"; exit 1; fi
echo PASS
