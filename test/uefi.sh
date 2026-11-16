#!/usr/bin/env bash
# Boots out/<arch>/thir.img on edk2 firmware (no -kernel). Boot 1: first-boot setup, a file on /data. Boot 2: the file is still there.
set -euo pipefail
cd "$(dirname "$0")/.."; source ./env.sh
[ -f "$EDK2_CODE" ] || { echo "FAIL: $EDK2_CODE missing (brew install qemu)"; exit 1; }
PW=uefi-pass-123; SECRETS=("$PW"); STEP=${UEFI_STEP_TIMEOUT:-120}
source test/lib.sh
TIMEOUT=${UEFI_TIMEOUT:-300}
cp -c "$OUT/thir.img" "$d/thir.img" 2>/dev/null || cp "$OUT/thir.img" "$d/thir.img"; cp "$EDK2_VARS" "$d/vars.fd"
case $THIR_ARCH in
  x86_64)  dev=sda2; disk=(-device qemu-xhci -device usb-storage,drive=d0 -device usb-kbd -device VGA) ;;
  aarch64) dev=vda2; disk=(-device virtio-blk-pci,drive=d0 "${QEMU_GPU[@]}") ;;
esac
uefi=("${QEMU_BASE[@]}" -drive if=pflash,format=raw,readonly=on,file="$EDK2_CODE" -drive if=pflash,format=raw,file="$d/vars.fd"
      -drive file="$d/thir.img",format=raw,if=none,id=d0 "${disk[@]}")
login() {
  wait_for 'thir login: ' "$STEP"; send alice; wait_for 'password: '; send "$PW"; wait_for '\$ $'
}

boot boot1 "${uefi[@]}"
wait_for '^thir: init ready$' "$TIMEOUT"; echo "boot-to-marker: ${SECONDS}s"
wait_for '^thir: mounted /data$' 20
wait_for 'den: [0-9]+x[0-9]+ 32bpp$' 30
wait_for 'first boot: create the first user'
wait_for 'username: '
send alice;  wait_for 'password: '
send "$PW"; wait_for 'repeat password: '
send "$PW"; wait_for 'utc offset'
send ''; wait_for 'user alice created'
wait_for '\$ $'
run 'cat /proc/mounts' "^/dev/$dev /data ext4 "
run 'echo persisted > note.txt' '\$ $'
off
boot boot2 "${uefi[@]}"
wait_for '^thir: init ready$' "$TIMEOUT"
login
run 'cat note.txt' '^persisted$'
off; leak_check
echo "PASS (boot-to-marker above, total ${SECONDS}s)"
