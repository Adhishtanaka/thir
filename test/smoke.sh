#!/usr/bin/env bash
# Headless boot test on a fresh data disk. Boot 1: first-boot setup, file commands, denied access, passwd, poweroff.
# Boot 2 (same disk): old password rejected, new one works, files survived. Passwords must never appear in the serial log.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/${THIR_ARCH:-aarch64}/smoke-data.img
export AUDIODEV="wav,path=$PWD/out/${THIR_ARCH:-aarch64}/smoke.wav"   # what the guest plays is captured here (checked after boot 1)
source ./env.sh
TIMEOUT=${SMOKE_TIMEOUT:-30}   # argon2 is slow under x86_64 emulation (STEP)
PW1='correct-horse-1'; PW2='tr0ub4dor-two-2'; PWX='wrongpass-xx'
SECRETS=("$PW1" "$PW2" "$PWX"); STEP=${SMOKE_STEP_TIMEOUT:-60}
source test/lib.sh
./build.sh data > /dev/null 2>&1          # fresh disk for this run only

python3 test/mock.py "$d" & mpid=$!      # web server for the curl tests; certificates exist before the guest boots
for _ in $(seq 1 50); do [ -f "$d/srv.pem" ] && (echo > /dev/tcp/127.0.0.1/18443) 2>/dev/null && break; sleep 0.2; done

boot boot1 "${QEMU[@]}"
wait_for '^thir: init ready$' "$TIMEOUT"
echo "boot-to-marker: ${SECONDS}s"
wait_for '^thir: mounted /data$' 5
wait_for '^thir: iface eth0 ' 5
if tr -d '\r' < "$log" | grep -qE '^thir: iface (sit0|tunl0) '; then echo "FAIL: tunnel device present"; cat "$log"; exit 1; fi
wait_for 'dhcp: eth0 10\.0\.2\.15 mask 255\.255\.255\.0 gw 10\.0\.2\.2 dns 10\.0\.2\.3$' 10   # no ^: may follow a prompt
wait_for 'first boot: create the first user'
wait_for 'username: '
send alice;  wait_for 'password: '
send "$PW1"; wait_for 'repeat password: '
send "$PW1"; wait_for 'utc offset'
send '+05:30'; wait_for 'user alice created'
wait_for '\$ $'
run whoami '^alice$'
run pwd '^/data/home/alice$'
run 'mkdir docs' '\$ $'
run 'echo hello > docs/a.txt' '\$ $'
run 'echo world >> docs/a.txt' '\$ $'
send 'cat docs/a.txt'; wait_for '^hello$'; wait_for '^world$'
run 'cat /data/etc/users' 'cat: /data/etc/users: Permission denied'
run 'cat /dev/vda' 'cat: /dev/vda: Permission denied'
run 'dhcp lo' '\[exit [1-9]'              # needs root: fails at once as a user
run date '^(Mon|Tue|Wed|Thu|Fri|Sat|Sun) [0-9]{2} [A-Z][a-z]{2} 20[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} \+05:30$'
run tz '^\+05:30$'
run 'tz 5' 'use \+HH:MM'
run "find . -name '*.txt'" '^\./docs/a\.txt$'
run 'find docs -type d' '^docs$'
run 'help > help.txt' '\$ $'
run 'cat help.txt' '^  programs +edit'
run 'cat help.txt' '^  apps +clock'
run 'help find' 'find \[dir\]'
send 'edit docs/b.txt'; wait_for 'edit  docs/b.txt'; sendk 'line one'; sendk $'\x13'; wait_for 'Saved 1 line'
sendk $'\x11'; wait_for '\$ $'
run 'cat docs/b.txt' '^line one$'
send neofetch; wait_for 'alice@thir'; wait_for 'Kernel: 6\.'; wait_for '\$ $'
send top; wait_for 'PID +USER'
sendk q; wait_for '\$ $'
run 'curl http://10.0.2.2:18080/hello' '^hello from mock$'
run 'curl -s http://10.0.2.2:18080/chunked' '^chunk-two$'
run 'curl -sL http://10.0.2.2:18080/redirect' '^hello from mock$'
run 'curl -sL http://10.0.2.2:18080/loop' '\[exit 47\]'
run 'curl -sf http://10.0.2.2:18080/404' '\[exit 22\]'
run 'curl -sI http://10.0.2.2:18080/hello' '^HTTP/1\.[01] 200'
run 'curl -s -o bin.dat http://10.0.2.2:18080/bin' '\$ $'
run 'curl -s --data-binary @bin.dat http://10.0.2.2:18080/echo' '^POST same noauth$'
run 'curl -k https://10.0.2.2:18443/hello' 'always checks certificates'
run 'curl -s https://10.0.2.2:18443/hello' '\[exit 60\]'
run 'curl -s -o ca.pem http://10.0.2.2:18080/ca.pem' '\$ $'
run 'curl -s --cacert ca.pem https://10.0.2.2:18443/hello' '^hello from mock$'
run 'curl -sL --cacert ca.pem https://10.0.2.2:18443/downgrade' 'https to http'
run 'curl -s --max-time 2 http://10.0.2.2:18080/slow' '\[exit 28\]'
run 'curl -s http://10.0.2.2:1/' '\[exit 7\]'
run 'curl -s http://no-such-host.invalid/' '\[exit 6\]'

