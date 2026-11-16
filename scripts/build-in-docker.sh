#!/usr/bin/env bash
set -euo pipefail
source /work/env.sh
export CARGO_HOME=/build/cargo-home CARGO_TARGET_DIR=/build/target
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-gnu-gcc   # the container is arm64
if [ "${1:-all}" = test ]; then cd /work && exec cargo test; fi

SRC=/build/linux-$KERNEL_VERSION; OBJ=/build/kobj-$THIR_ARCH; O=/work/$OUT
KMAKE=(make -C "$SRC" O="$OBJ" ARCH="$KARCH" CROSS_COMPILE="$CROSS_COMPILE")
[ -d "$SRC" ] || curl -fL "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KERNEL_VERSION.tar.xz" | tar -xJ -C /build
mkdir -p "$OBJ" "$O"
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
"${KMAKE[@]}" -j"$(nproc)" "$(basename "$KIMAGE")"
cp "$OBJ/$KIMAGE" "$O/Image"
cc -O2 -o /build/gen_init_cpio "$SRC/usr/gen_init_cpio.c"

cd /work && cargo build --release --target "$RUST_TARGET"
for b in init nsh dhcp den; do cp "$CARGO_TARGET_DIR/$RUST_TARGET/release/$b" "$O/"; done

cat > /build/initramfs.list <<EOL
dir /dev 0755 0 0
nod /dev/console 0600 0 0 c 5 1
dir /proc 0755 0 0
dir /sys 0755 0 0
dir /tmp 01777 0 0
dir /run 0755 0 0
dir /etc 0755 0 0
dir /bin 0755 0 0
dir /share 0755 0 0
file /init $O/init 0755 0 0
file /bin/nsh $O/nsh 0755 0 0
file /bin/dhcp $O/dhcp 0755 0 0
file /bin/den $O/den 0755 0 0
file /share/wallpaper.png /work/assets/wallpaper.png 0644 0 0
file /share/font.psf /work/assets/fonts/Uni3-Terminus16.psf 0644 0 0
EOL
/build/gen_init_cpio /build/initramfs.list | gzip -9 > "$O/initramfs.cpio.gz"
ls -l "$O"
