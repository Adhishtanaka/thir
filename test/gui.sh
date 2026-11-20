#!/usr/bin/env bash
# Headless GUI test (aarch64, 2560x1664): type into den with the QEMU monitor, screendump, check the picture. Usage: test/gui.sh [out.png]
set -euo pipefail
cd "$(dirname "$0")/.."
export GUI_RES=2560x1664 DATA_IMG=out/${THIR_ARCH:-aarch64}/gui-data.img; source ./env.sh   # its own fresh disk
./build.sh data > /dev/null 2>&1
PNG=${1:-docs/den.png}; mkdir -p "$(dirname "$PNG")"
tmp=$(mktemp -d -t thir-gui); log=$tmp/serial.log; sock=$tmp/mon.sock
"${QEMU[@]}" -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; trap 'kill "$qpid" 2>/dev/null || true' EXIT; sleep 0.5
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() {  # lowercase letters, digits, space, / and . only
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
shot() { mon "screendump $tmp/$1.ppm"; sleep 1; [ -s "$tmp/$1.ppm" ] || { echo "FAIL: no screendump $1"; exit 1; }; }
USER_NAME=alice; PW=guipass1234   # throwaway test credentials for the fresh disk; typed with sendkey, never logged
wait_for 'den: 2560x1664 32bpp$' 30
wait_for '^thir: init ready$' 5
sleep 4                        # dhcp lease and the first redraw
# first boot: pane 1 asks for the first user (the typed password is not echoed)
type_str $USER_NAME; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3   # utc offset +05:30
sleep 2; keys 0 ret; sleep 2   # AI setup: no AI
keys alt-q; sleep 2            # closing the last pane = logout: a fresh login screen
type_str $USER_NAME; keys ret; sleep 1; type_str $PW   # password typed, Enter not yet pressed
shot login; cp "$tmp/login.ppm" "$tmp/login-keep.ppm"
keys ret; sleep 4
type_str ip; keys ret
keys alt-ret                   # split: the new pane logs in as the same user
sleep 4
type_str whoami; keys ret; sleep 1
type_str date; keys ret; sleep 1
type_str ca; keys tab; type_str /etc/res; keys tab; keys ret   # Tab completion
sleep 2
shot s
magick "$tmp/s.ppm" "$PNG"
magick "$tmp/login-keep.ppm" "${PNG%.png}-login.png"
colors=$(magick identify -format %k "$PNG")
[ "$(magick identify -format %wx%h "$PNG")" = 2560x1664 ] || { echo "FAIL: wrong screenshot size"; exit 1; }
has() { magick "$1" -unique-colors txt:- | grep -qi "#$2"; }   # does the picture contain this exact colour
has "$PNG" F0B27F || { echo "FAIL: no peach (focused border, pane title) on the desktop"; exit 1; }
has "$PNG" 08152B || { echo "FAIL: no wall/bar blue on the desktop"; exit 1; }
has "$PNG" 0B1C36 || { echo "FAIL: no pane fill"; exit 1; }
has "${PNG%.png}-login.png" F0B27F || { echo "FAIL: the login wordmark (peach) is missing"; exit 1; }
if grep -qF -- "$PW" "$log"; then echo "FAIL: password in the serial log"; exit 1; fi
[ "$colors" -gt 4 ] || { echo "FAIL: screenshot has only $colors colors"; exit 1; }
keys alt-q alt-q; sleep 3      # both panes closed: back to the login screen
shot out; magick "$tmp/out.ppm" "${PNG%.png}-logout.png"
has "${PNG%.png}-logout.png" F0B27F || { echo "FAIL: the logout screen has no wordmark"; exit 1; }
echo "PASS: $PNG ($colors colors); also ${PNG%.png}-login.png and ${PNG%.png}-logout.png"
