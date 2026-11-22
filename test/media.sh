#!/usr/bin/env bash
# media test (aarch64, 1024 MB, 2 vCPUs): `play` on every format. Build the add-on image first: ./build.sh. Usage: test/media.sh [outdir for the PNGs]
# Fixtures (2 s clips: 160x120 colour-quadrant H.264+AAC mp4, VP9+Opus webm, GIF, and a 440 Hz tone as mp3, ogg, flac, wav) are made once in the Alpine container with the full ffmpeg.
# Part A (serial console): `play --check` per format, the music screen and its keys, the guest audio captured by QEMU (440 Hz energy).
# Part B (den, 1280x800): video pixels (quadrant colours), pause, seek, a video next to a text pane, the music screen, the mouse cursor; screendumps become PNGs.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/aarch64/media-data.img QEMU_MEM=1024 QEMU_SMP=2 GUI_RES=1280x800
FX=out/aarch64/media-fixtures
source ./env.sh
[ -f out/aarch64/web.sqsh ] || { echo "no out/aarch64/web.sqsh: run ./build.sh first"; exit 1; }
PW='correct-horse-1'; SECRETS=("$PW"); STEP=${MEDIA_STEP_TIMEOUT:-60}
source test/lib.sh
OUTD=${1:-$d/png}; mkdir -p "$OUTD" "$FX"
trap 'kill "$qpid" ${mpid:-} 2>/dev/null || true; rm -f "$DATA_IMG"; rm -rf "$d"' EXIT

if [ ! -f $FX/quad.mp4 ] || [ ! -f $FX/tone.wav ]; then
docker run --rm -i -v "$PWD/$FX":/fx thir-web-build sh -s <<'GEN'
set -eu
apk add -q --no-cache ffmpeg
cd /fx
Q="color=c=red:s=160x120:r=30:d=2,drawbox=x=80:y=0:w=80:h=60:color=0x00ff00:t=fill,drawbox=x=0:y=60:w=80:h=60:color=0x0000ff:t=fill,drawbox=x=80:y=60:w=80:h=60:color=0xffff00:t=fill"
S="sine=frequency=440:duration=2:sample_rate=44100"
F="ffmpeg -y -v error"
$F -f lavfi -i "$Q" -f lavfi -i "$S" -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest quad.mp4
$F -f lavfi -i "$Q" -f lavfi -i "$S" -c:v libvpx-vp9 -pix_fmt yuv420p -deadline realtime -cpu-used 8 -c:a libopus -ar 48000 -shortest quad.webm
$F -f lavfi -i "$Q" quad.gif
$F -f lavfi -i "$S" -metadata title="Tone A" -metadata artist="Test Band" tone.mp3
$F -f lavfi -i "$S" -c:a libvorbis -metadata title="Tone B" tone.ogg
$F -f lavfi -i "$S" -metadata title="Tone C" tone.flac
$F -f lavfi -i "$S" -c:a pcm_s16le tone.wav
chmod 644 *
GEN
fi
mkdisk() { ./build.sh data > /dev/null 2>&1
  docker run --rm -v "$PWD":/work -v "$PWD/$FX":/fx thir-build sh -c "for f in /fx/*; do echo write \$f \$(basename \$f); done | debugfs -w -f - /work/$DATA_IMG" > /dev/null; }
