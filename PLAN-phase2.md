# thir — Phase 2: the GUI ("den")

`den` is a tiling terminal GUI that draws straight to `/dev/fb0`. It does not use X11 or Wayland.
It shows the wallpaper and a top status bar, and runs `nsh` in each tiled pane.
Read README.md and the current sources first. Phase 1 conventions apply: small code, `libc` is the
only general dependency, comments only for the why, and unit tests for every non-trivial pure function.

## Hard constraints

- Do not break phase 1. The serial shell on the console stays. `./test/smoke.sh` must still PASS
  for both `THIR_ARCH=aarch64` and `THIR_ARCH=x86_64`.
- Crates: `libc`, plus **`miniz_oxide`** (pinned, `default-features = false`, with the features that
  inflate needs) for PNG inflate only. No other crates. Write the PNG decoder, PSF font parser, VT
  parser, layout and keymap yourself.
- Everything after the kernel is still our own Rust code. No busybox and no shell scripts in the image.
- Never print or log key input (a password could be typed in a pane later).

## Files (new unless marked)

```
assets/wallpaper.png          1920x1080 8-bit RGB (exists; may be replaced, keep the decoder generic)
assets/fonts/<terminus>.psf   Terminus PSF (bitmap). Add its OFL license file next to it
src/gui/mod.rs                pub mod list
src/gui/fb.rs                 open /dev/fb0, FBIOGET_VSCREENINFO/FSCREENINFO, mmap, present(back buffer)
src/gui/png.rs                PNG decode: IHDR, IDAT concat, inflate (miniz_oxide), all 5 filters,
                              color types 2 (RGB) and 6 (RGBA, alpha ignored), 8-bit only, else Err
src/gui/font.rs               PSF2 parser (+ PSF1 if trivial), glyph lookup by Unicode (use the unicode
                              table if present, else ASCII index)
src/gui/vt.rs                 terminal grid + escape parser (subset below)
src/gui/layout.rs             split tree, rects, focus by direction
src/gui/keymap.rs             evdev keycode + modifiers -> bytes for the pty, or a WM action
src/gui/theme.rs              ALL colors and sizes in one place (I will retune them for the final wallpaper)
src/bin/den.rs                event loop
src/bin/init.rs (modify)      also supervise den
kernel/common.config (modify) input + console changes
env.sh, run.sh (modify)       keyboard device, GUI run mode
test/gui.sh                   headless GUI test with screendump
```

## Behaviour

**Startup.** `init` starts `/bin/den` when `/dev/fb0` exists, beside the serial shell (do not replace
the serial shell). If den exits, init restarts it after 1 s, using the same respawn guard as the shell.
If den exits with code 120 or 121 (`EXIT_POWEROFF` or `EXIT_REBOOT`), init powers off or reboots,
using the same `decide()` as for the shell. Restructure init's wait loop so that it handles both pids.

**Screen.** Read the fb geometry. Support 32 bpp XRGB8888 (use the red/green/blue offsets from
vscreeninfo); fail with a clear message for other formats. Scale the wallpaper to "cover" the screen
once at startup (bilinear or box filter is enough). Draw into a back buffer, then copy only the dirty
rows to the mmap.

**Status bar.** A thin bar at the top, about 28 px: `thir` on the left, the focused pane number in
the middle, and on the right the eth0 IPv4 address (from `net::interfaces()`, or `offline`) and
`HH:MM` UTC. Update it every second.

**Panes.** Each pane has a pty with `nsh` running in it. Set up the pty with
posix_openpt/grantpt/unlockpt/ptsname. The child runs setsid, TIOCSCTTY, dup2 to 0/1/2 and execs
`/bin/nsh`. Set TIOCSWINSZ on every resize.
Pane look: the wallpaper alpha-blended with the theme panel color (precompute this backdrop when the
layout changes), 12 px gaps, 1 px border (accent color if focused, dim otherwise), and 8 px inner padding.
When a pane's shell exits, close the pane. When the last pane closes, open a new one.
If a pane's shell exits with 120 or 121, den exits with that code (this is how `poweroff` works from the GUI).

