#!/usr/bin/env bash
# THIR_WEB=1 GUI test (aarch64, 1280x800): the surface protocol, the mouse and `view`. Boots the web image headless, types into den with the QEMU monitor, moves and clicks the
# virtio-tablet with QMP input-send-event, and checks pixels of screendumps. Build the add-on image first: THIR_WEB=1 ./build.sh. Usage: test/view.sh [outdir for the PNGs]
set -euo pipefail
cd "$(dirname "$0")/.."
export THIR_WEB=1 DATA_IMG=out/aarch64/view-data.img QEMU_MEM=1024 QEMU_SMP=2 GUI_RES=1280x800; source ./env.sh
[ -f out/aarch64/web.sqsh ] || { echo "no out/aarch64/web.sqsh: run THIR_WEB=1 ./build.sh first"; exit 1; }
W=1280; H=800
tmp=$(mktemp -d -t thir-view); log=$tmp/serial.log; sock=$tmp/mon.sock; qsock=$tmp/qmp.sock; OUTD=${1:-$tmp/png}; mkdir -p "$OUTD"
trap 'kill "${qpid:-}" 2>/dev/null || true; rm -f "$DATA_IMG"; rm -rf "$tmp"' EXIT
# the fixture: 400x300, four colour quadrants (red green / blue yellow); written into the data disk's root, so the guest sees /data/quad.png
magick -size 400x300 xc:'rgb(255,0,0)' \( -size 200x150 xc:'rgb(0,255,0)' \) -geometry +200+0 -composite \( -size 200x150 xc:'rgb(0,0,255)' \) -geometry +0+150 -composite \
  \( -size 200x150 xc:'rgb(255,255,0)' \) -geometry +200+150 -composite PNG24:"$tmp/quad.png"
