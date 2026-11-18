//! edit [file]: full-screen text editor (also reachable as `nano`). Buffer logic lives in thir::edit.
use std::{fs::{self, File}, io::Read, path::Path, process::exit};
use thir::{edit::{render, Buf}, line::{Decoder, Key}, term::{self, put, Raw}};

const MAX: u64 = 4 << 20;

fn die(m: &str) -> ! { eprintln!("edit: {m}"); exit(1) }

fn load(name: &str) -> (Buf, &'static str) {
    match fs::metadata(name) {
        Ok(m) if m.is_dir() => die(&format!("{name}: is a directory")),
        Ok(m) if !m.is_file() => die("not a text file"),
        Ok(m) if m.len() > MAX => die("file too large (max 4 MB)"),
        Ok(_) => {
            let mut v = vec![];
            if let Err(e) = File::open(name).and_then(|f| f.take(MAX + 1).read_to_end(&mut v)) { die(&format!("{name}: {e}")); }
            if v.len() as u64 > MAX { die("file too large (max 4 MB)") }
            match String::from_utf8(v) { Ok(s) if !s.contains('\0') => (Buf::from_text(&s), ""), _ => die("not a text file") }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Buf::from_text(""), "New file"),
        Err(e) => die(&format!("{name}: {e}")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 1 { die("usage: edit [file]") }
    if unsafe { libc::isatty(0) != 1 || libc::isatty(1) != 1 } { die("not a terminal") }
    let mut name = args.first().cloned();
    let (mut buf, m0) = name.as_deref().map_or((Buf::from_text(""), ""), load);
    let mut msg_s = m0.to_string();
    let mut dec = Decoder::new();
    let Some(_raw) = Raw::enter() else { die("not a terminal") };
    let mut last_find = String::new();
    'main: loop {
        let (cols, rows) = term::size();
        buf.scroll(rows, cols);
        put(&render(&buf, cols, rows, name.as_deref().unwrap_or("[No Name]"), &msg_s));
        let Some(keys) = term::read_keys(&mut dec, 500) else { break };
        for k in keys {
            msg_s.clear();
            let mut quit = false;
            match k {
                Key::Ctrl(b's') => msg_s = do_save(&mut buf, &mut name, &mut dec, rows),
                Key::Ctrl(b'q') | Key::Ctrl(b'x') => {
                    if !buf.modified { quit = true } else {
                        match term::prompt(&mut dec, rows - 1, "Save modified buffer? (y/n) ").as_deref().map(|s| s.trim().to_ascii_lowercase()) {
                            Some(a) if a.starts_with('y') => { msg_s = do_save(&mut buf, &mut name, &mut dec, rows); quit = !buf.modified; }
                            Some(a) if a.starts_with('n') => quit = true,
                            _ => msg_s = "Cancelled".into(),
                        }
                    }
                }
                Key::Ctrl(b'f') => {
                    if let Some(p) = term::prompt(&mut dec, rows - 1, "Find: ") {
                        if !p.is_empty() { last_find = p; }
                        msg_s = if last_find.is_empty() { String::new() } else if buf.find(&last_find) { String::new() } else { format!("Not found: {last_find}") };
                    }
                }
                _ => buf.key(&k, rows),
            }
            if quit { break 'main }
        }
    }
    put(b"\x1b[H\x1b[2J");
}

fn do_save(buf: &mut Buf, name: &mut Option<String>, dec: &mut Decoder, rows: usize) -> String {
    if name.is_none() {
        match term::prompt(dec, rows - 1, "Save as: ").filter(|s| !s.is_empty()) { Some(n) => *name = Some(n), None => return "Cancelled".into() }
    }
    let n = name.as_deref().unwrap();
    match thir::files::save(Path::new(n), buf.text().as_bytes()) {
        Ok(()) => { buf.modified = false; let c = buf.lines.len(); format!("Saved {c} line{} to {n}", if c == 1 { "" } else { "s" }) }
        Err(e) => format!("Cannot save: {e}"),
    }
}
