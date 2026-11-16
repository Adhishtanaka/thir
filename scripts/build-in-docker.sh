#!/usr/bin/env bash
set -euo pipefail
source /work/env.sh
export CARGO_HOME=/build/cargo-home CARGO_TARGET_DIR=/build/target
if [ "${1:-all}" = test ]; then cd /work && exec cargo test; fi

SRC=/build/linux-$KERNEL_VERSION; OBJ=/build/kobj-$NETOS_ARCH
[ -d "$SRC" ] || curl -fL "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KERNEL_VERSION.tar.xz" | tar -xJ -C /build
mkdir -p "$OBJ"
make -C "$SRC" O="$OBJ" ARCH="$KARCH" tinyconfig
cat /work/kernel/common.config "/work/kernel/$NETOS_ARCH.config" >> "$OBJ/.config"
make -C "$SRC" O="$OBJ" ARCH="$KARCH" olddefconfig
# every requested line must survive olddefconfig (catches unmet dependencies)
grep -hv '^#' /work/kernel/common.config "/work/kernel/$NETOS_ARCH.config" | grep . | while read -r line; do
  grep -qxF "$line" "$OBJ/.config" || { echo "kernel config: '$line' was not applied (missing dependency?)" >&2; exit 1; }
done
make -C "$SRC" O="$OBJ" ARCH="$KARCH" -j"$(nproc)" Image
cp "$OBJ/$KIMAGE" /work/out/Image
cc -O2 -o /build/gen_init_cpio "$SRC/usr/gen_init_cpio.c"

cd /work && cargo build --release --target "$RUST_TARGET"
cp "$CARGO_TARGET_DIR/$RUST_TARGET/release/init" "$CARGO_TARGET_DIR/$RUST_TARGET/release/nsh" /work/out/

cat > /build/initramfs.list <<EOL
dir /dev 0755 0 0
nod /dev/console 0600 0 0 c 5 1
dir /proc 0755 0 0
dir /sys 0755 0 0
dir /tmp 01777 0 0
dir /run 0755 0 0
dir /bin 0755 0 0
file /init /work/out/init 0755 0 0
file /bin/nsh /work/out/nsh 0755 0 0
EOL
/build/gen_init_cpio /build/initramfs.list | gzip -9 > /work/out/initramfs.cpio.gz
ls -l /work/out