**Keys** (Alt is the modifier, because the macOS Cmd key does not reach QEMU):
- `Alt+Enter`: split the focused pane along its longer side, and focus the new pane
- `Alt+Arrow` or `Alt+h/j/k/l`: move the focus in that direction (use the rect geometry)
- `Alt+q`: close the focused pane (send SIGHUP to its shell)
- `Alt+f`: show the focused pane full screen, or go back to the tiled layout
- Other keys go to the focused pty: printable characters with Shift and Caps Lock, Enter=`\r`,
  Backspace=`0x7f`, Tab, Esc, arrows as `ESC[A..D`, and Ctrl+letter as a control byte (Ctrl-C=0x03).
  Keyboard layout: US.

**VT subset** (enough for nsh and simple tools): UTF-8 printable characters, CR, LF, BS, TAB, BEL (ignore),
line wrap and scrolling, and these CSI sequences: `A B C D H f J K m`, plus `?25h/l` (show or hide the
cursor). SGR: 0, 1, 7, 22, 27, 30–37, 39, 40–47, 49, 90–97, 100–107. Ignore unknown sequences
safely. Never panic on any byte sequence, and add a test that sends random bytes. Show the cursor as a block
in the accent color in the focused pane only.

**Event loop.** Use libc `poll()` on all `/dev/input/event*` fds (open them all, and keep those that
report EV_KEY) and on all pty master fds, with a 1000 ms timeout for the clock. Redraw only the dirty
panes and the bar.

## Kernel (`kernel/common.config` and arch files)

Add `CONFIG_INPUT=y`, `CONFIG_INPUT_EVDEV=y`, `CONFIG_VIRTIO_INPUT=y` and
`# CONFIG_FRAMEBUFFER_CONSOLE is not set` (otherwise fbcon draws kernel text over den).
x86_64 also needs `CONFIG_SERIO=y`, `CONFIG_SERIO_I8042=y`, `CONFIG_INPUT_KEYBOARD=y` and `CONFIG_KEYBOARD_ATKBD=y`.
The build's config check must pass. If a symbol needs a dependency, add the dependency; never
delete a requested line.

## QEMU

- `env.sh`: add `-device virtio-keyboard-pci` to QEMU.
- `run.sh gui`: use a window (`-display cocoa`) with the serial console still on the terminal
  (`-serial mon:stdio`). Plain `./run.sh` stays as it is now (`-nographic`).
- `virtio-gpu-pci` default mode is 1280x800. den must work at any size (also test `-device virtio-gpu-pci,xres=1920,yres=1080`).

## Tests and acceptance

1. `./build.sh test`: all old tests and the new ones pass. New tests cover the PNG filters (a tiny
   hand-made PNG), the PSF header, the VT parser (cursor moves, SGR, wrap/scroll, erase, random bytes
   never panic), layout (split, close, focus by direction, fullscreen) and the keymap.
2. `./build.sh` passes for both arches, and the config check reports nothing.
3. `./test/smoke.sh` PASS for both arches (the serial path is unchanged).
4. `test/gui.sh` (new, aarch64): boot headless with a QEMU monitor socket (`-monitor unix:...`).
   Wait for den's startup line on serial (`den: 1280x800 32bpp`). Then use the monitor's `sendkey` to type
   `ip`, Enter, Alt+Enter (split), and `cat /etc/resolv.conf`, Enter. Then `screendump` to a PPM and convert it to PNG with
   `magick`. The script checks that the file exists and is not all one color. Look at the PNG
   yourself with the Read tool: the wallpaper, the bar, two panes and the command output must be visible and readable.
   Save the screenshot as `docs/den.png` and commit it.
5. Image size: report `out/aarch64/initramfs.cpio.gz`. The total must stay < 30 MB.
6. README: add a GUI section (keys, `./run.sh gui`, `test/gui.sh`) and update the layout and limits.
7. One local commit at the end, ending with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Do not push.

## Report back

A PASS/FAIL list with evidence for each acceptance item, sizes, every deviation from this plan with
its reason, and anything not done. Do not report done while an acceptance item fails without saying so.
