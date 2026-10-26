//! Entity selectors and names (§ 2.5): reading, options, suggestions according to position.

use hashbrown::HashSet;

use crate::reader::{Reader, read_id};
use crate::suggest::{Builder, Tooltip};
use crate::{CmdError, Data};

const ENTITY_TYPE: &str = "minecraft:entity_type";

/// What to suggest at the place where reading stopped.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Sugg {
    NameOrSelector,
    Name,
    Selector,
    OpenOptions,
    KeyOrClose,
    Key,
    NextOrClose,
    Nothing,
    Sort,
    Gamemode,
    Type,
}

/// Option with an invertible set (name, game mode, team, type).
#[derive(Default, Clone, Copy, PartialEq)]
enum Lim {
    #[default]
    None,
    Single,
    Multiple,
}

#[derive(Default)]
struct Inv {
    lim: Lim,
    tags: HashSet<String>,
}

impl Inv {
    fn pos_ok(&self) -> bool {
        self.lim == Lim::None
    }
    fn neg_ok(&self) -> bool {
        self.lim != Lim::Single
    }
    fn any(&self) -> bool {
        self.lim != Lim::Single
    }
    fn elem_ok(&self, inverted: bool) -> bool {
        if inverted { self.neg_ok() } else { self.pos_ok() }
    }
    fn mark(&mut self, inverted: bool) {
        self.lim = if inverted { Lim::Multiple } else { Lim::Single };
    }
}

/// Result useful to argument checks.
#[derive(Debug, Clone, Copy)]
pub struct Selector {
    pub max: i64,
    pub includes_entities: bool,
    pub current: bool,
}

pub struct SelParser<'a, 'r> {
    pub r: &'r mut Reader<'a>,
    allow: bool,
    data: &'r Data,
    sugg: Sugg,
    start: usize,
    max: i64,
    includes_entities: bool,
    current: bool,
    name: Inv,
    gamemode: Inv,
    team: Inv,
    ty: Inv,
    once: HashSet<&'static str>,
}

const OPTIONS: [&str; 20] = [
    "name",
    "distance",
    "level",
    "x",
    "y",
    "z",
    "dx",
    "dy",
    "dz",
    "x_rotation",
    "y_rotation",
    "limit",
    "sort",
    "gamemode",
    "team",
    "type",
    "tag",
    "nbt",
    "scores",
    "advancements",
];
const PREDICATE: &str = "predicate";

impl<'a, 'r> SelParser<'a, 'r> {
    pub fn new(r: &'r mut Reader<'a>, allow: bool, data: &'r Data) -> Self {
        SelParser {
            r,
            allow,
            data,
            sugg: Sugg::NameOrSelector,
            start: 0,
            max: 0,
            includes_entities: false,
            current: false,
            name: Inv::default(),
            gamemode: Inv::default(),
            ty: Inv::default(),
            team: Inv::default(),
            once: HashSet::new(),
        }
    }

    pub fn parse(&mut self) -> Result<Selector, CmdError> {
        self.start = self.r.cursor;
        self.sugg = Sugg::NameOrSelector;
        if self.r.next_is('@') {
            if !self.allow {
                return Err(self.r.err0("argument.entity.selector.not_allowed"));
            }
            self.r.skip();
            self.parse_selector()?;
        } else {
            self.parse_name()?;
        }
        Ok(Selector { max: self.max, includes_entities: self.includes_entities, current: self.current })
    }

    fn parse_selector(&mut self) -> Result<(), CmdError> {
        self.sugg = Sugg::Selector;
        if !self.r.can_read() {
            return Err(self.r.err0("argument.entity.selector.missing"));
        }
        let start = self.r.cursor;
        let t = self.r.read();
        let (max, ent, cur) = match t {
            'a' => (i64::from(i32::MAX), false, false),
            'e' => (i64::from(i32::MAX), true, false),
            'n' => (1, true, false),
            'p' | 'r' => (1, false, false),
            's' => (1, true, true),
            _ => {
                self.r.cursor = start;
                return Err(self.r.err("argument.entity.selector.unknown", vec![format!("@{t}")]));
            }
        };
        self.max = max;
        self.includes_entities = ent;
        self.current = cur;
        self.sugg = Sugg::OpenOptions;
        if self.r.next_is('[') {
            self.r.skip();
            self.sugg = Sugg::KeyOrClose;
            self.parse_options()?;
        }
        Ok(())
    }

