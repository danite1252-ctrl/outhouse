//! Reading Wowhead pages (zone, NPC and object pages) for loot lists.
//!
//! Wowhead puts its tables in the page as JavaScript, e.g.
//! `new Listview({template: 'item', id: 'drops', data: [{"id":17076, …}]})`
//! and item details (icons) in `WH.Gatherer.addData(3, 4, {…})`. That's
//! JavaScript, not JSON (unquoted and single-quoted keys, `WH.TERMS.drops`,
//! function calls, trailing commas), so this has its own small, forgiving
//! parser: anything it can't make sense of becomes `Null` instead of an
//! error. If even that fails, [`fallback_items`] scrapes `"id":NNN` and
//! names out of the drops table. Plain std code, no network.

use std::collections::HashMap;

/// A JavaScript value, loosely.
#[derive(Debug, Clone, PartialEq)]
pub enum Js {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Js>),
    Obj(Vec<(String, Js)>),
}

impl Js {
    pub fn get(&self, key: &str) -> Option<&Js> {
        match self {
            Js::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Js::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn num(&self) -> Option<f64> {
        match self {
            Js::Num(n) => Some(*n),
            Js::Str(s) => s.trim().parse().ok(),
            Js::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
    pub fn int(&self) -> Option<i64> {
        self.num()
            .filter(|n| n.is_finite() && n.fract() == 0.0)
            .map(|n| n as i64)
    }
    pub fn arr(&self) -> &[Js] {
        match self {
            Js::Arr(a) => a,
            _ => &[],
        }
    }
    fn truthy(&self) -> bool {
        match self {
            Js::Null => false,
            Js::Bool(b) => *b,
            Js::Num(n) => *n != 0.0,
            Js::Str(s) => !s.is_empty(),
            _ => true,
        }
    }
}

const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

fn ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.i += 1;
            } else if b == b'/' && self.s.get(self.i + 1) == Some(&b'/') {
                while let Some(b) = self.peek() {
                    if b == b'\n' {
                        break;
                    }
                    self.i += 1;
                }
            } else if b == b'/' && self.s.get(self.i + 1) == Some(&b'*') {
                self.i += 2;
                while self.i < self.s.len()
                    && !(self.s[self.i] == b'*' && self.s.get(self.i + 1) == Some(&b'/'))
                {
                    self.i += 1;
                }
                self.i = (self.i + 2).min(self.s.len());
            } else {
                break;
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        let quote = self.peek()?;
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        while let Some(b) = self.peek() {
            self.i += 1;
            if b == quote {
                return Some(String::from_utf8_lossy(&out).into_owned());
            }
            if b != b'\\' {
                out.push(b);
                continue;
            }
            let e = self.peek()?;
            self.i += 1;
            match e {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'b' | b'f' | b'v' | b'0' => {}
                b'u' => {
                    let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                    let mut code = u32::from_str_radix(hex, 16).ok()?;
                    self.i += 4;
                    // Surrogate pairs (😀).
                    if (0xD800..0xDC00).contains(&code)
                        && self.s.get(self.i) == Some(&b'\\')
                        && self.s.get(self.i + 1) == Some(&b'u')
                    {
                        if let Some(lo) = self
                            .s
                            .get(self.i + 2..self.i + 6)
                            .and_then(|h| std::str::from_utf8(h).ok())
                            .and_then(|h| u32::from_str_radix(h, 16).ok())
                        {
                            if (0xDC00..0xE000).contains(&lo) {
                                code = 0x10000 + ((code - 0xD800) << 10) + (lo - 0xDC00);
                                self.i += 6;
                            }
                        }
                    }
                    let c = char::from_u32(code).unwrap_or('\u{FFFD}');
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                b'x' => {
                    let hex = std::str::from_utf8(self.s.get(self.i..self.i + 2)?).ok()?;
                    let c = char::from_u32(u32::from_str_radix(hex, 16).ok()?)?;
                    self.i += 2;
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                b'\n' => {}
                other => out.push(other),
            }
        }
        None
    }

    /// Skips a bracketed run (`(…)`, `[…]`, `{…}`), minding strings.
    fn skip_balanced(&mut self) -> Option<()> {
        let mut stack: Vec<u8> = Vec::new();
        loop {
            self.skip_ws();
            let b = self.peek()?;
            match b {
                b'(' | b'[' | b'{' => {
                    stack.push(match b {
                        b'(' => b')',
                        b'[' => b']',
                        _ => b'}',
                    });
                    self.i += 1;
                }
                b')' | b']' | b'}' => {
                    if stack.pop() != Some(b) {
                        return None;
                    }
                    self.i += 1;
                    if stack.is_empty() {
                        return Some(());
                    }
                }
                b'"' | b'\'' | b'`' => {
                    self.string()?;
                }
                _ => {
                    if stack.is_empty() {
                        return None;
                    }
                    self.i += 1;
                }
            }
        }
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while self.peek().is_some_and(ident_byte) {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn value(&mut self) -> Option<Js> {
        let first = self.primary()?;
        let mut v = first;
        // Binary operators and ternaries (`WH.TERMS.a + ' ' + b`, `x || y`).
        loop {
            self.skip_ws();
            let Some(b) = self.peek() else { break };
            if !b"+-*|&?<>=!%".contains(&b) {
                break;
            }
            if b == b'=' && self.s.get(self.i + 1) == Some(&b'>') {
                // `=>` after a primary: an arrow function body.
                self.i += 2;
                self.skip_ws();
                if self.peek() == Some(b'{') {
                    self.skip_balanced()?;
                } else {
                    self.primary()?;
                }
                v = Js::Null;
                continue;
            }
            let start = self.i;
            while self.peek().is_some_and(|c| b"+-*|&?<>=!%".contains(&c)) {
                self.i += 1;
            }
            let op = std::str::from_utf8(&self.s[start..self.i])
                .unwrap_or("")
                .to_string();
            let rhs = self.primary()?;
            if op == "?" {
                self.skip_ws();
                if self.peek() == Some(b':') {
                    self.i += 1;
                    let alt = self.value()?;
                    v = if v.truthy() { rhs } else { alt };
                }
                continue;
            }
            v = match (op.as_str(), v, rhs) {
                ("+", Js::Str(a), Js::Str(b)) => Js::Str(a + &b),
                ("+", Js::Str(a), Js::Num(n)) => Js::Str(format!("{a}{n}")),
                ("+", Js::Num(a), Js::Num(b)) => Js::Num(a + b),
                ("-", Js::Num(a), Js::Num(b)) => Js::Num(a - b),
                ("*", Js::Num(a), Js::Num(b)) => Js::Num(a * b),
                ("||", a, b) => {
                    if a.truthy() {
                        a
                    } else {
                        b
                    }
                }
                (_, a, _) => a,
            };
        }
        Some(v)
    }

    fn primary(&mut self) -> Option<Js> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        self.skip_ws();
        let out = match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' | b'\'' | b'`' => self.string().map(Js::Str),
            b'(' => {
                // A bracketed expression or an arrow function's arguments.
                let save = self.i;
                self.i += 1;
                match self.value() {
                    Some(v) => {
                        self.skip_ws();
                        if self.peek() == Some(b')') {
                            self.i += 1;
                            Some(v)
                        } else {
                            self.i = save;
                            self.skip_balanced().map(|_| Js::Null)
                        }
                    }
                    None => {
                        self.i = save;
                        self.skip_balanced().map(|_| Js::Null)
                    }
                }
            }
            b'!' => {
                self.i += 1;
                self.primary().map(|v| Js::Bool(!v.truthy()))
            }
            b'-' | b'+' => {
                let neg = self.peek() == Some(b'-');
                self.i += 1;
                match self.primary()? {
                    Js::Num(n) => Some(Js::Num(if neg { -n } else { n })),
                    _ => Some(Js::Null),
                }
            }
            b'0'..=b'9' | b'.' => {
                let start = self.i;
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'.')
                {
                    self.i += 1;
                }
                // 1e-5
                if matches!(self.s.get(self.i.wrapping_sub(1)), Some(b'e' | b'E'))
                    && matches!(self.peek(), Some(b'-' | b'+'))
                {
                    self.i += 1;
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.i += 1;
                    }
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
                let n = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                    i64::from_str_radix(h, 16).ok().map(|n| n as f64)
                } else {
                    t.parse::<f64>().ok()
                };
                Some(n.map(Js::Num).unwrap_or(Js::Null))
            }
            b if ident_byte(b) => {
                let word = self.ident();
                match word.as_str() {
                    "true" => Some(Js::Bool(true)),
                    "false" => Some(Js::Bool(false)),
                    "null" | "undefined" | "NaN" => Some(Js::Null),
                    "new" | "typeof" | "void" => self.primary().map(|_| Js::Null),
                    "function" => {
                        self.skip_ws();
                        self.ident();
                        self.skip_ws();
                        self.skip_balanced()?; // (args)
                        self.skip_ws();
                        self.skip_balanced()?; // {body}
                        Some(Js::Null)
                    }
                    _ => {
                        // A name, maybe with .members, [index] and (calls).
                        loop {
                            self.skip_ws();
                            match self.peek() {
                                Some(b'.') => {
                                    self.i += 1;
                                    self.skip_ws();
                                    self.ident();
                                }
                                Some(b'(') | Some(b'[') => {
                                    self.skip_balanced()?;
                                }
                                _ => break,
                            }
                        }
                        Some(Js::Null)
                    }
                }
            }
            _ => None,
        };
        self.depth -= 1;
        out
    }

    fn object(&mut self) -> Option<Js> {
        self.i += 1;
        let mut kv = Vec::new();
        loop {
            self.skip_ws();
            match self.peek()? {
                b'}' => {
                    self.i += 1;
                    return Some(Js::Obj(kv));
                }
                b',' => {
                    self.i += 1;
                    continue;
                }
                b'"' | b'\'' => {
                    let k = self.string()?;
                    kv.push((k, Js::Null));
                }
                b'[' => {
                    // Computed key.
                    self.skip_balanced()?;
                    kv.push((String::new(), Js::Null));
                }
                b if ident_byte(b) => {
                    let k = self.ident();
                    kv.push((k, Js::Null));
                }
                _ => return None,
            }
            self.skip_ws();
            match self.peek()? {
                b':' => {
                    self.i += 1;
                    let v = self.value()?;
                    if let Some(last) = kv.last_mut() {
                        last.1 = v;
                    }
                }
                b'(' => {
                    // A method: name(args) { body }
                    self.skip_balanced()?;
                    self.skip_ws();
                    self.skip_balanced()?;
                }
                // Shorthand `{ a, b }`.
                b',' | b'}' => {}
                _ => return None,
            }
        }
    }

    fn array(&mut self) -> Option<Js> {
        self.i += 1;
        let mut out = Vec::new();
        let mut need_value = true;
        loop {
            self.skip_ws();
            match self.peek()? {
                b']' => {
                    self.i += 1;
                    return Some(Js::Arr(out));
                }
                b',' => {
                    if need_value {
                        out.push(Js::Null);
                    }
                    need_value = true;
                    self.i += 1;
                }
                _ => {
                    out.push(self.value()?);
                    need_value = false;
                }
            }
        }
    }
}

/// Parses the JavaScript value starting at byte `at` (`{`, `[`, …).
pub fn parse_at(src: &str, at: usize) -> Option<(Js, usize)> {
    let mut p = Parser {
        s: src.as_bytes(),
        i: at,
        depth: 0,
    };
    let v = p.value()?;
    Some((v, p.i))
}

/// Parses a whole JavaScript value.
pub fn parse(src: &str) -> Option<Js> {
    parse_at(src, 0).map(|(v, _)| v)
}

fn find_all<'a>(hay: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    hay.match_indices(needle).map(|(i, _)| i + needle.len())
}

/// Every `new Listview({...})` on the page that could be read.
pub fn listviews(html: &str) -> Vec<Js> {
    let mut out = Vec::new();
    for at in find_all(html, "new Listview(") {
        if let Some((v @ Js::Obj(_), _)) = parse_at(html, at) {
            out.push(v);
        }
    }
    out
}

/// Item details from `WH.Gatherer.addData(3, env, {"17076": {...}})`
/// (and the older `g_items.add(17076, {...})`): ID → (name, quality, icon).
pub fn gatherer_items(html: &str) -> HashMap<i64, (String, Option<i64>, String)> {
    let mut out = HashMap::new();
    for at in find_all(html, "WH.Gatherer.addData(") {
        let mut p = Parser {
            s: html.as_bytes(),
            i: at,
            depth: 0,
        };
        let Some(kind) = p.value() else { continue };
        if kind.int() != Some(3) {
            continue;
        }
        p.skip_ws();
        if p.peek() != Some(b',') {
            continue;
        }
        p.i += 1;
        if p.value().is_none() {
            continue;
        }
        p.skip_ws();
        if p.peek() != Some(b',') {
            continue;
        }
        p.i += 1;
        let Some(Js::Obj(items)) = p.value() else {
            continue;
        };
        for (k, v) in items {
            let Ok(id) = k.trim().parse::<i64>() else {
                continue;
            };
            let name = v
                .get("name_enus")
                .or_else(|| v.get("name"))
                .and_then(Js::str)
                .unwrap_or("")
                .to_string();
            let quality = v.get("quality").and_then(Js::int);
            let icon = v.get("icon").and_then(Js::str).unwrap_or("").to_string();
            out.insert(id, (name, quality, icon));
        }
    }
    for at in find_all(html, "g_items.add(") {
        let mut p = Parser {
            s: html.as_bytes(),
            i: at,
            depth: 0,
        };
        let Some(id) = p.value().and_then(|v| v.int()) else {
            continue;
        };
        p.skip_ws();
        if p.peek() != Some(b',') {
            continue;
        }
        p.i += 1;
        if let Some(v) = p.value() {
            let entry = out
                .entry(id)
                .or_insert((String::new(), None, String::new()));
            if let Some(icon) = v.get("icon").and_then(Js::str) {
                entry.2 = icon.to_string();
            }
            if entry.0.is_empty() {
                if let Some(n) = v.get("name_enus").and_then(Js::str) {
                    entry.0 = n.to_string();
                }
            }
            if entry.1.is_none() {
                entry.1 = v.get("quality").and_then(Js::int);
            }
        }
    }
    out
}

/// The page's `<title>` up to the first " - " (e.g. "Molten Core"), raw.
pub fn page_title(html: &str) -> String {
    let lower = html.get(..html.len().min(200_000)).unwrap_or("");
    let Some(start) = lower.find("<title>") else {
        return String::new();
    };
    let rest = &lower[start + 7..];
    let end = rest.find("</title>").unwrap_or(rest.len().min(300));
    let t = rest[..end].trim();
    t.split(" - ").next().unwrap_or(t).trim().to_string()
}

/// One item that drops on a page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Drop {
    pub item_id: i32,
    pub name: String,
    pub quality: i32,
    pub icon: String,
    pub item_class: i32,
    pub subclass: i32,
    pub slot: i32,
    pub req_class: i32,
    /// Who drops it (a boss, a chest, or "" for trash/unknown).
    pub boss: String,
    pub boss_id: i32,
    /// In percent.
    pub chance: Option<f32>,
    /// Not from a boss (the page told us who the bosses are).
    pub trash: bool,
}

