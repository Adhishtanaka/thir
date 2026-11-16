#!/usr/bin/env bash
set -euo pipefail
source /work/env.sh
export CARGO_HOME=/build/cargo-home CARGO_TARGET_DIR=/build/target
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-gnu-gcc   # the container is arm64
if [ "${1:-all}" = test ]; then cd /work && exec cargo test; fi
mkdata() { rm -f "/work/$DATA_IMG"; truncate -s "$DATA_SIZE" "/work/$DATA_IMG"; mkfs.ext4 -q -F -L thirdata "/work/$DATA_IMG"; echo "data image: /work/$DATA_IMG ($DATA_SIZE)"; }
if [ "${1:-all}" = data ]; then mkdata; exit 0; fi

SRC=/build/linux-$KERNEL_VERSION; OBJ=/build/kobj-$THIR_ARCH; O=/work/$OUT; LIST=/build/initramfs-$THIR_ARCH.list
KMAKE=(make -C "$SRC" O="$OBJ" ARCH="$KARCH" CROSS_COMPILE="$CROSS_COMPILE")
[ -d "$SRC" ] || curl -fL "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KERNEL_VERSION.tar.xz" | tar -xJ -C /build
mkdir -p "$OBJ" "$O"

cd /work && cargo build --release --target "$RUST_TARGET"
for b in init nsh dhcp den login passwd edit find top; do cp "$CARGO_TARGET_DIR/$RUST_TARGET/release/$b" "$O/"; done
rm -f "$O/initramfs.cpio.gz"
cat > "$LIST" <<EOL
dir /dev 0755 0 0
nod /dev/console 0600 0 0 c 5 1
dir /proc 0755 0 0
dir /sys 0755 0 0
dir /tmp 01777 0 0
dir /run 0755 0 0
dir /etc 0755 0 0
dir /bin 0755 0 0
dir /share 0755 0 0
dir /data 0755 0 0
file /init $O/init 0755 0 0
file /bin/nsh $O/nsh 0755 0 0
file /bin/dhcp $O/dhcp 0755 0 0
file /bin/den $O/den 0755 0 0
file /bin/login $O/login 0755 0 0
file /bin/passwd $O/passwd 04755 0 0
file /bin/edit $O/edit 0755 0 0
file /bin/find $O/find 0755 0 0
file /bin/top $O/top 0755 0 0
slink /bin/nano /bin/edit 0777 0 0
file /share/font16.psf /work/assets/fonts/Uni3-Terminus16.psf 0644 0 0
file /share/font32.psf /work/assets/fonts/Uni3-Terminus32x16.psf 0644 0 0
EOL

"${KMAKE[@]}" tinyconfig
cat /work/kernel/common.config "/work/kernel/$THIR_ARCH.config" >> "$OBJ/.config"
"${KMAKE[@]}" olddefconfig
# every requested line must survive olddefconfig (catches unmet dependencies)
cat /work/kernel/common.config "/work/kernel/$THIR_ARCH.config" | while read -r line; do
  case "$line" in
    "# CONFIG_"*" is not set") sym=${line#\# }; sym=${sym%% *}
      if grep -q "^$sym=" "$OBJ/.config"; then echo "kernel config: $sym is still enabled (selected by another option?)" >&2; exit 1; fi ;;
    "#"*|"") ;;
    *) grep -qxF "$line" "$OBJ/.config" || { echo "kernel config: '$line' was not applied (missing dependency?)" >&2; exit 1; } ;;
  esac
done
"${KMAKE[@]}" -j"$(nproc)" "$(basename "$KIMAGE")"      # the initramfs ($LIST) is built into the kernel
cp "$OBJ/$KIMAGE" "$O/Image"

mkimg() {   # GPT: p1 ESP (FAT32, the kernel as the default EFI app), p2 ext4 "thirdata". Plain files only: no loop devices in Docker.
  local img=$O/thir.img esp=/build/esp-$THIR_ARCH.img
  rm -f "$img" "$esp"; truncate -s 256M "$img"; truncate -s 64M "$esp"
  printf 'label: gpt\nstart=2048, size=64MiB, type=uefi, name=thiresp\nstart=133120, size=190MiB, type=linux, name=thirdata\n' | sfdisk -q "$img"
  mkfs.fat -F 32 -n THIRESP "$esp" > /dev/null
  mmd -i "$esp" ::/EFI ::/EFI/BOOT
  mcopy -i "$esp" "$O/Image" "::/EFI/BOOT/$EFI_NAME"
  dd if="$esp" of="$img" bs=1M seek=1 conv=notrunc,sparse status=none
  mkfs.ext4 -q -F -L thirdata -E offset=$((65 * 1048576)) "$img" 190M
  echo "uefi image: $img (ESP 64 MiB + thirdata 190 MiB)"
}
mkimg
[ -f "/work/$DATA_IMG" ] || mkdata
ls -l "$O"
