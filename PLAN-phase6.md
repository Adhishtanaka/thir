# thir — Phase 6 plan: local time, help, footer, `edit`, `find`, `top` (made by the Fable planner)

Conventions unchanged: libc + argon2 + subtle only, comments only for the why, pure logic in `src/*.rs` with unit tests, binaries thin. No package manager, no network monitor (user decision).
**Built AFTER phase 5 (UEFI).** Phase 5 reorders `scripts/build-in-docker.sh` (initramfs list now built into the kernel) and moves smoke-test helpers into `test/lib.sh`: apply this plan's build/test changes to that new layout. Do not change `src/bin/init.rs`, `kernel/*.config`, `env.sh`, `run.sh`, `build.sh`.

## 0. Decisions

| # | Decision | Choice | Why |
|---|---|---|---|
| 1 | Time source | QEMU default `-rtc base=utc` (already default); `RTC_HCTOSYS` sets the clock at boot. | Guest = host UTC. |
| 2 | Time zone storage | **Per user, `~/.tz`**, one line `+05:30` (strict `[+-]HH:MM`, ±14:00). Written by the user (`tz` builtin, 0600) and at first-boot setup. den (root) reads `/data/home/<name>/.tz` of the logged-in uid. | No setuid helper; `/data/etc` stays 0700 root. |
| 3 | Setup | `login` setup asks `utc offset (e.g. +05:30, Enter for UTC): ` after the password; empty = UTC (no file). | Correct time from the start. |
| 4 | `date`, `tz` | nsh builtins: `date` → `Tue 17 Nov 2026 14:32:05 +05:30`; `tz` shows, `tz +05:30` sets. Date math in `src/clock.rs`. | Shared with den and top. |
| 5 | help | One table `HELP` in nsh (sections × (usage, description)); builtins derived from it; `help <cmd>`; `help_text()` pure, tested ≤ 80 columns. | Never drifts from completion. |
| 6 | Footer | `foot = 44·s` (was 28·s); mark centre `cy = fy + 21·s`; text `y = cy − font.h/2`. | Logo lifted with breathing room. |
| 7 | Editor | `/bin/edit` + `slink /bin/nano → /bin/edit`. Buffer logic `src/edit.rs`; full-frame redraw with `ESC[r;1H … ESC[K` per row. den supports CSI A/B/C/D/H/f/J/K/m incl. 7/27 reverse and `?25h/l`: **no vt.rs change needed**. | Simple and works on serial and den. |
| 8 | `find` | External `/bin/find` (glob in `src/glob.rs`). | Ctrl-C works for external programs. |
| 9 | Monitor | `/bin/top`, parsers in `src/procinfo.rs`. User column: `root` for uid 0, `$USER` for your uid, number otherwise. | No new files, no security change. Kernel already has what's needed. |
| 10 | Shared terminal code | `src/term.rs`: raw-mode guard restored on drop, panic and SIGTERM/SIGHUP; `size()` via TIOCGWINSZ (0 → 80x24); `read_keys(timeout)`; `prompt()`. nsh's `raw_on/raw_off/put` move here. Extract `line::Decoder` from `Editor`; add PgUp/PgDn. | One copy of each. |
| 11 | Resize | No SIGWINCH: poll stdin with a timeout (edit 500 ms, top 1 s), re-read `size()` each frame. | No signal code. |
| 12 | Tabs/width | Tab → spaces to next multiple of 4; every char 1 column; control chars `?`; CRLF loads as LF, saves LF. | Simplest correct. |