    fn parse_name(&mut self) -> Result<(), CmdError> {
        if self.r.can_read() {
            self.sugg = Sugg::Name;
        }
        let start = self.r.cursor;
        let name = self.r.read_string()?;
        if is_uuid(&name) {
            self.includes_entities = true;
        } else {
            if name.is_empty() || name.chars().count() > 16 {
                self.r.cursor = start;
                return Err(self.r.err0("argument.entity.invalid"));
            }
            self.includes_entities = false;
        }
        self.max = 1;
        Ok(())
    }

    fn can_use(&self, key: &str) -> bool {
        match key {
            "name" => self.name.any(),
            "limit" | "sort" => !self.current && !self.once.contains(key),
            "gamemode" => self.gamemode.any(),
            "team" => self.team.any(),
            "type" => self.ty.any(),
            "tag" | "nbt" | PREDICATE => true,
            _ => !self.once.contains(key),
        }
    }

    fn parse_options(&mut self) -> Result<(), CmdError> {
        self.sugg = Sugg::Key;
        self.r.skip_whitespace();
        while self.r.can_read() && self.r.peek() != ']' {
            self.r.skip_whitespace();
            let start = self.r.cursor;
            let key = self.r.read_string()?;
            let known = OPTIONS.contains(&key.as_str()) || key == PREDICATE;
            if !known {
                self.r.cursor = start;
                return Err(self.r.err("argument.entity.options.unknown", vec![key]));
            }
            if !self.can_use(&key) {
                self.r.cursor = start;
                return Err(self.r.err("argument.entity.options.inapplicable", vec![key]));
            }
            self.r.skip_whitespace();
            if !self.r.next_is('=') {
                self.r.cursor = start;
                return Err(self.r.err("argument.entity.options.valueless", vec![key]));
            }
            self.r.skip();
            self.r.skip_whitespace();
            self.sugg = Sugg::Nothing;
            self.option(&key)?;
            self.r.skip_whitespace();
            self.sugg = Sugg::NextOrClose;
            if self.r.can_read() {
                if self.r.peek() != ',' {
                    if self.r.peek() != ']' {
                        return Err(self.r.err0("argument.entity.options.unterminated"));
                    }
                    break;
                }
                self.r.skip();
                self.sugg = Sugg::Key;
            }
        }
        if self.r.can_read() {
            self.r.skip();
            self.sugg = Sugg::Nothing;
            Ok(())
        } else {
            Err(self.r.err0("argument.entity.options.unterminated"))
        }
    }

    fn invert(&mut self) -> bool {
        self.r.skip_whitespace();
        if self.r.next_is('!') {
            self.r.skip();
            self.r.skip_whitespace();
            true
        } else {
            false
        }
    }

    fn is_tag(&mut self) -> bool {
        self.r.skip_whitespace();
        if self.r.next_is('#') {
            self.r.skip();
            self.r.skip_whitespace();
            true
        } else {
            false
        }
    }

    fn rollback(&mut self, start: usize, key: &'static str, args: Vec<String>) -> CmdError {
        self.r.cursor = start;
        self.r.err(key, args)
    }

