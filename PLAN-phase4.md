# thir — Phase 4 plan: HiDPI den, Mac modifier, visible brand, nsh line editor (made by the Fable planner)

Builds on the phase-3 end state. Conventions unchanged: libc-only (+ argon2/subtle), comments only for the why, unit tests for pure logic, no shell scripts in the image.

## 0. Decisions

| # | Decision | Choice | Why |
|---|---|---|---|
| 1 | Guest resolution | `./run.sh gui` asks macOS for the exact pixel area QEMU will use and sets `GUI_RES` to it (this Mac: **2940x1848** full screen, 2940x1790 windowed). Override with `GUI_RES=WxH`. Headless `test/gui.sh` uses a fixed `2560x1664`. Smoke/serial runs keep 1280x800. | QEMU 11 `ui/cocoa.m`: a non-resizable window is `guest / backingScaleFactor` points, so 1 guest px = 1 device px. Full screen letterboxes to the screen safe area (notch), so the guest must equal that area in px to stay 1:1. This Mac runs "looks like 1470x956" (backing 2, notch 32 pt). |
| 2 | Cocoa options | `-display cocoa,full-screen=on` by default (`GUI_FULL=0` = window). No `zoom-to-fit`, `zoom-interpolation`, `full-grab`, `swap-opt-cmd`. | zoom-to-fit resamples; full-grab would eat Cmd+Tab (user gets trapped). |
| 3 | UI scale | `s = 2 if fb.w >= 2000 else 1`. Font 16x32 Terminus at s=2, 8x16 at s=1. bar 28·s, gap 12·s, border 1·s, pad 8·s, footer 28·s. | Integer scale keeps bitmap fonts crisp. |
| 4 | Font | Commit `assets/fonts/Uni3-Terminus32x16.psf` (Debian `console-setup-linux_1.221_all.deb`, one-off download in the build container). Ship both PSFs in `/share`. | Real 32x16 glyphs beat pixel-doubled 8x16. Same family/license. |
| 5 | Wallpaper | **Procedural in Rust** (`src/gui/wall.rs`): variant-D radial gradient with ordered dither + anti-aliased bear head. Delete `assets/wallpaper.png`, `src/gui/png.rs`, the `miniz_oxide` crate and `cover()`. Keep `assets/wallpaper.html` as the design source. | The design is one gradient + 8 circles + 4 letters. PNG = 2–4.5 MB plus ~45 MB decode peak; dither never bands; den places the mark itself, so no crop at any aspect ratio. |
| 6 | Brand placement | A **footer strip** (28·s px) under the panes, never covered: bear head + "thir" centred, drawn once into the wallpaper buffer. Bar left shows the **help line**. Panel alpha unchanged (217). | Mark always visible; text stays on an opaque panel. |
| 7 | WM modifier | **Option (Alt) is the documented default; Cmd (Meta) is accepted too.** evdev 56/100 (Alt) and 125/126 (Meta) are the same WM key. | Option always reaches the guest. Cmd reaches it only while the keyboard is grabbed (full screen or after a click). Release = Ctrl+Option+G. |
| 8 | Shell | `src/line.rs`: pure editor (bytes→keys, key→new state + output bytes); nsh enters raw termios only while reading a line; `login` resets the tty to cooked at start. History in `~/.nsh_history` 0600. | Raw mode scoped to the read: children, `write`, `passwd` always see a cooked tty. |
| 9 | Guest RAM | Keep `-m 256`; verify MemAvailable. | ≈145 MB estimated use. Bump to 512 for gui runs only if MemAvailable < 64 MB. |

## 1. Host side

### `env.sh`
After `DATA_SIZE`: `GUI_RES=${GUI_RES:-1280x800}`; gpu device becomes
`-device virtio-gpu-pci,id=gpu0,xres=${GUI_RES%x*},yres=${GUI_RES#*x}`.

