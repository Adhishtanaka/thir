# thir — Phase 5 plan: UEFI-bootable disk image (made by the Fable planner)

Scope: boot path, kernel config, disk image, init data-partition lookup, den on the firmware framebuffer, UEFI test, README. Not the apps phase (planned separately). The "network tools" phase is cancelled; the dhcp client stays.
Conventions unchanged: libc-only userspace (+argon2/subtle), comments only for the why, unit tests for pure logic, fragment verification loop (add dependencies, never delete lines), no shell scripts in the image.

## 0. Decisions

| # | Decision | Choice | Why |
|---|---|---|---|
| 1 | Boot path | **Linux EFI stub, no bootloader.** Kernel = `EFI/BOOT/BOOTX64.EFI` / `BOOTAA64.EFI` on the ESP. initramfs **built into the kernel** (`CONFIG_INITRAMFS_SOURCE` = the existing gen_init_cpio list file; `usr/gen_initramfs.sh` accepts that format and tracks listed files). Built-in cmdline (x86: `CMDLINE_BOOL`; arm64: `CMDLINE` + `CMDLINE_FROM_BOOTLOADER`). | One file, no loader. A UKI/systemd-boot needs extra tools for the same result. arm64 `Image` and x86 bzImage with `EFI_STUB` are PE/COFF (`MZ`). |
| 2 | Secure Boot | **Unsigned; the user must disable Secure Boot.** | A Microsoft-signed shim is unobtainable; self-signing needs per-PC key enrolment. Out of scope. |
| 3 | Disk layout | GPT, **256 MiB sparse**: p1 ESP FAT32 `thiresp` at 1 MiB, 64 MiB (type `uefi`); p2 ext4 label + partlabel **`thirdata`** at 65 MiB, 190 MiB (type `linux`). Output `out/<arch>/thir.img`, regenerated on every build. | FAT32 needs ≥ ~33 MiB. Sparse ≈ 12 MB real. |
| 4 | Image tools (unprivileged Docker) | `sfdisk` (`fdisk` pkg), `mkfs.fat` + `mmd`/`mcopy` (`dosfstools`, `mtools`), `mkfs.ext4 -E offset=`. Plain files only. | ~1.1 MB apt download. |
| 5 | Finding `/data` | init scans `/sys/class/block/*`, reads 1 KiB at offset 1024 of each `/dev/<name>`, keeps the device whose **ext4 superblock label is `thirdata`** (magic 0xEF53 at 0x38, label at 0x78..0x88). Retry every 200 ms up to **15 s**. | One mechanism for `vda` (data.img), `vda2`, `sda2` (USB stick). Pure parser in `src/disk.rs`. |
| 6 | x86_64 framebuffer | **`CONFIG_FB=y` + `CONFIG_FB_EFI=y`** (efifb). | Maps the GOP framebuffer directly (32 bpp XRGB); no check_var, so den's forced FBIOPUT returns 0 and keeps the mode. virtio-gpu evicts efifb in QEMU, so the -kernel flow is unchanged. In common.config so UTM's ramfb works on aarch64 too. |
| 7 | aarch64 image | **Build it** (BOOTAA64.EFI). Last step; cap at ~1 h; if edk2 aarch64 misbehaves, document "untested". | ~20 lines; boots in UTM/edk2. Deliverable is x86_64. |
| 8 | AHCI/NVMe | **Not added.** | thir never touches the PC's internal disks; impossible to damage them by accident. |
| 9 | NIC drivers | **`E1000E`, `IGC`, `R8169`** (x86 only), ~150 KB compressed. | dhcp and the bar's address work on most wired PCs. No Wi-Fi. |
| 10 | USB | xHCI + EHCI, `USB_STORAGE` + `SCSI` + `BLK_DEV_SD`, `HID` + `HID_GENERIC` + `USB_HID`, `PCI_MSI`. | Boot stick + USB keyboard. den rescans /dev/input every second already. |
| 11 | cmdline | x86: `console=ttyS0,115200 panic=1`; arm64: `console=ttyAMA0 panic=1`. **No `console=tty0`.** | fbcon is off; ttyS0 always exists on x86. |
| 12 | Tests | Keep smoke.sh and gui.sh on the direct `-kernel` flow. New **`test/uefi.sh`** boots `thir.img` on edk2: x86_64 as a **USB stick on qemu-xhci with `-device VGA`** (real-PC path); aarch64 as virtio-blk + virtio-gpu. Shared helpers move to `test/lib.sh`. | UEFI under TCG is slow; no reason to slow every run. |
| 13 | QEMU firmware | pflash: `/opt/homebrew/share/qemu/edk2-x86_64-code.fd` + `edk2-i386-vars.fd`; `edk2-aarch64-code.fd` + `edk2-arm-vars.fd` (per-run copy of vars). | Pairings from `/opt/homebrew/share/qemu/firmware/60-edk2-*.json`. |
| 14 | den | **No logic change.** `src/gui/fb.rs` already ignores the FBIOPUT result; reword the comment. | Smallest change. |

