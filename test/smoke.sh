#!/usr/bin/env bash
# Headless boot test on a fresh data disk. Boot 1: first-boot setup, file commands, denied access, passwd, poweroff.
# Boot 2 (same disk): old password rejected, new one works, files survived. Passwords must never appear in the serial log.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/${THIR_ARCH:-aarch64}/smoke-data.img
source ./env.sh
TIMEOUT=${SMOKE_TIMEOUT:-30}   # argon2 is slow under x86_64 emulation (STEP)
PW1='correct-horse-1'; PW2='tr0ub4dor-two-2'; PWX='wrongpass-xx'; FAKEKEY=sk-ant-thir-smoke-not-a-key
SECRETS=("$PW1" "$PW2" "$PWX" "$FAKEKEY"); STEP=${SMOKE_STEP_TIMEOUT:-60}
source test/lib.sh
./build.sh data > /dev/null 2>&1          # fresh disk for this run only

python3 test/mock.py "$d" & mpid=$!      # web server for the curl and agent tests; certificates exist before the guest boots
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
# mandatory AI setup: a provider that cannot be reached (offered: retry, another provider), then the mock
wait_for 'SET UP YOUR AI'; wait_for 'provider \[1\]: '
send 6; wait_for 'name \[mine\]: '; send mock; wait_for 'protocol'; send anthropic; wait_for 'URL: '; send 'http://10.0.2.2:1/v1'
wait_for 'model \[claude-opus-5-5\]: '; send ''; wait_for 'could not connect'; wait_for '\[c\] choose another'; send c
wait_for 'provider \[1\]: '; send 6; wait_for 'name \[mine\]: '; send mock; wait_for 'protocol'; send anthropic; wait_for 'URL: '; send 'http://10.0.2.2:18080/anthropic/v1'
wait_for 'model \[claude-opus-5-5\]: '; send ''; wait_for 'mock answered'; wait_for 'agent will use mock'
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
run 'mkdir -p .config/thir' '\$ $'
for l in 'mocka anthropic http://10.0.2.2:18080/anthropic/v1' 'mockr anthropic http://10.0.2.2:18080/refuse/v1' 'mocko openai http://10.0.2.2:18080/openai/v1 mock-model' 'mocke openai http://10.0.2.2:18080/err/v1 mock-model'; do
  run "echo provider $l >> .config/thir/agent" '\$ $'   # append: a rewrite would drop the default line the setup made
done
run '!providers' 'mocka +anthropic +http://10\.0\.2\.2:18080/anthropic/v1 +no key needed'
send '!chat -p mocka hi'; wait_for 'mock reply 1 café$'; wait_for '^second line'; wait_for '\$ $'
run '!chat -p mocko hi' 'openai mock reply 1$'
run '!chat -p mockr hi' 'declined this request \(category: cyber\)'
run '!chat -p mocke hi' 'Incorrect API key provided'
run '!login mocka' 'plain http'
run '!chat -p openai hi' 'needs a model'
run '!chat -p anthropic hi' 'no API key for anthropic'
send '!login anthropic'; wait_for 'key'; send "$FAKEKEY"; wait_for 'saved'
run '!providers' 'anthropic .*key saved'
send '!chat -p mocka'; wait_for 'chat> '; send one; wait_for 'mock reply 1 '; send two; wait_for 'mock reply 3 '
send /clear; wait_for 'chat> '; send three; wait_for 'mock reply 1 '; send /exit; wait_for '\$ $'
if [ "${ONLINE:-}" = 1 ]; then run 'curl -sI https://example.com/' '^HTTP/1\.1 [23]0[0-9]'; run '!chat hi' 'rejected the API key'; fi
run '!logout anthropic' 'removed'
run 'rm /' "refusing to remove '/'"
run 'cat /proc/sys/kernel/hostname' '^thir$'
send $'ca\t/etc/res\t'   # Tab completion; the send's newline runs it. The echo is on the old partial line, so only the output proves it
wait_for '^nameserver 10\.0\.2\.3$'
wait_for '\$ $'
run history '^ *[0-9]+ +whoami$'
# apps (after the Tab test above: a calc app would make `ca` ambiguous)
run '!list' 'no apps yet'
run 'mkdir -p apps/calc apps/hello apps/crash' '\$ $'
for a in calc hello crash; do   # canned apps from test/apps, fetched from the mock server
  run "curl -s -o apps/$a/main.lua http://10.0.2.2:18080/apps/$a.lua" '\$ $'; run "curl -s -o apps/$a/app.json http://10.0.2.2:18080/apps/$a.json" '\$ $'