### `run.sh` (whole file)
```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if [ "${1:-}" = gui ]; then
  export GUI_FULL=${GUI_FULL:-1}
  # 1 guest px = 1 Retina px: QEMU sizes its window to guest/backingScale points, and full screen fits the safe area (notch)
  GUI_RES=${GUI_RES:-$(osascript -l JavaScript -e 'ObjC.import("AppKit"); const s=$.NSScreen.mainScreen, b=s.backingScaleFactor, f=s.frame, i=s.safeAreaInsets, v=s.visibleFrame, full=$.getenv("GUI_FULL")!="0"; const w=full?f.size.width-i.left-i.right:v.size.width, h=full?f.size.height-i.top-i.bottom:v.size.height-28; Math.round(w*b)+"x"+Math.round(h*b)' 2>/dev/null || echo 2560x1600)}
  export GUI_RES
fi
source ./env.sh
[ -f "$DATA_IMG" ] || ./build.sh data
if [ "${1:-}" = gui ]; then
  shift; full=; [ "$GUI_FULL" = 1 ] && full=,full-screen=on
  echo "thir: den at $GUI_RES. Option(Alt)+Enter splits (Cmd works while grabbed; Ctrl+Option+G releases). Ctrl-A X here quits" >&2
  exec "${QEMU[@]}" -display "cocoa$full" -serial mon:stdio "$@"
fi
echo "thir: Ctrl-A X quits QEMU" >&2
exec "${QEMU[@]}" -nographic "$@"
```

### Font fetch (one-off; commit the result)
```bash
docker run --rm --platform linux/arm64 -v "$PWD":/work thir-build bash -c \
 'cd /tmp && curl -fLO http://ftp.debian.org/debian/pool/main/c/console-setup/console-setup-linux_1.221_all.deb \
  && dpkg-deb --fsys-tarfile console-setup-linux_1.221_all.deb | tar -xO ./usr/share/consolefonts/Uni3-Terminus32x16.psf.gz | gunzip > /work/assets/fonts/Uni3-Terminus32x16.psf'
```
(If 1.221 is gone from the pool, use the current console-setup-linux version.)

### `scripts/build-in-docker.sh`
initramfs: remove `/share/wallpaper.png`; fonts become
```
file /share/font16.psf /work/assets/fonts/Uni3-Terminus16.psf 0644 0 0
file /share/font32.psf /work/assets/fonts/Uni3-Terminus32x16.psf 0644 0 0
```

### `Cargo.toml`
Remove `miniz_oxide`; refresh `Cargo.lock` with `./build.sh test`.

### Delete
`assets/wallpaper.png`, `src/gui/png.rs` (`pub mod png;` → `pub mod wall;`).

## 2. GUI

### `src/gui/theme.rs`
```rust
pub struct Metrics { pub bar: i32, pub gap: i32, pub border: i32, pub pad: i32, pub foot: i32 }
impl Metrics { pub const fn at(s: i32) -> Metrics { Metrics { bar: 28 * s, gap: 12 * s, border: s, pad: 8 * s, foot: 28 * s } } }
pub const MARK: u32 = 0x9c8c86;   // rgba(236,214,205,0.62) over the wallpaper, as in wallpaper.html variant D
pub const WALL: [(f32, u32); 3] = [(0.0, 0x1a1518), (0.5, 0x110e10), (1.0, 0x09080a)];   // radial stops, centre bottom-middle
pub const HELP: &str = "alt|cmd + enter split \u{b7} arrows focus \u{b7} q close \u{b7} f full";
```
Keep PANEL, PANEL_ALPHA, BAR, BAR_ALPHA, FG, DIM, ACCENT, BORDER_DIM, PALETTE. Delete BAR_H/GAP/BORDER/PAD.