# the apps: open each one and do one thing (keys go straight to the full-screen program)
send help; wait_for 'THE DESKTOP'; wait_for 'the thir guide'; sendk q; wait_for '\$ $'
send calc; wait_for 'CALC'; sendk '2+3*4'; sendk $'\r'; wait_for 'ans 14'; sendk $'\x04'; wait_for '\$ $'
run 'cat .config/thir/calc/history' '^2\+3\*4.14$'
send contacts; wait_for 'No contacts yet'; sendk a; wait_for 'name:'; sendk $'Kasun Perera\r'; wait_for 'phones'; sendk $'0771234567\r'; wait_for 'emails'; sendk $'kasun@example.lk\r'
wait_for 'note:'; sendk $'school friend\r'; wait_for 'contact saved'; sendk '/'; wait_for 'search:'; sendk $'0771\r'; wait_for 'search: 0771'; sendk q; wait_for '\$ $'
run 'cat .config/thir/contacts/contacts' '^1.Kasun Perera.0771234567.kasun@example.lk.school friend$'
send notes; wait_for 'No notes yet'; sendk a; wait_for 'title:'; sendk $'Shopping\r'; wait_for 'Empty. Press a'; sendk t; wait_for 'todo:'; sendk $'buy milk\r'; wait_for '☐ buy milk'
sendk ' '; wait_for '☑ buy milk'; sendk q; wait_for '1/1'; sendk q; wait_for '\$ $'
run 'cat .config/thir/notes/notes' 'Shopping.\[x\] buy milk$'
send calendar; wait_for 'CALENDAR'; sendk a; wait_for 'title:'; sendk $'Dentist\r'; wait_for 'date YYYY'; sendk $'\r'; wait_for 'time HH'; sendk $'14:30\r'; wait_for 'remind before'; sendk $'\r'
wait_for 'event saved'; sendk q; wait_for '\$ $'
run 'cat .config/thir/calendar/events' '^1.20[0-9-]+.14:30.15.Dentist$'
send settings; wait_for 'SETTINGS'; sendk $'\x1b[B'; sendk $'\r'; wait_for 'graphite'; sendk $'\x1b[B'; sendk $'\r'; wait_for 'sound off'; sendk q; wait_for '\$ $'
send 'cat .config/thir/settings'; wait_for '^wall=graphite$'; wait_for '^sound=off$'; wait_for '\$ $'
run beep 'beep: sound is off'
run 'echo sound=on > .config/thir/settings' '\$ $'
run 'ls /dev/snd' '^pcmC0D0p$'
run 'cat /dev/snd/timer' 'Permission denied'   # the mixer and timer (and any capture node: unit-tested) stay root only; only the playback stream is ours
run 'cat /dev/snd/controlC0' 'Permission denied'
run beep '\$ $'
# an alarm one to two minutes ahead (the guest is on +05:30); it rings with no app open, and the notice shows before a later prompt
AL=$(date -u -v+5H -v+30M -v+90S +%H:%M)
send clock; wait_for 'CLOCK'; sendk 4; wait_for 'No alarms yet'; sendk a; wait_for 'time HH'; sendk "$AL"$'\r'; wait_for 'repeat'; sendk $'\r'; wait_for 'label:'; sendk $'smoke test\r'; wait_for 'alarm saved'
sendk q; wait_for '\$ $'
t0=$SECONDS; until tail -n +$((mark + 1)) "$log" | tr -d '\r' | grep -q 'Alarm [0-9:]* smoke test'; do
  (( SECONDS - t0 < 170 )) || { echo "FAIL: the alarm did not show a notice within 170 s"; leak_check; cat "$log"; exit 1; }
  send ''; sleep 4   # the notice shows before the prompt this send brings; check after it has had time to arrive
