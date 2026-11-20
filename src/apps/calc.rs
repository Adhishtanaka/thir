//! calc: type an expression, press Enter. + - * / % ^, parentheses, pi, e, ans (the last result) and sqrt abs sin cos tan ln log exp floor ceil round.
//! The history is kept in ~/.config/thir/calc/history (EXPRESSION TAB RESULT, the newest 100).
use crate::{line::Key, store, tui::*};

const MAX_LEN: usize = 200;
const KEEP: usize = 100;

struct P<'a> { s: &'a [u8], i: usize, ans: f64, depth: u32 }
type R1 = Result<f64, String>;
impl P<'_> {
    fn ws(&mut self) { while self.s.get(self.i) == Some(&b' ') { self.i += 1; } }
    fn peek(&mut self) -> Option<u8> { self.ws(); self.s.get(self.i).copied() }
    fn expr(&mut self) -> R1 {
        let mut v = self.term()?;
        while let Some(c @ (b'+' | b'-')) = self.peek() { self.i += 1; let r = self.term()?; v = if c == b'+' { v + r } else { v - r }; }
        Ok(v)
    }
    fn term(&mut self) -> R1 {
        let mut v = self.unary()?;
        while let Some(c @ (b'*' | b'/' | b'%')) = self.peek() {
            self.i += 1; let r = self.unary()?;
            if c != b'*' && r == 0.0 { return Err("division by zero".into()) }
            v = match c { b'*' => v * r, b'/' => v / r, _ => v % r };
        }
        Ok(v)
    }
    fn unary(&mut self) -> R1 { match self.peek() { Some(b'-') => { self.i += 1; Ok(-self.unary()?) } Some(b'+') => { self.i += 1; self.unary() } _ => self.power() } }
    fn power(&mut self) -> R1 {
        let b = self.atom()?;
        if self.peek() == Some(b'^') { self.i += 1; let e = self.unary()?; return Ok(b.powf(e)) }
        Ok(b)
    }
    fn atom(&mut self) -> R1 {
        self.depth += 1; if self.depth > 60 { return Err("too deeply nested".into()) }
        let r = self.atom1(); self.depth -= 1; r
    }
    fn atom1(&mut self) -> R1 {
        match self.peek() {
            Some(b'(') => { self.i += 1; let v = self.expr()?; if self.peek() != Some(b')') { return Err("missing )".into()) } self.i += 1; Ok(v) }
            Some(c) if c.is_ascii_digit() || c == b'.' => {
                let st = self.i; while self.s.get(self.i).map_or(false, |c| c.is_ascii_digit() || *c == b'.') { self.i += 1; }
                std::str::from_utf8(&self.s[st..self.i]).ok().and_then(|t| t.parse().ok()).ok_or_else(|| "bad number".to_string())
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let st = self.i; while self.s.get(self.i).map_or(false, |c| c.is_ascii_alphabetic()) { self.i += 1; }
                let name = String::from_utf8_lossy(&self.s[st..self.i]).to_lowercase();
                match name.as_str() {
                    "pi" => Ok(std::f64::consts::PI), "e" => Ok(std::f64::consts::E), "ans" => Ok(self.ans),
                    f @ ("sqrt" | "abs" | "sin" | "cos" | "tan" | "ln" | "log" | "exp" | "floor" | "ceil" | "round") => {
                        if self.peek() != Some(b'(') { return Err(format!("{f} needs (...)")) }
                        let x = self.atom()?;
                        match f { "sqrt" if x < 0.0 => Err("sqrt of a negative number".into()), "ln" | "log" if x <= 0.0 => Err("log of zero or less".into()),
                            "sqrt" => Ok(x.sqrt()), "abs" => Ok(x.abs()), "sin" => Ok(x.sin()), "cos" => Ok(x.cos()), "tan" => Ok(x.tan()), "ln" => Ok(x.ln()), "log" => Ok(x.log10()),
                            "exp" => Ok(x.exp()), "floor" => Ok(x.floor()), "ceil" => Ok(x.ceil()), _ => Ok(x.round()) }
                    }
                    _ => Err(format!("unknown name {name}")),
                }
            }
            Some(c) => Err(format!("unexpected '{}'", if c.is_ascii_graphic() { c as char } else { '?' })),
            None => Err("incomplete expression".into()),
        }
    }
}
/// The value of `s`; `ans` is what the name `ans` means. Err = a short message.
pub fn eval(s: &str, ans: f64) -> R1 {
    if s.len() > MAX_LEN { return Err("expression too long".into()) }
    if s.trim().is_empty() { return Err("type an expression".into()) }
    let mut p = P { s: s.as_bytes(), i: 0, ans, depth: 0 };
    let v = p.expr()?;
    if p.peek().is_some() { return Err(format!("unexpected '{}'", if p.s[p.i].is_ascii_graphic() { p.s[p.i] as char } else { '?' })) }
    if v.is_finite() { Ok(v) } else { Err("the result is too large".into()) }
}
/// A result as short text: whole numbers plain, others with up to 10 decimals, huge or tiny ones in scientific form.
pub fn show(v: f64) -> String {
    if v == 0.0 { return "0".into() }
    if v.abs() >= 1e15 || v.abs() < 1e-9 { return format!("{v:e}") }
    let t = format!("{v:.10}"); let t = t.trim_end_matches('0').trim_end_matches('.'); t.to_string()
}
/// "Calculates" `expr` for the AI and the app: the result text or the error.
pub fn calc(expr: &str, ans: f64) -> Result<String, String> { eval(expr, ans).map(show) }