### `src/gui/wall.rs` (new, ~60 lines)
```rust
//! The wallpaper, drawn in code: assets/wallpaper.html variant D (warm charcoal radial gradient, small bear head).
/// Radial gradient from (w/2, h), radius 0.7*hypot(w,h), piecewise-linear `stops`, 4x4 Bayer dither (±0.5 level) so 8-bit never bands.
pub fn gradient(w: usize, h: usize, stops: &[(f32, u32)]) -> Vec<u32>
/// Bear head from wallpaper.html `bearHead`: ears (cx±0.7r, cy−0.72r, 0.36r) fill+stroke, head (cx,cy,r) fill+stroke,
/// muzzle ellipse outline (cx, cy+0.34r, 0.42r×0.3r), nose filled ellipse (cx, cy+0.24r, 0.13r×0.09r) in `stroke`,
/// eyes filled circles (cx±0.36r, cy−0.12r, 0.06r) in `stroke`. Anti-aliased with signed-distance coverage:
/// fill cov = clamp(0.5 − d), stroke cov = clamp(0.5 − (|d| − lw/2)); ellipse d ≈ (hypot(dx/rx, dy/ry) − 1)·min(rx, ry).
/// Only touches pixels inside cx±1.5r, cy±1.5r.
pub fn bear(px: &mut [u32], w: usize, cx: f32, cy: f32, r: f32, stroke: u32, fill: u32, lw: f32)
pub fn blend(fg: u32, bg: u32, a: u32) -> u32   // moved from den
```
Tests: `gradient(64, 64, &WALL)`: (32,63) within ±1/channel of 0x1a1518; (0,0) darker in every channel than (32,63); no pixel brighter than 0x1b1619. `bear` on a flat black 64x64: centre == fill, ≥ 50 pixels changed, pixels outside the 1.5r box unchanged.

### `src/gui/keymap.rs`
- `Keys { shift, ctrl, wm, caps }`; `56 | 100 | 125 | 126 => { self.wm = down; return None }`.
- `fixed`: add `102 => b"\x1b[H", 107 => b"\x1b[F"` (Home/End).
- Tests: `meta_acts_like_alt` (125 down → 28 gives Split; 125 up → 28 gives `\r`); Home/End bytes.

### `src/gui/font.rs`
Test: also parse `assets/fonts/Uni3-Terminus32x16.psf`, assert `(16, 32)`.

### `src/bin/den.rs`
- Remove `cover`, `blend`, the png import. Add `m: Metrics, s: i32`; replace every `BAR_H/GAP/BORDER/PAD` with `self.m.*`.
- `main`: `s = if fb.w >= 2000 { 2 } else { 1 }`; font = `/share/font32.psf` if s == 2 else `/share/font16.psf`. `wall = wall::gradient(w, h, &WALL)`; footer mark: `fy = h - m.foot; r = 9.0*s; cx = w/2 − 2.5*font.w; wall::bear(&mut wall, w, cx, fy + m.foot/2 + s, r, MARK, PANEL, 1.6*s)`; then "thir" with `glyph()` at `x = cx + r + 1.5*font.w`, advancing `font.w + font.w/2`, `y = fy + (m.foot − font.h)/2`, colour MARK. `area = Rect { x: m.gap, y: m.bar + m.gap, w: w − 2*m.gap, h: h − m.bar − m.foot − 2*m.gap }`.
- `login_rect`: centre between `m.bar` and `h − m.foot`.
- `draw_bar`: left shows HELP in DIM instead of "thir".
- Footer is baked into `wall`; panes never cover it.

## 3. Shell

### `src/line.rs` (new, pure)
```rust
#[derive(Debug, PartialEq, Clone)] pub enum Key { Char(char), Enter, Tab, Backspace, Delete, Left, Right, Home, End, Up, Down, Ctrl(u8) }
#[derive(Debug, PartialEq)] pub enum Act { None, Submit(String), Eof }
pub struct Editor { buf: Vec<char>, cur: usize, hist: Vec<String>, hpos: usize, stash: Vec<char>, esc: Vec<u8>, utf8: Vec<u8> }
impl Editor {
    pub fn new(hist: Vec<String>) -> Editor
    pub fn decode(&mut self, bytes: &[u8]) -> Vec<Key>          // escapes and UTF-8 may span reads
    pub fn key(&mut self, k: Key, prompt: &str) -> (Act, Vec<u8>)
    pub fn redraw(&self, prompt: &str) -> Vec<u8>              // "\r" + prompt + buf + ESC[K, then ESC[nD if cursor not at end
    pub fn word(&self) -> (bool, String)                      // (is_first_word, word ending at cursor)
    pub fn complete(&mut self, cands: &[String], prompt: &str) -> Vec<u8>
        // 1 → replace + " " (no space after '/'); >1 with longer common prefix → replace; else "\r\n" + cands joined by 2 spaces + "\r\n" + redraw; 0 → no bytes
    pub fn push_history(&mut self, line: &str) -> bool        // non-empty, differs from last; resets position
    pub fn history(&self) -> &[String]
}
pub fn common_prefix(words: &[String]) -> String
```
Decode: `\r`/`\n` Enter; `\t` Tab; 0x7f/0x08 Backspace; 0x01..=0x1a `Ctrl(b+0x60)`; ESC sequences: `[A` Up `[B` Down `[C` Right `[D` Left; `[H` `[1~` `[7~` `OH` Home; `[F` `[4~` `[8~` `OF` End; `[3~` Delete; others dropped; lone ESC + byte → byte handled normally; UTF-8 accumulates until valid (invalid dropped). Other control bytes ignored.

