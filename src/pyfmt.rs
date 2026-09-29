//! Python semantics the stored and printed values depend on: string
//! splitting and stripping, lossy decoding, `repr` of floats and strings,
//! and `json.dumps` output. An index or a result that differs here
//! differs from one Python repoglass wrote.

pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}'
        | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// `str.splitlines()`: no line ends kept, `\r\n` is one break, and no
/// trailing empty line.
pub fn py_splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if is_line_break(c) {
            out.push(&s[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = it.peek() {
                    it.next();
                    next = j + 1;
                }
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `str.splitlines(keepends=True)`.
pub fn py_splitlines_keepends(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if is_line_break(c) {
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = it.peek() {
                    it.next();
                    next = j + 1;
                }
            }
            out.push(&s[start..next]);
            start = next;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `bytes.decode(errors="ignore")`.
pub fn decode_ignore(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
    }
    out
}

/// `Path.read_text(errors="ignore")`, which also translates `\r\n` and
/// `\r` to `\n`.
pub fn read_text(path: &std::path::Path) -> std::io::Result<String> {
    let raw = std::fs::read(path)?;
    let text = decode_ignore(&raw);
    if !text.contains('\r') {
        return Ok(text);
    }
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `s[:n]`, counted in code points.
pub fn char_prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `repr(float)`: the shortest digits that round-trip, scientific below
/// 1e-4 and from 1e16, a two-digit exponent, and `.0` on whole numbers.
pub fn float_repr(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let sci = format!("{x:e}");
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let (sign, mant) = mant.strip_prefix('-').map(|m| ("-", m)).unwrap_or(("", mant));
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    if (-4..16).contains(&exp) {
        let point = exp + 1;
        let body = if point <= 0 {
            format!("0.{}{}", "0".repeat((-point) as usize), digits)
        } else if point as usize >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
        } else {
            format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
        };
        return format!("{sign}{body}");
    }
    let m = if digits.len() == 1 { digits } else { format!("{}.{}", &digits[..1], &digits[1..]) };
    let esign = if exp < 0 { '-' } else { '+' };
    format!("{sign}{m}e{esign}{:02}", exp.abs())
}

/// `round(x, n)`.
pub fn round(x: f64, n: usize) -> f64 {
    format!("{x:.n$}").parse::<f64>().unwrap()
}

/// `repr(str)`: single quotes unless the text holds one and no double quote.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// A value as `json.dumps` renders it with default separators.
#[derive(Clone, Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn dumps(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => out.push_str(&i.to_string()),
            Json::Float(f) => out.push_str(&match float_repr(*f).as_str() {
                "nan" => "NaN".to_string(),
                "inf" => "Infinity".to_string(),
                "-inf" => "-Infinity".to_string(),
                s => s.to_string(),
            }),
            Json::Str(s) => out.push_str(&json_str(s)),
            Json::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&json_str(k));
                    out.push_str(": ");
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// `json.dumps` of a string: ASCII only, surrogate pairs above the BMP.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `difflib.get_close_matches(word, possibilities, n=1, cutoff)`.
pub fn close_match(word: &str, possibilities: &[String], cutoff: f64) -> Option<String> {
    let mut best: Option<(f64, &String)> = None;
    for p in possibilities {
        let m = SequenceMatcher::new(p, word);
        if m.real_quick_ratio() >= cutoff && m.quick_ratio() >= cutoff {
            let r = m.ratio();
            if r >= cutoff && best.is_none_or(|(s, b)| (r, p) > (s, b)) {
                best = Some((r, p));
            }
        }
    }
    best.map(|(_, p)| p.clone())
}

/// `difflib.SequenceMatcher(None, a, b)` with autojunk, for `ratio()`.
struct SequenceMatcher {
    a: Vec<char>,
    b: Vec<char>,
    b2j: std::collections::HashMap<char, Vec<usize>>,
}

impl SequenceMatcher {
    fn new(a: &str, b: &str) -> Self {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        let mut b2j: std::collections::HashMap<char, Vec<usize>> = Default::default();
        for (i, c) in b.iter().enumerate() {
            b2j.entry(*c).or_default().push(i);
        }
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, v| v.len() <= ntest);
        }
        Self { a, b, b2j }
    }

    fn longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: std::collections::HashMap<usize, usize> = Default::default();
        for i in alo..ahi {
            let mut newj2len: std::collections::HashMap<usize, usize> = Default::default();
            if let Some(js) = self.b2j.get(&self.a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        while besti > alo && bestj > blo && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi
            && self.a[besti + bestsize] == self.b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn matches(&self) -> usize {
        let mut queue = vec![(0, self.a.len(), 0, self.b.len())];
        let mut total = 0;
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                total += k;
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        total
    }

    fn calc(&self, matches: usize) -> f64 {
        let length = self.a.len() + self.b.len();
        if length == 0 { 1.0 } else { 2.0 * matches as f64 / length as f64 }
    }

    fn ratio(&self) -> f64 {
        self.calc(self.matches())
    }

    fn quick_ratio(&self) -> f64 {
        let mut avail: std::collections::HashMap<char, i64> = Default::default();
        for c in &self.b {
            *avail.entry(*c).or_default() += 1;
        }
        let mut matches = 0;
        for c in &self.a {
            let n = avail.entry(*c).or_default();
            if *n > 0 {
                matches += 1;
            }
            *n -= 1;
        }
        self.calc(matches)
    }

    fn real_quick_ratio(&self) -> f64 {
        self.calc(self.a.len().min(self.b.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_matches_python() {
        for (x, want) in [(0.3, "0.3"), (1.0, "1.0"), (1e16, "1e+16"), (0.0001, "0.0001"),
                          (0.00001, "1e-05"), (-7.8612, "-7.8612"), (123456789.0, "123456789.0")] {
            assert_eq!(float_repr(x), want);
        }
    }

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(py_splitlines("a\r\nb\rc\n\nd\n"), vec!["a", "b", "c", "", "d"]);
        assert_eq!(py_splitlines(""), Vec::<&str>::new());
    }

    #[test]
    fn close_match_suggests_like_difflib() {
        let have = vec!["python".to_string(), "rust".to_string(), "markdown".to_string()];
        assert_eq!(close_match("pyton", &have, 0.6).as_deref(), Some("python"));
        assert_eq!(close_match("zzz", &have, 0.6), None);
    }
}
