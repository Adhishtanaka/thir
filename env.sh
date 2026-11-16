# Single source of truth for the guest arch. Sourced by build.sh, run.sh, test/smoke.sh, scripts/build-in-docker.sh.
# NOTE: variable is THIR_ARCH, not ARCH, because the kernel Makefile reads $ARCH.
THIR_ARCH=${THIR_ARCH:-aarch64}
KERNEL_VERSION=6.12.117
DOCKER_PLATFORM=linux/arm64        # build container is always native arm64
case "$THIR_ARCH" in
  aarch64)
    KARCH=arm64; KIMAGE=arch/arm64/boot/Image; RUST_TARGET=aarch64-unknown-linux-musl; CROSS_COMPILE=
    CONSOLE=ttyAMA0; QEMU_BIN=qemu-system-aarch64; ACCEL=${ACCEL:-hvf}
    if [ "$ACCEL" = hvf ]; then CPU=host; else CPU=cortex-a72; fi
    QEMU_MACHINE=(-M virt,gic-version=3 -cpu "$CPU" -accel "$ACCEL") ;;
  x86_64)  # cross-built in the arm64 container; QEMU runs it emulated (TCG), so it boots slower than aarch64
    KARCH=x86_64; KIMAGE=arch/x86/boot/bzImage; RUST_TARGET=x86_64-unknown-linux-musl; CROSS_COMPILE=x86_64-linux-gnu-
    CONSOLE=ttyS0; QEMU_BIN=qemu-system-x86_64; QEMU_MACHINE=(-M q35 -cpu max -accel tcg) ;;
  *) echo "unknown THIR_ARCH=$THIR_ARCH" >&2; return 1 ;;
esac
OUT=out/$THIR_ARCH
QEMU=("$QEMU_BIN" "${QEMU_MACHINE[@]}" -m 256 -smp 1
      -kernel $OUT/Image -initrd $OUT/initramfs.cpio.gz
      -append "console=$CONSOLE panic=1"
      -netdev user,id=n0 -device virtio-net-pci,netdev=n0
      -device virtio-gpu-pci)
