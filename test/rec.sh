#!/usr/bin/env bash
# Microphone broker + `rec` test (aarch64, 1280x800, QEMU intel-hda + hda-duplex with the `none` audio backend = a silent microphone). Build first: ./build.sh. Usage: test/rec.sh [outdir for the PNGs]
# Part A (serial console): rec refuses without den; the capture node is root-only ("Permission denied" for the user); the playback node goes to the user.
# Part B (den): a copy of the rec binary elsewhere gets no prompt; the prompt in den's bar; deny (n), timeout (30 s); allow (y): red indicator, rec screen, Alt+R, q, pane close; the WAV files are checked on the data disk.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/aarch64/rec-data.img AUDIODEV=none
source ./env.sh
PW='correct-horse-1'; SECRETS=("$PW"); STEP=${REC_STEP_TIMEOUT:-60}
source test/lib.sh
OUTD=${1:-$d/png}; mkdir -p "$OUTD"
trap 'kill "$qpid" ${mpid:-} 2>/dev/null || true; rm -f "$DATA_IMG"; rm -rf "$d"' EXIT

# ---- part A: serial ----
./build.sh data > /dev/null 2>&1
boot boota "${QEMU[@]}"
wait_for 'enter +create your account'; send ''; wait_for 'first boot: create the first user'
wait_for 'username: '; send alice; wait_for 'password: '; send "$PW"; wait_for 'repeat password: '; send "$PW"
wait_for 'utc offset'; send '+05:30'; wait_for 'user alice created'
ai_offline; wait_for '\$ $'
run 'rec' 'only be used from a den pane'
run 'rec extra' 'usage: rec'
run 'cat /dev/snd/pcmC0D0c' 'Permission denied'
# the sound card can show up after the login: init gives the playback node to the user within a second (no ls -l in nsh: a beep that works shows the user owns it)
for _ in $(seq 40); do send 'beep 440 50'; sleep 2; ! tail -n +$((mark + 1)) "$log" | tr -d '\r' | grep -q 'no sound' && break; sleep 1; done
send 'beep 440 50'; sleep 2; if tail -n +$((mark + 1)) "$log" | tr -d '\r' | grep -q 'no sound'; then echo "FAIL: the user cannot play"; exit 1; fi
run 'cat /dev/snd/pcmC0D0c' 'Permission denied'    # still root-only after the grant
echo "ok: serial: rec refuses, the capture and control nodes are root-only, playback works for the user"
off
rm -f "$DATA_IMG"