done
echo "alarm notice after $((SECONDS - t0)) s"
run 'cat .config/thir/clock/alarms' '^1.@[0-9]+.1.smoke test$'
run 'rm /' "refusing to remove '/'"
run 'cat /proc/sys/kernel/hostname' '^thir$'
send $'cu\t -s http://10.0.2.2:18080/hello'   # Tab completes a command name (curl is the only cu...)
wait_for '^hello from mock$'; wait_for '\$ $'
send $'cat /etc/res\t'   # and a file name; the send's newline runs it. The echo is on the old partial line, so only the output proves it
wait_for '^nameserver 10\.0\.2\.3$'
wait_for '\$ $'
run history '^ *[0-9]+ +whoami$'
# autosuggestion (grey rest of the newest matching history line; Right accepts it) and the Tab candidate list
run 'echo suggestion works' '^suggestion works$'
send ''; wait_for '\$ $'; sendk 'echo sugg'; sendk $'\x1b[C'; sendk $'\r'; wait_for '^suggestion works$'; wait_for '\$ $'
run 'mkdir -p tabdir/aa1 tabdir/aa2' '\$ $'
send ''; wait_for '\$ $'; sendk $'ls tabdir/a\t\t'; wait_for '^aa1/ +aa2/$'; sendk $'\x03'; wait_for '\^C'; wait_for '\$ $'
send passwd; wait_for 'old password: '
send "$PWX"; wait_for 'passwd: wrong password'; t0=$SECONDS
wait_for 'old password: '
(( SECONDS - t0 >= 2 )) || { echo "FAIL: wrong old password was not delayed"; exit 1; }
send "$PW1"; wait_for 'new password: '
send "$PW2"; wait_for 'repeat new password: '
send "$PW2"; wait_for 'passwd: password changed'
wait_for '\$ $'
off
cp "out/$THIR_ARCH/smoke.wav" "$d/boot1.wav"   # QEMU's wav backend is complete once the guest is off; boot 2 would overwrite it
python3 - "$d/boot1.wav" <<'PY'
import sys, array   # QEMU leaves the sizes in the wav header at 0 when it is stopped, so read the samples (stereo, 16 bit, 44.1 kHz) after the 44-byte header
raw = open(sys.argv[1], 'rb').read()[44:]; a = array.array('h', raw[:len(raw) // 2 * 2])
secs = len(a) / 2 / 44100; peak = max(abs(x) for x in a) if a else 0
assert secs > 3 and peak > 3000, f"guest audio: {secs:.1f} s, peak {peak}"
print(f"sound: {secs:.1f} s of audio from the guest, peak {peak}")
PY
echo "boot 1 ok"

boot boot2 "${QEMU[@]}"
wait_for 'thir login: ' "$TIMEOUT"
send alice;  wait_for 'password: '
send "$PW1"; wait_for 'login incorrect'
wait_for 'thir login: '
send alice;  wait_for 'password: '
send "$PW2"; wait_for '\$ $'
run 'cat docs/a.txt' '^hello$'
run date '\+05:30$'
run ls '^docs/$'
off
leak_check
echo PASS
