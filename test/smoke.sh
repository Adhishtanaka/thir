#!/usr/bin/env bash
# Headless boot test on a fresh data disk. Boot 1: first-boot setup, file commands, denied access, passwd, poweroff.
# Boot 2 (same disk): old password rejected, new one works, files survived. Passwords must never appear in the serial log.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/${THIR_ARCH:-aarch64}/smoke-data.img
export AUDIODEV="wav,path=$PWD/out/${THIR_ARCH:-aarch64}/smoke.wav"   # what the guest plays is captured here (checked after boot 1)
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
# AI setup: first "no AI", then settings turns the AI on: a provider that cannot be reached (retry, another provider), then the mock
wait_for 'SET UP YOUR AI'; wait_for 'choice \[0\]: '; send 0; wait_for 'the AI is off'
wait_for '\$ $'
run '! hello' 'the AI is off'
send settings; wait_for 'SETTINGS'; sendk $'\e[B\e[B\e[B\e[B\e[B'; sendk $'\r'   # down to the AI row, Enter: it is off, so the setup runs
wait_for 'choice \[0\]: '
send 6; wait_for 'name \[mine\]: '; send mock; wait_for 'protocol'; send anthropic; wait_for 'URL: '; send 'http://10.0.2.2:1/v1'
wait_for 'model \[claude-opus-5-5\]: '; send ''; wait_for 'could not connect'; wait_for '\[c\] choose another'; send c
wait_for 'choice \[0\]: '; send 6; wait_for 'name \[mine\]: '; send mock; wait_for 'protocol'; send anthropic; wait_for 'URL: '; send 'http://10.0.2.2:18080/anthropic/v1'
wait_for 'model \[claude-opus-5-5\]: '; send ''; wait_for 'mock answered'; wait_for 'agent will use mock'
sendk q; wait_for '\$ $'
run 'cat .config/thir/ai' 'No such file'
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
run 'mkdir -p .config/thir' '\$ $'
for l in 'mocka anthropic http://10.0.2.2:18080/anthropic/v1' 'mockr anthropic http://10.0.2.2:18080/refuse/v1' 'mocko openai http://10.0.2.2:18080/openai/v1 mock-model' 'mocke openai http://10.0.2.2:18080/err/v1 mock-model'; do
  run "echo provider $l >> .config/thir/agent" '\$ $'   # append: a rewrite would drop the default line the setup made
done
run '/bin/agent providers' 'mocka +anthropic +http://10\.0\.2\.2:18080/anthropic/v1 +no key needed'
send '/bin/agent chat -p mocka hi'; wait_for 'mock reply 1 café$'; wait_for '^second line'; wait_for '\$ $'
run '/bin/agent chat -p mocko hi' 'openai mock reply 1$'
run '/bin/agent chat -p mockr hi' 'declined this request \(category: cyber\)'
run '/bin/agent chat -p mocke hi' 'Incorrect API key provided'
run '/bin/agent login mocka' 'plain http'
run '/bin/agent chat -p openai hi' 'needs a model'
run '/bin/agent chat -p anthropic hi' 'no API key for anthropic'
send '/bin/agent login anthropic'; wait_for 'key'; send "$FAKEKEY"; wait_for 'saved'
run '/bin/agent providers' 'anthropic .*key saved'
send '/bin/agent chat -p mocka'; wait_for 'chat> '; send one; wait_for 'mock reply 1 '; send two; wait_for 'mock reply 3 '
send /clear; wait_for 'chat> '; send three; wait_for 'mock reply 1 '; send /exit; wait_for '\$ $'
if [ "${ONLINE:-}" = 1 ]; then run 'curl -sI https://example.com/' '^HTTP/1\.1 [23]0[0-9]'; run '/bin/agent chat hi' 'rejected the API key'; fi
run '/bin/agent logout anthropic' 'removed'

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
# apps (after the Tab test above: a calc app would make `ca` ambiguous)
run '/bin/agent list' 'no apps yet'
run 'mkdir -p apps/calc apps/hello apps/crash' '\$ $'
for a in calc hello crash; do   # canned apps from test/apps, fetched from the mock server
  run "curl -s -o apps/$a/main.lua http://10.0.2.2:18080/apps/$a.lua" '\$ $'; run "curl -s -o apps/$a/app.json http://10.0.2.2:18080/apps/$a.json" '\$ $'
