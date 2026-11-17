//! Small JSON reader and string quoter for the API replies. Pure code, no crates.
#[derive(Debug, Clone, PartialEq)]
pub enum Json { Null, Bool(bool), Num(f64), Str(String), Arr(Vec<Json>), Obj(Vec<(String, Json)>) }

const MAX_DEPTH: usize = 64;

impl Json {
    pub fn get(&self, k: &str) -> Option<&Json> { if let Json::Obj(o) = self { o.iter().find(|(n, _)| n == k).map(|(_, v)| v) } else { None } }
    pub fn idx(&self, i: usize) -> Option<&Json> { if let Json::Arr(a) = self { a.get(i) } else { None } }
    pub fn str(&self) -> Option<&str> { if let Json::Str(s) = self { Some(s) } else { None } }
    pub fn num(&self) -> Option<f64> { if let Json::Num(n) = self { Some(*n) } else { None } }
}

struct P<'a> { b: &'a [u8], i: usize }

pub fn parse(s: &str) -> Result<Json, String> {
    let mut p = P { b: s.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() { return Err("trailing data".into()); }
    Ok(v)
}

impl P<'_> {
    fn ws(&mut self) { while matches!(self.b.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) { self.i += 1; } }
    fn eat(&mut self, c: u8) -> Result<(), String> { if self.b.get(self.i) == Some(&c) { self.i += 1; Ok(()) } else { Err(format!("expected '{}'", c as char)) } }
    fn lit(&mut self, w: &str, v: Json) -> Result<Json, String> { if self.b[self.i..].starts_with(w.as_bytes()) { self.i += w.len(); Ok(v) } else { Err("bad literal".into()) } }
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth >= MAX_DEPTH { return Err("nested too deep".into()); }
        self.ws();
        match self.b.get(self.i) {
            None => Err("unexpected end".into()),
            Some(b'n') => self.lit("null", Json::Null),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'[') => {
                self.i += 1; let mut a = vec![]; self.ws();
                if self.b.get(self.i) == Some(&b']') { self.i += 1; return Ok(Json::Arr(a)); }
                loop {
                    a.push(self.value(depth + 1)?); self.ws();
                    match self.b.get(self.i) { Some(b',') => self.i += 1, Some(b']') => { self.i += 1; return Ok(Json::Arr(a)); } _ => return Err("bad array".into()) }
                }
            }
            Some(b'{') => {
                self.i += 1; let mut o = vec![]; self.ws();
                if self.b.get(self.i) == Some(&b'}') { self.i += 1; return Ok(Json::Obj(o)); }
                loop {
                    self.ws(); let k = self.string()?; self.ws(); self.eat(b':')?;
                    o.push((k, self.value(depth + 1)?)); self.ws();
                    match self.b.get(self.i) { Some(b',') => self.i += 1, Some(b'}') => { self.i += 1; return Ok(Json::Obj(o)); } _ => return Err("bad object".into()) }
                }
            }
            Some(b'-' | b'0'..=b'9') => {
                let st = self.i;
                while matches!(self.b.get(self.i), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) { self.i += 1; }
                std::str::from_utf8(&self.b[st..self.i]).ok().and_then(|t| t.parse().ok()).map(Json::Num).ok_or_else(|| "bad number".into())
            }
            _ => Err("unexpected character".into()),
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.b.get(self.i..self.i + 4).and_then(|h| std::str::from_utf8(h).ok()).filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit())).ok_or("bad \\u escape")?;
        self.i += 4; Ok(u32::from_str_radix(h, 16).unwrap())
    }
    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut o: Vec<u8> = vec![];
        loop {
            let c = *self.b.get(self.i).ok_or("unterminated string")?; self.i += 1;
            match c {
                b'"' => return String::from_utf8(o).map_err(|_| "bad utf-8".into()),
                0..=0x1f => return Err("control character in string".into()),
                b'\\' => {
                    let e = *self.b.get(self.i).ok_or("unterminated string")?; self.i += 1;
                    let ch = match e {
                        b'"' => '"', b'\\' => '\\', b'/' => '/', b'b' => '\x08', b'f' => '\x0c', b'n' => '\n', b'r' => '\r', b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            if (0xd800..0xdc00).contains(&hi) && self.b[self.i..].starts_with(b"\\u") {
                                let save = self.i; self.i += 2;
                                match self.hex4() {
                                    Ok(lo) if (0xdc00..0xe000).contains(&lo) => char::from_u32(0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)).unwrap_or('\u{fffd}'),
                                    _ => { self.i = save; '\u{fffd}' }   // lone high surrogate; the next escape is read on its own
                                }
                            } else { char::from_u32(hi).unwrap_or('\u{fffd}') }   // lone surrogate -> U+FFFD
                        }
                        _ => return Err("bad escape".into()),
                    };
                    let mut b = [0u8; 4]; o.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                }
                _ => o.push(c),
            }
        }
    }
}

