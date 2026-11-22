#!/usr/bin/env bash
# The VirtualBox-on-x86 flavour (THIR_FLAVOR=vbox) on edk2: disk on AHCI/SATA, QEMU's VMware SVGA-II screen (what vmwgfx drives).
# VBOX_GPU=vmware-svga tries vmwgfx (it fails on QEMU); the default VGA gives the EFI framebuffer (efifb).
# Boot 1: first-boot setup, a file on /data, a screendump. Boot 2: the file is still there. Screendump: $VBOX_SHOT (optional).
set -euo pipefail
cd "$(dirname "$0")/.."; export THIR_ARCH=x86_64 THIR_FLAVOR=vbox; source ./env.sh
PW=vbox-pass-123; SECRETS=("$PW"); STEP=${UEFI_STEP_TIMEOUT:-120}
source test/lib.sh
TIMEOUT=${UEFI_TIMEOUT:-300}
cp -c "$OUT/thir.img" "$d/thir.img" 2>/dev/null || cp "$OUT/thir.img" "$d/thir.img"; cp "$EDK2_VARS" "$d/vars.fd"
vbox=("$QEMU_BIN" "${QEMU_MACHINE[@]}" -m ${THIR_WEB:+1024}${THIR_SMALL:+512} -smp ${THIR_WEB:+2}${THIR_SMALL:+1} -netdev user,id=n0 -device virtio-net-pci,netdev=n0
      -drive if=pflash,format=raw,readonly=on,file="$EDK2_CODE" -drive if=pflash,format=raw,file="$d/vars.fd"
      -device ahci,id=ahci -drive file="$d/thir.img",format=raw,if=none,id=d0 -device ide-hd,drive=d0,bus=ahci.0
      -device ${VBOX_GPU:-VGA} -device qemu-xhci -device usb-kbd -qmp "unix:$d/qmp,server,nowait")
shot() { python3 - "$d/qmp" "$1" <<'P'
import socket,sys,json,time
s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f=s.makefile('rw')
f.readline(); f.write('{"execute":"qmp_capabilities"}\n'); f.flush(); f.readline()
f.write(json.dumps({"execute":"screendump","arguments":{"filename":sys.argv[2],"format":"png"}})+"\n"); f.flush(); print(f.readline().strip())
P
}
boot boot1 "${vbox[@]}"
wait_for '^thir: init ready$' "$TIMEOUT"; echo "boot-to-marker: ${SECONDS}s"
wait_for '^thir: mounted /data$' 20
wait_for 'den: [0-9]+x[0-9]+ 32bpp$' 30
grep -iE 'vmwgfx|efifb|fb0' "$log" | tr -d '\r' || true
wait_for 'enter +create your account'; send ''; wait_for 'first boot: create the first user'
wait_for 'username: '
send alice;  wait_for 'password: '
send "$PW"; wait_for 'repeat password: '
send "$PW"; wait_for 'utc offset'
send ''; wait_for 'user alice created'
ai_offline
wait_for '\$ $'
sleep 3; shot "${VBOX_SHOT:-$d/shot.png}"
run 'cat /proc/mounts' '^/dev/sda2 /data ext4 '
run 'echo persisted > note.txt' '\$ $'
off
boot boot2 "${vbox[@]}"
wait_for '^thir: init ready$' "$TIMEOUT"
wait_for 'thir login: ' "$STEP"; send alice; wait_for 'password: '; send "$PW"; wait_for '\$ $'
run 'cat note.txt' '^persisted$'
off; leak_check
echo "PASS (total ${SECONDS}s)"
