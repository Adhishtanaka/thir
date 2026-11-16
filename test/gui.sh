#!/usr/bin/env bash
# Headless GUI test (aarch64): type into den with the QEMU monitor, screendump, check the picture. Usage: test/gui.sh [out.png]
set -euo pipefail
cd "$(dirname "$0")/.."; source ./env.sh
PNG=${1:-docs/den.png}; mkdir -p "$(dirname "$PNG")"
tmp=$(mktemp -d -t thir-gui); log=$tmp/serial.log; sock=$tmp/mon.sock
"${QEMU[@]}" -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; trap 'kill "$qpid" 2>/dev/null || true' EXIT; sleep 0.5
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() {  # lowercase letters, space, / and . only
  local i c; for ((i = 0; i < ${#1}; i++)); do
    case "${1:i:1}" in ' ') c=spc;; /) c=slash;; .) c=dot;; *) c=${1:i:1};; esac; keys "$c"
  done
}
wait_for() {
  local deadline=$((SECONDS + $2))
  until tr -d '\r' < "$log" | grep -qE "$1"; do
    if ! kill -0 "$qpid" 2>/dev/null || (( SECONDS >= deadline )); then echo "FAIL: did not see /$1/ within $2s"; tail -20 "$log"; exit 1; fi
    sleep 0.2
  done
}
wait_for 'den: 1280x800 32bpp$' 30
sleep 4                        # dhcp lease and the first redraw
type_str ip; keys ret
keys alt-ret                   # split: the new pane gets the focus
sleep 1
type_str "cat /etc/resolv.conf"; keys ret
sleep 2
mon "screendump $tmp/s.ppm"; sleep 1
[ -s "$tmp/s.ppm" ] || { echo "FAIL: no screendump"; exit 1; }
magick "$tmp/s.ppm" "$PNG"
colors=$(magick identify -format %k "$PNG")
[ "$colors" -gt 100 ] || { echo "FAIL: screenshot has only $colors colors"; exit 1; }
echo "PASS: $PNG ($colors colors)"
