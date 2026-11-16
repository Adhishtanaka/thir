use netos::{net, parse::parse_line};
use std::{fs::File, io::{self, Write}, process::{exit, Command}};

fn main() {
    let stdin = io::stdin();
    loop {
        print!("nsh> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) { Ok(0) | Err(_) => exit(0), Ok(_) => {} }
        let argv = match parse_line(&line) { Ok(a) => a, Err(e) => { println!("nsh: {e}"); continue; } };
        let Some(cmd) = argv.first() else { continue };
        match cmd.as_str() {
            "cd" => { let d = argv.get(1).map_or("/", |s| s); if let Err(e) = std::env::set_current_dir(d) { println!("nsh: cd: {d}: {e}"); } }
            "exit" => exit(argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(0)),
            "help" => println!("nsh builtins: cd exit help ls cat ip poweroff reboot"),
            "ls" => {
                let d = argv.get(1).map_or(".", |s| s);
                match std::fs::read_dir(d) {
                    Ok(rd) => {
                        let mut names: Vec<String> = rd.filter_map(|e| e.ok())
                            .map(|e| { let n = e.file_name().to_string_lossy().into_owned(); if e.path().is_dir() { n + "/" } else { n } }).collect();
                        names.sort();
                        for n in names { println!("{n}"); }
                    }
                    Err(e) => println!("nsh: ls: {d}: {e}"),
                }
            }
            "cat" => for a in &argv[1..] {
                match File::open(a).and_then(|mut f| io::copy(&mut f, &mut io::stdout())) { Ok(_) => {} Err(e) => println!("nsh: cat: {a}: {e}") }
            },
            "ip" => match net::interfaces() { Ok(v) => for i in &v { println!("{}", i.line()); }, Err(e) => println!("nsh: ip: {e}") },
            "poweroff" => exit(netos::EXIT_POWEROFF),
            "reboot" => exit(netos::EXIT_REBOOT),
            _ => match Command::new(cmd).args(&argv[1..]).status() {
                Err(e) => println!("nsh: {cmd}: {e}"),
                Ok(s) if !s.success() => println!("[exit {}]", s.code().unwrap_or(-1)),
                Ok(_) => {}
            },
        }
    }
}