## 1. Host/build side

### `docker/Dockerfile`
Append `dosfstools mtools fdisk` to the apt line.

### `env.sh`
- Per arch: `EFI_NAME`, `EDK2_CODE`, `EDK2_VARS`:
  - aarch64: `EFI_NAME=BOOTAA64.EFI; EDK2_CODE=/opt/homebrew/share/qemu/edk2-aarch64-code.fd; EDK2_VARS=/opt/homebrew/share/qemu/edk2-arm-vars.fd`
  - x86_64: `EFI_NAME=BOOTX64.EFI; EDK2_CODE=/opt/homebrew/share/qemu/edk2-x86_64-code.fd; EDK2_VARS=/opt/homebrew/share/qemu/edk2-i386-vars.fd`
- Split the QEMU array (drop `-initrd`; initramfs is in the kernel):
```bash
QEMU_BASE=("$QEMU_BIN" "${QEMU_MACHINE[@]}" -m 256 -smp 1
      -netdev user,id=n0 -device virtio-net-pci,netdev=n0 -device virtio-rng-pci)
QEMU_GPU=(-device virtio-gpu-pci,id=gpu0,xres=${GUI_RES%x*},yres=${GUI_RES#*x} -device virtio-keyboard-pci)
QEMU=("${QEMU_BASE[@]}" "${QEMU_GPU[@]}" -kernel $OUT/Image -append "console=$CONSOLE panic=1"
      -drive file=$DATA_IMG,format=raw,if=none,id=d0 -device virtio-blk-pci,drive=d0)
```
(Keep whatever phase 4 added to QEMU. run.sh, smoke.sh, gui.sh keep using `QEMU`.)

### `scripts/build-in-docker.sh` — order: cargo, list, kernel, image
```bash
SRC=/build/linux-$KERNEL_VERSION; OBJ=/build/kobj-$THIR_ARCH; O=/work/$OUT; LIST=/build/initramfs-$THIR_ARCH.list
KMAKE=(make -C "$SRC" O="$OBJ" ARCH="$KARCH" CROSS_COMPILE="$CROSS_COMPILE")
[ -d "$SRC" ] || curl -fL "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KERNEL_VERSION.tar.xz" | tar -xJ -C /build
mkdir -p "$OBJ" "$O"

cd /work && cargo build --release --target "$RUST_TARGET"
for b in <all current binaries>; do cp "$CARGO_TARGET_DIR/$RUST_TARGET/release/$b" "$O/"; done
cat > "$LIST" <<EOL
... (the existing initramfs list, unchanged) ...
EOL

"${KMAKE[@]}" tinyconfig
cat /work/kernel/common.config "/work/kernel/$THIR_ARCH.config" >> "$OBJ/.config"
"${KMAKE[@]}" olddefconfig
... (existing verification loop) ...
"${KMAKE[@]}" -j"$(nproc)" "$(basename "$KIMAGE")"      # initramfs built in (CONFIG_INITRAMFS_SOURCE=$LIST)
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
```
Delete the gen_init_cpio compile and the `initramfs.cpio.gz` output. Fallback if `mkfs.ext4 -E offset=` fails: make a separate 190M ext4 file and `dd ... seek=65 conv=notrunc,sparse`.

## 2. Kernel fragments

### `kernel/common.config` (append)
```
CONFIG_PARTITION_ADVANCED=y
CONFIG_EFI_PARTITION=y
CONFIG_INITRAMFS_COMPRESSION_GZIP=y
CONFIG_FB=y
CONFIG_FB_EFI=y
```

