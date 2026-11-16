#!/usr/bin/env bash
# Records the thir demo from the VirtualBox window: macOS screen recording + keys sent with VBoxManage.
set -euo pipefail
: "${DEMO_PW:?set DEMO_PW}"
OUT=${1:?output .mov}
vm() { VBoxManage controlvm thir "$@" >/dev/null; }
typ() { local s=$1 i; for ((i = 0; i < ${#s}; i++)); do vm keyboardputstring "${s:i:1}"; sleep 0.04; done; }
ret() { vm keyboardputscancode 1c 9c; }
cmd() { typ "$1"; sleep 0.4; ret; sleep "${2:-1.6}"; }
ctrl() { vm keyboardputscancode 1d "$1" "$2" 9d; }        # Ctrl + key (make, break)
secret() { vm keyboardputstring "$DEMO_PW"; }             # one call: the password is never typed char by char into a log

osascript -e 'tell application id "org.virtualbox.app.VirtualBoxVM" to activate'; sleep 1   # the VM window must be in front
screencapture -v -V 150 "$OUT" & rec=$!
sleep 1; vm reset                                         # boot from the firmware on camera
sleep 16
cmd thir 1.2
secret; ret; sleep 1.2
secret; ret; sleep 1.2
ret; sleep 5                                              # accept the default time zone; Argon2 + login
cmd neofetch 3.5
cmd help 4
cmd 'mkdir projects' 1
cmd 'cd projects' 1
cmd 'edit notes.txt' 2
typ 'thir notes'; ret; typ 'a linux kernel and a rust userspace'; ret; typ 'small enough to read in one sitting'; sleep 1
ctrl 1f 9f; sleep 1.5                                     # Ctrl-S
ctrl 10 90; sleep 1.5                                     # Ctrl-Q
cmd 'cat notes.txt' 2
cmd 'mkdir -p ideas/2026' 1
cmd 'touch ideas/todo.txt' 1
cmd 'ls' 1.5
cmd "find . -name '*.txt'" 2
cmd 'mv notes.txt ideas/' 1
cmd 'ls ideas' 2
vm keyboardputscancode 38 1c 9c b8; sleep 2              # Alt+Enter: split
cmd top 5
typ q; sleep 1.5
cmd date 3
wait "$rec" 2>/dev/null || true