| Key | Effect |
|---|---|
| Char | insert at cursor |
| Left/Right, Home/Ctrl-A, End/Ctrl-E | move |
| Backspace, Delete | delete before/under cursor |
| Ctrl-U / Ctrl-K | delete to start / to end |
| Ctrl-W | delete the word before the cursor |
| Ctrl-L | `ESC[H ESC[2J` + redraw |
| Ctrl-C | clear line, `^C\r\n` + redraw |
| Ctrl-D | `Eof` if line empty (bytes `\r\n`), else nothing |
| Up/Down | history; leaving the bottom stashes the line; Down past the end restores it |
| Enter | `(Submit(line), b"\r\n")`, buffer cleared |
| Tab | caller: `word()` → candidates → `complete()` |

Unit tests (≥ 10, assert buffer, cursor and exact bytes): mid-line insert gives `ESC[2D`; backspace/delete/home/end; ctrl-u/k/w; up/down with stash; enter; ctrl-d empty/non-empty; decode `\x1b[` + `3~` across chunks → Delete, split `é` → Char('é'), `\x1b[A\x1b[D` → [Up, Left], `\x1bx` → [Char('x')]; complete: `cat` → `cat `, `docs/` no space, `["echo","exit"]` on `e` lists `echo  exit`, `["abc1","abc2"]` on `a` → `abc`; `common_prefix` of empty → "".

### `src/bin/nsh.rs`
- `const BUILTINS: &[&str] = &["cd","pwd","ls","cat","echo","whoami","mkdir","rmdir","touch","rm","mv","cp","write","clear","history","exit","help","ip","poweroff","reboot"]` (help text derived from it).
- `raw_on() -> Option<termios>`: tcgetattr(0) (None if not a tty); clear `ICANON|ECHO|ISIG|IEXTEN`, `ICRNL|IXON`; VMIN=1 VTIME=0; keep OPOST; TCSANOW. `raw_off(&t)`: TCSADRAIN.
- `prompt()`: `"\x1b[1;32m{user}@{host}\x1b[0m:\x1b[1;34m{cwd}\x1b[0m$ "`, `$HOME` → `~`.
- `read_cmd(ed, prompt) -> Option<String>`: not a tty → plain prompt + read_line. Else redraw, loop `read(0)`: `n <= 0` → None; Tab → `candidates(ed.word())` → `complete`; else `key` → Submit/Eof. **Always raw_off before returning.** On Submit, if `push_history` → append to `~/.nsh_history` (append, create, mode 0600).
- `candidates(first, word)`: first word without '/' → BUILTINS + executables in `$PATH` starting with word; else dir part (or ".") entries with the prefix (hidden only if prefix starts with '.'), dirs get '/'; sorted, dedup.
- `main`: load the last 1000 history lines; `None` → exit(0).
- Builtins: `clear` → `ESC[H ESC[2J`; `history` → numbered lines. `ls` colours dirs `\x1b[1;34m{n}/\x1b[0m` only when not redirected and stdout is a tty.
- `help`: builtins; redirect with > and >>; commands: passwd dhcp; tab completes, up/down history, ctrl-a/e/u/k/w/l.