/// A JSON string literal for `s`. Non-ASCII passes through; control bytes and 0x7f become \u00XX.
pub fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2); o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), '\r' => o.push_str("\\r"), '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\x7f' => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"'); o
}

/// `j` as compact JSON text (parse(dump(j)) == j for finite numbers).
pub fn dump(j: &Json) -> String {
    match j {
        Json::Null => "null".into(), Json::Bool(b) => b.to_string(),
        Json::Num(n) => if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", *n as i64) } else { format!("{n}") },
        Json::Str(s) => quote(s),
        Json::Arr(a) => format!("[{}]", a.iter().map(dump).collect::<Vec<_>>().join(",")),
        Json::Obj(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{}:{}", quote(k), dump(v))).collect::<Vec<_>>().join(",")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn quote_round_trip() {
        for b in 0u8..0x80 { let s = (b as char).to_string(); assert_eq!(parse(&quote(&s)).unwrap(), Json::Str(s)); }
        let s = "a\"b\\c\n🐻é"; assert_eq!(parse(&quote(s)).unwrap().str(), Some(s));
    }
    #[test] fn dump_round_trip() {
        let t = r#"{"a":[1,-2,3.5,true,null,"x\"y"],"b":{},"c":[],"d":1700000000}"#;
        let j = parse(t).unwrap(); assert_eq!(dump(&j), t); assert_eq!(parse(&dump(&j)).unwrap(), j);
    }
    #[test] fn unicode() {
        assert_eq!(parse(r#""\ud83d\udc3b""#).unwrap().str(), Some("🐻"));
        assert_eq!(parse(r#""\ud83d""#).unwrap().str(), Some("\u{fffd}"));
        assert_eq!(parse(r#""\ud83dx""#).unwrap().str(), Some("\u{fffd}x"));
        assert_eq!(parse(r#""\ud83d\u0041""#).unwrap().str(), Some("\u{fffd}A"));
        assert_eq!(parse(r#""\udc3b""#).unwrap().str(), Some("\u{fffd}"));
    }
    #[test] fn lookup() {
        let j = parse(r#" {"a":{"b":[1,"x",{"c":true}]},"n":-1.5e1,"z":null} "#).unwrap();
        assert_eq!(j.get("a").unwrap().get("b").unwrap().idx(1).unwrap().str(), Some("x"));
        assert_eq!(j.get("a").unwrap().get("b").unwrap().idx(2).unwrap().get("c"), Some(&Json::Bool(true)));
        assert_eq!(j.get("n").unwrap().num(), Some(-15.0));
        assert_eq!(j.get("z"), Some(&Json::Null)); assert!(j.get("q").is_none());
    }
    #[test] fn rejects() {
        for bad in ["{} x", "\"abc", "\"a\nb\"", "[1,]", "{\"a\" 1}", "", "[", "tru", "\"\\q\""] { assert!(parse(bad).is_err(), "{bad:?}"); }
        let deep = |n: usize| "[".repeat(n) + &"]".repeat(n);
        assert!(parse(&deep(64)).is_ok()); assert!(parse(&deep(65)).is_err());
    }
}
