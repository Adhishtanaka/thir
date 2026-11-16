# Single source of truth for the guest arch. Sourced by build.sh, run.sh, test/smoke.sh, scripts/build-in-docker.sh.
# NOTE: variable is NETOS_ARCH, not ARCH, because the kernel Makefile reads $ARCH.
NETOS_ARCH=${NETOS_ARCH:-aarch64}
KERNEL_VERSION=6.12.117
DOCKER_PLATFORM=linux/arm64        # build container is always native arm64
case "$NETOS_ARCH" in
  aarch64)
    KARCH=arm64; KIMAGE=arch/arm64/boot/Image; RUST_TARGET=aarch64-unknown-linux-musl
    CONSOLE=ttyAMA0; QEMU_BIN=qemu-system-aarch64; ACCEL=${ACCEL:-hvf}
    if [ "$ACCEL" = hvf ]; then CPU=host; else CPU=cortex-a72; fi
    QEMU_MACHINE=(-M virt,gic-version=3 -cpu "$CPU" -accel "$ACCEL") ;;
  x86_64)  # shortcut: untested; needs gcc-x86-64-linux-gnu + CROSS_COMPILE in the Dockerfile/script and kernel/x86_64.config (SERIAL_8250 + _CONSOLE). Upgrade when a PC target is wanted.
    KARCH=x86_64; KIMAGE=arch/x86/boot/bzImage; RUST_TARGET=x86_64-unknown-linux-musl
    CONSOLE=ttyS0; QEMU_BIN=qemu-system-x86_64; QEMU_MACHINE=(-M q35 -cpu max -accel tcg) ;;
  *) echo "unknown NETOS_ARCH=$NETOS_ARCH" >&2; return 1 ;;
esac
QEMU=("$QEMU_BIN" "${QEMU_MACHINE[@]}" -m 256 -smp 1
      -kernel out/Image -initrd out/initramfs.cpio.gz
      -append "console=$CONSOLE panic=1"
      -netdev user,id=n0 -device virtio-net-pci,netdev=n0
      -device virtio-gpu-pci)
