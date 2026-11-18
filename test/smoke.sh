#!/usr/bin/env bash
# Headless boot test on a fresh data disk. Boot 1: first-boot setup, file commands, denied access, passwd, poweroff.
# Boot 2 (same disk): old password rejected, new one works, files survived. Passwords must never appear in the serial log.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/${THIR_ARCH:-aarch64}/smoke-data.img
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
run help '^  programs +edit'
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
run 'rm /' "refusing to remove '/'"
run 'cat /proc/sys/kernel/hostname' '^thir$'
send $'ca\t/etc/res\t'   # Tab completion; the send's newline runs it. The echo is on the old partial line, so only the output proves it
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