done
run '!list' '^ +calc +v1 +A calculator +none$'
run '!context' '^time: .*\+05:30$'               # the summary agent sends with chats (never file contents, hidden names or history arguments)
run '!context' '^- calc v1: A calculator$'
run '!context' '^network: 10\.0\.2\.15$'
run '!context' '^recent commands: .*!'
run '!context' '^files: .*docs/'
send calc; wait_for 'q quit'; sendk '1+2='; wait_for '= 3'; sendk q; wait_for '\$ $'           # an app by name; net none needs no approval
send hello; wait_for 'connect to 10\.0\.2\.2:18080 \(not encrypted\)'; send n; wait_for 'did not run'; wait_for '\$ $'
send hello; wait_for 'connect to 10\.0\.2\.2:18080 \(not encrypted\)'; send y; wait_for 'hello from mock'; wait_for 'tz 05:30'; sendk q; wait_for '\$ $'   # tz 05:30: os.date follows ~/.tz
run 'cat .config/thir/approvals' '^hello http://10\.0\.2\.2:18080$'   # approvals live outside the app dir; hello.json ships a fake "approved" list that was ignored
send hello; wait_for 'hello from mock'; sendk q; wait_for '\$ $'                              # approved once: no question the second time
send '!run crash'; wait_for 'q quit'; sendk l; wait_for 'crash stopped: .*more than 2 s' 20; wait_for 'fix crash now with AI'; send n; wait_for 'later: !fix crash'; wait_for '\$ $'   # an endless loop is stopped
send '!run crash'; wait_for 'q quit'; sendk x; wait_for 'crash stopped: main\.lua:'; wait_for 'fix crash now with AI'; send n; wait_for 'later: !fix crash'; wait_for '\$ $'
run 'cat apps/crash/last_error' 'main\.lua:'
send $'cr\t'; wait_for 'q quit'; sendk q; wait_for '\$ $'                                      # Tab completes an app name; the send's newline runs it
send $'!ru\t'; wait_for 'usage: !run APP'; wait_for '\$ $'                                  # Tab completes a ! verb
send $'!run cr\t'; wait_for 'q quit'; sendk q; wait_for '\$ $'                                # and an app name after it
run '!run nosuch' 'no app called nosuch'
run '!run ../calc' 'no app called'
# !build, !edit, !fix, !undo and !rm against the mock (apps made by the AI)
run 'echo provider mockb anthropic http://10.0.2.2:18080/build/v1 >> .config/thir/agent' '\$ $'
# self-healing: a crash offers a fix now; yes runs the fix pipeline and opens the app again (the mock answers any ===ERROR=== request with the calculator)
run '!use mockb' 'agent will use mockb'
send '!run crash'; wait_for 'q quit'; sendk x; wait_for 'fix crash now with AI'; send y; wait_for 'saved crash v2'; wait_for 'q quit'; sendk '1+2='; wait_for '= 3'; sendk q; wait_for '\$ $'
run '!use mock' 'agent will use mock'
send '!build -p mockb a calculator'; wait_for 'fixing \(1/3\)'; wait_for 'saved calc-2 v1'
wait_for 'q quit'; sendk '1+2='; wait_for '= 3'; sendk q; wait_for 'Change something'; send ''; wait_for '\$ $'
run '!list' '^ +calc-2 +v1'
send '!edit -p mockb calc-2 "add memory"'; wait_for 'saved calc-2 v2'; wait_for 'Calc\+'; sendk q; wait_for 'Change'; send ''; wait_for '\$ $'
run '!undo calc-2' 'back to version 1'
run '!fix calc-2' 'no saved crash'
run '!build -p mockb how to cook lasagna' 'not an app I can build: a recipe'
send '!rm calc-2'; wait_for 'REMOVE CALC-2'; wait_for 'cannot be undone'; send y; wait_for 'removed'
run '!list' '^ +calc +v1'
# !memory: facts kept in ~/.config/thir/memory, sent only with chats and builds; a build uses them
run '!memory' 'remembers nothing yet'
run '!remember city: Colombo' 'remembered: city: Colombo'
run '!remember units: metric' 'remembered: units: metric'
run '!remember my password is hunter2' 'never remembers'   # typed on purpose; hunter2 is not in SECRETS
run '!memory' '^ +1 +city: Colombo$'
send '!build -p mockb a weather app'; wait_for 'memory.*city: Colombo'; wait_for 'saved weather v1'
wait_for 'Weather for Colombo'; sendk q; wait_for 'Change something'; send ''; wait_for '\$ $'
run '!remember city: Kandy' 'updated: city: Kandy'
run '!forget city' 'forgot: city: Kandy'
run '!build -p mockb a weather app' 'not an app I can build: mock: no city'
send '!forget all'; wait_for 'FORGET EVERYTHING'; send y; wait_for 'forgot everything'
# the background helper and reminders (the helper started at login; its notices show before the next prompt)
run 'agent daemon' 'already running'      # the hidden form: one helper per user
run '!remind +5s drink tea' 'reminder 1: in 5 s · drink tea'
run '!remind daily 07:00 open weather' 'reminder 2: daily 07:00 · open weather'
run '!schedule list' '^ +2 +daily 07:00 +open weather$'
run '!context' '^reminders: 2, next '
sleep 15; run 'echo tick' '→ [0-9]{2}:[0-9]{2}  drink tea'
run 'cat .notify' '^[0-9]+ drink tea$'
run '!schedule rm 2' 'removed reminder 2'
run '!schedule rm 9' 'no reminder 9'
run '!remind 25:00 x' 'TIME is HH:MM'
run '!remind +5s my password is hunter2' 'looks like a password'
# the AI shell: a typo gets a hint, plain words go to the AI, which picks one action and asks first (the mock answers by the words)
send '!'; wait_for 'THE ! COMMANDS'; wait_for 'effort for anthropic'; wait_for '\$ $'
run 'help' '^  ! cmds +!build'
run 'mkdri docs' 'nsh: mkdri: command not found\. Did you mean mkdir\?'
run 'foo bar' 'command not found \(3 or more words ask the AI'
send 'what apps do I have'; wait_for '→ answer'; wait_for 'mock: you have calc'; wait_for '\$ $'
send 'please open the calculator'; wait_for '→ open: calc +ok\? \[Y/n\]'; send ''; wait_for 'q quit'; sendk q; wait_for '\$ $'
send 'make me a weather app'; wait_for '→ build: make me a weather app'; send n; wait_for 'nothing done'; wait_for '\$ $'
send 'I live in Colombo'; wait_for '→ remember: city: Colombo'; send y; wait_for 'remembered: city: Colombo'; wait_for '\$ $'
run '!forget city' 'forgot: city: Colombo'
send 'remind me in 5 seconds to drink water'; wait_for '→ remind: in 5 s · drink water'; send y; wait_for 'reminder [0-9]+: in 5 s'; wait_for '\$ $'
run '!ask open ../../bin/sh' 'could not tell what to do'
run '!ask run a shell for me' 'could not tell what to do'
# !do: file tasks in plain words; the plan is shown and nothing changes before y
run 'echo provider mockd anthropic http://10.0.2.2:18080/do/v1 >> .config/thir/agent' '\$ $'
run '!do -p mockd make notes.txt > out.txt' 'needs a terminal'
send '!do -p mockd make notes.txt with my shopping list: milk, eggs'; wait_for '\+ +notes\.txt +new · 2 lines'; wait_for '^ +milk'; wait_for 'apply 1 change\? \[y/N\]'
send y; wait_for '1 total · 1 done'; wait_for 'undo: rm notes\.txt'; wait_for '\$ $'
run 'cat notes.txt' '^eggs$'
run '!do -p mockd make notes.txt with my shopping list: milk, eggs' 'notes\.txt already exists'
send '!do -p mockd move notes.txt into docs/'; wait_for '→ +notes\.txt → docs/notes\.txt'; wait_for 'apply 1 change\? \[y/N\]'; send n; wait_for 'nothing changed\.'; wait_for '\$ $'
run 'cat notes.txt' '^milk$'
send '!do -p mockd move notes.txt into docs/'; wait_for 'apply 1 change'; send y; wait_for 'undo: mv docs/notes\.txt notes\.txt'; wait_for '\$ $'
run 'cat docs/notes.txt' '^eggs$'
send '!do -p mockd download http://10.0.2.2:18080/people.json to data/people.json'; wait_for '\+ +data/ +new folder'; wait_for '↓ +data/people\.json +10\.0\.2\.2:18080 +not encrypted'
wait_for 'apply 2 changes\? \[y/N\]'; send y; wait_for '2 total · 2 done'; wait_for 'undo: rm -r data'; wait_for '\$ $'
run 'cat data/people.json' 'Grace'
send '!do -p mockd write the names in http://10.0.2.2:18080/people.json into names.txt'; wait_for 'read · '; wait_for '^ +Ada'
wait_for 'apply 1 change'; send y; wait_for '1 total · 1 done'; wait_for '\$ $'
run 'cat names.txt' '^Linus$'
run '!do -p mockd save my keys' 'hidden files and folders are off limits'
run '!do -p mockd break calc' '~/apps changes only with !build'
run '!do -p mockd sneaky' 'not in your request'
run '!do -p mockd delete everything' 'unknown action RUN'
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