/// What a page holds.
#[derive(Debug, Clone, Default)]
pub struct Page {
    pub title: String,
    /// Any `Listview` at all (a page with none isn't a real Wowhead page).
    pub has_listviews: bool,
    /// Bosses listed on a zone page: (NPC ID, name).
    pub bosses: Vec<(i32, String)>,
    pub drops: Vec<Drop>,
    /// The drops table had who-drops-it for (nearly) every item.
    pub has_sources: bool,
}

fn id_of(v: &Js) -> Option<String> {
    match v.get("id")? {
        Js::Str(s) => Some(s.clone()),
        Js::Num(_) => None,
        _ => None,
    }
}

const DROP_TABLES: &[&str] = &["drops", "contains", "loot", "normal-drops", "drops-normal"];

/// "3Bonereaver's Edge" (old Wowhead puts 7 − quality in front).
fn split_quality_prefix(name: &str, quality: Option<i64>) -> (String, Option<i64>) {
    let mut c = name.chars();
    match (c.next(), c.clone().next()) {
        (Some(d @ '0'..='7'), Some(n)) if !n.is_ascii_digit() => {
            let q = 7 - d.to_digit(10).unwrap_or(0) as i64;
            if quality.is_none() || quality == Some(q) {
                return (c.as_str().to_string(), Some(q));
            }
            (name.to_string(), quality)
        }
        _ => (name.to_string(), quality),
    }
}