### `kernel/x86_64.config` (append)
```
CONFIG_EFI=y
CONFIG_EFI_STUB=y
# CONFIG_RANDOMIZE_BASE is not set
CONFIG_CMDLINE_BOOL=y
CONFIG_CMDLINE="console=ttyS0,115200 panic=1"
CONFIG_INITRAMFS_SOURCE="/build/initramfs-x86_64.list"
CONFIG_PCI_MSI=y
CONFIG_USB_SUPPORT=y
CONFIG_USB=y
CONFIG_USB_PCI=y
CONFIG_USB_XHCI_HCD=y
CONFIG_USB_EHCI_HCD=y
CONFIG_SCSI=y
CONFIG_BLK_DEV_SD=y
CONFIG_USB_STORAGE=y
CONFIG_HID=y
CONFIG_HID_GENERIC=y
CONFIG_USB_HID=y
CONFIG_ETHERNET=y
CONFIG_NET_VENDOR_INTEL=y
CONFIG_E1000E=y
CONFIG_IGC=y
CONFIG_NET_VENDOR_REALTEK=y
CONFIG_R8169=y
```
If a line is "not applied", grep its `depends on` in `$SRC` and add the dependency; never delete.

### `kernel/aarch64.config` (append)
```
CONFIG_EFI=y
CONFIG_EFI_STUB=y
CONFIG_CMDLINE="console=ttyAMA0 panic=1"
CONFIG_CMDLINE_FROM_BOOTLOADER=y
CONFIG_INITRAMFS_SOURCE="/build/initramfs-aarch64.list"
```
Expected sizes: x86_64 ≈ 4.3–4.5 MB (incl. initramfs); aarch64 ≈ 7.5 MB.

## 3. init: find the data partition

### `src/disk.rs` (new; `pub mod disk;`)
```rust
//! Finding the data filesystem without udev or blkid: the ext4 superblock label.
/// Label from the 1024-byte ext2/3/4 superblock (the bytes at offset 1024 of the device); None if the magic is missing.
pub fn ext_label(sb: &[u8]) -> Option<&str> {
    if sb.len() < 0x88 || sb[0x38..0x3a] != [0x53, 0xef] { return None; }
    let l = &sb[0x78..0x88];
    std::str::from_utf8(&l[..l.iter().position(|&b| b == 0).unwrap_or(16)]).ok()
}
```
Tests: magic + `thirdata\0` → Some("thirdata"); no magic → None; 16-byte label without NUL → full 16 chars; short buffer → None.

### `src/bin/init.rs`
Replace the hardcoded `/dev/vda` wait with:
```rust
/// The block device whose ext4 label is "thirdata": vda (raw data.img), vda2/sda2 (GPT image, USB stick). USB enumeration is slow, so retry for 15 s.
fn find_data() -> Option<String> {
    let t = Instant::now();
    loop {
        for e in fs::read_dir("/sys/class/block").into_iter().flatten().flatten() {
            let dev = format!("/dev/{}", e.file_name().to_string_lossy());
            let mut sb = [0u8; 1024];
            let ok = File::open(&dev).and_then(|mut f| { f.seek(SeekFrom::Start(1024))?; f.read_exact(&mut sb) }).is_ok();
            if ok && thir::disk::ext_label(&sb) == Some("thirdata") { return Some(dev); }
        }
        if t.elapsed() >= Duration::from_secs(15) { return None; }
        sleep(Duration::from_millis(200));
    }
}
```
`mount_data()`: no device → `thir: /data unavailable: no ext4 volume labelled thirdata`; else mount with the same flags and keep the exact line `thir: mounted /data`.

## 4. den
`src/gui/fb.rs`: keep the forced FBIOPUT; comment: "virtio-gpu's scanout stays off until userspace sets a mode; efifb has no check_var and returns 0 leaving the firmware mode; the result is ignored either way".

## 5. Tests

### `test/lib.sh` (new; helpers moved out of smoke.sh)
Temp dir, fifo, trap, `boot <logname> <qemu args...>` (adds `-no-reboot -display none -monitor none -serial stdio`), `wait_for`, `send`, `run`, `off`, `leak_check` (over `"${SECRETS[@]}"`).

### `test/smoke.sh`
Source lib.sh; same behaviour; PASS both arches.