./build.sh data > /dev/null 2>&1
docker run --rm -v "$PWD":/work -v "$tmp":/fx thir-build sh -c "chmod 644 /fx/quad.png && debugfs -w -R 'write /fx/quad.png quad.png' /work/$DATA_IMG" > /dev/null
"${QEMU[@]}" -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" -qmp "unix:$qsock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; sleep 0.5
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
qmp() { printf '{"execute":"qmp_capabilities"}\n%s\n' "$1" | nc -U -w1 "$qsock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() { local i c; for ((i = 0; i < ${#1}; i++)); do case "${1:i:1}" in ' ') c=spc;; /) c=slash;; .) c=dot;; *) c=${1:i:1};; esac; keys "$c"; done; }
fail() { echo "FAIL: $*"; tail -20 "$log"; exit 1; }
wait_log() { local deadline=$((SECONDS + $2)); until tr -d '\r' < "$log" | grep -qE "$1"; do kill -0 "$qpid" 2>/dev/null && (( SECONDS < deadline )) || fail "did not see /$1/ within $2s"; sleep 0.2; done; }
shot() { mon "screendump $tmp/$1.ppm"; sleep 1; [ -s "$tmp/$1.ppm" ] || fail "no screendump $1"; }
px() { magick "$tmp/$1.ppm" -format '%[hex:p{'"$2,$3"'}]' info: | cut -c1-6 | tr 'A-F' 'a-f'; }   # shot name, x, y -> rrggbb
# expect NAME X Y RRGGBB: the pixel of the last shot must match
expect() { local got; got=$(px "$1" "$2" "$3"); [ "$got" = "$4" ] || { cp "$tmp/$1.ppm" "$OUTD/fail-$1.ppm" 2>/dev/null; fail "$1: pixel ($2,$3) is $got, wanted $4"; }; }
# until NAME X Y RRGGBB SECONDS: take shots until the pixel matches (the app needs a moment)
until_px() { local deadline=$((SECONDS + $5)); while true; do shot "$1"; [ "$(px "$1" "$2" "$3")" = "$4" ] && return 0; (( SECONDS < deadline )) || expect "$1" "$2" "$3" "$4"; done; }
move() { local ax=$(( ($1 * 32767 + W - 2) / (W - 1) )) ay=$(( ($2 * 32767 + H - 2) / (H - 1) ))   # the exact inverse of den's scaling
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"abs","data":{"axis":"x","value":'$ax'}},{"type":"abs","data":{"axis":"y","value":'$ay'}}]}}'; sleep 0.3; }
click() { move "$1" "$2"
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":true,"button":"left"}}]}}'; sleep 0.2
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":false,"button":"left"}}]}}'; sleep 0.5; }
RED=ff0000; GREEN=00ff00; BLUE=0000ff; YELLOW=ffff00; ACCENT=f0b27f; DIM=1e3050

USER_NAME=alice; PW=viewpass1234   # throwaway credentials for the fresh disk
wait_log '^thir: mounted /web$' 60
wait_log "den: ${W}x${H} 32bpp$" 30
wait_log '^thir: init ready$' 10
sleep 4
type_str $USER_NAME; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3
sleep 2; keys 0 ret; sleep 3

# 1. view in a single pane: the picture is centred at 1:1; surface = pane inside the border at (13,49) 1254x738, status line 22 rows at the bottom
type_str "view /data/quad.png"; keys ret
until_px v0 540 332 $RED 40
expect v0 740 332 $GREEN; expect v0 540 482 $BLUE; expect v0 740 482 $YELLOW
cp "$tmp/v0.ppm" "$tmp/view-image.ppm"
# the status line is den-drawn pixels of the app: bar colour at the bottom of the pane
expect v0 100 780 0b1c36
if tr -d '\r' < "$log" | grep -q 'surface refused\|surface dropped'; then fail "den refused or dropped view"; fi
echo "ok: view shows the picture (colour quadrants exact)"

# 2. mouse: the cursor appears where the tablet points (white outline, black fill; the tip is outline)
move 900 600; shot m0
expect m0 900 600 ffffff; expect m0 901 602 000000; expect m0 901 603 000000
cp "$tmp/m0.ppm" "$tmp/view-cursor.ppm"
move 100 100; shot m1; [ "$(px m1 901 602)" != 000000 ] || fail "old cursor not erased"; expect m1 101 102 000000
echo "ok: mouse cursor follows the tablet and leaves no trail"

# 3. keys reach view (r = rotate a quarter turn clockwise): the red quadrant moves from top-left to top-right; the picture is now 300x400 at (490,183)
keys r
until_px v1 715 307 $RED 15
expect v1 565 307 $BLUE; expect v1 565 507 $YELLOW; expect v1 715 507 $GREEN
echo "ok: key r rotates the picture"

# 4. split: pane 2 (new, text) has the focus; view loses it and gets a smaller surface; r goes to the shell, not to view
keys alt-ret; sleep 5; type_str ip; keys ret; sleep 2
shot s0; expect s0 646 394 $ACCENT; expect s0 12 394 $DIM
expect s0 398 307 $RED; expect s0 248 307 $BLUE   # view re-attached at 623x688: the picture (300x400, rotated once) sits at (172,183)
keys r; sleep 1; shot s1; expect s1 398 307 $RED
cp "$tmp/s0.ppm" "$tmp/view-split.ppm"
echo "ok: split resizes view; keys stay with the focused pane"

# 5. click on the view pane focuses it; then r rotates again (180 degrees in all: yellow top-left)
click 322 394; shot c0; expect c0 12 394 $ACCENT; expect c0 646 394 $DIM
keys r
until_px c1 223 332 $YELLOW 15
expect c1 423 332 $BLUE; expect c1 223 482 $GREEN; expect c1 423 482 $RED
echo "ok: click focuses the pane and the app gets the keys"
# 6. click on the text pane focuses it again
click 900 394; shot c2; expect c2 646 394 $ACCENT; expect c2 12 394 $DIM

# 7. q ends view: the pane shows its text again
click 322 394; keys q; sleep 2; shot q0
[ "$(px q0 223 332)" != $YELLOW ] || fail "the picture is still there after q"
echo "ok: view quit, the pane shows text again"

for n in view-image view-cursor view-split; do magick "$tmp/$n.ppm" "$OUTD/$n.png"; done
magick "$tmp/view-cursor.ppm" -crop 160x160+830+540 +repage -filter point -resize 400% "$OUTD/view-cursor-zoom.png"
echo "PASS: pictures in $OUTD"
