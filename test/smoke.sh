#!/usr/bin/env bash
# Headless boot test on a fresh data disk. Boot 1: first-boot setup, file commands, denied access, passwd, poweroff.
# Boot 2 (same disk): old password rejected, new one works, files survived. Passwords must never appear in the serial log.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/${THIR_ARCH:-aarch64}/smoke-data.img
source ./env.sh
TIMEOUT=${SMOKE_TIMEOUT:-30}; STEP=${SMOKE_STEP_TIMEOUT:-60}   # argon2 is slow under x86_64 emulation
PW1='correct-horse-1'; PW2='tr0ub4dor-two-2'; PWX='wrongpass-xx'
d=$(mktemp -d -t thir-smoke); fifo=$d/fifo; mkfifo "$fifo"; qpid=
trap 'kill "$qpid" 2>/dev/null || true; rm -rf "$d"' EXIT
./build.sh data > /dev/null 2>&1          # fresh disk for this run only
log=; mark=0

boot() {  # $1 = log name
  log=$d/$1.log; : > "$log"; mark=0
  "${QEMU[@]}" -no-reboot -display none -monitor none -serial stdio < "$fifo" > "$log" 2>&1 &
  qpid=$!
  exec 3>"$fifo"                      # keep the writer open until poweroff
}
wait_for() {  # $1 = extended regex on lines after the last send (CRs stripped), $2 = seconds (default STEP)
  local deadline=$((SECONDS + ${2:-$STEP}))
  until tail -n +$((mark + 1)) "$log" | tr -d '\r' | grep -qE "$1"; do
    if ! kill -0 "$qpid" 2>/dev/null || (( SECONDS >= deadline )); then
      echo "FAIL: did not see /$1/ within ${2:-$STEP}s"; echo "--- serial log ---"; leak_check; cat "$log"; exit 1
    fi
    sleep 0.2
  done
}
send() { mark=$(awk 'END { print NR }' "$log");   # a partial last line (a prompt) counts as old
  printf '%s\n' "$1" >&3; }
leak_check() {
  local p; for p in "$PW1" "$PW2" "$PWX"; do
    if cat "$d"/*.log | grep -qF -- "$p"; then echo "FAIL: a password is in the serial log (log not printed)"; exit 1; fi
  done
}
run() { send "$1"; wait_for "$2"; }   # command, expected output regex
off() {
  send poweroff
  for _ in $(seq 1 50); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.2; done
  if kill -0 "$qpid" 2>/dev/null; then echo "FAIL: QEMU still running after poweroff"; exit 1; fi
  exec 3>&-
}

boot boot1
wait_for '^thir: init ready$' "$TIMEOUT"
echo "boot-to-marker: ${SECONDS}s"
wait_for '^thir: mounted /data$' 5
wait_for '^thir: iface eth0 ' 5
if tr -d '\r' < "$log" | grep -qE '^thir: iface (sit0|tunl0) '; then echo "FAIL: tunnel device present"; cat "$log"; exit 1; fi
wait_for 'dhcp: eth0 10\.0\.2\.15 mask 255\.255\.255\.0 gw 10\.0\.2\.2 dns 10\.0\.2\.3$' 10   # no ^: may follow a prompt
wait_for 'first boot: create the first user'
wait_for 'username: '
send alice;  wait_for 'password: '
send "$PW1"; wait_for 'repeat password: '
send "$PW1"; wait_for 'user alice created'
wait_for 'nsh> '
run whoami '^alice$'
run pwd '^/data/home/alice$'
run 'mkdir docs' 'nsh> '
run 'echo hello > docs/a.txt' 'nsh> '
run 'echo world >> docs/a.txt' 'nsh> '
send 'cat docs/a.txt'; wait_for '^hello$'; wait_for '^world$'
run 'cat /data/etc/users' 'cat: /data/etc/users: Permission denied'
run 'cat /dev/vda' 'cat: /dev/vda: Permission denied'
run 'dhcp lo' '\[exit [1-9]'              # needs root: fails at once as a user
run 'rm /' "refusing to remove '/'"
run 'cat /proc/sys/kernel/hostname' '^thir$'
run 'cat /etc/resolv.conf' '^nameserver 10\.0\.2\.3$'
send passwd; wait_for 'old password: '
send "$PWX"; wait_for 'passwd: wrong password'; t0=$SECONDS
wait_for 'old password: '
(( SECONDS - t0 >= 2 )) || { echo "FAIL: wrong old password was not delayed"; exit 1; }
send "$PW1"; wait_for 'new password: '
send "$PW2"; wait_for 'repeat new password: '
send "$PW2"; wait_for 'passwd: password changed'
wait_for 'nsh> '
off
echo "boot 1 ok"

boot boot2
wait_for 'thir login: ' "$TIMEOUT"
send alice;  wait_for 'password: '
send "$PW1"; wait_for 'login incorrect'
wait_for 'thir login: '
send alice;  wait_for 'password: '
send "$PW2"; wait_for 'nsh> '
run 'cat docs/a.txt' '^hello$'
run ls '^docs/$'
off
leak_check
echo PASS
