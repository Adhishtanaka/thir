#!/usr/bin/env bash
# groundwork: boots the image (aarch64, or THIR_ARCH=x86_64) (build it first: ./build.sh) and, as a normal user on the serial console, checks that
# /web is mounted from the add-on, the shared-lib symlinks work (ffmpeg runs), Mesa/EGL is there, /dev/shm is mounted, and the user cannot touch /web or /data/system.
set -euo pipefail
cd "$(dirname "$0")/.."
export DATA_IMG=out/${THIR_ARCH:-aarch64}/web-base-data.img QEMU_MEM=1024 QEMU_SMP=2
source ./env.sh
[ -f out/${THIR_ARCH:-aarch64}/web.sqsh ] || { echo "no out/${THIR_ARCH:-aarch64}/web.sqsh: run ./build.sh first"; exit 1; }
PW='correct-horse-1'; SECRETS=("$PW"); STEP=${WEB_STEP_TIMEOUT:-60}
source test/lib.sh
trap 'rm -f "$DATA_IMG"; kill "$qpid" 2>/dev/null || true; rm -rf "$d"' EXIT
./build.sh data > /dev/null 2>&1
boot boot1 "${QEMU[@]}"
wait_for '^thir: mounted /web$' 60
wait_for 'enter +create your account'; send ''; wait_for 'first boot: create the first user'
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
# the browser itself: JavaScript, then a video decoded by GStreamer + gst-libav (the page's title says so once the video has played 0.6 s)
trap 'rm -f "$DATA_IMG"; kill "$qpid" ${mpid:-} 2>/dev/null || true; rm -rf "$d"' EXIT
python3 test/mock.py "$d" & mpid=$!
for _ in $(seq 50); do [ -f "$d/srv.pem" ] && (echo > /dev/tcp/127.0.0.1/18080) 2>/dev/null && break; sleep 0.2; done
for _ in $(seq 40); do send 'curl -s http://10.0.2.2:18080/hello'; sleep 3; tail -n +$((mark + 1)) "$log" | tr -d '\r' | grep -q '^hello from mock$' && break; done
run 'ls /usr/lib/gstreamer-1.0' 'libgstlibav\.so'
send 'web --dump-title http://10.0.2.2:18080/web/js.html'; wait_for '^js-ok-42$' 300
echo "ok: web --dump-title: WebKit runs JavaScript on ${THIR_ARCH:-aarch64}"
send 'web --dump-title http://10.0.2.2:18080/web/video.html#wait=60000'; wait_for '^video-ok-160x120$' 400
echo "ok: an HTML5 video (H.264) decoded and played on ${THIR_ARCH:-aarch64}"
off
echo "web-base: PASS"
