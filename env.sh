# Single source of truth for the guest arch. Sourced by build.sh, run.sh, test/smoke.sh, scripts/build-in-docker.sh.
# NOTE: variable is THIR_ARCH, not ARCH, because the kernel Makefile reads $ARCH.
THIR_ARCH=${THIR_ARCH:-aarch64}
KERNEL_VERSION=6.12.117
DOCKER_PLATFORM=linux/arm64        # build container is always native arm64
case "$THIR_ARCH" in
  aarch64)
    KARCH=arm64; KIMAGE=arch/arm64/boot/Image; RUST_TARGET=aarch64-unknown-linux-musl; CROSS_COMPILE=
    CONSOLE=ttyAMA0; EFI_NAME=BOOTAA64.EFI; EDK2_CODE=/opt/homebrew/share/qemu/edk2-aarch64-code.fd; EDK2_VARS=/opt/homebrew/share/qemu/edk2-arm-vars.fd
    QEMU_BIN=qemu-system-aarch64; ACCEL=${ACCEL:-hvf}
    if [ "$ACCEL" = hvf ]; then CPU=host; else CPU=cortex-a72; fi
    QEMU_MACHINE=(-M virt,gic-version=3 -cpu "$CPU" -accel "$ACCEL") ;;
  x86_64)  # cross-built in the arm64 container; QEMU runs it emulated (TCG), so it boots slower than aarch64
    KARCH=x86_64; KIMAGE=arch/x86/boot/bzImage; RUST_TARGET=x86_64-unknown-linux-musl; CROSS_COMPILE=x86_64-linux-gnu-
    CONSOLE=ttyS0; EFI_NAME=BOOTX64.EFI; EDK2_CODE=/opt/homebrew/share/qemu/edk2-x86_64-code.fd; EDK2_VARS=/opt/homebrew/share/qemu/edk2-i386-vars.fd
    QEMU_BIN=qemu-system-x86_64; QEMU_MACHINE=(-M q35 -cpu max -accel tcg) ;;
  *) echo "unknown THIR_ARCH=$THIR_ARCH" >&2; return 1 ;;
esac
THIR_FLAVOR=${THIR_FLAVOR:-}   # "vbox" (x86_64 only): AHCI disk + VMSVGA screen for VirtualBox on x86 hosts; never for the USB image
FSUF=${THIR_FLAVOR:+-$THIR_FLAVOR}
OUT=out/$THIR_ARCH$FSUF
DATA_IMG=${DATA_IMG:-$OUT/data.img}   # raw ext4 image; path relative to the repo root (also /work in the container)
DATA_SIZE=${DATA_SIZE:-128M}
GUI_RES=${GUI_RES:-1280x800}
QEMU_BASE=("$QEMU_BIN" "${QEMU_MACHINE[@]}" -m 256 -smp 1
      -netdev user,id=n0 -device virtio-net-pci,netdev=n0 -device virtio-rng-pci)
QEMU_GPU=(-device virtio-gpu-pci,id=gpu0,xres=${GUI_RES%x*},yres=${GUI_RES#*x} -device virtio-keyboard-pci)
QEMU=("${QEMU_BASE[@]}" "${QEMU_GPU[@]}" -kernel $OUT/Image -append "console=$CONSOLE panic=1"   # initramfs is inside the kernel
      -drive file=$DATA_IMG,format=raw,if=none,id=d0 -device virtio-blk-pci,drive=d0)