# wavcheck FILE MIN: QEMU's wav capture (stereo S16, rate from the header) holds at least MIN half-second windows that are a 440 Hz sine (at least 90% of the energy in that bin)
wavcheck() { python3 - "$1" "$2" <<'PY'
import sys, array, math
raw = open(sys.argv[1], 'rb').read(); rate = int.from_bytes(raw[24:28], 'little'); a = array.array('h', raw[44:44 + (len(raw) - 44) // 4 * 4])[0::2]
n = rate // 2; good = 0
for i in range(0, len(a) - n, n):
    w = a[i:i + n]; e = sum(x * x for x in w) / n
    if e < 1e4: continue
    c = sum(x * math.cos(2 * math.pi * 440 * k / rate) for k, x in enumerate(w)); s = sum(x * math.sin(2 * math.pi * 440 * k / rate) for k, x in enumerate(w))
    if 2 * (c * c + s * s) / (n * n) / e > 0.9: good += 1   # A^2/2 over the mean square
print(f"audio: {len(a) / rate:.1f} s at {rate} Hz, {good} windows of 440 Hz"); sys.exit(0 if good >= int(sys.argv[2]) else 1)
PY
}

# ---- part A: serial ----
AUDIODEV="wav,path=$PWD/out/aarch64/media-a.wav"; source ./env.sh
mkdisk
boot boota "${QEMU[@]}"
wait_for '^thir: mounted /web$' 60
wait_for 'enter +create your account'; send ''; wait_for 'first boot: create the first user'
wait_for 'username: '; send alice; wait_for 'password: '; send "$PW"; wait_for 'repeat password: '; send "$PW"
wait_for 'utc offset'; send '+05:30'; wait_for 'user alice created'
ai_offline; wait_for '\$ $'
# frames and samples decoded per format (60 frames = 2 s at 30 fps; samples are per channel at the file's rate, codec padding included)
run 'play --check /data/quad.mp4' 'fmt=mov dur=[0-9.]+ video=h264 160x120 frames=60 audio=aac 44100Hz samples=(8[6-9]|9[0-2])[0-9]{3}$'
run 'play --check /data/quad.webm' 'fmt=matroska dur=[0-9.]+ video=vp9 160x120 frames=60 audio=opus 48000Hz samples=(9[4-9]|10[0-2])[0-9]{3}$'
run 'play --check /data/quad.gif' 'fmt=gif dur=[0-9.]+ video=gif 160x120 frames=60 audio=none$'
run 'play --check /data/tone.mp3' 'fmt=mp3 dur=[0-9.]+ video=none audio=mp3 44100Hz samples=(8[6-9]|9[0-2])[0-9]{3}$'
run 'play --check /data/tone.ogg' 'fmt=ogg dur=[0-9.]+ video=none audio=vorbis 44100Hz samples=(8[6-9]|9[0-2])[0-9]{3}$'
run 'play --check /data/tone.flac' 'fmt=flac dur=[0-9.]+ video=none audio=flac 44100Hz samples=88200$'
run 'play --check /data/tone.wav' 'fmt=wav dur=[0-9.]+ video=none audio=pcm_s16le 44100Hz samples=88200$'
run 'play --check /data/nothing.xyz' 'not a file ffmpeg can play|nothing|No such'
run 'play /data/quad.mp4' 'video needs the desktop'          # video on the serial console: no den
echo "ok: --check reports codec, size and decoded frames for every format"
# the sound card appears a moment after boot; login hands its playback node to the user
for _ in $(seq 40); do send 'ls -l /dev/snd/pcmC0D0p'; sleep 1.5; tail -n +$((mark + 1)) "$log" | tr -d '\r' | grep -q ' alice .*pcmC0D0p' && break; sleep 1; done
# music screen: tags, progress, keys
send 'play /data/tone.mp3'; wait_for 'Tone A'; wait_for 'Test Band'; wait_for 'playing +0:0[0-9] / 0:02'
sendk ' '; wait_for 'paused'; sendk ' '; wait_for 'playing'
sendk '+'; wait_for 'volume 75%'; sendk '-'; sendk '-'; wait_for 'volume 65%'; sendk '+'
sendk 'q'; wait_for '\$ $'
run 'play /data/tone.wav' '\$ $'                              # a whole file: it ends by itself
send 'play /data'; wait_for '1/4'; wait_for 'Tone C'; sendk 'n'; wait_for '2/4'; wait_for 'Tone A'; sendk 'n'; wait_for '3/4'; wait_for 'Tone B'; sendk 'p'; wait_for '2/4'; sendk 'q'; wait_for '\$ $'   # by name: tone.flac mp3 ogg wav
send 'play -s /data'; wait_for '1/4'; sendk 'q'; wait_for '\$ $'
echo "ok: music screen"
off
cp out/aarch64/media-a.wav "$d/a.wav"; wavcheck "$d/a.wav" 3
echo "ok: the guest played a 440 Hz tone"
rm -f "$DATA_IMG"

# ---- part B: den ----
export AUDIODEV="wav,path=$PWD/out/aarch64/media-b.wav"; source ./env.sh
W=1280; H=800; tmp=$d; log=$tmp/serial.log; sock=$tmp/mon.sock; qsock=$tmp/qmp.sock
mkdisk
"${QEMU[@]}" -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" -qmp "unix:$qsock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; sleep 0.5
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
qmp() { printf '{"execute":"qmp_capabilities"}\n%s\n' "$1" | nc -U -w1 "$qsock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() { local i c; for ((i = 0; i < ${#1}; i++)); do case "${1:i:1}" in ' ') c=spc;; /) c=slash;; .) c=dot;; -) c=minus;; *) c=${1:i:1};; esac; keys "$c"; done; }
fail() { echo "FAIL: $*"; tail -20 "$log"; exit 1; }
wait_log() { local deadline=$((SECONDS + $2)); until tr -d '\r' < "$log" | grep -qE "$1"; do kill -0 "$qpid" 2>/dev/null && (( SECONDS < deadline )) || fail "did not see /$1/ within $2s"; sleep 0.2; done; }
shot() { mon "screendump $tmp/$1.ppm"; sleep 1; [ -s "$tmp/$1.ppm" ] || fail "no screendump $1"; }
px() { magick "$tmp/$1.ppm" -format '%[hex:p{'"$2,$3"'}]' info: | cut -c1-6 | tr 'A-F' 'a-f'; }
# near NAME X Y RRGGBB: the pixel is within 40 per channel (H.264 and VP9 are lossy)
near() { local got i; got=$(px "$1" "$2" "$3"); for i in 0 2 4; do local a=$((16#${got:i:2})) b=$((16#${4:i:2})); (( a - b < 40 && b - a < 40 )) || { cp "$tmp/$1.ppm" "$OUTD/fail-$1.ppm" 2>/dev/null; fail "$1: pixel ($2,$3) is $got, wanted about $4"; }; done; }
# until_near NAME X Y RRGGBB SECONDS: shots until it matches
until_near() { local deadline=$((SECONDS + $5)); while true; do shot "$1"; ( near "$1" "$2" "$3" "$4" ) > /dev/null 2>&1 && return 0; (( SECONDS < deadline )) || near "$1" "$2" "$3" "$4"; done; }
# the colours as a fraction of a region (crop WxH+X+Y of a shot): how much of it is within 20% of COLOUR
share() { magick "$tmp/$1.ppm" -crop "$2" +repage -fuzz 20% -fill black +opaque "$3" -fill white -opaque "$3" -format '%[fx:mean]' info:; }
move() { local ax=$(( ($1 * 32767 + W - 2) / (W - 1) )) ay=$(( ($2 * 32767 + H - 2) / (H - 1) ))
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"abs","data":{"axis":"x","value":'$ax'}},{"type":"abs","data":{"axis":"y","value":'$ay'}}]}}'; sleep 0.3; }
click() { move "$1" "$2"
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":true,"button":"left"}}]}}'; sleep 0.2
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":false,"button":"left"}}]}}'; sleep 0.5; }
bar() { magick "$tmp/$1.ppm" -crop 1254x3+13+761 +repage -depth 8 rgb:- | md5; }   # the progress line of the control strip (single pane: surface at 13,49, 1254x738, strip is the last 26 rows)
RED=ff0000; GREEN=00ff00; BLUE=0000ff; YELLOW=ffff00; ACCENT=f0b27f

wait_log '^thir: mounted /web$' 60; wait_log "den: ${W}x${H} 32bpp$" 30; wait_log '^thir: init ready$' 10
sleep 4
keys ret; sleep 2   # the welcome page: Enter goes on to the account
type_str alice; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3
sleep 2; keys 0 ret; sleep 3

# 1. mp4 (H.264 + AAC) in one pane: the video is 949x712 centred in the 1254x712 area above the strip
type_str "play /data/quad.mp4"; keys ret
until_near v0 412 227 $RED 40
near v0 868 227 $GREEN; near v0 412 583 $BLUE; near v0 868 583 $YELLOW
[ "$(px v0 100 300)" = 000000 ] || fail "the letterbox is not black"
cp "$tmp/v0.ppm" "$tmp/video-single.ppm"
if tr -d '\r' < "$log" | grep -q 'surface refused\|surface dropped'; then fail "den refused or dropped play"; fi
echo "ok: mp4 video pixels (colour quadrants)"
# the clip ends after 2 s: the last picture stays and the progress line fills up; space plays it again, space again pauses
until_px_bar() { local deadline=$((SECONDS + $2)); while true; do shot "$1"; [ "$(px "$1" 1255 762)" = $ACCENT ] && return 0; (( SECONDS < deadline )) || fail "the progress line never filled"; done; }
until_px_bar v1 20; near v1 412 227 $RED
keys spc; sleep 0.5; keys spc; shot p0; sleep 2; shot p1
[ "$(bar p0)" = "$(bar p1)" ] || fail "the progress line moved while paused"
[ "$(px p1 1255 762)" != $ACCENT ] || fail "pause did not stop the clip"
keys spc; until_px_bar p2 20
echo "ok: ended, replay, pause and resume"
# seek: left rewinds to the start (the paused picture comes back); a click on the line seeks
keys left; sleep 2; shot s0; [ "$(px s0 1255 762)" != $ACCENT ] || fail "left did not rewind"; near s0 412 227 $RED
keys q; sleep 2; shot q0; [ "$(px q0 412 210)" != $RED ] || fail "the video is still there after q"
echo "ok: seek and quit"
# 2. webm (VP9 + Opus) and animated GIF (no sound, loops)
type_str "play /data/quad.webm"; keys ret; until_near w0 412 227 $RED 40; near w0 868 227 $GREEN; near w0 412 583 $BLUE; near w0 868 583 $YELLOW; keys q; sleep 2
type_str "play /data/quad.gif"; keys ret; until_near g0 412 227 $RED 40; near g0 868 583 $YELLOW; sleep 4; shot g1; near g1 868 227 $GREEN
[ "$(px g1 1255 762)" != $ACCENT ] || fail "the gif stopped instead of looping"; keys q; sleep 2
echo "ok: webm and gif"
# 3. the music screen in a pane
type_str "play /data/tone.mp3"; keys ret; sleep 1.2; keys spc; sleep 0.5; shot mu; cp "$tmp/mu.ppm" "$tmp/music-ui.ppm"; keys q; sleep 2   # paused, so the screen stays
# 4. a video next to a text pane: split, play in the new (right) pane while the old one shows text
type_str "ls -l /data"; keys ret; sleep 1; keys alt-ret; sleep 5; type_str "play /data/quad.mp4"; keys ret; sleep 1.5; shot sp
cp "$tmp/sp.ppm" "$tmp/video-split.ppm"
[ "$(share sp 400x300+700+70 rgb\(255,0,0\) | cut -c1-3)" != 0 ] || fail "no video in the right pane"
echo "ok: video next to a text pane"
sleep 4; keys q; sleep 1
# 5. the mouse cursor on a dark pane and on the wallpaper
move 300 400; shot cu; cp "$tmp/cu.ppm" "$tmp/cursor-pane.ppm"
move 200 770; shot cw; cp "$tmp/cw.ppm" "$tmp/cursor-wall.ppm"
keys alt-q 2>/dev/null || true

mon quit; sleep 1; kill "$qpid" 2>/dev/null || true
cp out/aarch64/media-b.wav "$d/b.wav"; wavcheck "$d/b.wav" 3
for n in video-single music-ui video-split; do magick "$tmp/$n.ppm" "$OUTD/$n.png"; done
magick "$tmp/cursor-pane.ppm" -crop 120x120+270+370 +repage -filter point -resize 400% "$OUTD/cursor-pane.png"
magick "$tmp/cursor-wall.ppm" -crop 120x120+170+680 +repage -filter point -resize 400% "$OUTD/cursor-wall.png"
echo "PASS: pictures in $OUTD"
