#!/usr/bin/env bash
# THIR_WEB=1 groundwork: boots the aarch64 web image (build it first: THIR_WEB=1 ./build.sh) and, as a normal user on the serial console, checks that
# /web is mounted from the add-on, the shared-lib symlinks work (ffmpeg runs), Mesa/EGL is there, /dev/shm is mounted, and the user cannot touch /web or /data/system.
set -euo pipefail
cd "$(dirname "$0")/.."
export THIR_WEB=1 DATA_IMG=out/aarch64/web-base-data.img QEMU_MEM=1024 QEMU_SMP=2
source ./env.sh
[ -f out/aarch64/web.sqsh ] || { echo "no out/aarch64/web.sqsh: run THIR_WEB=1 ./build.sh first"; exit 1; }
PW='correct-horse-1'; SECRETS=("$PW"); STEP=${WEB_STEP_TIMEOUT:-60}
source test/lib.sh
trap 'rm -f "$DATA_IMG"; kill "$qpid" 2>/dev/null || true; rm -rf "$d"' EXIT
./build.sh data > /dev/null 2>&1
boot boot1 "${QEMU[@]}"
wait_for '^thir: mounted /web$' 60
wait_for 'first boot: create the first user'
wait_for 'username: '; send alice; wait_for 'password: '; send "$PW"; wait_for 'repeat password: '; send "$PW"
wait_for 'utc offset'; send '+05:30'; wait_for 'user alice created'
ai_offline; wait_for '\$ $'
run whoami '^alice$'
run 'ls /web' 'usr/'
run 'ls /usr/bin' 'ffmpeg'
run '/usr/bin/ffmpeg -version' '^ffmpeg version 7\.'
run 'view --selftest' '^sandbox ok$'                  # Landlock holds: a read outside the rules fails
run 'view x.png' 'view needs the desktop'          # not in a den pane: den refuses the connection
run '/usr/bin/ffmpeg -decoders' 'png '
run 'ls /usr/lib' 'libEGL\.so\.1'
run 'ls /usr/lib' 'libgallium-.*\.so'
run 'ls /usr/lib/dri' 'libgallium|dri'
run 'cat /web/etc/web.env' '^EGL_PLATFORM=surfaceless$'
run 'ls /etc/fonts' 'fonts\.conf'
run 'ls /etc/ssl/certs' 'ca-certificates\.crt'
run 'cat /proc/mounts' '^tmpfs /dev/shm tmpfs rw,nosuid,nodev,noexec'
run 'cat /proc/mounts' '^/dev/loop[0-9]+ /web squashfs ro,nosuid,nodev'
run 'echo x > /dev/shm/t' '\$ $'
run 'cat /dev/shm/t' '^x$'
run 'ls /run' 'den/'
run 'cat /proc/sys/kernel/yama/ptrace_scope' '^1$'
run 'echo x > /web/x' '(Read-only file system|Permission denied)'
run 'ls /data/system' 'Permission denied'
run 'cat /data/system/web.sqsh' 'Permission denied'
run 'cat /dev/loop0' 'Permission denied'
off
echo "web-base: PASS"
