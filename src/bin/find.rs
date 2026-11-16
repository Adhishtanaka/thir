//! find [dir] [-name PAT] [-type f|d]: walk a tree, print matching paths. Never follows symlinks.
use std::{ffi::OsString, fs, io::Write, path::PathBuf, process::exit};
use thir::{files::printable, glob::glob};

const MAX_DEPTH: usize = 64;

fn usage() -> ! { eprintln!("usage: find [dir] [-name PATTERN] [-type f|d]"); exit(2) }

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let root = if args.peek().is_some_and(|a| !a.starts_with('-')) { args.next().unwrap() } else { ".".into() };
    let (mut name, mut ty) = (None::<String>, None::<char>);
    while let Some(a) = args.next() {
        match (a.as_str(), args.next()) {
            ("-name", Some(p)) => name = Some(p),
            ("-type", Some(t)) if t == "f" || t == "d" => ty = t.chars().next(),
            _ => usage(),
        }
    }
    let (mut out, mut failed) = (std::io::stdout().lock(), false);
    let mut stack: Vec<(PathBuf, usize)> = vec![(PathBuf::from(&root), 0)];
    while let Some((path, depth)) = stack.pop() {
        let shown = printable(&path.to_string_lossy());
        let md = match fs::symlink_metadata(&path) { Ok(m) => m, Err(e) => { eprintln!("find: {shown}: {e}"); failed = true; continue } };
        let base = path.file_name().map_or(root.clone(), |n| n.to_string_lossy().into_owned());
        let type_ok = match ty { Some('f') => md.is_file(), Some(_) => md.is_dir(), None => true };
        if type_ok && name.as_ref().map_or(true, |p| glob(p, &base)) && writeln!(out, "{shown}").is_err() { exit(1) }
        if !md.is_dir() { continue }
        if depth >= MAX_DEPTH { eprintln!("find: {shown}: too deep"); failed = true; continue }
        match fs::read_dir(&path) {
            Ok(rd) => {
                let mut names: Vec<OsString> = rd.flatten().map(|e| e.file_name()).collect();
                names.sort(); names.reverse();   // popped in sorted order
                stack.extend(names.into_iter().map(|n| (path.join(n), depth + 1)));
            }
            Err(e) => { eprintln!("find: {shown}: {e}"); failed = true; }
        }
    }
    let _ = out.flush();
    exit(failed as i32);
}
