#!/usr/bin/env bash
# The browser (aarch64, image, 1280x800, -m 1024 -smp 2). Build first: ./build.sh. Usage: test/web.sh [outdir for the PNGs]
# Part A (serial, no den): `web --dump-title` prints the title JavaScript computed; an HTTPS page signed by an untrusted CA fails; `web --check` (sandbox); only http(s) URLs open.
# Part B (den, QMP tablet + sendkey + screendump): JS colours the page (exact pixels); a click; typing into an autofocus input; the wheel; the download prompt and the saved files (0600, never
# overwritten); a TLS error; a page next to a text pane; quit. The mock server (test/mock.py) serves the pages in test/web/.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/aarch64/web-data.img QEMU_MEM=1024 QEMU_SMP=2 GUI_RES=1280x800
AUDIO_WAV=$PWD/out/aarch64/web-a.wav; AUDIODEV="wav,path=$AUDIO_WAV"; source ./env.sh
[ -f out/aarch64/web.sqsh ] || { echo "no out/aarch64/web.sqsh: run ./build.sh first"; exit 1; }
PW='correct-horse-1'; SECRETS=("$PW"); STEP=${WEB_STEP_TIMEOUT:-150}
source test/lib.sh
OUTD=${1:-$d/png}; mkdir -p "$OUTD"
trap 'kill "$qpid" ${mpid:-} 2>/dev/null || true; rm -f "$DATA_IMG"; rm -rf "$d"' EXIT
python3 test/mock.py "$d" & mpid=$!
for _ in $(seq 1 50); do [ -f "$d/srv.pem" ] && (echo > /dev/tcp/127.0.0.1/18443) 2>/dev/null && break; sleep 0.2; done
BASE=http://10.0.2.2:18080/web

# ---- part A: serial ----
./build.sh data > /dev/null 2>&1
boot boota "${QEMU[@]}"
wait_for 'enter +create your account'; send ''; wait_for 'first boot: create the first user'
wait_for 'username: '; send alice; wait_for 'password: '; send "$PW"; wait_for 'repeat password: '; send "$PW"
wait_for 'utc offset'; send '+05:30'; wait_for 'user alice created'
ai_offline; wait_for '\$ $'
run 'web file:///etc/passwd' 'only http and https'
run 'web javascript:alert(1)' 'only http and https'
run 'web data:text/html,x' 'only http and https'
run "web $BASE/js.html" 'web needs the desktop'
run 'web --check' '^sandbox ok$' 120
sed -E $'s/\e\\[[0-9;?]*[A-Za-z~]//g' "$log" | tr -d '\r' | grep -E '^(ok: |FAIL:)' | sed 's/^/  /'
tr -d '\r' < "$log" | grep -q '^FAIL:' && { echo "FAIL: a sandbox check failed"; exit 1; }
echo "ok: --check: the browser and both helper processes are locked in"
for _ in $(seq 30); do send 'curl -s http://10.0.2.2:18080/hello'; sleep 2; tail -n +$((mark + 1)) "$log" | tr -d '\r' | grep -q '^hello from mock$' && break; done   # the network comes up a few seconds after the login
run "web --dump-title $BASE/js.html" '^js-ok-42$' 150
echo "ok: --dump-title prints the title JavaScript set (WebKit runs, JIT, network process)"
run "web --dump-title https://10.0.2.2:18443/web/js.html" 'TLS error' 150
run 'echo done' '^done$'
if tail -n +$((mark - 3)) "$log" | tr -d '\r' | grep -q '^js-ok-42$'; then echo "FAIL: the untrusted HTTPS page was loaded"; exit 1; fi
echo "ok: an HTTPS page signed by an unknown CA fails (no 'proceed anyway')"
run "web --dump-title $BASE/tz.html" '^tz:-330:Asia/(Kolkata|Calcutta):17:30:00 GMT\+0530$' 150
echo "ok: JavaScript dates follow ~/.tz (+05:30): getTimezoneOffset -330, Date.toString GMT+0530"
# the AI is the mock (turned on in settings once the network is up): it picks the browser and media commands of prompts/route.txt; nothing in a page or a file goes to it
send settings; wait_for 'setting +value'; sendk $'\e[B\e[B\e[B\e[B\e[B\e[B\e[B'; sendk $'\r'; wait_for 'choice \[0\]: '
send 6; wait_for 'name \[mine\]: '; send mock; wait_for 'protocol'; send anthropic; wait_for 'URL: '; send 'http://10.0.2.2:18080/anthropic/v1'
wait_for 'model \[claude-opus-5-5\]: '; send ''; wait_for 'mock answered'; wait_for 'agent will use mock'; sendk q; wait_for '\$ $'
send '! open wikipedia sri lanka'; wait_for '→ web https://en\.wikipedia\.org/wiki/Sri_Lanka +run\? +\[y\] run \(default\)'; send n; wait_for 'nothing done'; bye
send '! play my music'; wait_for '→ play ~/Music +run\? +\[y\] run \(default\)'; send n; wait_for 'nothing done'; bye
send '! show my photos'; wait_for '→ view ~/Pictures +run\? +\[y\] run \(default\)'; send n; wait_for 'nothing done'; bye
send '! record a voice note'; wait_for '! this turns on the microphone'; wait_for '→ rec +run\? +\[y\] run'; send ''; wait_for 'nothing done'; bye
send '! open the file etc passwd in the browser'; wait_for '→ web file:///etc/passwd +run\? +\[y\] run \(default\)'; send y; wait_for 'web: only http and https addresses'; bye
echo "ok: the AI proposes web/play/view/rec; rec asks with default no; an AI web file:// address is refused by web"
off
rm -f "$DATA_IMG"