    fn option(&mut self, key: &str) -> Result<(), CmdError> {
        let start = self.r.cursor;
        match key {
            "name" => {
                let inv = self.invert();
                self.r.read_string()?;
                if !self.name.elem_ok(inv) {
                    return Err(self.rollback(start, "argument.entity.options.inapplicable", vec!["name".into()]));
                }
                self.name.mark(inv);
            }
            "distance" => {
                let (a, b) = read_range(self.r, false)?;
                if a.is_some_and(|v| v < 0.0) || b.is_some_and(|v| v < 0.0) {
                    return Err(self.rollback(start, "argument.entity.options.distance.negative", vec![]));
                }
                self.once.insert("distance");
            }
            "level" => {
                let (a, b) = read_range(self.r, true)?;
                if a.is_some_and(|v| v < 0.0) || b.is_some_and(|v| v < 0.0) {
                    return Err(self.rollback(start, "argument.entity.options.level.negative", vec![]));
                }
                self.includes_entities = false;
                self.once.insert("level");
            }
            "x" | "y" | "z" | "dx" | "dy" | "dz" => {
                self.r.read_double()?;
                self.once.insert(static_key(key));
            }
            "x_rotation" | "y_rotation" => {
                read_float_range(self.r)?;
                self.once.insert(static_key(key));
            }
            "limit" => {
                let n = self.r.read_int()?;
                if n < 1 {
                    return Err(self.rollback(start, "argument.entity.options.limit.toosmall", vec![]));
                }
                self.max = i64::from(n);
                self.once.insert("limit");
            }
            "sort" => {
                let name = self.r.read_unquoted();
                self.sugg = Sugg::Sort;
                if !matches!(name.as_str(), "nearest" | "furthest" | "random" | "arbitrary") {
                    return Err(self.rollback(start, "argument.entity.options.sort.irreversible", vec![name]));
                }
                self.once.insert("sort");
            }
            "gamemode" => {
                self.sugg = Sugg::Gamemode;
                let inv = self.invert();
                if !self.gamemode.elem_ok(inv) {
                    return Err(self.rollback(start, "argument.entity.options.inapplicable", vec!["gamemode".into()]));
                }
                let name = self.r.read_unquoted();
                if !GAMEMODES.contains(&name.as_str()) {
                    return Err(self.rollback(start, "argument.entity.options.mode.invalid", vec![name]));
                }
                self.includes_entities = false;
                self.gamemode.mark(inv);
            }
            "team" => {
                let inv = self.invert();
                self.r.read_unquoted();
                if !self.team.elem_ok(inv) {
                    return Err(self.rollback(start, "argument.entity.options.inapplicable", vec!["team".into()]));
                }
                self.team.mark(inv);
            }
            "type" => {
                self.sugg = Sugg::Type;
                let inv = self.invert();
                if self.is_tag() {
                    if !self.ty.neg_ok() {
                        return Err(self.rollback(start, "argument.entity.options.inapplicable", vec!["type".into()]));
                    }
                    let id = read_id(self.r)?;
                    if self.ty.tags.contains(&id) {
                        return Err(self.rollback(start, "argument.entity.options.inapplicable", vec!["type".into()]));
                    }
                    self.ty.lim = Lim::Multiple;
                    self.ty.tags.insert(id);
                } else {
                    if !self.ty.elem_ok(inv) {
                        return Err(self.rollback(start, "argument.entity.options.inapplicable", vec!["type".into()]));
                    }
                    let id = read_id(self.r)?;
                    if self.data.has_registry(ENTITY_TYPE) && !self.data.contains(ENTITY_TYPE, &id) {
                        return Err(self.rollback(start, "argument.entity.options.type.invalid", vec![id]));
                    }
                    if id == "minecraft:player" && !inv {
                        self.includes_entities = false;
                    }
                    self.ty.mark(inv);
                }
            }
            "tag" => {
                self.invert();
                self.r.read_unquoted();
            }
            "nbt" => {
                self.invert();
                crate::snbt::compound_argument(self.r)?;
            }
            "scores" => {
                self.r.expect('{')?;
                self.r.skip_whitespace();
                while self.r.can_read() && self.r.peek() != '}' {
                    self.r.skip_whitespace();
                    self.r.read_unquoted();
                    self.r.skip_whitespace();
                    self.r.expect('=')?;
                    self.r.skip_whitespace();
                    read_range(self.r, true)?;
                    self.r.skip_whitespace();
                    if self.r.next_is(',') {
                        self.r.skip();
                    }
                }
                self.r.expect('}')?;
                self.once.insert("scores");
            }
            "advancements" => {
                self.r.expect('{')?;
                self.r.skip_whitespace();
                let mut any = false;
                while self.r.can_read() && self.r.peek() != '}' {
                    self.r.skip_whitespace();
                    read_id(self.r)?;
                    self.r.skip_whitespace();
                    self.r.expect('=')?;
                    self.r.skip_whitespace();
                    if self.r.next_is('{') {
                        self.r.skip_whitespace();
                        self.r.expect('{')?;
                        self.r.skip_whitespace();
                        while self.r.can_read() && self.r.peek() != '}' {
                            self.r.skip_whitespace();
                            self.r.read_unquoted();
                            self.r.skip_whitespace();
                            self.r.expect('=')?;
                            self.r.skip_whitespace();
                            self.r.read_bool()?;
                            self.r.skip_whitespace();
                            if self.r.next_is(',') {
                                self.r.skip();
                            }
                        }
                        self.r.skip_whitespace();
                        self.r.expect('}')?;
                        self.r.skip_whitespace();
                    } else {
                        self.r.read_bool()?;
                    }
                    any = true;
                    self.r.skip_whitespace();
                    if self.r.next_is(',') {
                        self.r.skip();
                    }
                }
                self.r.expect('}')?;
                if any {
                    self.includes_entities = false;
                }
                self.once.insert("advancements");
            }
            _ => {
                self.invert();
                read_id(self.r)?;
            }
        }
        Ok(())
    }

