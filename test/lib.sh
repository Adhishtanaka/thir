# Shared helpers for test/smoke.sh and test/uefi.sh (sourced after env.sh). Callers set SECRETS=(...) and may set STEP.
STEP=${STEP:-60}
export LC_ALL=C   # bytes, not characters: a log read while QEMU writes it can end in half a UTF-8 character
d=$(mktemp -d -t thir-test); fifo=$d/fifo; mkfifo "$fifo"; qpid=
trap 'kill "$qpid" ${mpid:-} 2>/dev/null || true; rm -rf "$d"' EXIT
log=; mark=0

boot() {  # $1 = log name, rest = full QEMU command
  log=$d/$1.log; : > "$log"; mark=0; shift
  "$@" -no-reboot -display none -monitor none -serial stdio < "$fifo" > "$log" 2>&1 &
  qpid=$!
  exec 3>"$fifo"                      # keep the writer open until poweroff
}
wait_for() {  # $1 = extended regex on lines after the last send (CRs stripped), $2 = seconds (default STEP)
  local deadline=$((SECONDS + ${2:-$STEP}))
  until tail -n +$((mark + 1)) "$log" | tr -d '\r' | sed -E $'s/\e\\[[0-9;?]*[A-Za-z~]//g' | grep -qE "$1"; do
    if ! kill -0 "$qpid" 2>/dev/null || (( SECONDS >= deadline )); then
      echo "FAIL: did not see /$1/ within ${2:-$STEP}s"; echo "--- serial log ---"; leak_check; cat "$log"; exit 1
    fi
    sleep 0.2
  done
}
send() { mark=$(awk 'END { print NR }' "$log");   # a partial last line (a prompt) counts as old
  printf '%s\n' "$1" >&3; }
leak_check() {
  local p; for p in "${SECRETS[@]}"; do
    if cat "$d"/*.log | grep -qF -- "$p"; then echo "FAIL: a password is in the serial log (log not printed)"; exit 1; fi
  done
}
sendk() { printf '%s' "$1" >&3; }   # keys, no newline; keeps `mark` because full-screen output has no newlines
run() { send "$1"; wait_for "$2"; }   # command, expected output regex
off() {
  send poweroff
  for _ in $(seq 1 50); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.2; done
  if kill -0 "$qpid" 2>/dev/null; then echo "FAIL: QEMU still running after poweroff"; exit 1; fi
  exec 3>&-
}
