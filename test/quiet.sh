#!/usr/bin/env bash
# Quiet screens (aarch64 full image, 1280x800): no stray log text over the login screen or in a pane after web, play, view and settings wallpaper.
# Boots headless, drives den with the QEMU monitor, screendumps, and reads the picture with tesseract. Build the add-on image first: ./build.sh. Usage: test/quiet.sh [outdir for the PNGs]
# Also: the browser's own log is ~/.cache/thir-web/log (0600) and the guest serial console shows no kernel warnings after init starts.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/aarch64/quiet-data.img QEMU_MEM=1024 QEMU_SMP=2 GUI_RES=1280x800; source ./env.sh
[ -f out/aarch64/web.sqsh ] || { echo "no out/aarch64/web.sqsh: run ./build.sh first"; exit 1; }
command -v tesseract > /dev/null || { echo "FAIL: tesseract (OCR) is needed"; exit 1; }
FX=out/aarch64/media-fixtures
W=1280; H=800
tmp=$(mktemp -d -t thir-quiet); log=$tmp/serial.log; sock=$tmp/mon.sock; OUTD=${1:-$tmp/png}; mkdir -p "$OUTD"
trap 'kill "${qpid:-}" ${mpid:-} 2>/dev/null || true; rm -f "$DATA_IMG"; rm -rf "$tmp"' EXIT
python3 test/mock.py "$tmp" & mpid=$!
for _ in $(seq 50); do [ -f "$tmp/srv.pem" ] && (echo > /dev/tcp/127.0.0.1/18080) 2>/dev/null && break; sleep 0.2; done
magick -size 400x300 xc:'rgb(255,0,0)' PNG24:"$tmp/quad.png"
[ -f $FX/tone.wav ] || { echo "no $FX: run test/media.sh once first"; exit 1; }
./build.sh data > /dev/null 2>&1
docker run --rm -v "$PWD":/work -v "$tmp":/fx -v "$PWD/$FX":/mx thir-build sh -c "chmod 644 /fx/quad.png && { echo write /fx/quad.png quad.png; echo write /mx/tone.wav tone.wav; echo write /mx/quad.mp4 quad.mp4; } | debugfs -w -f - /work/$DATA_IMG" > /dev/null
"${QEMU[@]}" -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; sleep 0.5
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() { local i c; for ((i = 0; i < ${#1}; i++)); do case "${1:i:1}" in ' ') c=spc;; /) c=slash;; .) c=dot;; -) c=minus;; :) c=shift-semicolon;; *) c=${1:i:1};; esac; keys "$c"; done; }
fail() { echo "FAIL: $*"; tail -20 "$log"; exit 1; }
wait_log() { local deadline=$((SECONDS + $2)); until tr -d '\r' < "$log" | grep -qE "$1"; do kill -0 "$qpid" 2>/dev/null && (( SECONDS < deadline )) || fail "did not see /$1/ within $2s"; sleep 0.2; done; }
shot() { mon "screendump $tmp/$1.ppm"; sleep 1; [ -s "$tmp/$1.ppm" ] || fail "no screendump $1"; }
# ocr NAME: the text of the screendump (upscaled and thresholded so the 16 px font reads cleanly)
ocr() { magick "$tmp/$1.ppm" -colorspace Gray -resize 200% -negate -threshold 35% "$tmp/$1.png"; tesseract "$tmp/$1.png" - --psm 6 2> /dev/null | tr -s ' \n' ' '; }
# clean NAME: fail when the screen text holds a word that only a log line has
STRAY='[Ff]ontconfig|libEGL|GLib|GIO|Gtk|MESA|Mesa|WebKit|warning|ffmpeg|Parsed_|libav|Stream #|Duration:|\[ +[0-9]+\.[0-9]+\]|kernel:|virtio|EXT4|Unable to|cannot open|error:'
clean() { local t; t=$(ocr "$1"); if printf '%s' "$t" | grep -qE "$STRAY"; then cp "$tmp/$1.ppm" "$OUTD/fail-$1.ppm"; fail "stray log text on screen $1: $t"; fi; echo "ok: $1 is quiet"; }

USER_NAME=alice; PW=quietpass1234   # throwaway credentials for the fresh disk
wait_log '^thir: mounted /web$' 60
wait_log "den: ${W}x${H} 32bpp$" 30
wait_log '^thir: init ready$' 10
sleep 4
shot l0; clean l0                      # the first-boot login screen
keys ret; sleep 2   # the welcome page: Enter goes on to the account
type_str $USER_NAME; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3
sleep 2; keys 0 ret; sleep 3
type_str clear; keys ret; sleep 1
BASE=http://10.0.2.2:18080/web
type_str "web $BASE/js.html"; keys ret; sleep 25; keys ctrl-q; sleep 3; shot w0; clean w0
type_str "play /data/tone.wav"; keys ret; sleep 4; keys q; sleep 2; shot p0; clean p0
type_str "play /data/quad.mp4"; keys ret; sleep 5; keys q; sleep 2; shot p1; clean p1
type_str "view /data/quad.png"; keys ret; sleep 4; keys q; sleep 2; shot v0; clean v0
type_str settings; keys ret; sleep 2; keys down; keys right; sleep 3; shot s0; keys q; sleep 2; shot s1; clean s1
keys alt-q; sleep 3; shot l1; clean l1     # logged out: the login screen again
if tr -d '\r' < "$log" | grep -qE '^\[ +[0-9]+\.[0-9]+\]'; then fail "kernel messages reached the console (loglevel)"; fi
echo "ok: the serial console holds no kernel messages"
# the browser's own log: after a clean shutdown ~/.cache/thir-web/log holds nothing a user would call noise (no denied files, no fontconfig, cache or GStreamer complaints)
type_str $USER_NAME; keys ret; sleep 1; type_str $PW; keys ret; sleep 5; type_str poweroff; keys ret
for _ in $(seq 60); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.5; done
kill -0 "$qpid" 2>/dev/null && fail "QEMU still running after poweroff"
docker run --rm -v "$PWD":/work thir-build sh -c "debugfs -R 'cat /home/$USER_NAME/.cache/thir-web/log' /work/$DATA_IMG 2>/dev/null" > "$tmp/webkit.log" || true
NOISE='denied|Permission|fontconfig|Fontconfig|cache|Cannot|cannot|error|Error|warning|Warning|GStreamer|gst|ALSA|alsa|critical|CRITICAL|Failed|failed'
if grep -qE "$NOISE" "$tmp/webkit.log"; then cp "$tmp/webkit.log" "$OUTD/webkit.log"; fail "the browser log is noisy: $(grep -E "$NOISE" "$tmp/webkit.log" | head -5)"; fi
echo "ok: the browser log has no noise ($(wc -c < "$tmp/webkit.log" | tr -d ' ') bytes)"
echo "PASS: no stray log text on the login screen or in the pane after web, play, view and settings"