    /// Suggestions at the stopping point (`names`: names proposed outside a selector).
    pub fn suggest(&self, b: &mut Builder<'_>, names: &dyn Fn(&mut Builder<'_>)) {
        let mut sub = b.offset(self.r.cursor);
        match self.sugg {
            Sugg::NameOrSelector => {
                names(&mut sub);
                if self.allow {
                    selector_suggestions(&mut sub);
                }
            }
            Sugg::Name => {
                let mut n = b.offset(self.start);
                names(&mut n);
                b.add(n);
                return;
            }
            Sugg::Selector => {
                let mut s = b.offset(self.r.cursor.saturating_sub(1));
                selector_suggestions(&mut s);
                b.add(s);
                return;
            }
            Sugg::OpenOptions => sub.suggest("["),
            Sugg::KeyOrClose => {
                sub.suggest("]");
                self.suggest_keys(&mut sub);
            }
            Sugg::Key => self.suggest_keys(&mut sub),
            Sugg::NextOrClose => {
                sub.suggest(",");
                sub.suggest("]");
            }
            Sugg::Nothing => {}
            Sugg::Sort => sub.suggest_matching(["nearest", "furthest", "random", "arbitrary"]),
            Sugg::Gamemode => {
                let mut p = sub.remaining_lower.clone();
                let (mut pos, mut neg) = (self.gamemode.pos_ok(), self.gamemode.neg_ok());
                if let Some(rest) = p.strip_prefix('!') {
                    pos = false;
                    p = rest.to_owned();
                } else if !p.is_empty() {
                    neg = false;
                }
                for g in GAMEMODES {
                    if g.starts_with(&p) {
                        if neg {
                            sub.suggest(format!("!{g}"));
                        }
                        if pos {
                            sub.suggest(g);
                        }
                    }
                }
            }
            Sugg::Type => {
                let types = self.data.registry(ENTITY_TYPE).unwrap_or(&[]);
                if self.ty.neg_ok() {
                    sub.suggest_resources_with(types, "!", "", |_| None);
                }
                if self.ty.pos_ok() {
                    sub.suggest_resources(types);
                }
                if self.ty.neg_ok() {
                    let tags: Vec<&String> =
                        self.data.tag_names(ENTITY_TYPE).into_iter().filter(|t| !self.ty.tags.contains(*t)).collect();
                    if !tags.is_empty() {
                        sub.suggest_resources_with(tags.iter(), "#", "", |_| None);
                        sub.suggest_resources_with(tags.iter(), "!#", "", |_| None);
                    }
                }
            }
        }
        b.add(sub);
    }

    fn suggest_keys(&self, b: &mut Builder<'_>) {
        let p = b.remaining_lower.clone();
        for k in OPTIONS.iter().chain(std::iter::once(&PREDICATE)) {
            if self.can_use(k) && k.starts_with(&p) {
                b.suggest_tip(format!("{k}="), Some(Tooltip::Key(format!("argument.entity.options.{k}.description"))));
            }
        }
    }
}

fn static_key(k: &str) -> &'static str {
    OPTIONS.iter().find(|o| **o == k).copied().unwrap_or("x")
}

const GAMEMODES: [&str; 4] = ["survival", "creative", "adventure", "spectator"];

pub fn selector_suggestions(b: &mut Builder<'_>) {
    for (s, k) in [
        ("@p", "nearestPlayer"),
        ("@a", "allPlayers"),
        ("@r", "randomPlayer"),
        ("@s", "self"),
        ("@e", "allEntities"),
        ("@n", "nearestEntity"),
    ] {
        b.suggest_tip(s, Some(Tooltip::Key(format!("argument.entity.selector.{k}"))));
    }
}

