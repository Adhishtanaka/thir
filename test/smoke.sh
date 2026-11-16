#!/usr/bin/env bash
# Headless boot test: wait for init marker, check eth0 was found, run one shell command, check output, power off.
set -euo pipefail
cd "$(dirname "$0")/.."; source ./env.sh
TIMEOUT=${SMOKE_TIMEOUT:-30}
log=$(mktemp -t thir-smoke); fifo=$(mktemp -u -t thir-fifo); mkfifo "$fifo"
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
wait_for '^thir: init ready$' "$TIMEOUT"
echo "boot-to-marker: ${SECONDS}s"
wait_for '^thir: iface eth0 ' 1
if tr -d '\r' < "$log" | grep -qE '^thir: iface (sit0|tunl0) '; then echo "FAIL: tunnel device present"; cat "$log"; exit 1; fi
wait_for 'dhcp: eth0 10\.0\.2\.15 mask 255\.255\.255\.0 gw 10\.0\.2\.2 dns 10\.0\.2\.3$' 10   # no ^: may follow the "nsh> " prompt
printf 'cat /proc/sys/kernel/hostname\n' >&3
wait_for '^thir$' 10
printf 'cat /etc/resolv.conf\n' >&3
wait_for '^nameserver 10\.0\.2\.3$' 5
printf 'dhcp lo\n' >&3              # never gets a reply: runs ~6 s unless Ctrl-C kills it
sleep 1; printf '\003' >&3
wait_for '\[signal 2\]$' 3   # no ^: the tty echoes ^C first
printf 'poweroff\n' >&3
for _ in $(seq 1 50); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.2; done
if kill -0 "$qpid" 2>/dev/null; then echo "FAIL: QEMU still running after poweroff"; exit 1; fi
echo PASS