### `test/uefi.sh` (new)
```bash
#!/usr/bin/env bash
# Boots out/<arch>/thir.img on edk2 firmware (no -kernel). Boot 1: first-boot setup, a file on /data. Boot 2: the file is still there.
set -euo pipefail
cd "$(dirname "$0")/.."; source ./env.sh; source test/lib.sh
[ -f "$EDK2_CODE" ] || { echo "FAIL: $EDK2_CODE missing (brew install qemu)"; exit 1; }
TIMEOUT=${UEFI_TIMEOUT:-300}
PW=uefi-pass-123; SECRETS=("$PW")
cp -c "$OUT/thir.img" "$d/thir.img"; cp "$EDK2_VARS" "$d/vars.fd"
case $THIR_ARCH in
  x86_64)  dev=sda2; disk=(-device qemu-xhci -device usb-storage,drive=d0 -device usb-kbd -device VGA) ;;
  aarch64) dev=vda2; disk=(-device virtio-blk-pci,drive=d0 "${QEMU_GPU[@]}") ;;
esac
uefi=("${QEMU_BASE[@]}" -drive if=pflash,format=raw,readonly=on,file="$EDK2_CODE" -drive if=pflash,format=raw,file="$d/vars.fd"
      -drive file="$d/thir.img",format=raw,if=none,id=d0 "${disk[@]}")
boot boot1 "${uefi[@]}"
wait_for '^thir: init ready$' "$TIMEOUT"; echo "boot-to-marker: ${SECONDS}s"
wait_for '^thir: mounted /data$' 20
wait_for 'den: [0-9]+x[0-9]+ 32bpp$' 30
first-boot setup (alice / $PW twice) → 'cat /proc/mounts' shows "^/dev/$dev /data ext4 " → 'echo persisted > note.txt'
off
boot boot2 "${uefi[@]}"; log in; 'cat note.txt' → '^persisted$'; off; leak_check; echo PASS
```
(If OVMF stalls at 256 MiB, use `-m 512` for uefi runs.) Expected: aarch64 < 2 min; x86_64 TCG < 10 min.

## 6. README
- Phase 5 line; `./test/uefi.sh`; `out/<arch>/thir.img` in Layout; remove `initramfs.cpio.gz` mentions.
- How boot works: initramfs inside the kernel; `/data` = the ext4 volume labelled `thirdata` (15 s retry).
- **Booting a real PC from a USB stick** (x86_64):
  1. `THIR_ARCH=x86_64 ./build.sh` → `out/x86_64/thir.img`.
  2. **Warning: `dd` overwrites the whole target disk without asking. A wrong disk number destroys your Mac's data or another drive. Check `diskutil list` twice: the stick is `(external, physical)` with the stick's size.**
     ```
     diskutil list
     diskutil unmountDisk /dev/diskN
     sudo dd if=out/x86_64/thir.img of=/dev/rdiskN bs=4m
     diskutil eject /dev/diskN
     ```
     macOS may say the disk is not readable (ext4): choose Ignore/Eject. Writing the image again erases the user's accounts and files on the stick.
  3. On the PC: **disable Secure Boot** → boot menu (F12/F8/Esc/Del) → `UEFI: <stick>`. Legacy/CSM boot does not work.
  4. Works: den on the firmware framebuffer, USB and laptop keyboards, e1000e/igc/r8169 wired NICs via dhcp. Not touched: internal disks. No Wi-Fi.
- QEMU/UTM note; measured sizes and UEFI boot times; limitations (unsigned; image regenerated by every build; three NIC families; no internal-disk access).

## 7. Checklist
1. Dockerfile; rebuild; confirm `sfdisk mkfs.fat mcopy mmd` exist.
2. disk.rs + tests, lib.rs, init.rs → tests green.
3. env.sh, build-in-docker.sh, kernel fragments, fb.rs comment.
4. `THIR_ARCH=x86_64 ./build.sh`: loop silent; `head -c 2 out/x86_64/Image` = `MZ`; thir.img is GPT.
5. `THIR_ARCH=x86_64 ./test/smoke.sh` PASS.
6. `THIR_ARCH=x86_64 ./test/uefi.sh` PASS; note time.
7. aarch64: build, smoke, gui, `MZ`, uefi (cap effort; document if not).
8. README; one commit. **Never run `dd` against a real device.**

## 8. Acceptance
- `./build.sh test` exit 0, ≥ previous count + 3.
- Both arch builds exit 0, loop silent; `Image` starts with `MZ`; `thir.img` 256 MiB apparent, < 20 MB real, GPT with ESP (`EFI/BOOT/BOOT{X64,AA64}.EFI`) and ext4 `thirdata`; no `initramfs.cpio.gz`.
- x86_64 bzImage ≤ 5 MB; aarch64 Image ≤ 8.5 MB.
- smoke.sh PASS both arches; gui.sh PASS aarch64.
- uefi.sh PASS x86_64 (USB stick on xHCI, `/dev/sda2 /data ext4`, `den: WxH 32bpp`, file persists) and aarch64 (`/dev/vda2`). No password in serial logs.
- Boot with no `thirdata` volume prints `thir: /data unavailable: ...` after ~15 s and still reaches `thir: init ready`.
- README has the USB section with the dd warning, the Secure Boot statement and the UEFI test command.
