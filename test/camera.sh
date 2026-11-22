#!/usr/bin/env bash
# The camera app (aarch64, den, 1280x800) on the kernel's virtual camera (vivid, a TEST kernel: `THIR_VIVID=1 ./build.sh` makes out/aarch64-vivid; release kernels have UVC only).
# Checks: the question in den's bar, the preview (live frames), the red cam light, a photo in ~/Pictures (0600, a real PNG with the test pattern), a refused question, and "no camera found" on a kernel without one.
# Build first. Usage: test/camera.sh [outdir for the PNGs]. Skips (exit 0) when out/aarch64-vivid is not built.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -f out/aarch64-vivid/Image ] || { echo "SKIP: no out/aarch64-vivid (run: THIR_VIVID=1 ./build.sh) - there is no virtual camera without it"; exit 0; }
export THIR_VIVID=1 DATA_IMG=out/aarch64-vivid/cam-data.img QEMU_MEM=1024 QEMU_SMP=2 GUI_RES=1280x800; source ./env.sh
W=1280; H=800
tmp=$(mktemp -d -t thir-cam); log=$tmp/serial.log; sock=$tmp/mon.sock; OUTD=${1:-$tmp/png}; mkdir -p "$OUTD"
trap 'kill "${qpid:-}" 2>/dev/null || true; rm -f out/aarch64-vivid/cam-data.img out/aarch64/cam-data.img; rm -rf "$tmp"' EXIT
./build.sh data > /dev/null 2>&1
start() { "${QEMU[@]}" "$@" -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" > "$tmp/qemu.out" 2>&1 & qpid=$!; sleep 0.5; }
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() { local i c; for ((i = 0; i < ${#1}; i++)); do case "${1:i:1}" in ' ') c=spc;; /) c=slash;; .) c=dot;; -) c=minus;; *) c=${1:i:1};; esac; keys "$c"; done; }
fail() { echo "FAIL: $*"; tail -20 "$log"; exit 1; }
wait_log() { local deadline=$((SECONDS + $2)); until tr -d '\r' < "$log" | grep -qE "$1"; do kill -0 "$qpid" 2>/dev/null && (( SECONDS < deadline )) || fail "did not see /$1/ within $2s"; sleep 0.2; done; }
shot() { mon "screendump $tmp/$1.ppm"; sleep 1; [ -s "$tmp/$1.ppm" ] || fail "no screendump $1"; }
share() { magick "$tmp/$1.ppm" -crop "$2" +repage -fuzz 1% -fill black +opaque "$3" -fill white -opaque "$3" -format '%[fx:mean]' info:; }
colors() { magick "$tmp/$1.ppm" -crop "$2" +repage -format %k info:; }
PW='correct-horse-1'; PEACH='#f0b27f'; ERRC='#f2808b'
login() {
  wait_log '^thir: mounted /web$' 60; wait_log "den: ${W}x${H} 32bpp$" 30; wait_log '^thir: init ready$' 10; sleep 4
  keys ret; sleep 2; type_str alice; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
  keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3; sleep 2; keys 0 ret; sleep 3
}
start -device virtio-mouse-pci
login
# 1. the question: den's own box in the bar; a deny (n) ends the app calmly
type_str camera; keys ret; sleep 3
shot q0; cp "$tmp/q0.ppm" "$OUTD/camera-ask.ppm"
[ "$(echo "$(share q0 460x24+410+2 "$PEACH") > 0.01" | bc -l)" = 1 ] || fail "no question box in the bar"
keys n; sleep 3; shot q1
[ "$(echo "$(share q1 460x24+410+2 "$PEACH") < 0.001" | bc -l)" = 1 ] || fail "the question is still up after n"
echo "ok: den asks in its own bar; n refuses, the app ends"
# 2. y: the preview runs, the red cam light shows in the bar, frames change
type_str camera; keys ret; sleep 3; keys y; sleep 4
shot c0; sleep 2; shot c1; cp "$tmp/c1.ppm" "$OUTD/camera-preview.ppm"
[ "$(colors c0 1000x500+150+150)" -gt 8 ] || fail "no picture in the pane (colours: $(colors c0 1000x500+150+150))"
[ "$(echo "$(share c0 300x24+490+2 "$ERRC") > 0.002" | bc -l)" = 1 ] || fail "no red cam light in the bar"
[ "$(magick compare -metric AE "$tmp/c0.ppm" "$tmp/c1.ppm" null: 2>&1 | cut -d' ' -f1 | cut -d. -f1)" -gt 50 ] || fail "the preview does not change (not live)"
echo "ok: preview is live (many colours, frames change), the red cam light is on"
# 3. space: a photo; Alt+R stops the camera (light off, app ends)
keys spc; sleep 2; shot c2
keys alt-r; sleep 3; shot c3
[ "$(echo "$(share c3 300x24+490+2 "$ERRC") < 0.001" | bc -l)" = 1 ] || fail "the cam light is still on after Alt+R"
echo "ok: space took a photo, Alt+R stopped the camera"
type_str poweroff; keys ret
for _ in $(seq 50); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.2; done
kill -0 "$qpid" 2>/dev/null && fail "QEMU still running after poweroff"
# 4. the photo: in ~/Pictures, 0600, a real PNG
docker run --rm -v "$PWD":/work -v "$tmp":/o thir-build sh -c "debugfs -R 'ls -l /home/alice/Pictures' /work/$DATA_IMG > /o/ls.txt 2>&1; f=\$(debugfs -R 'ls -p /home/alice/Pictures' /work/$DATA_IMG 2>/dev/null | grep -o 'photo-[0-9-]*\.png' | head -1); echo \$f > /o/name.txt; debugfs -R \"dump /home/alice/Pictures/\$f /o/photo.png\" /work/$DATA_IMG > /dev/null 2>&1" > /dev/null
name=$(cat "$tmp/name.txt"); [ -n "$name" ] || { cat "$tmp/ls.txt"; fail "no photo in ~/Pictures"; }
grep -q "100600" "$tmp/ls.txt" || { cat "$tmp/ls.txt"; fail "the photo is not 0600"; }
[ "$(magick identify -format '%wx%h %m' "$tmp/photo.png")" = "640x480 PNG" ] || fail "the photo is not a 640x480 PNG: $(magick identify "$tmp/photo.png" 2>&1)"
cp "$tmp/photo.png" "$OUTD/camera-photo.png"
[ "$(magick "$tmp/photo.png" -format %k info: 2>&1)" -gt 6 ] || fail "the photo has no test pattern: $(magick "$tmp/photo.png" -format %k info: 2>&1 | head -3)"
echo "ok: ~/Pictures/$name is a 640x480 PNG with the test pattern, mode 0600"
# 5. a kernel without a camera: "no camera found", calmly (the release kernel has no virtual camera)
if [ -f out/aarch64/Image ]; then
  THIR_VIVID=; export THIR_VIVID DATA_IMG=out/aarch64/cam-data.img; source ./env.sh; ./build.sh data > /dev/null 2>&1
  : > "$log"; start -device virtio-mouse-pci; login
  type_str camera; keys ret; sleep 4; shot n0; cp "$tmp/n0.ppm" "$OUTD/camera-none.ppm"
  [ "$(echo "$(share n0 460x24+410+2 "$PEACH") < 0.001" | bc -l)" = 1 ] || fail "a question was asked although there is no camera"
  echo "ok: with no camera the app says so at once and asks nothing"
  kill "$qpid" 2>/dev/null || true
fi
for f in camera-ask camera-preview camera-none; do [ -f "$OUTD/$f.ppm" ] && magick "$OUTD/$f.ppm" "$OUTD/$f.png" && rm "$OUTD/$f.ppm"; done
echo "PASS: pictures in $OUTD"