# ---- part B: den ----
W=1280; H=800; tmp=$d; log=$tmp/serial.log; sock=$tmp/mon.sock; qsock=$tmp/qmp.sock
./build.sh data > /dev/null 2>&1
"${QEMU[@]}" -device virtio-mouse-pci -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" -qmp "unix:$qsock,server,nowait" > "$tmp/qemu.out" 2>&1 &
qpid=$!; sleep 0.5
mon() { printf '%s\n' "$1" | nc -U -w1 "$sock" > /dev/null; }
qmp() { printf '{"execute":"qmp_capabilities"}\n%s\n' "$1" | nc -U -w1 "$qsock" > /dev/null; }
keys() { local c; for c in "$@"; do mon "sendkey $c"; sleep 0.08; done; }
type_str() { local i c; for ((i = 0; i < ${#1}; i++)); do case "${1:i:1}" in ' ') c=spc;; /) c=slash;; .) c=dot;; -) c=minus;; :) c=shift-semicolon;; *) c=${1:i:1};; esac; keys "$c"; done; }
fail() { echo "FAIL: $*"; tail -20 "$log"; exit 1; }
wait_log() { local deadline=$((SECONDS + $2)); until tr -d '\r' < "$log" | grep -qE "$1"; do kill -0 "$qpid" 2>/dev/null && (( SECONDS < deadline )) || fail "did not see /$1/ within $2s"; sleep 0.2; done; }
shot() { mon "screendump $tmp/$1.ppm"; sleep 1; [ -s "$tmp/$1.ppm" ] || fail "no screendump $1"; }
px() { magick "$tmp/$1.ppm" -format '%[hex:p{'"$2,$3"'}]' info: | cut -c1-6 | tr 'A-F' 'a-f'; }
expect() { local got; got=$(px "$1" "$2" "$3"); [ "$got" = "$4" ] || { cp "$tmp/$1.ppm" "$OUTD/fail-$1.ppm" 2>/dev/null; fail "$1: pixel ($2,$3) is $got, wanted $4"; }; }
until_px() { local deadline=$((SECONDS + $5)); while true; do shot "$1"; [ "$(px "$1" "$2" "$3")" = "$4" ] && return 0; (( SECONDS < deadline )) || expect "$1" "$2" "$3" "$4"; done; }
move() { local ax=$(( ($1 * 32767 + W - 2) / (W - 1) )) ay=$(( ($2 * 32767 + H - 2) / (H - 1) ))
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"abs","data":{"axis":"x","value":'$ax'}},{"type":"abs","data":{"axis":"y","value":'$ay'}}]}}'; sleep 0.3; }
click() { move "$1" "$2"
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":true,"button":"left"}}]}}'; sleep 0.2
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":false,"button":"left"}}]}}'; sleep 0.5; }
wheel() { local i; for ((i = 0; i < $1; i++)); do
  qmp '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"down":true,"button":"wheel-down"}},{"type":"btn","data":{"down":false,"button":"wheel-down"}}]}}'; sleep 0.1; done; }
share() { magick "$tmp/$1.ppm" -crop "$2" +repage -fuzz 8% -fill black +opaque "$3" -fill white -opaque "$3" -format '%[fx:mean]' info:; }
share1() { magick "$tmp/$1.ppm" -crop "$2" +repage -fuzz 1% -fill black +opaque "$3" -fill white -opaque "$3" -format '%[fx:mean]' info:; }   # like share, but only that colour
STRIP=1254x41+13+49; STRIP2=1254x16+13+70; YELLOW='#ecc87c'; RED='#f2808b'
goto() { keys ctrl-l ctrl-u; type_str "$1"; keys ret; }   # the address line: Ctrl-L, clear, type, Enter
# the page starts under the strip: pane inside (13,49) 1254x738, strip 41 rows (tabs, then address), hint row 24 rows at the bottom, so page (x,y) is screen (13+x, 90+y)
JS=02c896; BTN_BG=0a14c8; BTN_ON=c81e3c; IN_ON=1ea028; RED_BAND=dc1414; GREEN_BAND=14b43c

den_login() {
  wait_log '^thir: mounted /web$' 60
  wait_log "den: ${W}x${H} 32bpp$" 30
  wait_log 'thir: init ready' 10
  sleep 4
  keys ret; sleep 2   # the welcome page: Enter goes on to the account
  type_str alice; keys ret; sleep 1; type_str $PW; keys ret; sleep 1; type_str $PW; keys ret; sleep 1
  keys shift-equal 0 5 shift-semicolon 3 0 ret; sleep 3
  sleep 2; keys 0 ret; sleep 3
}
den_login

# 1. `web` with no address: the line is open; Enter on an address without a dot-less word loads it (an IP address gets http://); JavaScript runs and colours the page
type_str web; keys ret; sleep 4
shot e0; [ "$(echo "$(share e0 $STRIP2 '#f0b27f') > 0.002" | bc -l)" = 1 ] || fail "no cursor in the strip"
cp "$tmp/e0.ppm" "$tmp/web-strip-edit.ppm"
type_str "10.0.2.2:18080/web/js.html"; keys ret; t0=$SECONDS
until_px j0 640 400 $JS 120
echo "first frame: $((SECONDS - t0)) s after Enter (cold start of WebKit and the helper processes)"
cp "$tmp/j0.ppm" "$tmp/web-js.ppm"
shot j1; expect j1 900 60 0b1c36
echo "ok: the page JavaScript set the page colour rgb(2,200,150) exactly; the strip shows the thir colours"

# 2. a click on the button changes the page colour
goto "10.0.2.2:18080/web/btn.html"
until_px b0 900 500 $BTN_BG 60
click 213 240
until_px b1 900 500 $BTN_ON 30
echo "ok: a mouse click reached the page (button onclick changed the colour)"

# 3. typing into an autofocus input
goto "10.0.2.2:18080/web/input.html"
until_px i0 900 500 $BTN_BG 60
sleep 1; keys a b c
until_px i1 900 500 $IN_ON 30
echo "ok: keys typed into the autofocus input reached the page"

# 4. the wheel scrolls a tall page: the red band at the top scrolls away
goto "10.0.2.2:18080/web/tall.html"
until_px t0 640 190 $RED_BAND 60
move 640 400; wheel 30
until_px t1 640 190 $GREEN_BAND 30
echo "ok: the wheel scrolled the page"

# 5. a download: the question in the strip; y saves ~/Downloads/thir_test.bin (WebKit cleans the server name; "../" is gone); the second time it becomes "thir_test (1).bin"; n cancels
goto "10.0.2.2:18080/dl.bin"
for _ in $(seq 40); do shot d0; [ "$(echo "$(share d0 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
[ "$(echo "$(share d0 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] || { cp "$tmp/d0.ppm" "$OUTD/fail-d0.ppm"; fail "no download question in the strip"; }
cp "$tmp/d0.ppm" "$tmp/web-download.ppm"
keys y; sleep 3
goto "10.0.2.2:18080/dl.bin"; for _ in $(seq 40); do shot d1; [ "$(echo "$(share d1 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
keys y; sleep 3
goto "10.0.2.2:18080/dl.bin"; for _ in $(seq 40); do shot d2; [ "$(echo "$(share d2 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
keys n; sleep 2; shot d3; [ "$(echo "$(share d3 $STRIP2 $YELLOW) < 0.001" | bc -l)" = 1 ] || fail "the question is still up after n"
echo "ok: download question shown; y twice and n once answered"

# 6. TLS: a page signed by an unknown CA is not shown; the strip says why (red) and the page is the error page, never the real one
goto "https://10.0.2.2:18443/web/js.html"
for _ in $(seq 40); do shot s0; [ "$(echo "$(share s0 $STRIP2 $RED) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
[ "$(echo "$(share s0 $STRIP2 $RED) > 0.008" | bc -l)" = 1 ] || { cp "$tmp/s0.ppm" "$OUTD/fail-s0.ppm"; fail "no TLS error in the strip"; }
until_px s1 640 500 ffffff 30; [ "$(px s1 640 400)" != $JS ] || fail "the untrusted page was shown"
cp "$tmp/s1.ppm" "$tmp/web-tls.ppm"; [ "$(echo "$(share s1 $STRIP2 $RED) > 0.008" | bc -l)" = 1 ] || fail "the TLS error left the strip"
echo "ok: TLS error in the strip and in the page; the page was not loaded"

# 6b. JavaScript dates follow ~/.tz (+05:30): the page is green only when getTimezoneOffset() is -330
goto "10.0.2.2:18080/web/tz.html"; until_px z0 900 500 $JS 60
echo "ok: the page's JavaScript sees thir's time zone"

# 6c. history: Ctrl-[ goes back, Ctrl-] forward (the mouse side buttons do the same)
goto "10.0.2.2:18080/web/js.html"; until_px h0 900 500 $JS 60
goto "10.0.2.2:18080/web/btn.html"; until_px h1 900 500 $BTN_BG 60
keys ctrl-bracket_left; until_px h2 900 500 $JS 30
keys ctrl-bracket_right; until_px h3 900 500 $BTN_BG 30
echo "ok: back and forward by key"

# 6d. clipboard: type abc in the input (green), select all and copy, reload (blue again, empty input), paste: green
goto "10.0.2.2:18080/web/input.html"; until_px c0 900 500 $BTN_BG 60
sleep 1; keys a b c; until_px c1 900 500 $IN_ON 30
keys ctrl-a ctrl-c; sleep 1; keys ctrl-r; until_px c2 900 500 $BTN_BG 60
sleep 1; keys ctrl-v; until_px c3 900 500 $IN_ON 30
echo "ok: Ctrl-A, Ctrl-C and Ctrl-V work inside the page"

# 6e. tabs: btn (blue), js (green), tall (red band at the top); switch with Ctrl-PgUp, Ctrl-Tab, Ctrl-PgDn; close with Ctrl-W
goto "10.0.2.2:18080/web/btn.html"; until_px g0 900 500 $BTN_BG 60
keys ctrl-t; sleep 1; type_str "10.0.2.2:18080/web/js.html"; keys ret; until_px g1 900 500 $JS 60
keys ctrl-t; sleep 1; type_str "10.0.2.2:18080/web/tall.html"; keys ret; until_px g2 640 190 $RED_BAND 60
shot g3; [ "$(echo "$(share g3 $STRIP '#f0b27f') > 0.003" | bc -l)" = 1 ] || fail "no highlighted tab in the strip"
cp "$tmp/g3.ppm" "$tmp/web-tabs.ppm"
keys ctrl-pgup; until_px g4 900 500 $JS 30
keys ctrl-pgup; until_px g5 900 500 $BTN_BG 30
keys ctrl-tab; until_px g6 900 500 $JS 30
keys ctrl-pgdn; until_px g7 640 190 $RED_BAND 30
keys ctrl-pgdn; until_px g8 900 500 $BTN_BG 30
keys ctrl-pgup; until_px g9 640 190 $RED_BAND 30
keys ctrl-pgup; until_px g10 900 500 $JS 30
keys ctrl-w; until_px g11 640 190 $RED_BAND 30
echo "ok: three tabs: Ctrl-T, Ctrl-PgUp/PgDn/Tab switch, Ctrl-W closes (the right neighbour shows)"

# 6f. target=_blank opens a tab: the gray page is a link to js.html; a click opens it in a new tab (green); Ctrl-W comes back to the gray page
goto "10.0.2.2:18080/web/blank.html"; until_px o0 900 500 787878 60
click 402 329; until_px o1 900 500 $JS 60
keys ctrl-w; until_px o2 900 500 787878 30
echo "ok: a target=_blank link opened a new tab"

# 6g. at most 8 tabs: now 2 tabs; six more fill the list; the ninth is refused with a red note in the strip
keys ctrl-t; keys ctrl-t; keys ctrl-t; keys ctrl-t; keys ctrl-t; keys ctrl-t; sleep 1; keys esc; keys ctrl-t; sleep 1
shot m0; [ "$(echo "$(share m0 $STRIP2 $RED) > 0.008" | bc -l)" = 1 ] || { cp "$tmp/m0.ppm" "$OUTD/fail-m0.ppm"; fail "no 'no more than 8 tabs' in the strip"; }
keys ctrl-w; keys ctrl-w; keys ctrl-w; keys ctrl-w; keys ctrl-w; keys ctrl-w; sleep 2; until_px m1 900 500 787878 30
echo "ok: the ninth tab is refused; closed back to the page"

# 6h. page dialogs in the strip: alert (Enter), confirm (y), prompt (type x, Enter) give a green page; confirm n gives red
goto "10.0.2.2:18080/web/dialog.html"; until_px a0 900 500 $BTN_BG 60
click 213 240
for _ in $(seq 20); do shot a1; [ "$(echo "$(share a1 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
[ "$(echo "$(share a1 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] || { cp "$tmp/a1.ppm" "$OUTD/fail-a1.ppm"; fail "no alert in the strip"; }
cp "$tmp/a1.ppm" "$tmp/web-alert.ppm"
keys ret; sleep 1; for _ in $(seq 20); do shot a2; [ "$(echo "$(share a2 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
keys y; sleep 1; for _ in $(seq 20); do shot a3; [ "$(echo "$(share a3 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
type_str x; keys ret; until_px a4 900 500 $JS 30
goto "10.0.2.2:18080/web/dialog.html"; until_px a5 900 500 $BTN_BG 60; click 213 240; sleep 2; keys ret; sleep 2; keys n; until_px a6 900 500 $BTN_ON 30
echo "ok: alert, confirm and prompt were answered in the strip (never blocking den)"

# 6i. find: Ctrl-F, type "needle": the matches are highlighted in the page; Enter goes on, Shift-Enter back; Esc ends the find
goto "10.0.2.2:18080/web/find.html"; until_px f0 900 500 ffffff 60
[ "$(echo "$(share f0 216x60+15+92 '#ffffff') > 0.7" | bc -l)" = 1 ] || fail "find page is not white before the find"
keys ctrl-f; sleep 1; type_str needle; sleep 3; keys ret; sleep 2; shot f1
cp "$tmp/f1.ppm" "$tmp/web-find.ppm"
[ "$(echo "$(share f1 216x60+15+92 '#ffffff') < 0.3" | bc -l)" = 1 ] || { cp "$tmp/f1.ppm" "$OUTD/fail-f1.ppm"; fail "no highlight on the found word"; }
keys shift-ret; sleep 1; keys esc; sleep 2; shot f2   # the found word stays selected (gray): that is WebKit's, not the find
echo "ok: find highlighted the match (Enter next, Shift-Enter previous, Esc closes)"

# 6j. upload: the file input opens the picker in the strip with the files of ~/Downloads; Down and Enter choose thir_test.bin (the page turns green)
goto "10.0.2.2:18080/web/upload.html"; until_px u0 900 500 $BTN_BG 60
click 62 129
for _ in $(seq 20); do shot u1; [ "$(echo "$(share u1 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
[ "$(echo "$(share u1 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] || { cp "$tmp/u1.ppm" "$OUTD/fail-u1.ppm"; fail "no file picker in the strip"; }
cp "$tmp/u1.ppm" "$tmp/web-upload.ppm"
keys down ret; until_px u2 900 500 $JS 30
echo "ok: the upload picker listed ~/Downloads and the page got the file"

# 6k. fonts: Hindi, Arabic, Hebrew, Thai, CJK, Sinhala and Tamil text (the picture is looked at, there is no pixel check for letters)
goto "10.0.2.2:18080/web/lang.html"; sleep 10; shot l0; cp "$tmp/l0.ppm" "$tmp/web-lang.ppm"
# 6l. the text caret in an input field blinks: dark pixels at the left of the empty input show up in some shots and not in others
goto "10.0.2.2:18080/web/input.html"; until_px k0 900 500 $BTN_BG 60
dark() { magick "$tmp/$1.ppm" -crop 12x24+15+98 +repage -colorspace Gray -threshold 25% -negate -format '%[fx:mean]' info:; }   # share of dark pixels in the caret column
on=0; off=0; for i in $(seq 14); do sleep 0.$((RANDOM % 9)); shot k1; d=$(dark k1); [ "$(echo "$d > 0.04" | bc -l)" = 1 ] && { on=$((on + 1)); [ -f "$tmp/caret-on.ppm" ] || cp "$tmp/k1.ppm" "$tmp/caret-on.ppm"; }; [ "$(echo "$d < 0.005" | bc -l)" = 1 ] && off=$((off + 1)); done
[ "$on" -ge 1 ] && [ "$off" -ge 1 ] || { cp "$tmp/k1.ppm" "$OUTD/fail-caret.ppm"; fail "the caret does not blink in the input (on $on, off $off of 14 shots)"; }
cp "$tmp/caret-on.ppm" "$tmp/web-caret.ppm" 2> /dev/null || true
echo "ok: the text caret is drawn and blinks in a page input (on in $on shots, off in $off)"

# 6m. the address line: Ctrl-A selects all (accent fill), typing replaces it; Ctrl-C copies, Ctrl-X cuts, Ctrl-V pastes (also into a new tab)
goto "10.0.2.2:18080/web/btn.html"; until_px y0 900 500 $BTN_BG 60
keys ctrl-l; sleep 1; shot y1; b1=$(share y1 $STRIP2 '#f0b27f'); keys ctrl-a; sleep 1; shot y2; b2=$(share y2 $STRIP2 '#f0b27f')
[ "$(echo "$b2 > $b1 * 4 && $b2 > 0.03" | bc -l)" = 1 ] || { cp "$tmp/y2.ppm" "$OUTD/fail-y2.ppm"; fail "Ctrl-A does not show a selection in the address line ($b1 -> $b2)"; }
cp "$tmp/y2.ppm" "$tmp/web-addr-select.ppm"
keys ctrl-c; sleep 1; type_str "10.0.2.2:18080/web/js.html"; keys ret; until_px y3 900 500 $JS 60
echo "ok: Ctrl-A selected the whole address (strip drew the selection) and typing replaced it"
keys ctrl-t; sleep 1; keys ctrl-v; sleep 1; keys ret; until_px y4 900 500 $BTN_BG 60
echo "ok: Ctrl-C in the address line and Ctrl-V in another tab's address line: the pasted address opened the page"
keys ctrl-w; until_px y5 900 500 $JS 30
keys ctrl-l; keys ctrl-a ctrl-x; sleep 1; shot y6; [ "$(echo "$(share y6 $STRIP2 '#d4dcec') < 0.005" | bc -l)" = 1 ] || fail "Ctrl-X did not clear the address line"
keys ctrl-v; sleep 1; shot y7; [ "$(echo "$(share y7 $STRIP2 '#d4dcec') > 0.01" | bc -l)" = 1 ] || fail "Ctrl-V did not paste back what Ctrl-X cut"; keys esc
echo "ok: Ctrl-X cut the address and Ctrl-V pasted it back"

# 6n. copy and paste in a page form: field one holds "hello world"; Ctrl-A, Ctrl-C there, Tab to field two, Ctrl-V: the page turns green when field two reads "hello world"
goto "10.0.2.2:18080/web/form.html"; until_px r0 900 500 $BTN_BG 60
sleep 1; keys ctrl-a ctrl-c; sleep 1; keys tab; sleep 1; keys ctrl-v; until_px r1 900 500 $IN_ON 30
cp "$tmp/r1.ppm" "$tmp/web-form.ppm"
echo "ok: Ctrl-A, Ctrl-C and Ctrl-V in a page form (copy in one field, paste in the next)"
keys ctrl-l; keys ctrl-u; keys ctrl-v; sleep 1; shot r2; [ "$(echo "$(share r2 $STRIP2 '#d4dcec') > 0.004" | bc -l)" = 1 ] || fail "the text copied in the page did not reach the address line"; keys esc
echo "ok: the clipboard is shared: text copied in the page pastes into the address line"

# 6o. <select>: the page's drop-down is shown by the browser (a list over the page); down, down, Enter picks Plum (the page turns green); a click on an item does the same
goto "10.0.2.2:18080/web/select.html"; until_px q1 900 500 $BTN_BG 60
click 133 130; sleep 2; shot q2; cp "$tmp/q2.ppm" "$tmp/web-select.ppm"
[ "$(echo "$(share q2 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] || { cp "$tmp/q2.ppm" "$OUTD/fail-q2.ppm"; fail "no <select> popup (the strip does not say 'choose')"; }
keys down down ret; until_px q3 900 500 $IN_ON 30
goto "10.0.2.2:18080/web/select.html"; until_px q4 900 500 $BTN_BG 60
click 133 130; sleep 2; shot q5; click 133 205; until_px q6 900 500 $IN_ON 30
echo "ok: <select> popup: keys and a mouse click choose the option"

# 6p. downloads: a slow 1.5 MB file (the server allows Range): progress in Ctrl-J's list, pause, resume, done, and the bytes are right
goto "10.0.2.2:18080/dl/slow.bin"
for _ in $(seq 40); do shot w0; [ "$(echo "$(share w0 $STRIP2 $YELLOW) > 0.008" | bc -l)" = 1 ] && break; sleep 1; done
keys y; sleep 1; keys ctrl-j; sleep 3; shot w1; cp "$tmp/w1.ppm" "$tmp/web-downloads.ppm"
DLBODY=1254x120+13+95; acc() { share "$1" $DLBODY '#f0b27f'; }
a1=$(acc w1); sleep 4; shot w2; a2=$(acc w2); [ "$(echo "$a2 > $a1" | bc -l)" = 1 ] || { cp "$tmp/w2.ppm" "$OUTD/fail-w2.ppm"; fail "the download bar does not grow ($a1 -> $a2)"; }
keys p; sleep 3; shot w3; a3=$(acc w3); sleep 4; shot w4; a4=$(acc w4); [ "$a3" = "$a4" ] || fail "the download kept growing after pause ($a3 -> $a4)"
[ "$(echo "$(share w4 $DLBODY '#ecc87c') > 0.0005" | bc -l)" = 1 ] || { cp "$tmp/w4.ppm" "$OUTD/fail-w4.ppm"; fail "no 'paused' (warn colour) in the downloads list"; }
cp "$tmp/w4.ppm" "$tmp/web-downloads-paused.ppm"
keys p; for _ in $(seq 60); do shot w5; [ "$(echo "$(share w5 $DLBODY '#9fd08a') > 0.01" | bc -l)" = 1 ] && break; sleep 2; done
[ "$(echo "$(share w5 $DLBODY '#9fd08a') > 0.01" | bc -l)" = 1 ] || { cp "$tmp/w5.ppm" "$OUTD/fail-w5.ppm"; fail "the resumed download did not finish (no green bar)"; }
cp "$tmp/w5.ppm" "$tmp/web-downloads-done.ppm"; keys esc; sleep 1
echo "ok: downloads view: progress grew, pause stopped it, resume finished it"

# 6q. history: Ctrl-H lists the pages visited (SEL row), typing filters (none for zzz), Enter opens the page
keys ctrl-h; sleep 2; shot x1; cp "$tmp/x1.ppm" "$tmp/web-history.ppm"
[ "$(echo "$(share1 x1 $DLBODY '#15294a') > 0.02" | bc -l)" = 1 ] || { cp "$tmp/x1.ppm" "$OUTD/fail-x1.ppm"; fail "the history list shows no row"; }
type_str zzz; sleep 1; shot x2; [ "$(echo "$(share1 x2 $DLBODY '#15294a') < 0.001" | bc -l)" = 1 ] || fail "the history search does not filter"
keys ctrl-u; type_str btn; sleep 1; keys ret; until_px x3 900 500 $BTN_BG 60
echo "ok: history: listed, searched, opened with Enter"

wavcheck() { python3 - "$1" "$2" <<'PY'
import sys, array, math
raw = open(sys.argv[1], 'rb').read(); rate = int.from_bytes(raw[24:28], 'little'); a = array.array('h', raw[44:44 + (len(raw) - 44) // 4 * 4])[0::2]
n = rate // 2; good = 0
for i in range(0, len(a) - n, n):
    w = a[i:i + n]; e = sum(x * x for x in w) / n
    if e < 1e4: continue
    c = sum(x * math.cos(2 * math.pi * 440 * k / rate) for k, x in enumerate(w)); s = sum(x * math.sin(2 * math.pi * 440 * k / rate) for k, x in enumerate(w))
    if 2 * (c * c + s * s) / (n * n) / e > 0.9: good += 1
print(f"audio: {len(a) / rate:.1f} s at {rate} Hz, {good} windows of 440 Hz"); sys.exit(0 if good >= int(sys.argv[2]) else 1)
PY
}

# 6r. HTML5 video: an mp4 (H.264 + AAC, GStreamer and gst-libav) plays in the page: the marker turns green and the quadrants show; the sound is captured by QEMU
goto "10.0.2.2:18080/web/video.html"; until_px vv0 700 130 $IN_ON 150
[ "$(magick "$tmp/vv0.ppm" -format '%[fx:p{150,200}.r*255>200&&p{150,200}.g*255<80?1:0]' info:)" = 1 ] || { cp "$tmp/vv0.ppm" "$OUTD/fail-vv0.ppm"; fail "the video's red quadrant is not on screen"; }
sleep 4; shot vv1; cp "$tmp/vv1.ppm" "$tmp/web-video.ppm"
echo "ok: an HTML5 video decoded and played in the page (currentTime advanced, 160x120 H.264, quadrants on screen)"
cp "$AUDIO_WAV" "$tmp/a.wav"; wavcheck "$tmp/a.wav" 2 || { cp "$tmp/a.wav" "$OUTD/fail-video.wav"; fail "no 440 Hz tone from the video in QEMU's audio capture"; }
echo "ok: the video's sound (440 Hz AAC) reached the sound card"

# 7. a page next to a text pane
goto "10.0.2.2:18080/web/js.html"; until_px p0 300 400 $JS 60
keys alt-ret; sleep 5; type_str top; keys ret; sleep 2
until_px p1 300 400 $JS 60
click 900 400; keys m; sleep 3; shot p2; cp "$tmp/p2.ppm" "$tmp/web-split.ppm"
echo "ok: the page re-rendered at the new size next to a text pane"
click 300 400; goto "10.0.2.2:18080/web/big.html"; sleep 25; click 900 400; keys m; sleep 3; shot p3; click 300 400; cp "$tmp/p3.ppm" "$tmp/web-top-big.ppm"
for n in web-strip-edit web-js web-download web-tls web-split web-top-big web-tabs web-alert web-find web-upload web-lang web-addr-select web-form web-select web-downloads web-downloads-paused web-downloads-done web-history web-video web-caret; do [ -f "$tmp/$n.ppm" ] || continue; magick "$tmp/$n.ppm" "$OUTD/$n.png"; done
magick "$tmp/web-download.ppm" -crop 1254x41+13+49 +repage -filter point -resize 150% "$OUTD/web-download-strip.png"
for n in web-tabs web-alert web-upload; do magick "$tmp/$n.ppm" -crop 1254x41+13+49 +repage -filter point -resize 150% "$OUTD/$n-strip.png"; done

# 8. Ctrl-Q ends the browser and the pane shows text again
keys ctrl-q; sleep 3; shot q0; [ "$(px q0 300 400)" != $JS ] || fail "the page is still there after Ctrl-Q"
click 900 400; keys q; sleep 1; type_str poweroff; keys ret
for _ in $(seq 60); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.5; done
kill -0 "$qpid" 2>/dev/null && fail "QEMU still running after poweroff"
if tr -d '\r' < "$log" | grep -q 'surface refused\|surface dropped'; then fail "den refused or dropped web"; fi

# 9. the files on the data disk: ~/Downloads 0700 with the two saved files, 0600, 3072 bytes each, never overwritten; the browser's folders 0700
docker run --rm -v "$PWD":/work -v "$tmp":/o thir-build sh -c "
  debugfs -R 'ls -p /home/alice/Downloads' /work/$DATA_IMG 2>/dev/null | cut -d/ -f6 | grep -v '^[.]*\$' > /o/dl-names
  debugfs -R 'ls -p /home/alice/Downloads' /work/$DATA_IMG 2>/dev/null | awk -F/ '\$6 ~ /^thir_test/ { print \$6 \" \" \$3 \" \" \$7 }' > /o/dl-stat
  debugfs -R 'dump /home/alice/Downloads/slow.bin /o/slow.bin' /work/$DATA_IMG 2>/dev/null
  for d in Downloads .local/share/thir-web .cache/thir-web; do echo \"\$d \$(debugfs -R \"stat /home/alice/\$d\" /work/$DATA_IMG 2>/dev/null | grep -m1 -o 'Mode: *[0-7]*' | tr -d ' ')\"; done > /o/dirs"
[ "$(sort "$tmp/dl-names" | tr '\n' ',')" = "slow.bin,thir_test (1).bin,thir_test.bin," ] || fail "files in ~/Downloads: $(cat "$tmp/dl-names")"
[ "$(grep -c ' 100600 3072$' "$tmp/dl-stat")" = 2 ] || fail "downloads are not 0600 3072 bytes: $(cat "$tmp/dl-stat")"
[ "$(grep -c 'Mode:0700' "$tmp/dirs")" = 3 ] || fail "browser folders are not 0700: $(cat "$tmp/dirs")"
python3 - "$tmp/slow.bin" <<'PY' || fail "the paused and resumed download is not the bytes the server sent"
import sys; d = open(sys.argv[1], 'rb').read(); sys.exit(0 if d == bytes((i * 7 + i // 251) % 256 for i in range(1500000)) else 1)
PY
echo "ok: the paused and resumed 1.5 MB download is byte for byte what the server sent"
echo "ok: files: two downloads 0600, the second not overwriting the first; ~/Downloads and the profile folders 0700"

# ---- part C: scale 2 (a 2560x1664 screen, like test/gui.sh): the page renders at device scale 2 and the pointer lands where it should ----
kill "$qpid" 2>/dev/null || true; rm -f "$DATA_IMG"
export GUI_RES=2560x1664; source ./env.sh; W=2560; H=1664; log=$tmp/serial2.log
./build.sh data > /dev/null 2>&1
"${QEMU[@]}" -device virtio-mouse-pci -no-reboot -display none -serial "file:$log" -monitor "unix:$sock,server,nowait" -qmp "unix:$qsock,server,nowait" > "$tmp/qemu2.out" 2>&1 &
qpid=$!; sleep 0.5
den_login
bbox() { magick "$tmp/$1.ppm" -fuzz 3% -transparent "#$2" -alpha extract -negate -trim -format '%wx%h%X%Y' info: ; }   # WxH+X+Y of the colour's area
type_str web; keys ret; sleep 4; type_str "10.0.2.2:18080/web/dpr.html"; keys ret
until_px s2a 1280 900 $JS 150
bb=$(bbox s2a $JS); echo "scale 2: the green page area is $bb"
PW_=${bb%%x*}; r=${bb#*x}; PH_=${r%%+*}; r=${r#*+}; PX=${r%%+*}; PY=${r#*+}
[ "$PW_" -ge 2400 ] && [ "$PH_" -ge 1300 ] || fail "the page does not fill the pane at scale 2 ($bb)"
expect s2a $((PX + 2000)) $((PY - 20)) 0b1c36   # the strip above it is the panel colour and 82 rows high (tabs and address, 2 x 32 font rows + 18)
cp "$tmp/s2a.ppm" "$tmp/web-scale2.ppm"
echo "ok: devicePixelRatio is 2 (the page is green) and the page fills the pane at scale 2"
goto "10.0.2.2:18080/web/btn.html"; until_px s2b $((PX + 1500)) $((PY + 900)) $BTN_BG 60
bb=$(bbox s2b $BTN_BG); r=${bb#*+}; PX=${r%%+*}; PY=${r#*+}
click $((PX + 820)) $((PY + 300)); sleep 1; shot s2c; expect s2c $((PX + 1500)) $((PY + 900)) $BTN_BG   # outside the button (css 410): no click
click $((PX + 780)) $((PY + 580)); until_px s2d $((PX + 1500)) $((PY + 900)) $BTN_ON 30   # inside (css 390,290): clicked
cp "$tmp/s2d.ppm" "$tmp/web-scale2-click.ppm"
magick "$tmp/web-scale2.ppm" -resize 50% "$OUTD/web-scale2.png"
magick "$tmp/web-scale2.ppm" -crop 2516x82+$PX+$((PY - 82)) +repage "$OUTD/web-scale2-strip.png"
echo "ok: at scale 2 a click lands on the right page pixel (inside the button clicks, 10 css px outside does not)"

echo "PASS: pictures in $OUTD"