/// Reads a Wowhead zone, NPC or object page.
pub fn read_page(html: &str) -> Page {
    let views = listviews(html);
    let gatherer = gatherer_items(html);
    let mut page = Page {
        title: page_title(html),
        has_listviews: !views.is_empty() || html.contains("new Listview("),
        ..Default::default()
    };
    // Bosses (zone pages).
    for v in &views {
        if v.get("template").and_then(Js::str) != Some("npc") {
            continue;
        }
        for row in v.get("data").map(Js::arr).unwrap_or(&[]) {
            let boss = row.get("boss").is_some_and(Js::truthy)
                || row.get("classification").and_then(Js::int) == Some(3);
            if !boss {
                continue;
            }
            let (Some(id), Some(name)) = (
                row.get("id").and_then(Js::int),
                row.get("name").and_then(Js::str),
            ) else {
                continue;
            };
            if !page.bosses.iter().any(|b| b.0 as i64 == id) {
                page.bosses.push((id as i32, name.to_string()));
            }
        }
    }
    // The drops table: the first of these that's there.
    let table = DROP_TABLES.iter().find_map(|want| {
        views.iter().find(|v| {
            v.get("template").and_then(Js::str) == Some("item")
                && id_of(v).as_deref() == Some(*want)
        })
    });
    let Some(table) = table else {
        if page.drops.is_empty() {
            page.drops = fallback_items(html, &gatherer);
        }
        return page;
    };
    let boss_names: Vec<&str> = page.bosses.iter().map(|b| b.1.as_str()).collect();
    let mut with_source = 0;
    for row in table.get("data").map(Js::arr).unwrap_or(&[]) {
        let Some(id) = row
            .get("id")
            .and_then(Js::int)
            .filter(|i| (1..=10_000_000).contains(i))
        else {
            continue;
        };
        if page.drops.iter().any(|d| d.item_id as i64 == id) {
            continue;
        }
        let g = gatherer.get(&id);
        let raw_name = row
            .get("name")
            .and_then(Js::str)
            .map(str::to_string)
            .or_else(|| g.map(|g| g.0.clone()))
            .unwrap_or_default();
        let (name, quality) = split_quality_prefix(&raw_name, row.get("quality").and_then(Js::int));
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let quality = quality
            .or_else(|| g.and_then(|g| g.1))
            .unwrap_or(1)
            .clamp(0, 7);
        let int = |k: &str| row.get(k).and_then(Js::int);
        let chance = match (
            row.get("percent").and_then(Js::num),
            int("count"),
            int("outof"),
        ) {
            (Some(p), _, _) if p > 0.0 => Some(p),
            (_, Some(c), Some(o)) if c > 0 && o > 0 => Some(c as f64 * 100.0 / o as f64),
            _ => None,
        }
        .map(|p| ((p.min(100.0) * 100.0).round() / 100.0) as f32);
        // Who drops it: sourcemore [{n: "Ragnaros", t: 1, ti: 11502}].
        let sources: Vec<(String, i64, i64)> = row
            .get("sourcemore")
            .map(Js::arr)
            .unwrap_or(&[])
            .iter()
            .filter_map(|s| {
                let n = s.get("n").and_then(Js::str)?.to_string();
                Some((
                    n,
                    s.get("t").and_then(Js::int).unwrap_or(0),
                    s.get("ti").and_then(Js::int).unwrap_or(0),
                ))
            })
            .filter(|s| s.1 == 1 || s.1 == 2 || s.1 == 0)
            .collect();
        if !sources.is_empty() {
            with_source += 1;
        }
        let pick = sources
            .iter()
            .find(|s| boss_names.iter().any(|b| b.eq_ignore_ascii_case(&s.0)))
            .or_else(|| sources.iter().find(|s| s.1 == 2))
            .or(sources.first());
        let (boss, boss_id) = pick.map(|s| (s.0.clone(), s.2 as i32)).unwrap_or_default();
        let trash = boss.is_empty()
            || (!boss_names.is_empty()
                && pick.is_some_and(|s| s.1 == 1)
                && !boss_names.iter().any(|b| b.eq_ignore_ascii_case(&boss)));
        let icon = g
            .map(|g| g.2.clone())
            .filter(|i| !i.is_empty())
            .or_else(|| row.get("icon").and_then(Js::str).map(str::to_string))
            .unwrap_or_default();
        page.drops.push(Drop {
            item_id: id as i32,
            name,
            quality: quality as i32,
            icon: clean_icon(&icon),
            item_class: int("classs").or_else(|| int("class")).unwrap_or(-1) as i32,
            subclass: int("subclass").unwrap_or(-1) as i32,
            slot: int("slot").or_else(|| int("slotbak")).unwrap_or(0) as i32,
            req_class: int("reqclass").unwrap_or(0) as i32,
            boss: if trash && pick.is_some_and(|s| s.1 == 1) {
                String::new()
            } else {
                boss
            },
            boss_id: if trash { 0 } else { boss_id },
            chance,
            trash,
        });
    }
    page.has_sources = !page.drops.is_empty() && with_source * 10 >= page.drops.len() * 8;
    if page.drops.is_empty() {
        page.drops = fallback_items(html, &gatherer);
    }
    page
}