done
run '/bin/agent list' '^ +calc +v1 +A calculator +none$'
run '/bin/agent context' '^time: .*\+05:30$'               # the summary agent sends with chats (never file contents, hidden names or history arguments)
run '/bin/agent context' '^- calc v1: A calculator$'
run '/bin/agent context' '^network: 10\.0\.2\.15$'
run '/bin/agent context' '^recent commands: .*/bin/agent'
run '/bin/agent context' '^files: .*docs/'
send '/bin/agent run calc'; wait_for 'q quit'; sendk '1+2='; wait_for '= 3'; sendk q; wait_for '\$ $'           # an app by name; net none needs no approval
send hello; wait_for 'connect to 10\.0\.2\.2:18080 \(not encrypted\)'; send n; wait_for 'did not run'; wait_for '\$ $'
send hello; wait_for 'connect to 10\.0\.2\.2:18080 \(not encrypted\)'; send y; wait_for 'hello from mock'; wait_for 'tz 05:30'; sendk q; wait_for '\$ $'   # tz 05:30: os.date follows ~/.tz
run 'cat .config/thir/approvals' '^hello http://10\.0\.2\.2:18080$'   # approvals live outside the app dir; hello.json ships a fake "approved" list that was ignored
send hello; wait_for 'hello from mock'; sendk q; wait_for '\$ $'                              # approved once: no question the second time
send '/bin/agent run crash'; wait_for 'q quit'; sendk l; wait_for 'crash stopped: .*more than 2 s' 20; wait_for 'fix crash now with AI'; send n; wait_for 'later: type ! fix crash'; wait_for '\$ $'   # an endless loop is stopped
send '/bin/agent run crash'; wait_for 'q quit'; sendk x; wait_for 'crash stopped: main\.lua:'; wait_for 'fix crash now with AI'; send n; wait_for 'later: type ! fix crash'; wait_for '\$ $'
run 'cat apps/crash/last_error' 'main\.lua:'
send $'cr\t'; wait_for 'q quit'; sendk q; wait_for '\$ $'                                      # Tab completes an app name; the send's newline runs it
run '/bin/agent run nosuch' 'no app called nosuch'
run '/bin/agent run ../calc' 'no app called'
# build, edit, fix, undo and rm against the mock (apps made by the AI)
run 'echo provider mockb anthropic http://10.0.2.2:18080/build/v1 >> .config/thir/agent' '\$ $'
# self-healing: a crash offers a fix now; yes runs the fix pipeline and opens the app again (the mock answers any ===ERROR=== request with the calculator)
run '/bin/agent use mockb' 'agent will use mockb'
send '/bin/agent run crash'; wait_for 'q quit'; sendk x; wait_for 'fix crash now with AI'; send y; wait_for 'saved crash v2'; wait_for 'q quit'; sendk '1+2='; wait_for '= 3'; sendk q; wait_for '\$ $'
run '/bin/agent use mock' 'agent will use mock'
send '/bin/agent build -p mockb a calculator'; wait_for 'fixing \(1/3\)'; wait_for 'saved calc-2 v1'
wait_for 'q quit'; sendk '1+2='; wait_for '= 3'; sendk q; wait_for 'Change something'; send ''; wait_for '\$ $'
run '/bin/agent list' '^ +calc-2 +v1'
send '/bin/agent edit -p mockb calc-2 "add memory"'; wait_for 'saved calc-2 v2'; wait_for 'Calc\+'; sendk q; wait_for 'Change'; send ''; wait_for '\$ $'
run '/bin/agent undo calc-2' 'back to version 1'
run '/bin/agent fix calc-2' 'no saved crash'
run '/bin/agent build -p mockb how to cook lasagna' 'not an app I can build: a recipe'
send '/bin/agent rm calc-2'; wait_for 'REMOVE CALC-2'; wait_for 'cannot be undone'; send y; wait_for 'removed'
run '/bin/agent list' '^ +calc +v1'
# !memory: facts kept in ~/.config/thir/memory, sent only with chats and builds; a build uses them
run '/bin/agent memory' 'remembers nothing yet'
run '/bin/agent remember city: Colombo' 'remembered: city: Colombo'
run '/bin/agent remember units: metric' 'remembered: units: metric'
run '/bin/agent remember my password is hunter2' 'never remembers'   # typed on purpose; hunter2 is not in SECRETS
run '/bin/agent memory' '^ +1 +city: Colombo$'
send '/bin/agent build -p mockb a weather app'; wait_for 'memory.*city: Colombo'; wait_for 'saved weather v1'
wait_for 'Weather for Colombo'; sendk q; wait_for 'Change something'; send ''; wait_for '\$ $'
run '/bin/agent remember city: Kandy' 'updated: city: Kandy'
run '/bin/agent forget city' 'forgot: city: Kandy'
run '/bin/agent build -p mockb a weather app' 'not an app I can build: mock: no city'
send '/bin/agent forget all'; wait_for 'FORGET EVERYTHING'; send y; wait_for 'forgot everything'
# the background helper and reminders (the helper started at login; its notices show before the next prompt)
run 'agent daemon' 'already running'      # the hidden form: one helper per user
run '/bin/agent remind +5s drink tea' 'reminder 1: in 5 s · drink tea'
run '/bin/agent remind daily 07:00 open weather' 'reminder 2: daily 07:00 · open weather'
run '/bin/agent schedule list' '^ +2 +daily 07:00 +open weather$'
run '/bin/agent context' '^reminders: 2, next '
sleep 15; run 'echo tick' '→ [0-9]{2}:[0-9]{2}  drink tea'
run 'cat .notify' '^[0-9]+ drink tea$'
run '/bin/agent schedule rm 2' 'removed reminder 2'
run '/bin/agent schedule rm 9' 'no reminder 9'
run '/bin/agent remind 25:00 x' 'TIME is HH:MM'
run '/bin/agent remind +5s my password is hunter2' 'looks like a password'
# the AI shell: a typo gets a hint, `! words` go to the AI, which proposes one command or action and asks first (the mock answers by the words)
send '!'; wait_for 'Type ! and your words'; wait_for 'Plain lines without ! are a normal shell'; wait_for '\$ $'
run 'help > h2.txt' '\$ $'
run 'cat h2.txt' '^  ! +your words +ask the AI'
run 'mkdri docs' 'nsh: mkdri: command not found\. Did you mean mkdir\?'
run 'foo bar' 'command not found \(type ! and your words to ask the AI\)'
run 'what apps do I have' 'nsh: what: command not found'   # plain words are a normal shell now: no AI call
send '! what apps do I have'; wait_for '→ answer'; wait_for 'mock: you have calc'; wait_for '\$ $'
send '!please open the calculator'; wait_for '→ calc +run\? \[Y/n\]'; send ''; wait_for 'CALC'; sendk $'\x04'; wait_for '\$ $'   # the built-in calc app
send '! make me a weather app'; wait_for '→ build an app: make me a weather app'; send n; wait_for 'nothing done'; wait_for '\$ $'
send '! I live in Colombo'; wait_for '→ remember: city: Colombo'; send y; wait_for 'remembered: city: Colombo'; wait_for '\$ $'
run '/bin/agent forget city' 'forgot: city: Colombo'
send '! remind me in 5 seconds to drink water'; wait_for '→ reminder: in 5 s · drink water'; send y; wait_for 'reminder [0-9]+: in 5 s'; wait_for '\$ $'
# plain words work the apps: a change is shown and asked first, a read shows what it sends (only the matches), arithmetic is done by thir
send '! add Nimal to my contacts, 0719876543'; wait_for '→ contacts: add Nimal · 0719876543 +run\? \[Y/n\]'; send y; wait_for 'saved Nimal in contacts'; wait_for '\$ $'
run 'cat .config/thir/contacts/contacts' '^2.Nimal.0719876543'
run '! add evil to my contacts' 'is not a phone number'
send "! what is Kasun's number"; wait_for '→ private data: sends 1 contact matching "kasun".*only this goes to the AI'; send y; wait_for 'mock saw: Kasun Perera \| phones: 0771234567'; wait_for '\$ $'
if tail -n +$((mark + 1)) "$log" | grep -a 'mock saw' | grep -q Nimal; then echo "FAIL: a contact that was not asked for went to the AI"; exit 1; fi
run "! what is Zed's number" 'no contact matches "zed"'
send '! add buy milk to my notes as a todo'; wait_for '→ notes: add "\[ \] buy milk" to the note Todo +run\? \[Y/n\]'; send y; wait_for 'added to the note Todo'; wait_for '\$ $'
run 'cat .config/thir/notes/notes' '^2.[0-9]+.[0-9]+..Todo.\[ \] buy milk$'
send '! add dentist to my calendar today at 14:30'; wait_for '→ calendar: Dentist · 20[0-9-]+ 14:30 · remind 15 min before'; send y; wait_for 'saved Dentist in the calendar'; wait_for '\$ $'
send '! what is on my calendar today'; wait_for '→ private data: sends 2 events from 20[0-9-]+ \(date, time, title\)'; send y; wait_for 'mock saw: 20[0-9-]+ 14:30 Dentist ; 20[0-9-]+ 14:30 Dentist'; wait_for '\$ $'
send '! set an alarm for 6:30'; wait_for '→ alarm: 06:30 once, (today|tomorrow) 06:30 +· it rings even with no app open +run\? \[Y/n\]'; send y; wait_for 'alarm set'; wait_for '\$ $'
run 'cat .config/thir/clock/alarms' '^2.@[0-9]+.1.$'
run '! what is 12 times 15' '→ 12\*15 = 180'
# one command line from the AI: shown exactly, run only after a yes; a dangerous one defaults to no; shell tricks are refused
run 'echo hi > notes.txt' '\$ $'
send '! move notes into docs'; wait_for '→ mv notes\.txt docs/ +run\? \[Y/n\]'; send ''; wait_for '\$ $'
run 'ls docs' 'notes\.txt'
run 'rm docs/notes.txt' '\$ $'
send '! delete everything'; wait_for '! this deletes files or folders'; wait_for '→ rm -r docs +run\? \[y/N\]'; send ''; wait_for 'nothing done'; wait_for '\$ $'
run 'ls' 'docs/'
run '! run a shell for me' 'could not turn that into one command'
run '! open ../../bin/sh' 'could not turn that into one command'
send '! forget my city'; wait_for '! this removes it from what the AI remembers'; wait_for 'forget +city +run\? \[y/N\]'; send n; wait_for 'nothing done'; wait_for '\$ $'
# file tasks in plain words; the plan is shown and nothing changes before y
run 'echo provider mockd anthropic http://10.0.2.2:18080/do/v1 >> .config/thir/agent' '\$ $'
run '/bin/agent do -p mockd make notes.txt > out.txt' 'needs a terminal'
send '/bin/agent do -p mockd make notes.txt with my shopping list: milk, eggs'; wait_for '\+ +notes\.txt +new · 2 lines'; wait_for '^ +milk'; wait_for 'apply 1 change\? \[y/N\]'
send y; wait_for '1 total · 1 done'; wait_for 'undo: rm notes\.txt'; wait_for '\$ $'
run 'cat notes.txt' '^eggs$'
run '/bin/agent do -p mockd make notes.txt with my shopping list: milk, eggs' 'notes\.txt already exists'
send '/bin/agent do -p mockd move notes.txt into docs/'; wait_for '→ +notes\.txt → docs/notes\.txt'; wait_for 'apply 1 change\? \[y/N\]'; send n; wait_for 'nothing changed\.'; wait_for '\$ $'
run 'cat notes.txt' '^milk$'
send '/bin/agent do -p mockd move notes.txt into docs/'; wait_for 'apply 1 change'; send y; wait_for 'undo: mv docs/notes\.txt notes\.txt'; wait_for '\$ $'
run 'cat docs/notes.txt' '^eggs$'
send '/bin/agent do -p mockd download http://10.0.2.2:18080/people.json to data/people.json'; wait_for '\+ +data/ +new folder'; wait_for '↓ +data/people\.json +10\.0\.2\.2:18080 +not encrypted'
wait_for 'apply 2 changes\? \[y/N\]'; send y; wait_for '2 total · 2 done'; wait_for 'undo: rm -r data'; wait_for '\$ $'
run 'cat data/people.json' 'Grace'
send '/bin/agent do -p mockd write the names in http://10.0.2.2:18080/people.json into names.txt'; wait_for 'read · '; wait_for '^ +Ada'
wait_for 'apply 1 change'; send y; wait_for '1 total · 1 done'; wait_for '\$ $'
run 'cat names.txt' '^Linus$'
run '/bin/agent do -p mockd save my keys' 'hidden files and folders are off limits'
run '/bin/agent do -p mockd break calc' '~/apps changes only when the AI builds'
run '/bin/agent do -p mockd sneaky' 'not in your request'
run '/bin/agent do -p mockd delete everything' 'unknown action RUN'
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