/// Textual UUID (the game's lenient `8-4-4-4-12` form: 5 hexadecimal groups).
pub fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && s.len() <= 36
        && parts.iter().all(|p| !p.is_empty() && p.len() <= 16 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Range `n`, `n..`, `..m`, `n..m` (§ 2.4); integers or reals. Error: cursor at the start.
pub fn read_range(r: &mut Reader<'_>, ints: bool) -> Result<(Option<f64>, Option<f64>), CmdError> {
    if !r.can_read() {
        return Err(r.err0("argument.range.empty"));
    }
    let start = r.cursor;
    let res = (|| {
        let a = range_number(r, ints)?;
        let b = if r.peek_at(0) == Some('.') && r.peek_at(1) == Some('.') {
            r.cursor += 2;
            range_number(r, ints)?
        } else {
            a
        };
        if a.is_none() && b.is_none() {
            return Err(r.err0("argument.range.empty"));
        }
        Ok((a, b))
    })();
    match res {
        Ok((Some(a), Some(b))) if a > b => {
            r.cursor = start;
            Err(r.err0("argument.range.swapped"))
        }
        Ok(v) => Ok(v),
        Err(mut e) => {
            r.cursor = start;
            e.cursor = Some(start);
            Err(e)
        }
    }
}

fn range_number(r: &mut Reader<'_>, ints: bool) -> Result<Option<f64>, CmdError> {
    let start = r.cursor;
    while r.can_read() {
        let c = r.peek();
        let ok = c.is_ascii_digit() || c == '-' || (c == '.' && r.peek_at(1) != Some('.'));
        if !ok {
            break;
        }
        r.skip();
    }
    let s = r.slice(start, r.cursor);
    if s.is_empty() {
        return Ok(None);
    }
    if ints {
        s.parse::<i32>().map(|v| Some(f64::from(v))).map_err(|_| r.err("parsing.int.invalid", vec![s.clone()]))
    } else {
        s.parse::<f64>().map(Some).map_err(|_| r.err("parsing.double.invalid", vec![s.clone()]))
    }
}

/// Angle range: min > max allowed (interval crossing ±180°).
fn read_float_range(r: &mut Reader<'_>) -> Result<(), CmdError> {
    match read_range(r, false) {
        Err(e) if e.key == "argument.range.swapped" => {
            // Re-read without the order check.
            range_number(r, false)?;
            if r.peek_at(0) == Some('.') && r.peek_at(1) == Some('.') {
                r.cursor += 2;
                range_number(r, false)?;
            }
            Ok(())
        }
        other => other.map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(s: &str, allow: bool) -> Result<Selector, CmdError> {
        let c: Vec<char> = s.chars().collect();
        let d = Data::default();
        let mut r = Reader::new(&c, 0);
        SelParser::new(&mut r, allow, &d).parse()
    }

    #[test]
    fn selector_forms() {
        assert_eq!(sel("@e", true).unwrap().max, i64::from(i32::MAX));
        assert!(sel("@s", true).unwrap().current);
        assert_eq!(sel("@e[limit=1]", true).unwrap().max, 1);
        assert!(!sel("@e[type=player]", true).unwrap().includes_entities);
        assert_eq!(sel("@x", true).unwrap_err().key, "argument.entity.selector.unknown");
        assert_eq!(sel("@", true).unwrap_err().key, "argument.entity.selector.missing");
        assert_eq!(sel("@e", false).unwrap_err().key, "argument.entity.selector.not_allowed");
        assert_eq!(sel("@e[foo=1]", true).unwrap_err().key, "argument.entity.options.unknown");
        assert_eq!(sel("@e[limit=1,limit=2]", true).unwrap_err().key, "argument.entity.options.inapplicable");
        assert_eq!(sel("@s[limit=1]", true).unwrap_err().key, "argument.entity.options.inapplicable");
        assert_eq!(sel("@e[distance=..5", true).unwrap_err().key, "argument.entity.options.unterminated");
        assert_eq!(sel("@e[tag]", true).unwrap_err().key, "argument.entity.options.valueless");
        assert!(sel("@e[ type = !minecraft:cow , tag=a, scores={x=1..}, nbt={a:1b}]", true).is_ok());
        assert_eq!(sel("Steve", true).unwrap().max, 1);
        assert_eq!(sel("abcdefghijklmnopq", true).unwrap_err().key, "argument.entity.invalid");
        assert!(sel("dd12be42-52a9-4a91-a8a1-11c01849e498", true).unwrap().includes_entities);
    }

    #[test]
    fn range_args() {
        let c: Vec<char> = "1..5 ..2 3.. 1.5 5..1".chars().collect();
        let mut r = Reader::new(&c, 0);
        assert_eq!(read_range(&mut r, true).unwrap(), (Some(1.0), Some(5.0)));
        r.skip();
        assert_eq!(read_range(&mut r, true).unwrap(), (None, Some(2.0)));
        r.skip();
        assert_eq!(read_range(&mut r, true).unwrap(), (Some(3.0), None));
        r.skip();
        assert_eq!(read_range(&mut r, true).unwrap_err().key, "parsing.int.invalid");
        r.cursor = 17;
        assert_eq!(read_range(&mut r, true).unwrap_err().key, "argument.range.swapped");
        let mut r = Reader::new(&c, 17);
        assert!(read_float_range(&mut r).is_ok());
        assert_eq!(r.cursor, c.len());
    }
}