/// Icon names are file names: letters, digits, `_` and `-`.
pub fn clean_icon(icon: &str) -> String {
    icon.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(80)
        .collect()
}

/// Last resort: `"id":17076 … "name":"…"` pairs inside the drops table.
pub fn fallback_items(
    html: &str,
    gatherer: &HashMap<i64, (String, Option<i64>, String)>,
) -> Vec<Drop> {
    let start = [
        "id: 'drops'",
        "id:'drops'",
        "id: \"drops\"",
        "\"id\":\"drops\"",
        "id: 'contains'",
    ]
    .iter()
    .filter_map(|m| html.find(m))
    .min();
    let Some(start) = start else {
        return Vec::new();
    };
    let region = &html[start..];
    let region = &region[..region.find("});").unwrap_or(region.len()).min(4_000_000)];
    let mut out: Vec<Drop> = Vec::new();
    let mut rest = region;
    while let Some(pos) = rest.find("\"id\":") {
        let after = &rest[pos + 5..];
        let digits: String = after
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .take(9)
            .collect();
        rest = after;
        let Ok(id) = digits.parse::<i64>() else {
            continue;
        };
        if id <= 0 || out.iter().any(|d| d.item_id as i64 == id) {
            continue;
        }
        // The name, nearby in the same row.
        let window = &rest[..rest.len().min(600)];
        let window = &window[..window.find("\"id\":").unwrap_or(window.len())];
        let name = window.find("\"name\":\"").and_then(|n| {
            // Read it as a JS string, from its opening quote.
            let mut p = Parser {
                s: window[n + 7..].as_bytes(),
                i: 0,
                depth: 0,
            };
            p.string()
        });
        let g = gatherer.get(&id);
        let Some(raw) = name
            .or_else(|| g.map(|g| g.0.clone()))
            .filter(|n| !n.trim().is_empty())
        else {
            continue;
        };
        let quality_near = window
            .find("\"quality\":")
            .and_then(|q| window[q + 10..].trim_start().chars().next())
            .and_then(|c| c.to_digit(10))
            .map(|d| d as i64);
        let (name, quality) = split_quality_prefix(&raw, quality_near);
        out.push(Drop {
            item_id: id as i32,
            name: name.trim().to_string(),
            quality: quality
                .or_else(|| g.and_then(|g| g.1))
                .unwrap_or(1)
                .clamp(0, 7) as i32,
            icon: g.map(|g| clean_icon(&g.2)).unwrap_or_default(),
            item_class: -1,
            subclass: -1,
            trash: false,
            ..Default::default()
        });
        if out.len() >= 1000 {
            break;
        }
    }
    out
}