pub fn parse(text: &str) -> Vec<(String, String)> { text.lines().filter_map(|l| { let f = store::split(l, 2)?; (!f[0].is_empty() && f[0].len() <= MAX_LEN).then(|| (crate::files::printable(&f[0]), crate::files::printable(&f[1]))) }).collect() }
pub fn format(v: &[(String, String)]) -> String { v[v.len().saturating_sub(KEEP)..].iter().map(|(e, r)| store::join(&[e, r]) + "\n").collect() }

pub fn run() -> i32 {
    let Some(mut s) = Screen::start("calc") else { return 1 };
    let mut hist = parse(&store::read("calc", "history"));
    let (mut buf, mut recall) = (String::new(), hist.len());
    let mut ans = hist.iter().rev().find_map(|(_, r)| r.parse::<f64>().ok()).unwrap_or(0.0);
    loop {
        let (cols, rows) = crate::term::size(); let w = width(cols);
        let ok = hist.iter().filter(|h| h.1.parse::<f64>().is_ok()).count();
        let mut l = vec![head("calc", &format!("ans {}", show(ans)), w), String::new(), hdr(&format!("   {}result", pad("expression", w.saturating_sub(20)))), rule(w)];
        let room = rows.saturating_sub(l.len() + 5);
        let shown = &hist[hist.len().saturating_sub(room)..];
        for (e, r) in shown {
            let good = r.parse::<f64>().is_ok();
            l.push(format!(" {} {}{}{R}", if good { format!("{GOOD}\u{2713}{R}") } else { format!("{BAD}\u{d7}{R}") }, pad(&fit(e, w.saturating_sub(22)), w.saturating_sub(20)), format!("{}{}", if good { FG } else { BAD }, pad(&fit(r, 16), 16))));
        }
        if hist.is_empty() { l.push(format!(" {DIM}Try 2+3*4, (1+2)^3, sqrt(2), 10/4 or ans*2{R}")); }
        while l.len() < rows.saturating_sub(4) { l.push(String::new()); }
        l.push(rule(w)); l.push(dots(&[format!("{DIM}{} total{R}", hist.len()), format!("{GOOD}{ok} ok{R}"), if hist.len() > ok { format!("{BAD}{} errors{R}", hist.len() - ok)} else { format!("{DIM}0 errors{R}") }]));
        l.push(format!(" {ACC}\u{203a}{R} {}{ACCBG} {R}", fit(&buf, w.saturating_sub(4))));
        l.push(hints(&[("enter", "calculate"), ("up/down", "history"), ("ctrl+u", "clear"), ("q", "quit")]));
        draw(&l);
        let Some(keys) = s.keys(-1) else { return 0 };
        for k in keys {
            match k {
                Key::Ctrl(b'c') | Key::Ctrl(b'd') => return 0,
                Key::Ctrl(b'u') => buf.clear(),
                Key::Char('q') if buf.is_empty() => return 0,
                Key::Char(c) if buf.len() < MAX_LEN && !crate::files::unsafe_char(c) => buf.push(c),
                Key::Backspace => { buf.pop(); }
                Key::Up => { recall = recall.saturating_sub(1); if let Some(h) = hist.get(recall) { buf = h.0.clone(); } }
                Key::Down => { recall = (recall + 1).min(hist.len()); buf = hist.get(recall).map_or(String::new(), |h| h.0.clone()); }
                Key::Enter if !buf.trim().is_empty() => {
                    let r = eval(&buf, ans);
                    let text = match &r { Ok(v) => { ans = *v; show(*v) } Err(m) => m.clone() };
                    hist.push((buf.trim().to_string(), text)); let _ = store::write("calc", "history", &format(&hist));
                    if hist.len() > KEEP { hist.remove(0); }
                    buf.clear(); recall = hist.len();
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(s: &str) -> String { calc(s, 0.0).unwrap_or_else(|e| format!("ERR {e}")) }
    #[test] fn arithmetic_and_precedence() {
        for (e, r) in [("2+3*4", "14"), ("(1+2)^3", "27"), ("10/4", "2.5"), ("2^3^2", "512"), ("-2^2", "-4"), ("7%3", "1"), ("0.1+0.2", "0.3"), ("1/3", "0.3333333333"), (" 2 * ( 3 + 4 ) ", "14"), ("--5", "5"), ("2*-3", "-6"),
            ("sqrt(16)", "4"), ("abs(-3.5)", "3.5"), ("round(2.5)", "3"), ("floor(2.9)+ceil(2.1)", "5"), ("log(1000)", "3"), ("pi", "3.1415926536"), ("ln(e)", "1"), ("1e", "ERR unexpected 'e'")] { assert_eq!(c(e), r, "{e}"); }
    }
    #[test] fn ans_and_errors() {
        assert_eq!(calc("ans*2", 21.0), Ok("42".into()));
        for (e, m) in [("1/0", "division by zero"), ("5%0", "division by zero"), ("2+", "incomplete expression"), ("(1+2", "missing )"), ("1+x", "unknown name x"), ("sqrt(-1)", "sqrt of a negative number"), ("sqrt 4", "sqrt needs (...)"),
            ("2 3", "unexpected '3'"), ("1..2", "bad number"), ("", "type an expression"), ("2$", "unexpected '$'"), ("10^400", "the result is too large"), ("ln(0)", "log of zero or less")] { assert_eq!(calc(e, 0.0), Err(m.into()), "{e}"); }
        assert!(calc(&"(".repeat(100), 0.0).is_err()); assert!(calc(&"1+".repeat(150), 0.0).is_err()); assert!(calc("\u{1b}[2J", 0.0).is_err());
    }
    #[test] fn shown_forms() { assert_eq!((show(2.0), show(-0.5), show(1e20), show(1e-12), show(0.0)), ("2".into(), "-0.5".into(), "1e20".into(), "1e-12".into(), "0".into())); }
    #[test] fn history_file() {
        let h: Vec<(String, String)> = (0..150).map(|i| (format!("{i}+1"), format!("{}", i + 1))).collect();
        let t = format(&h); let p = parse(&t); assert_eq!(p.len(), KEEP); assert_eq!(p.last().unwrap(), &("149+1".to_string(), "150".to_string()));
        assert_eq!(parse("junk\n\t5\n1+1\t2\n").len(), 1);
    }
}
