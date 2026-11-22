#!/usr/bin/env bash
# Theme test (aarch64, 1280x800): a picture wallpaper made by `settings wallpaper FILE` (decoded in the sandbox, read by den) and a hex accent, checked by pixels of screendumps.
# Build first: ./build.sh. Usage: test/theme.sh [outdir for the PNGs]
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/aarch64/theme-data.img QEMU_MEM=1024 QEMU_SMP=2 GUI_RES=1280x800; source ./env.sh
[ -f out/aarch64/web.sqsh ] || { echo "no out/aarch64/web.sqsh: run ./build.sh first"; exit 1; }
W=1280; H=800
tmp=$(mktemp -d -t thir-theme); log=$tmp/serial.log; sock=$tmp/mon.sock; qsock=$tmp/qmp.sock; OUTD=${1:-$tmp/png}; mkdir -p "$OUTD"
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
RED=ff0000; BLUE=0000ff; GREEN=00ff00; WALL=08152b; PEACH=f0b27f

USER_NAME=alice; PW=themepass1234   # throwaway credentials for the fresh disk
wait_log '^thir: mounted /web$' 60
wait_log "den: ${W}x${H} 32bpp$" 30
wait_log '^thir: init ready$' 10
sleep 4
keys ret; sleep 2   # the welcome page: Enter goes on to the account
type_str $USER_NAME; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3
sleep 2; keys 0 ret; sleep 3

# the gaps between the panes and the screen edge show the wallpaper: (4,300) is left of the pane (pane starts at x=12), (1276,300) right of it
shot w0; expect w0 4 300 $WALL; expect w0 12 300 $PEACH
echo "ok: colour wallpaper and the peach focused border"

# 1. a picture wallpaper: quad.png (400x300: red green / blue yellow) covers 1280x800 scaled 3.2x and cropped at the top and bottom
type_str "settings wallpaper /data/quad.png"; keys ret
until_px w1 4 300 $RED 60
expect w1 1276 300 $GREEN; expect w1 4 600 $BLUE
if tr -d '\r' < "$log" | grep -q 'surface refused'; then fail "den refused something"; fi
echo "ok: the picture is the wallpaper (cover fit, exact colours in the gaps)"
cp "$tmp/w1.ppm" "$tmp/wallpaper-picture.ppm"

# 2. back to the colour wallpaper
type_str "settings wallpaper reset"; keys ret
until_px w2 4 300 $WALL 30
echo "ok: reset returns the colour wallpaper"

# 3. a hex accent: the focused border takes it live; a colour that is too dark is refused (the border stays)
type_str "settings accent "; keys shift-3; type_str 7fb2f0; keys ret
until_px a1 12 300 7fb2f0 30
type_str "settings accent "; keys shift-3; type_str 101010; keys ret; sleep 3
shot a2; expect a2 12 300 7fb2f0
type_str "settings accent peach"; keys ret
until_px a3 12 300 $PEACH 30
echo "ok: accent #7fb2f0 applied live, #101010 refused, peach restored"

for f in wallpaper-picture; do [ -f "$tmp/$f.ppm" ] && magick "$tmp/$f.ppm" "$OUTD/$f.png"; done
echo "PASS: pictures in $OUTD"