/// Decodes the HTML entities Wowhead uses.
pub fn decode_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest[..rest.len().min(12)].find(';');
        let Some(end) = end else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One tooltip row: (left, right, colour class).
pub type TipRow = (String, String, String);

fn q_class(tag: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find("class=")?;
    let rest = &tag[at + 6..];
    let quote = rest.chars().next()?;
    let body = if quote == '"' || quote == '\'' {
        let r = &rest[1..];
        &r[..r.find(quote).unwrap_or(r.len())]
    } else {
        &rest[..rest.find([' ', '>']).unwrap_or(rest.len())]
    };
    body.split_whitespace()
        .find(|c| {
            let c = c.as_bytes();
            c[0] == b'q' && (c.len() == 1 || (c.len() == 2 && c[1].is_ascii_digit()))
        })
        .map(str::to_string)
}

/// Wowhead's tooltip HTML as rows, keeping its colours and the right-hand
/// column ("Two-Hand" … "Sword", "44 - 115 Damage" … "Speed 1.90").
pub fn tooltip_rows(html: &str, name: &str) -> Vec<TipRow> {
    // Drop comments (`<!--ilvl-->`).
    let mut s = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        s.push_str(&rest[..start]);
        rest = match rest[start..].find("-->") {
            Some(end) => &rest[start + end + 3..],
            None => "",
        };
    }
    s.push_str(rest);

    let mut rows: Vec<TipRow> = Vec::new();
    let mut cur: TipRow = Default::default();
    let mut right = false;
    let mut stack: Vec<(String, Option<String>)> = Vec::new();
    let flush = |cur: &mut TipRow, rows: &mut Vec<TipRow>| {
        let l = cur.0.split_whitespace().collect::<Vec<_>>().join(" ");
        let r = cur.1.split_whitespace().collect::<Vec<_>>().join(" ");
        if !l.is_empty() || !r.is_empty() {
            rows.push((l, r, std::mem::take(&mut cur.2)));
        }
        *cur = Default::default();
    };
    let mut rest = s.as_str();
    while !rest.is_empty() {
        let lt = rest.find('<').unwrap_or(rest.len());
        let text = decode_html(&rest[..lt]);
        if !text.trim().is_empty() {
            if cur.0.trim().is_empty() && cur.1.trim().is_empty() && cur.2.is_empty() {
                cur.2 = stack
                    .iter()
                    .rev()
                    .find_map(|(_, q)| q.clone())
                    .unwrap_or_default();
            }
            if right {
                cur.1.push_str(&text);
            } else {
                cur.0.push_str(&text);
            }
        } else if !text.is_empty() {
            if right {
                cur.1.push(' ');
            } else {
                cur.0.push(' ');
            }
        }
        rest = &rest[lt..];
        if rest.is_empty() {
            break;
        }
        let gt = rest.find('>').unwrap_or(rest.len() - 1);
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let closing = tag.starts_with('/');
        let tname: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match tname.as_str() {
            "br" => {
                flush(&mut cur, &mut rows);
                continue;
            }
            "table" | "tr" | "div" | "p" | "li" => flush(&mut cur, &mut rows),
            "th" => right = !closing,
            "td" if !closing => right = false,
            _ => {}
        }
        if matches!(tname.as_str(), "img" | "input" | "hr" | "meta" | "link") || tag.ends_with('/')
        {
            continue;
        }
        if closing {
            if let Some(pos) = stack.iter().rposition(|(n, _)| *n == tname) {
                stack.truncate(pos);
            }
        } else if !tname.is_empty() {
            stack.push((tname, q_class(tag)));
            if stack.len() > 64 {
                stack.remove(0);
            }
        }
    }
    flush(&mut cur, &mut rows);
    let mut out: Vec<TipRow> = Vec::new();
    for r in rows {
        if out.is_empty() && r.1.is_empty() && r.0 == name {
            continue;
        }
        if out.last() == Some(&r) {
            continue;
        }
        let clip = |s: String| s.chars().take(200).collect::<String>();
        out.push((clip(r.0), clip(r.1), r.2));
        if out.len() >= 30 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_javascript_objects() {
        let v = parse(r#"{template: 'item', id: "drops", name: WH.TERMS.drops, tabs: tabsRelated, parent: 'lv-x', extraCols: [Listview.extraCols.count, Listview.extraCols.percent], sort: ['-percent', 'name'], computeDataFunc: Listview.funcBox.initLootTable, onAfterCreate: function(a) { return a && {x: '}'}; }, note: WH.sprintf(WH.TERMS.x, '<a href="/y">z</a>'), data: [{"id":17076,"name":"Bonereaver's Edge",count:12,outof:100,'quality':4,}, {id: 18814, name: 'Choker of the Fire Lord', quality: 4, modes: {"mode":1}},],}"#).unwrap();
        assert_eq!(v.get("template").and_then(Js::str), Some("item"));
        assert_eq!(v.get("id").and_then(Js::str), Some("drops"));
        let data = v.get("data").unwrap().arr();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].get("count").and_then(Js::int), Some(12));
        assert_eq!(
            data[1].get("name").and_then(Js::str),
            Some("Choker of the Fire Lord")
        );
        assert_eq!(parse(r#"'it\'s é \x41'"#), Some(Js::Str("it's é A".into())));
        assert_eq!(
            parse("[1,,2,]"),
            Some(Js::Arr(vec![Js::Num(1.0), Js::Null, Js::Num(2.0)]))
        );
        assert_eq!(parse("-1.5e2"), Some(Js::Num(-150.0)));
        assert_eq!(parse("'a' + \"b\""), Some(Js::Str("ab".into())));
        assert_eq!(
            parse("{a: (x) => { return 1; }, b: 2}")
                .unwrap()
                .get("b")
                .and_then(Js::int),
            Some(2)
        );
        assert_eq!(
            parse("{/* c */ a: 1 // d\n , b: !0}").unwrap().get("b"),
            Some(&Js::Bool(true))
        );
        assert!(parse("{a: ").is_none());
    }

    const PAGE: &str = r#"<html><head><title>Molten Core - Zone - World of Warcraft Classic</title></head><body>
<script>
WH.Gatherer.addData(3, 4, {"17076":{"name_enus":"Bonereaver's Edge","quality":4,"icon":"INV_Sword_12","jsonequip":{}},"18814":{"name_enus":"Choker of the Fire Lord","quality":4,"icon":"inv_jewelry_necklace_17"}});
var tabsRelated = new Tabs({parent: WH.ge('tabs-generic')});
new Listview({template: 'npc', id: 'npcs', name: WH.TERMS.npcs, tabs: tabsRelated, data: [{"boss":1,"classification":3,"id":11502,"name":"Ragnaros","react":[-1,-1]},{"classification":1,"id":11659,"name":"Molten Destroyer"},{"boss":1,"id":12056,"name":"Baron Geddon"}]});
new Listview({template: 'item', id: 'drops', name: WH.TERMS.drops, tabs: tabsRelated, parent: 'lkljbjkb574', extraCols: [Listview.extraCols.count, Listview.extraCols.percent], sort:['-percent', 'name'], data: [
 {"classs":2,"id":17076,"level":78,"name":"Bonereaver's Edge","quality":4,"reqlevel":60,"slot":17,"source":[2],"sourcemore":[{"n":"Ragnaros","t":1,"ti":11502,"z":2717}],"subclass":8,count:12,outof:100},
 {"classs":4,"id":18814,"name":"3Choker of the Fire Lord","slot":2,"subclass":0,"sourcemore":[{"n":"Ragnaros","t":1,"ti":11502}],percent:15.5},
 {"classs":4,"id":18563,"name":"Bindings of the Windseeker","quality":5,"slot":0,"sourcemore":[{"n":"Baron Geddon","t":1,"ti":12056}]},
 {"classs":2,"id":17010,"name":"Fiery Core","quality":3,"sourcemore":[{"n":"Molten Destroyer","t":1,"ti":11659}]},
 {"classs":15,"id":19017,"name":"Essence of the Firelord","quality":5,"sourcemore":[{"n":"Cache of the Firelord","t":2,"ti":179703}]},
]});
</script></body></html>"#;

    #[test]
    fn reads_zone_pages() {
        let page = read_page(PAGE);
        assert_eq!(page.title, "Molten Core");
        assert!(page.has_listviews);
        assert_eq!(
            page.bosses,
            vec![(11502, "Ragnaros".into()), (12056, "Baron Geddon".into())]
        );
        assert_eq!(page.drops.len(), 5);
        assert!(page.has_sources);
        let be = &page.drops[0];
        assert_eq!(
            (be.item_id, be.quality, be.slot, be.item_class, be.subclass),
            (17076, 4, 17, 2, 8)
        );
        assert_eq!(be.icon, "inv_sword_12");
        assert_eq!(be.boss, "Ragnaros");
        assert_eq!(be.chance, Some(12.0));
        let choker = &page.drops[1];
        assert_eq!(
            (choker.name.as_str(), choker.quality, choker.chance),
            ("Choker of the Fire Lord", 4, Some(15.5))
        );
        assert_eq!(page.drops[2].boss, "Baron Geddon");
        // A trash mob's drop.
        assert!(page.drops[3].trash && page.drops[3].boss.is_empty());
        // A chest counts as who drops it.
        assert_eq!(page.drops[4].boss, "Cache of the Firelord");
        assert!(!page.drops[4].trash);
    }

    #[test]
    fn scrapes_when_the_table_is_odd() {
        let html = r#"<title>Odd - NPC</title><script>new Listview({template: 'item', id: 'drops', data: [{"id":19019,"name":"2Thunderfury, Blessed Blade",count:1 + }, {"id":17204,"name":"Eye of Sulfuras","quality":5}]});</script>"#;
        let page = read_page(html);
        assert!(page.has_listviews);
        let ids: Vec<i32> = page.drops.iter().map(|d| d.item_id).collect();
        assert_eq!(ids, vec![19019, 17204]);
        assert_eq!(page.drops[0].quality, 5);
        assert_eq!(page.drops[0].name, "Thunderfury, Blessed Blade");
        assert_eq!(page.drops[1].quality, 5);
        assert!(read_page("<html>nothing here</html>").drops.is_empty());
    }

    #[test]
    fn tooltip_rows_keep_colours_and_columns() {
        let html = r#"<table><tr><td><!--nstart--><b class="q5">Thunderfury, Blessed Blade of the Windseeker</b><!--nend--><br><!--bo-->Binds when picked up<br>Unique<table width="100%"><tr><td>One-Hand</td><th>Sword</th></tr></table><table width="100%"><tr><td><span><!--dmg-->44 - 115 Damage</span></td><th>Speed <!--spd-->1.90</th></tr></table>(41.6 damage per second)<br>+5 Agility<br>Item Level <!--ilvl-->80<br>Requires Level 60</td></tr></table><table><tr><td><span class="q2">Chance on hit: Blasts your enemy &amp; slows them.</span><br><span class="q">Classes: Priest</span></td></tr></table>"#;
        let rows = tooltip_rows(html, "Thunderfury, Blessed Blade of the Windseeker");
        assert_eq!(
            rows[0],
            ("Binds when picked up".into(), String::new(), String::new())
        );
        assert_eq!(rows[2], ("One-Hand".into(), "Sword".into(), String::new()));
        assert_eq!(
            rows[3],
            ("44 - 115 Damage".into(), "Speed 1.90".into(), String::new())
        );
        assert!(rows.contains(&(
            "Chance on hit: Blasts your enemy & slows them.".into(),
            String::new(),
            "q2".into()
        )));
        assert!(rows.contains(&("Classes: Priest".into(), String::new(), "q".into())));
        assert!(rows.iter().all(|r| !r.0.contains('<')));
        assert_eq!(
            decode_html("a &amp; b &#39;c&#x27; &bogus"),
            "a & b 'c' &bogus"
        );
    }
}