# ---- part B: den ----
W=1280; H=800; tmp=$d; log=$tmp/serial.log; sock=$tmp/mon.sock
./build.sh data > /dev/null 2>&1
"${QEMU[@]}" -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; sleep 0.5
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() { local i c; for ((i = 0; i < ${#1}; i++)); do case "${1:i:1}" in ' ') c=spc;; /) c=slash;; .) c=dot;; -) c=minus;; *) c=${1:i:1};; esac; keys "$c"; done; }
fail() { echo "FAIL: $*"; tail -20 "$log"; exit 1; }
wait_log() { local deadline=$((SECONDS + $2)); until tr -d '\r' < "$log" | grep -qE "$1"; do kill -0 "$qpid" 2>/dev/null && (( SECONDS < deadline )) || fail "did not see /$1/ within $2s"; sleep 0.2; done; }
count_log() { tr -d '\r' < "$log" | grep -cE "$1" || true; }
shot() { mon "screendump $tmp/$1.ppm"; sleep 1; [ -s "$tmp/$1.ppm" ] || fail "no screendump $1"; }
# share NAME CROP COLOUR: the fraction of the crop (WxH+X+Y) within 8% of the colour
share() { magick "$tmp/$1.ppm" -crop "$2" +repage -fuzz 8% -fill black +opaque "$3" -fill white -opaque "$3" -format '%[fx:mean]' info:; }
BAR_L=620x24+330+2   # the centre of den's bar, where the question box and the red light go
PEACH='#f0b27f'; RED='#f2808b'
is_asking() { [ "$(echo "$(share "$1" $BAR_L $PEACH) > 0.03" | bc -l)" = 1 ]; }   # the peach outline of the question box
is_red() { [ "$(echo "$(share "$1" $BAR_L $RED) > 0.001" | bc -l)" = 1 ]; }
no_light() { [ "$(echo "$(share "$1" $BAR_L $RED) < 0.0002" | bc -l)" = 1 ]; }
rec_start() { type_str rec; keys ret; sleep 2; }

wait_log "den: ${W}x${H} 32bpp$" 30; wait_log '^thir: init ready$' 10
sleep 4
keys ret; sleep 2   # the welcome page: Enter goes on to the account
type_str alice; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3
sleep 2; keys 0 ret; sleep 3

# 1. a copy of the program at another path asks for the microphone: den refuses it, no question appears
type_str "cp /bin/rec /tmp/rec"; keys ret; sleep 1; type_str "/tmp/rec"; keys ret; sleep 2.5
wait_log 'den: microphone request refused: not /bin/rec' 10
shot c0; is_asking c0 && fail "a copy of the program raised the question"
[ "$(count_log 'den: microphone on')" = 0 ] || fail "the microphone opened for a copy"
echo "ok: a program at another path gets no question"

# 2. the question in den's bar; n denies. Nothing is recorded, the device was never opened
rec_start; shot q0; is_asking q0 || fail "no question in den's bar"
cp "$tmp/q0.ppm" "$tmp/prompt.ppm"
keys n; sleep 1.5; shot q1; is_asking q1 && fail "the question stays after n"; no_light q1 || fail "a red light after n"
[ "$(count_log 'den: microphone on')" = 0 ] || fail "the microphone opened after n"
echo "ok: the question shows in den's own bar, n denies"

# 3. no answer for 30 s: no
rec_start; shot t0; is_asking t0 || fail "no question (timeout run)"
sleep 33; shot t1; is_asking t1 && fail "the question did not time out"
[ "$(count_log 'den: microphone on')" = 0 ] || fail "the microphone opened without a yes"
echo "ok: 30 s without an answer is a no"

# 4. y, a recording of about 5 s ended with Alt+R: the red light and rec's screen, then both gone
rec_start; keys y; T0=$(date +%s); sleep 1.5; shot r0; is_red r0 || fail "no red light"; is_asking r0 && fail "the question is still there after y"
cp "$tmp/r0.ppm" "$tmp/recording.ppm"
[ "$(count_log 'den: microphone on')" = 1 ] || fail "the microphone did not open after y"
sleep 3.5; keys alt-r; T1=$(date +%s); sleep 1.5; shot r1; no_light r1 || fail "the red light stays after Alt+R"
wait_log '^den: microphone off$' 5
EXP_A=$((T1 - T0))
echo "ok: y opens the microphone, the red light shows, Alt+R stops it ($EXP_A s)"

# 5. y, about 3 s, q in rec
rec_start; keys y; T2=$(date +%s); sleep 3; keys q; T3=$(date +%s); sleep 1.5; shot r2; no_light r2 || fail "the red light stays after q"
[ "$(count_log '^den: microphone off$')" = 2 ] || fail "the microphone is not closed after q"
EXP_B=$((T3 - T2))
echo "ok: q in rec ends the recording ($EXP_B s)"

# 6. a second recording while one runs: denied at once; closing the pane of the recording closes the device
rec_start; keys y; sleep 1.5; keys alt-ret; sleep 5      # pane 2 is new and focused; the recording runs in pane 1
rec_start; shot s0; is_asking s0 && fail "a question appeared while a recording runs"; is_red s0 || fail "the red light went out"
cp "$tmp/s0.ppm" "$tmp/second.ppm"
keys alt-left; sleep 1; keys alt-q; sleep 2; shot s1; no_light s1 || fail "the red light stays after the pane closed"
[ "$(count_log '^den: microphone off$')" = 3 ] && [ "$(count_log '^den: microphone on$')" = 3 ] || fail "on/off do not match after the pane closed"
echo "ok: a second request is denied; closing the pane closes the microphone"
type_str poweroff; keys ret
for _ in $(seq 60); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.5; done
kill -0 "$qpid" 2>/dev/null && fail "QEMU still running after poweroff"

# 7. the files, read from the data disk: dir 0700, files 0600, header and size match the time recorded
docker run --rm -v "$PWD":/work -v "$tmp":/o thir-build sh -c "
  debugfs -R 'stat /home/alice/Recordings' /work/$DATA_IMG 2>/dev/null | grep -m1 Mode: | tr -s ' ' > /o/dirmode
  for n in \$(debugfs -R 'ls -p /home/alice/Recordings' /work/$DATA_IMG 2>/dev/null | cut -d/ -f6 | grep '^rec-.*[.]wav\$'); do
    debugfs -R \"dump -p /home/alice/Recordings/\$n /o/\$n\" /work/$DATA_IMG 2>/dev/null
    echo \"\$n \$(debugfs -R \"stat /home/alice/Recordings/\$n\" /work/$DATA_IMG 2>/dev/null | grep -m1 -o 'Mode: *[0-7]*' | tr -d ' ')\" >> /o/filemodes
  done"
grep -q 'Mode: 0700' "$tmp/dirmode" || fail "~/Recordings is not 0700: $(cat "$tmp/dirmode")"
[ "$(wc -l < "$tmp/filemodes")" -ge 3 ] || fail "expected 3 recordings: $(cat "$tmp/filemodes")"
grep -qv 'Mode:0600' "$tmp/filemodes" && fail "a recording is not 0600: $(cat "$tmp/filemodes")"
python3 - "$tmp" "$EXP_A" "$EXP_B" <<'PY'
import sys, glob, struct
d, ea, eb = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
fs = sorted(glob.glob(d + '/rec-*.wav')); durs = []
for f in fs:
    b = open(f, 'rb').read()
    assert b[:4] == b'RIFF' and b[8:16] == b'WAVEfmt ' and b[36:40] == b'data', f
    ch, rate, bits = struct.unpack('<H', b[22:24])[0], struct.unpack('<I', b[24:28])[0], struct.unpack('<H', b[34:36])[0]
    data = struct.unpack('<I', b[40:44])[0]
    assert (ch, bits) == (1, 16) and rate in (48000, 44100, 32000, 16000), (f, ch, rate, bits)
    left = len(b) - 44 - data   # a recording cut short by a closed pane has a header up to a second old
    assert struct.unpack('<I', b[4:8])[0] == data + 36 and (left == 0 or (f is fs[2] and 0 < left <= 2 * rate * 2)), (f, data, len(b))
    durs.append(data / 2 / rate); print(f'{f.split("/")[-1]}: {data / 2 / rate:.1f} s at {rate} Hz mono, {len(b)} bytes')
assert abs(durs[0] - ea) <= 1.8 and abs(durs[1] - eb) <= 1.8, (durs, ea, eb)   # y to Alt+R, y to q (a second of slack each way for the keys and the prompt)
assert durs[2] >= 1.0, durs                                                      # cut short by the pane close: what was recorded is kept, header fixed
PY
echo "ok: files: ~/Recordings 0700, three recordings 0600, mono S16 WAV, sizes match the time"
mkdir -p "$OUTD"
magick "$tmp/prompt.ppm" "$OUTD/mic-prompt.png"; magick "$tmp/prompt.ppm" -crop 1280x28+0+0 +repage -filter point -resize 200% "$OUTD/mic-prompt-bar.png"
magick "$tmp/recording.ppm" "$OUTD/mic-recording.png"; magick "$tmp/recording.ppm" -crop 1280x28+0+0 +repage -filter point -resize 200% "$OUTD/mic-recording-bar.png"
magick "$tmp/second.ppm" "$OUTD/mic-second-denied.png"
echo "PASS: pictures in $OUTD"
