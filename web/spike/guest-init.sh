#!/bin/busybox sh
# P0 spike guest /init (busybox initramfs): /dev/vda = web.sqsh, /dev/vdb = test.sqsh (p0, js.html, strace). Prints results on the serial console.
export PATH=/bin:/usr/bin:/t/bin
/bin/busybox --install -s /bin
mount -t proc proc /proc; mount -t sysfs sys /sys; mount -t devtmpfs dev /dev
mkdir -p /web /t /tmp /run /dev/shm /home
mount -t tmpfs -o mode=1777,nosuid,nodev,noexec tmpfs /dev/shm; mount -t tmpfs tmpfs /tmp
mount -t squashfs -o ro,nodev,nosuid /dev/vda /web || mount -t squashfs -o loop,ro /dev/vda /web || echo "MOUNT-FAIL web"
mount -t squashfs -o loop,ro /dev/vdb /t || echo "MOUNT-FAIL test"
# the links thir's initramfs will carry
rm -rf /lib /usr; ln -s /web/lib /lib; ln -s /web/usr /usr; mkdir -p /etc; ln -s /web/etc/fonts /etc/fonts; ln -s /web/etc/ssl /etc/ssl
export LD_LIBRARY_PATH=/t:/t/stub HOME=/tmp XDG_RUNTIME_DIR=/run XDG_CACHE_HOME=/tmp XDG_DATA_HOME=/tmp WEBKIT_SKIA_ENABLE_CPU_RENDERING=1 G_MESSAGES_DEBUG=
if grep -q mesa= /proc/cmdline; then export LD_LIBRARY_PATH=/t GALLIUM_DRIVER=softpipe LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless LIBGL_DEBUG=verbose EGL_LOG_LEVEL=debug MESA_DEBUG=1; grep -q mesa=2 /proc/cmdline && unset GALLIUM_DRIVER; fi
grep -q jit=0 /proc/cmdline && export JSC_useJIT=false
echo "=== kernel $(uname -r) mem $(grep MemTotal /proc/meminfo)"
echo "=== ffmpeg"; /usr/bin/ffmpeg -version 2>&1 | head -3
if grep -q http=1 /proc/cmdline; then
  ifconfig lo up; ifconfig eth0 10.0.2.15 netmask 255.255.255.0 up; route add default gw 10.0.2.2
  echo "=== p0 via http (load_uri)"; P0_LOAD_URI=1 /t/p0 http://10.0.2.2:18080/js.html /tmp/out3.png 60 2>&1 | grep -v "GIO-DEBUG\|libEGL\|libGL\|TimeZone" | tail -12
  echo "=== p0 via file:// (load_uri)"; P0_LOAD_URI=1 /t/p0 file:///t/js.html /tmp/out4.png 40 2>&1 | grep -v "GIO-DEBUG\|libEGL\|libGL\|TimeZone" | tail -8
fi
echo "=== p0 plain"
G_MESSAGES_DEBUG=all /t/p0 file:///t/js.html /tmp/out.png 40 2>&1 | tail -40; echo "p0 exit=$?"
if [ -n "$STRACE" ] || grep -q strace=1 /proc/cmdline; then
  echo "=== strace -f -Z"
  LD_LIBRARY_PATH=/t /t/strace -f -Z -o /tmp/strace.txt /t/p0 file:///t/js.html /tmp/out2.png 240 2>&1 | tail -10
  echo "--- ENOSYS/EACCES/EPERM/EINVAL counts"; grep -c ENOSYS /tmp/strace.txt; grep -c EACCES /tmp/strace.txt; grep -c EPERM /tmp/strace.txt
  echo "--- ENOSYS lines"; grep ENOSYS /tmp/strace.txt | sed 's/^[0-9]* //' | cut -c1-160 | sort | uniq -c | sort -rn | head -20
  echo "--- EACCES/EPERM lines"; grep -E "EACCES|EPERM" /tmp/strace.txt | sed 's/^[0-9]* //' | cut -c1-160 | sort | uniq -c | sort -rn | head -20
  echo "--- all failing syscall names"; grep -o '^[0-9]* [a-z_0-9]*(' /tmp/strace.txt | awk '{print $2}' | sort | uniq -c | sort -rn | head -30
  echo "--- crash/sig"; grep -E "SIGSEGV|SIGABRT|SIGILL|SIGBUS|killed" /tmp/strace.txt | head
  echo "=== STRACE-GZ"; gzip -c /tmp/strace.txt | base64; echo "=== END-STRACE-GZ"
fi
for f in out.png out2.png; do [ -f /tmp/$f ] && { echo "=== PNG $f"; base64 /tmp/$f; echo "=== END-PNG"; }; done
echo "=== DONE"; poweroff -f