### `src/bin/login.rs`
After the uid check, reset the tty to cooked in case a shell died raw: `tcgetattr(0)` ok → `c_lflag |= ICANON|ECHO|ECHOE|ISIG|IEXTEN; c_iflag |= ICRNL; c_oflag |= OPOST|ONLCR; tcsetattr(0, TCSANOW)`.

## 4. Tests

### `test/smoke.sh`
- `wait_for` strips CSI: `... | tr -d '\r' | sed -E $'s/\e\\[[0-9;?]*[A-Za-z~]//g' | grep -qE "$1"`.
- Replace the `'nsh> '` prompt pattern with `'\$ $'`.
- Add a Tab completion check: `send $'ca\t/etc/res\t'; wait_for 'cat /etc/resolv.conf'; send ''; wait_for '^nameserver 10\.0\.2\.3$'`. Add `history` check `'^ *[0-9]+ +whoami$'`.
- Both arches PASS.

### `test/gui.sh` (aarch64, fixed 2560x1664, fresh disk)
```
export GUI_RES=2560x1664 DATA_IMG=out/aarch64/gui-data.img; source ./env.sh; ./build.sh data >/dev/null
wait_for 'den: 2560x1664 32bpp$' 30; wait_for '^thir: init ready$' 5
first-boot setup in the den pane (alice / password twice); ip; alt-ret; "ca" tab "/etc/res" tab ret
screendump → magick → $PNG
checks: size 2560x1664; colors > 100; footer crop around the bear mark has > 20 colours; serial log has no password
```
Save as `docs/den.png` (report size).

### Unit
Existing (minus png tests) + line.rs ≥ 10 + wall.rs 2 + keymap +1 + font +1.

## 5. Keys (README + bar help line)

| Keys | Action |
|---|---|
| Option(Alt)+Enter, Cmd+Enter | split focused pane |
| Option/Cmd + Arrows or h/j/k/l | move focus |
| Option/Cmd + q | close pane |
| Option/Cmd + f | fullscreen pane |
| Ctrl+Option+G | release QEMU's keyboard grab |
| Ctrl-A X (terminal) or `poweroff` | quit |

## 6. HiDPI numbers (full screen 2940x1848, s=2)
font 16x32; bar 56; gap 24; border 2; pad 16; footer 56; one full-width pane ≈ 177x51 cells.

## 7. README
`GUI_RES`, `GUI_FULL`; layout changes; HiDPI paragraph (for pixel-perfect output pick the "1280x832" display mode in macOS); key table; shell section (prompt, editing keys, Tab, `~/.nsh_history` 0600); limitations (lines wider than the terminal redraw wrongly; Cmd reliable only while grabbed); drop miniz_oxide from pinned crates.

## 8. Checklist
1. Font fetch; Cargo.toml; delete png.rs + wallpaper.png; gui/mod.rs.
2. theme, wall + tests, keymap + tests, font test → test green.
3. line.rs + tests → green.
4. den.rs, env.sh, run.sh, build-in-docker.sh.
5. nsh.rs, login.rs.
6. Both arch builds; report initramfs size.
7. smoke.sh → PASS both arches.
8. gui.sh → PASS; look at docs/den.png.
9. Real-window check is done by the coordinator/user (the builder cannot see a window): leave `./run.sh gui` ready.
10. README; one commit.

## 9. Acceptance
- `./build.sh test` exit 0; no miniz_oxide in Cargo.lock.
- Smoke PASS both arches incl. Tab completion and history; no password in serial log.
- `./test/gui.sh docs/den.png` PASS at 2560x1664, footer mark visible, completion visible.
- Home/End, Up, Tab (single and double) work; `~/.nsh_history` 0600; `passwd` and `write` still work.
- MemAvailable ≥ 64 MB after login at 2560x1664 (check via `cat /proc/meminfo` in gui.sh or a serial run with GUI_RES set); else `-m 512` for gui runs only.
- initramfs < 2 MB; `docs/den.png` committed.
