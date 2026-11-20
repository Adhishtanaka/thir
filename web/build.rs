// Compiles platform.c against the WebKit install and links it. pkg-config finds WPE WebKit in /usr (the build container installs the staged WebKit there first).
use std::process::Command;
fn pc(args: &[&str]) -> String {
    let o = Command::new("pkg-config").args(args).args(["wpe-webkit-2.0", "wpe-platform-2.0", "xkbcommon", "glib-2.0", "gio-2.0"]).output().expect("pkg-config");
    assert!(o.status.success(), "pkg-config {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}
fn main() {
    println!("cargo:rerun-if-changed=platform.c"); println!("cargo:rerun-if-changed=platform.h");
    let out = std::env::var("OUT_DIR").unwrap(); let obj = format!("{out}/platform.o");
    let cflags = pc(&["--cflags"]);
    let ok = Command::new("cc").args(["-O2", "-Wall", "-Wextra", "-Wno-unused-parameter", "-fPIC", "-c", "platform.c", "-o", &obj]).args(cflags.split_whitespace()).status().expect("cc").success();
    assert!(ok, "platform.c does not compile");
    assert!(Command::new("ar").args(["crs", &format!("{out}/libthirplatform.a"), &obj]).status().expect("ar").success());
    println!("cargo:rustc-link-search=native={out}"); println!("cargo:rustc-link-lib=static=thirplatform");
    for t in pc(&["--libs"]).split_whitespace() {
        if let Some(l) = t.strip_prefix("-L") { println!("cargo:rustc-link-search=native={l}"); }
        else if let Some(l) = t.strip_prefix("-l") { println!("cargo:rustc-link-lib={l}"); }
    }
}
