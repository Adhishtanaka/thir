#!/usr/bin/env bash
# Records the demo: boots thir headless in QEMU (aarch64, HVF, 2560x1600), drives den with `sendkey` through the QEMU monitor,
# grabs frames with `screendump`, and makes docs/demo.mp4, docs/demo.gif and docs/screenshots/*.png with ffmpeg and ImageMagick.
# The first user is `thir`; its password comes from the environment and is never written anywhere:  DEMO_PW=... test/demo.sh
set -euo pipefail
export LC_ALL=C   # [A-Z] in the key map must not match lowercase
cd "$(dirname "$0")/.."
[ -n "${DEMO_PW:-}" ] || { echo "demo.sh: set DEMO_PW (the password for the demo user); it is never stored" >&2; exit 1; }
export GUI_RES=2560x1600 DATA_IMG=out/aarch64/demo-data.img THIR_ARCH=aarch64; source ./env.sh
./build.sh data > /dev/null 2>&1                       # fresh disk: den shows the first-boot screen
SHOTS=docs/screenshots; mkdir -p "$SHOTS"
tmp=$(mktemp -d -t thir-demo); fr=$tmp/frames; mkdir "$fr"; log=$tmp/serial.log; sock=$tmp/mon.sock; fifo=$tmp/mon.fifo; mkfifo "$fifo"
"${QEMU[@]}" -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; cpid=; npid=
trap 'kill $cpid $npid $qpid 2>/dev/null || true; rm -rf "$tmp"' EXIT
sleep 0.5
exec 5<>"$fifo"                                         # one long-lived monitor connection; commands are lines written to fd 5
nc -U "$sock" <&5 > /dev/null & npid=$!
mon() { printf '%s\n' "$1" >&5; }
key() { mon "sendkey $1"; sleep "0.0$((6 + RANDOM % 4))"; }   # human-ish: 60-110 ms per key
typ() {  # printable ASCII a-z A-Z 0-9 and the punctuation the storyboard uses
  local i c; for ((i = 0; i < ${#1}; i++)); do
    c=${1:i:1}
    case "$c" in
      ' ') c=spc;; /) c=slash;; .) c=dot;; -) c=minus;; "'") c=apostrophe;; ,) c=comma;; '*') c=shift-8;; _) c=shift-minus;;
      [A-Z]) c=shift-$(tr A-Z a-z <<< "$c");;
    esac
    key "$c"
  done
}
cmd() { typ "$1"; sleep 0.5; key ret; sleep "${2:-1.2}"; }   # type, pause so the viewer can read it, Enter, pause
still() { local f=$tmp/$1.ppm; mon "screendump $f"; sleep 1.5; [ -s "$f" ] || { echo "FAIL: no screendump $1"; exit 1; }
  magick "$f" -define png:compression-level=9 "$SHOTS/$1.png"; }
wait_log() { local t=$((SECONDS + $2)); until tr -d '\r' < "$log" | grep -qE "$1"; do
  kill -0 "$qpid" 2>/dev/null && (( SECONDS < t )) || { echo "FAIL: did not see /$1/ within $2s"; exit 1; }; sleep 0.2; done; }
capture() { local n=0 f; while :; do f=$fr/$(printf %05d $n).ppm; mon "screendump $f"; n=$((n + 1))
  for _ in $(seq 1 50); do [ -s "$f" ] && break; sleep 0.02; done; sleep 0.12; done; }

wait_log 'den: 2560x1600 32bpp$' 30; wait_log '^thir: init ready$' 5; sleep 4
t0=$(date +%s); capture & cpid=$!
sleep 2.5                                               # the first-boot screen
typ thir; key ret; sleep 1; typ "$DEMO_PW"; key ret; sleep 1; typ "$DEMO_PW"; key ret; sleep 1
key ret; sleep 3.5                                      # time zone prompt: accept the default; Argon2 hashes meanwhile
mon "sendkey alt-q"; sleep 2.5                          # close the only pane = log out: the login screen
typ thir; key ret; sleep 1; still login                 # user name typed, password not yet (and never echoed)
typ "$DEMO_PW"; key ret; sleep 4
cmd neofetch 2; still neofetch
cmd help 2.5
cmd 'mkdir projects'
cmd 'cd projects'
typ 'edit notes.txt'; sleep 0.5; key ret; sleep 1.5
typ 'thir notes'; key ret; typ 'a linux kernel and a rust userspace'; key ret; typ 'small enough to read in one sitting'; sleep 1
mon "sendkey ctrl-s"; sleep 1.5; still edit
mon "sendkey ctrl-q"; sleep 1.2
cmd 'cat notes.txt' 2
cmd 'mkdir -p ideas/2026'
cmd 'touch ideas/todo.txt'
cmd ls
cmd "find . -name '*.txt'" 2; still files
cmd 'mv notes.txt ideas/'
cmd 'ls ideas'
mon "sendkey alt-ret"; sleep 4                          # split: the new pane logs in as the same user, no password
typ top; sleep 0.5; key ret; sleep 2.5; still split-top; sleep 0.5
key q; sleep 1.2
cmd date 2.5
kill "$cpid"; wait "$cpid" 2>/dev/null || true; cpid=; sleep 1
secs=$(( $(date +%s) - t0 )); n=$(ls "$fr"/*.ppm | wc -l | tr -d ' ')
for f in $(ls "$fr"/*.ppm | tail -2); do [ -s "$f" ] || rm -f "$f"; done   # a frame cut short by the kill
n=$(ls "$fr"/*.ppm | wc -l | tr -d ' ')
fps=$(awk -v n="$n" -v s="$secs" 'BEGIN { printf "%.3f", n / s }')
if grep -qF -- "$DEMO_PW" "$log"; then echo "FAIL: the password is in the serial log"; exit 1; fi
echo "captured $n frames in ${secs}s ($fps fps)"
ffmpeg -v error -y -framerate "$fps" -i "$fr/%05d.ppm" -vf "scale=1280:800:flags=lanczos,format=yuv420p" -r 10 \
  -c:v libx264 -preset slow -crf 27 -movflags +faststart docs/demo.mp4
ffmpeg -v error -y -framerate "$fps" -i "$fr/%05d.ppm" -vf "fps=${GIF_FPS:-6},scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" docs/demo.gif
ls -l docs/demo.mp4 docs/demo.gif "$SHOTS"
echo "PASS: docs/demo.mp4 docs/demo.gif $SHOTS/*.png"