## 1. `src/clock.rs` (new, pure)
```rust
pub struct Local { pub year: i64, pub month: u32, pub day: u32, pub wday: u32 /*0=Sun*/, pub hour: u32, pub min: u32, pub sec: u32 }
pub const WDAY: [&str; 7]; pub const MONTH: [&str; 12];
pub fn civil_from_days(z: i64) -> (i64, u32, u32)        // Howard Hinnant's algorithm; weekday = (days + 4).rem_euclid(7)
pub fn local(utc: i64, off_min: i32) -> Local
pub fn parse_offset(s: &str) -> Option<i32>              // exactly "[+-]HH:MM", MM < 60, |v| <= 14*60
pub fn format_offset(m: i32) -> String                   // "+05:30"
pub fn date_line(utc: i64, off: i32) -> String           // "Tue 17 Nov 2026 14:32:05 +05:30"
pub fn bar_line(utc: i64, off: Option<i32>) -> String    // "Tue 17 Nov  14:32"; None → "... UTC"
pub fn read_tz(path: &str) -> Option<i32>                // SEE SECURITY NOTE BELOW
pub fn home_tz() -> Option<i32>                          // read_tz("$HOME/.tz")
pub fn now() -> i64
```
**Security (den runs as root and reads a user-owned path every second):** `read_tz` must open with `O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, then `fstat` and return None unless it is a **regular file** (a FIFO would otherwise block den forever; a device or socket is refused too), read at most 64 bytes, trim, `parse_offset` strictly, and only the parsed number is ever displayed. Add a unit test that `read_tz` on a FIFO (mkfifo in a temp dir) returns None without blocking, and on a symlink returns None.
Tests: `0` → `Thu 01 Jan 1970 00:00:00 +00:00`; `951782400` → `Tue 29 Feb 2000`; `1700000000` → `Tue 14 Nov 2023 22:13:20`, with `+05:30` → `Wed 15 Nov 2023 03:43:20 +05:30`; `1709164800` → `Thu 29 Feb 2024`; `100` with `-01:00` → `Wed 31 Dec 1969 23:01:40 -01:00`; `1794906125` → `Tue 17 Nov 2026 09:02:05`; `bar_line(1794906125, Some(330))` == `"Tue 17 Nov  14:32"`, None ends with `" UTC"`; `parse_offset`: `+05:30`→330, `-03:00`→−180, `+00:00`→0; rejects `5:30`, `+5:30`, `+05:60`, `+15:00`, ``, `+05:30 IST`.

## 2. `src/line.rs`
- `pub struct Decoder { esc, utf8 }` with `new()` and `decode()` (moved verbatim); `Editor` holds a `Decoder` and delegates (existing tests unchanged).
- `Key::PgUp` (`[5~`), `Key::PgDn` (`[6~`); `Editor::key` ignores them like Tab/Ctrl.
- Test: `decode(b"\x1b[5~\x1b[6~")` → `[PgUp, PgDn]`.
- `src/gui/keymap.rs` `fixed`: `104 => b"\x1b[5~", 109 => b"\x1b[6~"`; extend the specials test.

## 3. `src/term.rs` (new)
```rust
//! Shared full-screen terminal bits for edit and top. Raw mode is restored on drop, on panic and on SIGTERM/SIGHUP.
static SAVED: OnceLock<libc::termios>;
pub fn raw_on() -> Option<libc::termios>; pub fn raw_off(t: &libc::termios); pub fn put(b: &[u8])   // moved from nsh
pub struct Raw(libc::termios);
impl Raw { pub fn enter() -> Option<Raw> }  // raw_on; SAVED.set; sigaction(SIGTERM|SIGHUP, restore_and_raise, sa_flags 0); panic hook restores
impl Drop for Raw { raw_off }
extern "C" fn restore_and_raise(sig) { tcsetattr(0, TCSANOW, SAVED); signal(sig, SIG_DFL); raise(sig) }
pub fn size() -> (usize, usize)            // TIOCGWINSZ on fd 1; 0 or error → (80, 24)
pub fn read_keys(dec: &mut Decoder, timeout_ms: i32) -> Option<Vec<Key>>   // poll fd 0; None = EOF; Some([]) = timeout; EINTR retry
pub fn prompt(dec: &mut Decoder, row: usize, label: &str) -> Option<String>   // one-line input on `row`; Ctrl-C / Ctrl-D-on-empty cancel
pub const REV: &str = "\x1b[7m"; pub const RST: &str = "\x1b[0m";
```

## 4. den footer + bar
- `theme.rs`: `foot: 44 * s`.
- `den.rs` main: `let (fy, r) = (h as i32 - m.foot, 9.0 * sc as f32); let cy = fy + 21 * sc;` bear at `cy`; text at `x = cx + r + 1.5*font.w`, `y = cy − font.h/2`.
  s=2 at 2560x1664: footer top 1576, head 1600..1636, 28 px below the head. s=1 at 1280x800: footer top 756, 14 px below.
- Bar time: `tz_path: Option<String>` in `Den`, set on login from the user's home (`{home}/.tz`), cleared on logout. `draw_bar`: `clock::bar_line(clock::now(), tz_path.and_then(read_tz))`.

## 5. nsh: help table, `date`, `tz`
```rust
const HELP: &[(&str, &[(&str, &str)])] = &[
  ("files",   &[("ls [dir]", "list a directory"), ("cat file...", "print files"), ("cd [dir]", "change directory (default: home)"), ("pwd", "print the working directory"),
               ("mkdir [-p] dir...", "make directories"), ("rmdir dir...", "remove empty directories"), ("touch file...", "create files"), ("rm [-r] path...", "remove files (-r: directories too)"),
               ("mv src dest", "move or rename"), ("cp src dest", "copy a file")]),
  ("text",    &[("echo text...", "print text"), ("write file", "store lines until a line with only ."), ("clear", "clear the screen")]),
  ("system",  &[("date", "local date and time"), ("tz [+HH:MM]", "show or set the UTC offset (~/.tz)"), ("ip", "interfaces and addresses"), ("poweroff", "shut down"), ("reboot", "reboot")]),
  ("session", &[("whoami", "your user name"), ("history", "command history"), ("help [cmd]", "this text, or one command"), ("exit", "log out")]),
  ("programs",&[("edit [file]", "full-screen editor (also: nano)"), ("find [dir] [-name P]", "find files; P uses * and ?; -type f|d"), ("top", "CPU, memory and processes"),
               ("passwd", "change your password"), ("dhcp [iface]", "request an address (root only)")]),
];
fn builtins() -> impl Iterator<Item = &'static str>   // all sections except "programs", first word of usage
fn help_text() -> String; fn help_cmd(name) -> Option<String>
```
Delete `const BUILTINS`; completion uses `builtins()`. Format `"  {:<8} {:<20} {}"`.
`help` output:
```
nsh, the thir shell

  files    ls [dir]             list a directory
           cat file...          print files
           ...
  programs edit [file]          full-screen editor (also: nano)
           find [dir] [-name P] find files; P uses * and ?; -type f|d
           top                  CPU, memory and processes
           passwd               change your password
           dhcp [iface]         request an address (root only)

Redirect output with > file or >> file.
Keys: Tab completes, Up/Down history, Ctrl-A/E start/end, Ctrl-U/K cut to
start/end, Ctrl-W cut word, Ctrl-L clear screen, Ctrl-C cancel, Ctrl-D exit.
```
Builtins: `date` (uses `home_tz()`, default 0); `tz` (show / set with `parse_offset`, write `$HOME/.tz` mode 0600, error `nsh: tz: use +HH:MM or -HH:MM`).
Move `unsafe_char`/`printable` to `src/files.rs` (pub). Tests: `help_lines_fit_80_cols`; `help_names_unique_and_programs_known`.

## 6. `src/bin/login.rs`
Setup asks `utc offset (e.g. +05:30, Enter for UTC): ` (loop until empty or valid; else `use +HH:MM or -HH:MM`). After dropping privileges in the home: write `.tz` with mode 0600 when not empty.

## 7. `src/edit.rs` + `src/bin/edit.rs`
```rust
pub const TABW: usize = 4;
pub struct Buf { pub lines: Vec<Vec<char>>, pub cy, pub cx, want, pub top, pub left, pub modified: bool, cut: Vec<Vec<char>>, last_cut: bool }
impl Buf {
  pub fn from_text(s: &str) -> Buf; pub fn text(&self) -> String
  pub fn insert(c); newline(); backspace() /*joins at col 0*/; delete() /*joins at end*/
  pub fn left/right/up/down/home/end(); pub fn page(rows, down)
  pub fn cut_line() /*consecutive cuts append*/; pub fn paste() /*above the cursor line*/
  pub fn find(pat) -> bool /*after cursor, wraps*/
  pub fn key(&Key, rows); pub fn scroll(rows, cols)
}
pub fn expand(line) -> Vec<char>; pub fn col_of(line, cx) -> usize
pub fn render(b, cols, rows, name, msg) -> Vec<u8>
   // ?25l; row 1 REV "  edit  {name}  [Modified]" + right "Ln y, Col x"; text rows 2..rows-2; row rows-1 msg; row rows REV help line padded to cols-1; cursor; ?25h
   // rows < 4 or cols < 20 → "edit: terminal too small"
```
Help line: `^S save  ^Q quit  ^F find  ^K cut line  ^U paste  ^X quit`.
Binary: `edit [file]`; directory → error; > 4 MiB → `edit: file too large (max 4 MB)`; not UTF-8 or NUL → `edit: not a text file`; missing → empty + `New file`. Needs a tty (`edit: not a terminal`). Loop: size, scroll, render, `read_keys(500)`; Ctrl-S save; Ctrl-Q/Ctrl-X quit (modified → `Save modified buffer? (y/n)`); Ctrl-F search prompt (empty reuses last). Save: name prompt if none; atomic `{dir}/.{base}.edit-tmp` mode 0600 (old mode kept), write, sync, rename, fsync dir; `Saved N lines to {name}`. Exit clears the screen and restores the tty.
Tests (≥ 10): text round trip, empty, CRLF; insert + UTF-8; newline; backspace/delete joins; sticky column; page; cut×2 + paste; cut only line; find wraps / absent; expand tab and control char; col_of; scroll; render (cursor, [Modified], no row longer than cols).

## 8. `src/glob.rs` + `src/bin/find.rs`
`glob(pat, name)`: `*`, `?`, literal; iterative with one backtrack point. Tests: `*` matches anything incl. empty; `*.txt`; `a*b*c`; `?` one char (UTF-8); empty pattern; dotfiles; no `[]`.
`find [dir] [-name PAT] [-type f|d]`: default `.`; bad args → usage, exit 2; explicit stack, `symlink_metadata` only (never follows), entries sorted; unreadable dir → one error line, continue; depth > 64 → `too deep`; names via `printable`; exit 1 if any error.

## 9. `src/procinfo.rs` + `src/bin/top.rs`
```rust
pub struct Cpu { pub busy: u64, pub total: u64 }; pub fn parse_stat(s) -> Vec<Cpu>
pub struct Mem { pub total, pub avail, pub cached }; pub fn parse_meminfo(s) -> Mem
pub fn parse_pid_stat(s) -> Option<(String, char, u64)>   // comm between '(' and the LAST ')'
pub fn parse_status(s) -> (u32, u64)                      // Uid real, VmRSS kB (0 if absent)
pub fn uptime_str(secs) -> String; pub fn pct(dbusy, dtotal) -> f32; pub fn bar(frac, width) -> String; pub fn size_kb(kb) -> String
```
Tests on sample text: 2-core stat; meminfo; comm with parens; status Uid + VmRSS / missing; pct; bar(0.5,10) == "[|||||     ]"; uptime_str(93785) == "1d 02:03".
`top`: refresh 1 s; /proc/stat, meminfo, loadavg, uptime, and per pid stat/status/cmdline. %CPU from tick deltas / (dt × CLK_TCK). Sort cpu (default) / m mem / p pid; colour bars green < 50 %, yellow < 80 %, red; rows truncated to cols. `k` → `kill pid:` prompt → confirm y → SIGTERM; EPERM → `not your process`; ESRCH → `no such process`. `q` quits cleanly.
```
 top  Tue 17 Nov 14:32:05  up 02:13  load 0.12 0.08 0.01  7 tasks          q quit
 CPU  [||||||||||||||||||||                    ]  48%
 Mem  [|||||||||                               ]  23%   58M used / 245M   31M cached
 sort: cpu   (s cpu  m mem  p pid  k kill)

   PID USER      S  %CPU    RSS  COMMAND
   312 alice     R  48.0   1.2M  top
     1 root      S   0.0   0.4M  /init
```

## 10. Build / install
Binary loop gains `edit find top`; initramfs list adds
```
file /bin/edit $O/edit 0755 0 0
file /bin/find $O/find 0755 0 0
file /bin/top $O/top 0755 0 0
slink /bin/nano /bin/edit 0777 0 0
```
`src/lib.rs`: `pub mod clock; pub mod edit; pub mod glob; pub mod procinfo; pub mod term;`. No Cargo changes.

## 11. Tests
`test/smoke.sh` (both arches) — add `sendk() { printf '%s' "$1" >&3; }` (no newline):
- setup: after `repeat password` → `wait_for 'utc offset'`; `send '+05:30'`.
- `date` matches `^(Mon|...|Sun) [0-9]{2} [A-Z][a-z]{2} 20[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} \+05:30$`; `tz` → `+05:30`; `tz 5` → `use \+HH:MM`.
- `find . -name '*.txt'` → `./docs/a.txt`; `find docs -type d` → `docs`.
- `edit docs/b.txt` → type `line one`, Ctrl-S → `Saved 1 line`, Ctrl-Q → prompt; `cat docs/b.txt` → `line one`.
- `top` → `PID +USER`; `q` → prompt.
- `help` → `^  programs edit`; `help find` → `find \[dir\]`.
- Boot 2: `date` ends in `+05:30` (persisted).
`test/gui.sh`: type `+05:30` at the offset prompt (`shift-equal 0 5 shift-semicolon 3 0 ret`); `date` before the screenshot; footer crop `240x88+1160+1576`; save `docs/den.png`.

## 12. README
Commands (edit/nano, find, top, date, tz); layout; `~/.tz` and the setup prompt (bar shows local time, UTC until login); limitations (edit redraws the whole screen per key, 1 column per char, CRLF → LF, replaces hard links on save; top names only root and you; find has no `[]` and never follows symlinks; serial size unknown → 80x24).

## 13. Checklist
1. clock, glob, procinfo, edit + tests; line Decoder/PgUp/PgDn; files printable; lib.rs → tests green.
2. term.rs; nsh HELP/date/tz + tests.
3. bin edit/find/top; keymap; theme + den footer/bar; login prompt.
4. build-in-docker.sh; both arch builds (report sizes).
5. smoke PASS both arches; gui.sh PASS, look at footer and bar; uefi.sh still PASS (x86_64).
6. README; one commit.

## 14. Acceptance
- `./build.sh test` exit 0 with new tests; no new crates.
- Smoke PASS aarch64 + x86_64 incl. date/tz/find/edit/top; no password in logs.
- `docs/den.png`: bar shows local `Tue 17 Nov  HH:MM`; footer mark ≥ 28 px above the bottom at 2560x1664.
- `read_tz` refuses FIFOs and symlinks without blocking (unit test).
- `tz +05:30` changes the bar within 1 s; `~/.tz` 0600; `/data/etc` still 0700.
- initramfs (now inside the kernel) grows < 1.5 MB.
