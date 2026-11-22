#!/usr/bin/env bash
# Copy and paste, and the welcome page (aarch64, den, 1280x800): the first-boot welcome page (colours, wordmark), a mouse drag, a double click and a triple click select text in the theme's selected-row
# colour, Ctrl+Shift+C then Ctrl+Shift+V (and Cmd+V) paste it into the shell, the same text goes into the browser's address line, and a copy in the address line comes back to the shell. A multi-line
# paste into nsh joins the lines and runs nothing. Checked by pixels of screendumps. Build first: ./build.sh. Usage: test/clip.sh [outdir for the PNGs]
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/aarch64/clip-data.img QEMU_MEM=1024 QEMU_SMP=2 GUI_RES=1280x800; source ./env.sh
[ -f out/aarch64/web.sqsh ] || { echo "no out/aarch64/web.sqsh: run ./build.sh first"; exit 1; }
W=1280; H=800
tmp=$(mktemp -d -t thir-clip); log=$tmp/serial.log; sock=$tmp/mon.sock; qsock=$tmp/qmp.sock; OUTD=${1:-$tmp/png}; mkdir -p "$OUTD"
trap 'kill "${qpid:-}" 2>/dev/null || true; rm -f "$DATA_IMG"; rm -rf "$tmp"' EXIT
./build.sh data > /dev/null 2>&1
"${QEMU[@]}" -device virtio-mouse-pci -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" -qmp "unix:$qsock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; sleep 0.5
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
qmp() { printf '{"execute":"qmp_capabilities"}\n%s\n' "$1" | nc -U -w1 "$qsock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() { local i c; for ((i = 0; i < ${#1}; i++)); do case "${1:i:1}" in ' ') c=spc;; /) c=slash;; .) c=dot;; -) c=minus;; *) c=${1:i:1};; esac; keys "$c"; done; }
fail() { echo "FAIL: $*"; tail -20 "$log"; exit 1; }
wait_log() { local deadline=$((SECONDS + $2)); until tr -d '\r' < "$log" | grep -qE "$1"; do kill -0 "$qpid" 2>/dev/null && (( SECONDS < deadline )) || fail "did not see /$1/ within $2s"; sleep 0.2; done; }
shot() { mon "screendump $tmp/$1.ppm"; sleep 1; [ -s "$tmp/$1.ppm" ] || fail "no screendump $1"; }
px() { magick "$tmp/$1.ppm" -format '%[hex:p{'"$2,$3"'}]' info: | cut -c1-6 | tr 'A-F' 'a-f'; }
expect() { local got; got=$(px "$1" "$2" "$3"); [ "$got" = "$4" ] || { cp "$tmp/$1.ppm" "$OUTD/fail-$1.ppm" 2>/dev/null; fail "$1: pixel ($2,$3) is $got, wanted $4"; }; }
ev() { qmp '{"execute":"input-send-event","arguments":{"events":['"$1"']}}'; }
move() { local ax=$(( ($1 * 32767 + W - 2) / (W - 1) )) ay=$(( ($2 * 32767 + H - 2) / (H - 1) )); ev '{"type":"abs","data":{"axis":"x","value":'$ax'}},{"type":"abs","data":{"axis":"y","value":'$ay'}}'; }
down() { ev '{"type":"btn","data":{"down":true,"button":"left"}}'; }
up() { ev '{"type":"btn","data":{"down":false,"button":"left"}}'; }
drag() { move "$1" "$2"; sleep 0.3; down; sleep 0.2; move "$3" "$4"; sleep 0.3; up; sleep 0.5; }
multi() { move "$1" "$2"; sleep 0.3; local i; for ((i = 0; i < $3; i++)); do down; up; done; sleep 0.5; }   # N quick clicks
# a cell of the shell pane: x = 33 + 8 col, y = 65 + 16 row (pane at 12, border 1, padding 20 and 16, the title takes 8)
cx() { echo $((33 + 8 * $1 + 4)); }; cy() { echo $((65 + 16 * $1 + 8)); }
same() { magick compare -metric AE -crop "$3" +repage "$tmp/$1.ppm" "$tmp/$2.ppm" null: 2>&1 | grep -q '^0' || { cp "$tmp/$1.ppm" "$OUTD/fail-$1.ppm"; cp "$tmp/$2.ppm" "$OUTD/fail-$2.ppm"; fail "$1 and $2 differ in $3: $4"; }; }
share() { magick "$tmp/$1.ppm" -crop "$2" +repage -fuzz 1% -fill black +opaque "$3" -fill white -opaque "$3" -format '%[fx:mean]' info:; }
PW='correct-horse-1'; SELC=15294a; PEACH='#f0b27f'

wait_log '^thir: mounted /web$' 60
wait_log "den: ${W}x${H} 32bpp$" 30
wait_log '^thir: init ready$' 10
sleep 4
# 0. the welcome page: peach title, the wordmark above, text in the box; Enter goes on
shot wel; cp "$tmp/wel.ppm" "$OUTD/welcome.ppm"
[ "$(echo "$(share wel 56x16+464+472 "$PEACH") > 0.02" | bc -l)" = 1 ] || fail "no peach 'Hello.' on the welcome page"
[ "$(echo "$(share wel 256x96+512+271 "$PEACH") > 0.2" | bc -l)" = 1 ] || fail "no wordmark above the welcome box"
[ "$(echo "$(share wel 360x16+464+568 '#d4dcec') > 0.01" | bc -l)" = 1 ] || fail "no text lines on the welcome page"
echo "ok: the welcome page (peach title, wordmark, plain lines)"
keys ret; sleep 2
type_str alice; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3
sleep 2; keys 0 ret; sleep 3
keys ctrl-l; sleep 1

# 1. a drag selects a row of output in the SEL colour; Ctrl+Shift+C copies it
type_str "echo alpha beta gamma"; keys ret; sleep 1     # row 0: the command, row 1: the output, row 2: the next prompt
shot s0; [ "$(px s0 41 81)" != $SELC ] || fail "something is selected before any click"
drag $(cx 0) $(cy 1) $(cx 15) $(cy 1)
shot s1; expect s1 33 81 $SELC; expect s1 $((33 + 8 * 15 + 7)) 81 $SELC; [ "$(px s1 $((33 + 8 * 16 + 1)) 81)" != $SELC ] || fail "the selection is longer than the drag"
cp "$tmp/s1.ppm" "$OUTD/selection.ppm"
echo "ok: a mouse drag selects text in the theme's selection colour"
keys ctrl-shift-c; sleep 1
keys ctrl-u; type_str "echo alpha beta gamma"; sleep 1; shot ref1      # the reference: the same text typed
keys ctrl-u; sleep 0.5; type_str "echo "; keys ctrl-shift-v; sleep 1; shot p1
same ref1 p1 "640x16+33+113" "Ctrl+Shift+V did not paste the copied text"
echo "ok: Ctrl+Shift+C then Ctrl+Shift+V pasted 'alpha beta gamma' (same pixels as typing it)"
keys ctrl-u; sleep 0.5; keys meta_l-v; sleep 1; shot p2; keys ctrl-u; sleep 0.5; type_str "alpha beta gamma"; sleep 1; shot ref2
same ref2 p2 "640x16+33+113" "Cmd+V did not paste"
echo "ok: Cmd+V pastes too"

# 2. a double click takes a word, a triple click the whole line; the window keys still work with Cmd
keys ctrl-u; sleep 0.5
multi $(cx 7) $(cy 1) 2; shot d1; expect d1 $((33 + 8 * 0 + 1)) 81 "$(px d1 34 81)"
[ "$(px d1 34 81)" != $SELC ] && [ "$(px d1 $((33 + 8 * 6 + 1)) 81)" = $SELC ] && [ "$(px d1 $((33 + 8 * 10 + 1)) 81)" != $SELC ] || fail "a double click did not select just the word 'beta' (cells 6 to 9)"
multi $(cx 7) $(cy 1) 3; shot t1; expect t1 34 81 $SELC; expect t1 $((33 + 8 * 40)) 81 $SELC
echo "ok: double click = a word, triple click = the line"
keys ctrl-shift-c; keys ctrl-u; sleep 0.5

# 3. several lines pasted into nsh run nothing: the lines are joined and wait for Enter (bracketed paste)
keys ctrl-l; sleep 0.5
type_str "write multi.txt"; keys ret; sleep 0.5; type_str "one"; keys ret; sleep 0.3; type_str "two"; keys ret; sleep 0.3; type_str "three"; keys ret; sleep 0.3; type_str "."; keys ret; sleep 1
keys ctrl-l; sleep 0.5; type_str "cat multi.txt"; keys ret; sleep 1
drag $(cx 0) $(cy 1) $(cx 4) $(cy 3)          # the three lines of output
keys ctrl-shift-c; sleep 0.5; keys ctrl-l; sleep 0.5; keys ctrl-shift-v; move 1200 700; sleep 2; shot m1
[ "$(magick "$tmp/m1.ppm" -crop 640x16+33+65 +repage -format %k info:)" -gt 2 ] || fail "nothing on the prompt after the multi-line paste"
[ "$(magick "$tmp/m1.ppm" -crop 640x64+33+81 +repage -format %k info:)" = 1 ] || { cp "$tmp/m1.ppm" "$OUTD/fail-m1.ppm"; fail "the paste ran something or made more lines: the rows under the prompt are not empty"; }
echo "ok: a three-line paste stays on one prompt line (no command ran)"
keys ctrl-u; sleep 0.5; keys ctrl-l; sleep 0.5

# 4. the browser: the den clipboard goes into the address line, and a copy there comes back
keys ctrl-l; type_str "echo alpha beta gamma"; keys ret; sleep 1
drag $(cx 0) $(cy 1) $(cx 15) $(cy 1); keys ctrl-shift-c; sleep 0.5
type_str web; keys ret; sleep 8
keys ctrl-shift-v; sleep 1; shot w1
type_str ""; keys ctrl-a; type_str "alpha beta gamma"; sleep 1; shot wref     # the reference: typed over the pasted text
same w1 wref "1254x16+13+70" "the browser address line did not get the pasted text"
cp "$tmp/w1.ppm" "$OUTD/browser-paste.ppm"
echo "ok: Ctrl+Shift+V pasted the den clipboard into the browser address line"
keys ctrl-a; type_str "zeta eta"; keys ctrl-a ctrl-c; sleep 1
keys ctrl-q; sleep 3
keys ctrl-l; sleep 0.5; keys ctrl-shift-v; sleep 1; shot b1; keys ctrl-u; sleep 0.5; type_str "zeta eta"; sleep 1; shot bref
same b1 bref "640x16+33+65" "the text copied in the browser did not arrive in the shell"
echo "ok: a copy in the browser address line pastes in the shell"
grep -q 'surface refused' "$log" && fail "den refused a surface"
for f in welcome selection browser-paste; do [ -f "$OUTD/$f.ppm" ] && magick "$OUTD/$f.ppm" "$OUTD/$f.png" && rm "$OUTD/$f.ppm"; done
echo "PASS: pictures in $OUTD"
