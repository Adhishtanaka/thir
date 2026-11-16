# thir — Phase 3 plan: persistent disk, accounts, login, file commands (made by the Fable planner)

All paths are under the repo root. Phase 1 conventions stay: small code, `libc` only (plus the explicit crypto exception below), comments only for the why, unit tests for every non-trivial pure function, no busybox, no shell scripts in the image. Phase 2 (den) touches this plan only through the contract in section 7; if `src/bin/den.rs` exists when you build, apply that contract to it.

## 0. Decisions

| Decision | Choice | Why |
|---|---|---|
| Filesystem | **ext4** (journal on, mkfs defaults) | Users and the smoke test kill QEMU / power off without unmounting. ext4 replays its journal on the next mount with no fsck; ext2 would silently mount dirty. |
| Where the image is formatted | Host side, **in the Docker build container** (`mkfs.ext4` from `e2fsprogs`, on the raw file, no loop device) | macOS has no mkfs; a Rust mkfs is far too much code; the guest has no e2fsprogs. |
| Image | `out/<arch>/data.img`, raw, 128 MiB sparse, created when missing by `./build.sh` and `./run.sh`; `./build.sh data` recreates it | raw so mkfs can write it directly; sparse so it costs ~2 MB. |
| Mount | `/dev/vda` → `/data`, `nosuid,nodev,noexec` | Nothing executes from /data today; drop `noexec` when a toolchain exists. |
| Accounts file | `/data/etc/users`, dir 0700 root, file 0600 root, one line per user | Persistent; unreadable by users. |
| KDF | RustCrypto **`argon2` 0.5.x**, Argon2id, `default-features = false, features = ["alloc"]`, raw `hash_password_into`; 16 B salt from `getrandom`; 32 B hash; constant-time compare with **`subtle`** | Do not invent crypto. OWASP params m=19456 KiB, t=2, p=1. Params stored per line so they can be raised later. |
| Login program | `/bin/login`, one binary: no args (serial, spawned by init), `--as <uid>` (den's extra panes); setup is automatic when no user exists | One auth path for both consoles. login execs `nsh` after dropping privileges, so init's `decide()` exit-code protocol is unchanged. |
| den sessions | den spawns `/bin/login` in pane 1; den learns the login happened from `Uid:` in `/proc/<pid>/status` of pane 1; new panes run `/bin/login --as <uid>`; when the last pane closes den spawns `/bin/login` again (logout) | No pipe/fd contract, no password handling in den. Masking = termios ECHO off on the pty slave, done by login. |
| passwd | separate **setuid-root** `/bin/passwd` (`04755` in the cpio) | Classic vetted mechanism. Hardened: refuses if fds 0-2 are not a tty, ignores argv/env, uses the real uid, atomic file replace. |
| Put text in a file | `echo text > file`, `>>`, and a `write <file>` builtin (reads lines until a line `.`) | Smallest thing that works over serial and in den, scriptable from the smoke test. |
| Disk encryption | **Deferred** (limitation) | dm-crypt/LUKS needs header + key management; fscrypt needs a wrapped master key (an AEAD crate). README states: anyone with `out/<arch>/data.img` on the Mac reads everything. |

**New crates (the only exception to libc-only), pinned by `Cargo.lock`:**
- `argon2 = { version = "=0.5.3", default-features = false, features = ["alloc"] }` (newest 0.5.x if 0.5.3 is unavailable; do not jump to 0.6 without checking `hash_password_into`).
- `subtle = { version = "2", default-features = false }` (already transitive via argon2).
- Transitive: `blake2`, `digest`, `block-buffer`, `crypto-common`, `generic-array`, `typenum`, `version_check`, `base64ct`, `cpufeatures`. Total download < 500 KB.

## 1. Threat model

- Protects against: a person at the serial console or GUI keyboard who does not know the password; a logged-in user reading `/data/etc/users`, raw devices (`/dev/vda`, `/dev/fb0`, `/dev/input/*`, `/dev/ttyAMA0`), root processes' memory/environment, or escalating via den, login, passwd, or dhcp; an unclean power-off corrupting user data; passwords leaking into logs or the serial transcript.
- Does not protect against: anyone with access to the macOS host (plaintext `data.img`, QEMU monitor, host RAM); kernel bugs; a malicious root binary; timing side channels beyond constant-time compare; offline brute force of a copied image (Argon2 only slows it).
- Root runs: `init`, `dhcp`, `den` (fb + input), `login` (until it execs nsh), `passwd` (setuid, short-lived). Everything the user launches runs as uid ≥ 1000 with umask 077 and may write only to its home and `/tmp`.
- No root login; recovery from a lost password = delete `out/<arch>/data.img` on the host (documented).

## 2. Build/host side

### 2.1 `docker/Dockerfile`
Add `e2fsprogs` to the apt line.

### 2.2 `env.sh`
After `OUT=out/$THIR_ARCH`:
```bash
DATA_IMG=${DATA_IMG:-$OUT/data.img}   # raw ext4 image; path relative to the repo root (also /work in the container)
DATA_SIZE=${DATA_SIZE:-128M}
```
Append to the `QEMU` array:
```
      -drive file=$DATA_IMG,format=raw,if=none,id=d0 -device virtio-blk-pci,drive=d0
      -device virtio-rng-pci
```
(`getrandom(0)` blocks until the CRNG is seeded; first-boot setup can run within a second of boot.)

### 2.3 `build.sh`
Add `-e DATA_IMG="$DATA_IMG" -e DATA_SIZE="$DATA_SIZE"` to `docker run`. Subcommands: `all` (default), `test`, `data`.

### 2.4 `scripts/build-in-docker.sh`
- After the `test` line:
```bash
mkdata() { rm -f "/work/$DATA_IMG"; truncate -s "$DATA_SIZE" "/work/$DATA_IMG"; mkfs.ext4 -q -F -L thirdata "/work/$DATA_IMG"; echo "data image: /work/$DATA_IMG ($DATA_SIZE)"; }
if [ "${1:-all}" = data ]; then mkdata; exit 0; fi
```
- At the end of `all`: `[ -f "/work/$DATA_IMG" ] || mkdata` (never reformat an existing image).
- Binaries loop: `for b in init nsh dhcp login passwd; do ...` (plus `den` from phase 2).
- initramfs list additions:
```
dir /data 0755 0 0
file /bin/login $O/login 0755 0 0
file /bin/passwd $O/passwd 04755 0 0
```
- Fallback if mkfs on the bind mount fails on Kiri: `mkdata` into `/build/data.img` then `cp` to `/work/$DATA_IMG`.

### 2.5 `run.sh`
Before `exec`: `[ -f "$DATA_IMG" ] || ./build.sh data`.

### 2.6 `kernel/common.config` (append; keep phase 2's lines)
```
CONFIG_BLOCK=y
CONFIG_BLK_DEV=y
CONFIG_VIRTIO_BLK=y
CONFIG_EXT4_FS=y
CONFIG_HW_RANDOM=y
CONFIG_HW_RANDOM_VIRTIO=y
```
If the config check reports a missing line, add its dependency; never delete. Both arches use `/dev/vda`. No partition table.

## 3. `Cargo.toml`
```toml
[dependencies]
libc = "0.2"
argon2 = { version = "=0.5.3", default-features = false, features = ["alloc"] }   # Argon2id KDF; the one crypto exception, see README
subtle = { version = "2", default-features = false }                              # constant-time hash compare
```
Run `./build.sh test` first so `Cargo.lock` resolves inside Docker; commit the lock.

## 4. Library: `src/user.rs` (new; `pub mod user;` in lib.rs)

Account file `/data/etc/users`, one user per line:
```
name:uid:argon2id:m_kib:t:p:salt_hex:hash_hex
```
gid = uid (private group, no group file).

```rust
pub const USERS: &str = "/data/etc/users";
pub const HOME_ROOT: &str = "/data/home";
pub const FIRST_UID: u32 = 1000;
pub const MIN_PASSWORD: usize = 8;   pub const MAX_PASSWORD: usize = 128;   // bytes

#[derive(Debug, Clone, Copy, PartialEq)] pub struct Params { pub m_kib: u32, pub t: u32, pub p: u32 }
pub const DEFAULT: Params = Params { m_kib: 19456, t: 2, p: 1 };   // OWASP minimum for Argon2id
#[cfg(test)] pub const FAST: Params = Params { m_kib: 8, t: 1, p: 1 };

#[derive(Debug, Clone, PartialEq)]
pub struct User { pub name: String, pub uid: u32, pub params: Params, pub salt: [u8; 16], pub hash: [u8; 32] }

pub fn valid_name(s: &str) -> bool          // ^[a-z_][a-z0-9_-]{0,31}$ and not "root"
pub fn hash(password: &str, salt: &[u8; 16], p: &Params) -> Result<[u8; 32], String>
pub fn new(name: &str, uid: u32, password: &str, salt: [u8; 16], p: &Params) -> Result<User, String>  // pure; validates name/length
pub fn create(name: &str, uid: u32, password: &str) -> Result<User, String>  // random(salt) + new(.., &DEFAULT)
pub fn parse(text: &str) -> Result<Vec<User>, String>   // Err on any malformed line; validates name, uid >= FIRST_UID, 8 <= m_kib <= 1<<20, 1<=t<=64, 1<=p<=16, hex lengths
impl User {
    pub fn line(&self) -> String
    pub fn verify(&self, password: &str) -> bool   // hash() then subtle::ConstantTimeEq; false on any error
    pub fn home(&self) -> String                   // "/data/home/<name>"
}
pub fn load() -> io::Result<Vec<User>>
pub fn save(users: &[User]) -> io::Result<()>      // write USERS.tmp mode 0600, sync_all, rename over USERS, fsync /data/etc
pub fn random(buf: &mut [u8]) -> io::Result<()>    // getrandom loop, retry EINTR, Err otherwise; no weak fallback
pub fn become(uid: u32, gid: u32) -> io::Result<()>
    // setgroups(0, null) → setgid → setuid, each checked; then assert real==effective==target for uid and gid and that setuid(0) fails
pub fn read_line(prompt: &str) -> io::Result<String>
pub fn read_secret(prompt: &str) -> io::Result<String>  // tcgetattr(0) (Err if not a tty) → clear ECHO, TCSAFLUSH → prompt → read → restore → "\n"
pub fn dummy_verify(password: &str)                // unknown user costs the same as a wrong password
```
Unit tests (`FAST`): round trip `parse(&u.line())`; `verify` right/wrong/empty; `parse` rejects missing field, uid 0, bad hex length, non-numeric m, unknown algorithm, `root`; `valid_name` cases; `new` rejects passwords < 8 and > 128 bytes.

## 5. Binaries

### 5.1 `src/bin/login.rs`
```
if getuid() != 0 → "login: must be run by init or den" → exit 1
ignore SIGINT SIGQUIT SIGTSTP
if !/data/etc exists → "login: /data is not mounted" → sleep 5 → exit 1
--as <uid>: load(); find uid (≠ 0) else exit 1; start(u)
serial:
  load(): NotFound ⇒ empty; other Err ⇒ "login: account file corrupt", sleep 5, exit 1
  empty → setup() → start(u)
  else up to 5 tries: name = read_line("thir login: "); pw = read_secret("password: ")
       match user: verify ok → start; wrong → nothing; unknown → dummy_verify
       println "login incorrect"; sleep 3 s
  → "login: too many failures"; exit 1
setup():
  println "thir: first boot: create the first user"
  loop: username (valid_name else "invalid name (a-z, 0-9, _ -, max 32)"),
        password + "repeat password" (mismatch → "passwords do not match"; < 8 → "password too short (min 8)")
        if another console created a user meanwhile → "a user was already created on another console", back to login
        create(name, FIRST_UID, pw); save; println "user {name} created"
start(u): (never returns)
  mkdir home 0700 if missing; chown to uid; umask(0o077); become(uid, uid) (Err → exit 1)
  chdir(home) else "/"
  exec /bin/nsh with env PATH=/bin HOME=<home> USER=<name>
```
Never print the password or its hash. Any read error → exit 1.

### 5.2 `src/bin/passwd.rs` (setuid root)
```
fds 0-2 must be ttys, euid must be 0; ignore SIGINT SIGQUIT SIGTSTP; ignore argv and env
uid = getuid(); find the user by uid else exit 1
3 tries of "old password: " (wrong → "passwd: wrong password", sleep 3) → exit 1
"new password: " + "repeat new password: " (mismatch / < 8 / > 128 → message, exit 1)
replace with create(name, uid, new) (fresh salt); save; "passwd: password changed"
```

### 5.3 `src/bin/init.rs` (re-read first; phase 2 changed it)
- `mount()` takes `flags`; `/tmp` gets `MS_NOSUID|MS_NODEV`.
- `mount_data()` after devtmpfs/proc/sys: wait up to 3 s for `/dev/vda`; mount ext4 at `/data` with `MS_NOSUID|MS_NODEV|MS_NOEXEC`; then mkdir `/data/etc` 0700 and `/data/home` 0755 (EEXIST ok); print `thir: mounted /data`; on failure print `thir: /data unavailable: {e}` and continue.
- The serial console spawns `/bin/login` (same tty/setsid/TIOCSCTTY code, env PATH only). Message "shell exited, restarting" → "console session ended, restarting".
- `shutdown()`: `sync(); umount2("/data", 0)` (ignore result); `sync()`; `reboot()`.
- dhcp, the `thir: init ready` marker and den supervision unchanged.

### 5.4 `src/parse.rs`
```rust
#[derive(Debug, PartialEq, Default)]
pub struct Cmd { pub argv: Vec<String>, pub redirect: Option<(String, bool)> }  // (path, append)
pub fn parse_cmd(line: &str) -> Result<Cmd, &'static str>
pub fn parse_line(line: &str) -> Result<Vec<String>, &'static str> { parse_cmd(line).map(|c| c.argv) }
```
Unquoted, unescaped `>` ends the current token; a following `>` means append; the next token is the target. A second redirect → Err("multiple redirects"); `>` with no target → Err("missing redirect target").
Tests: `echo hi > f`; `echo hi >> f`; `echo hi>f`; `echo ">"` (not a redirect); `cat a > b c` → [cat,a,c] + b; `echo >` → Err; `echo > a > b` → Err; `echo 'x' > "my file"`.

### 5.5 `src/files.rs` (new)
```rust
/// Why `rm` must refuse `path`, or None: '/', '.', '..', and paths ending in '/.' or '/..'.
pub fn rm_refuses(path: &str) -> Option<&'static str>
```
Tests: `/`, `//`, `.`, `..`, `a/..`, `/data/home/x/.`, `` → Some; `a`, `a/`, `./a`, `/data/home/x/docs`, `..a` → None.

### 5.6 `src/bin/nsh.rs`
- Use `parse_cmd`. Open the redirect file first (create, truncate or append; umask makes it 0600); on error print `nsh: {path}: {e}` and skip. Builtins write to the redirect target; external commands get it as stdout. All errors go to stderr.
- `cd` with no argument → `$HOME`.
- New builtins: `pwd`, `echo`, `whoami`, `mkdir [-p]`, `rmdir`, `touch`, `rm [-r]` (uses `rm_refuses`, error message `rm: refusing to remove '/'` style; no `-r` on a dir → "is a directory (use -r)"; never follows symlinks), `mv`, `cp` (into an existing dir → `dir/basename`), `write file` (lines until `.` or EOF).
- `help`: `nsh builtins: cd pwd ls cat echo whoami mkdir rmdir touch rm mv cp write exit help ip poweroff reboot; redirect with > and >>; commands: passwd dhcp`.

## 6. Permissions
umask 077 → files 0600, dirs 0700; home 0700 user:user; `/data/home` 0755 root; `/data/etc` 0700 root. Device nodes stay 0600 root. Never print secrets (`/dev/kmsg` is 0644). Logout = `exit`.

## 7. Contract for den
1. Pane 1 of a session runs `["/bin/login"]`. While no user is logged in, den shows only this pane (centered, e.g. 60×12 cells, bar text `login`) and ignores `Alt+Enter`.
2. "Logged in" = the `Uid:` line in `/proc/<pid of pane 1 child>/status` is not 0. Check it whenever pane 1 produces output; the first time it is not 0, switch to the tiled layout and remember `uid`.
3. Every later pane runs `["/bin/login", "--as", uid]`.
4. When the last pane closes, forget `uid` and go back to step 1 (logout).
5. Pty masters are opened with `O_CLOEXEC` (`posix_openpt(O_RDWR|O_NOCTTY|O_CLOEXEC)`); input and fb fds likewise. Children inherit only fds 0-2 (the slave).
6. den never does local echo, never logs key input, and execs nothing but `/bin/login`.
7. `test/gui.sh` must type the user name and password with `sendkey`, on its own fresh disk.

## 8. Tests
- Unit: existing + parse redirect + user + files; ≥ 33 total.
- `test/smoke.sh` rewrite (both arches): fresh disk → boot 1 (first-boot setup, `whoami`, `pwd`, `mkdir`, `echo >`/`>>`, `cat`; `cat /data/etc/users` and `cat /dev/vda` denied; `dhcp lo` fails as user; `rm /` refused; hostname and resolv.conf; `passwd` change) → poweroff → boot 2 on the same disk (old password → `login incorrect`; new password works; `docs/a.txt` still has `hello`; `ls` shows `docs/`) → poweroff. A `send` marks the log line count and `wait_for` only searches after the mark. Fail if either password ever appears in the serial log. `SMOKE_TIMEOUT=0` must exit 1. Slow timeouts (60 s) for argon2 under x86_64 TCG.

## 9. README
`./build.sh data`; layout (`src/user.rs`, `src/files.rs`, `src/bin/login.rs`, `src/bin/passwd.rs`, `out/<arch>/data.img`); sections **Storage**, **Accounts and sessions** (with the den contract), **Security** (root vs user, threat model, crate exception, no root login, recovery = delete data.img). Limitations: disk not encrypted; single user (no `adduser`); `write` replaces a whole file; `mv` across filesystems fails; no recovery shell; `/data` is noexec.

## 10. Ordered build checklist
1. Dockerfile, env.sh, build.sh, build-in-docker.sh, run.sh, kernel config.
2. Cargo.toml deps; lib.rs modules.
3. user.rs, files.rs, parse.rs with tests → `./build.sh test` green; commit Cargo.lock.
4. login.rs, passwd.rs.
5. init.rs, nsh.rs.
6. `./build.sh`: config check silent; login, passwd, data.img exist; passwd is `-rwsr-xr-x` in the cpio.
7. Interactive check with `./run.sh`.
8. smoke.sh rewrite → PASS; `SMOKE_TIMEOUT=0` → exit 1.
9. x86_64 build + smoke → PASS.
10. Apply section 7 to den; re-run `test/gui.sh`.
11. README; one commit.

## 11. Acceptance criteria
- `./build.sh test` exit 0, ≥ 33 tests; no password or hash in test output.
- Both arch builds exit 0; config check silent; `data.img` created when missing, untouched when present.
- Direct deps only `libc`, `argon2`, `subtle`.
- Smoke PASS both arches (aarch64 ≤ 60 s, x86_64 ≤ 4 min); `SMOKE_TIMEOUT=0` → exit 1.
- Passwords never in the serial log; users file 0600 root in a 0700 dir; `/dev/vda` unreadable by the user; `dhcp` fails as the user; `rm /` refused; wrong old password in `passwd` fails after 3 s; old password rejected after a change; files survive a power-off.
- User files 0600, dirs 0700, home 0700 owned by the user.
- With den: pane 1 shows `thir login:`; the password is not visible (screendump); `Alt+Enter` opens a pane as the same user; closing all panes shows the login screen again.
- README documents storage, accounts, den contract, threat model, crate exception and the unencrypted-disk limitation.
