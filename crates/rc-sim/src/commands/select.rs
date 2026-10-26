//! Server-side entity selectors (`commands.md` § 2.5): complete reading of the options
//! (the text has already been validated by parsing), then search in the world like the game:
//! reference position, box, distances, filters, sort and limit.

use rc_proto::nbt::Nbt;

use super::ents::E;
use super::snbt;
use super::src::Src;
use crate::server::Server;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sort {
    Arbitrary,
    Nearest,
    Furthest,
    Random,
}

#[derive(Debug, Clone, Default)]
pub struct Range {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Range {
    pub fn test(&self, v: f64) -> bool {
        self.min.is_none_or(|m| v >= m) && self.max.is_none_or(|m| v <= m)
    }
}

/// Selector read.
#[derive(Debug, Clone)]
pub struct Sel {
    /// `@x` (letter), player name or UUID.
    pub letter: Option<char>,
    pub name: Option<String>,
    pub uuid: Option<u128>,
    pub limit: usize,
    pub sort: Sort,
    pub players_only: bool,
    pub current: bool,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    pub d: [Option<f64>; 3],
    pub distance: Option<Range>,
    pub level: Option<Range>,
    pub x_rot: Option<Range>,
    pub y_rot: Option<Range>,
    pub names: Vec<(bool, String)>,
    pub types: Vec<(bool, String, bool)>,
    pub tags: Vec<(bool, String)>,
    pub teams: Vec<(bool, String)>,
    pub modes: Vec<(bool, String)>,
    pub nbts: Vec<(bool, Nbt)>,
    pub scores: Vec<(String, Range)>,
    /// S9: `advancements={id=bool|{criterion=bool}}`.
    pub advancements: Vec<(String, serde_json::Value)>,
    pub type_limited: bool,
}

pub fn read_range(s: &str) -> Option<Range> {
    if let Some((a, b)) = s.split_once("..") {
        let p = |t: &str| if t.is_empty() { Some(None) } else { t.parse::<f64>().ok().map(Some) };
        Some(Range { min: p(a)?, max: p(b)? })
    } else {
        let v = s.parse::<f64>().ok()?;
        Some(Range { min: Some(v), max: Some(v) })
    }
}

/// Reading a selector from an already validated text.
pub fn parse(text: &str) -> Option<Sel> {
    let chars: Vec<char> = text.chars().collect();
    let mut s = Sel {
        letter: None,
        name: None,
        uuid: None,
        limit: usize::MAX,
        sort: Sort::Arbitrary,
        players_only: false,
        current: false,
        x: None,
        y: None,
        z: None,
        d: [None; 3],
        distance: None,
        level: None,
        x_rot: None,
        y_rot: None,
        names: Vec::new(),
        types: Vec::new(),
        tags: Vec::new(),
        teams: Vec::new(),
        modes: Vec::new(),
        nbts: Vec::new(),
        scores: Vec::new(),
        advancements: Vec::new(),
        type_limited: false,
    };
    if chars.first() != Some(&'@') {
        let t = text.trim_matches('"');
        if let Some(u) = snbt::parse_uuid(t).filter(|_| t.contains('-')) {
            s.uuid = Some(u);
            s.limit = 1;
        } else {
            s.name = Some(t.to_owned());
            s.limit = 1;
            s.players_only = true;
        }
        return Some(s);
    }
    let l = *chars.get(1)?;
    s.letter = Some(l);
    match l {
        'p' => {
            s.limit = 1;
            s.sort = Sort::Nearest;
            s.players_only = true;
        }
        'a' => s.players_only = true,
        'r' => {
            s.limit = 1;
            s.sort = Sort::Random;
            s.players_only = true;
        }
        's' => {
            s.limit = 1;
            s.current = true;
        }
        'n' => {
            s.limit = 1;
            s.sort = Sort::Nearest;
        }
        _ => {}
    }
    let mut i = 2;
    if chars.get(i) != Some(&'[') {
        return Some(s);
    }
    i += 1;
    let skip_ws = |i: &mut usize| {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
    };
    let read_word = |i: &mut usize| -> String {
        skip_ws(i);
        if matches!(chars.get(*i), Some('"' | '\'')) {
            let mut p = snbt::P { s: &chars, i: *i };
            let v = p.quoted().unwrap_or_default();
            *i = p.i;
            return v;
        }
        let st = *i;
        while *i < chars.len() && !matches!(chars[*i], ',' | ']' | '=' | '}' | '{') && !chars[*i].is_whitespace() {
            *i += 1;
        }
        chars[st..*i].iter().collect()
    };
    loop {
        skip_ws(&mut i);
        if i >= chars.len() || chars[i] == ']' {
            break;
        }
        let key = read_word(&mut i);
        skip_ws(&mut i);
        i += 1; // '='
        skip_ws(&mut i);
        let neg = if chars.get(i) == Some(&'!') {
            i += 1;
            skip_ws(&mut i);
            true
        } else {
            false
        };
        match key.as_str() {
            "nbt" => {
                let v = snbt::parse_at(&chars, &mut i).ok()?;
                s.nbts.push((neg, v));
            }
            "scores" => {
                skip_ws(&mut i);
                i += 1; // '{'
                loop {
                    skip_ws(&mut i);
                    if chars.get(i) == Some(&'}') {
                        i += 1;
                        break;
                    }
                    let obj = read_word(&mut i);
                    skip_ws(&mut i);
                    i += 1;
                    let r = read_word(&mut i);
                    s.scores.push((obj, read_range(&r)?));
                    skip_ws(&mut i);
                    if chars.get(i) == Some(&',') {
                        i += 1;
                    }
                }
            }
            "advancements" => {
                // S9: progress read (`id=bool` or `id={criterion=bool}`).
                let start = i;
                let mut depth = 0;
                while i < chars.len() {
                    match chars[i] {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let body: String = chars[start..i].iter().collect();
                s.advancements = parse_advancements(&body);
            }
            _ => {
                let is_tag = chars.get(i) == Some(&'#');
                if is_tag {
                    i += 1;
                }
                let v = read_word(&mut i);
                match key.as_str() {
                    "name" => s.names.push((neg, v)),
                    "distance" => s.distance = read_range(&v),
                    "level" => s.level = read_range(&v),
                    "x" => s.x = v.parse().ok(),
                    "y" => s.y = v.parse().ok(),
                    "z" => s.z = v.parse().ok(),
                    "dx" => s.d[0] = v.parse().ok(),
                    "dy" => s.d[1] = v.parse().ok(),
                    "dz" => s.d[2] = v.parse().ok(),
                    "x_rotation" => s.x_rot = read_range(&v),
                    "y_rotation" => s.y_rot = read_range(&v),
                    "limit" => s.limit = v.parse().unwrap_or(1),
                    "sort" => {
                        s.sort = match v.as_str() {
                            "nearest" => Sort::Nearest,
                            "furthest" => Sort::Furthest,
                            "random" => Sort::Random,
                            _ => Sort::Arbitrary,
                        }
                    }
                    "gamemode" => s.modes.push((neg, v)),
                    "team" => s.teams.push((neg, v)),
                    "tag" => s.tags.push((neg, v)),
                    "type" => {
                        let id = if v.contains(':') { v } else { format!("minecraft:{v}") };
                        if !neg && !is_tag {
                            s.type_limited = true;
                            if id == "minecraft:player" {
                                s.players_only = true;
                            }
                        }
                        s.types.push((neg, id, is_tag));
                    }
                    _ => {}
                }
            }
        }
        skip_ws(&mut i);
        if chars.get(i) == Some(&',') {
            i += 1;
        }
    }
    Some(s)
}

/// `{id=true, id2={crit=false}}` → (identifier, expected in JSON).
fn parse_advancements(body: &str) -> Vec<(String, serde_json::Value)> {
    let inner = body.trim().trim_start_matches('{').trim_end_matches('}');
    let mut out = Vec::new();
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        let st = i;
        while i < chars.len() && chars[i] != '=' {
            i += 1;
        }
        let id = super::args::id(chars[st..i].iter().collect::<String>().trim());
        i += 1;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if chars.get(i) == Some(&'{') {
            let st = i + 1;
            while i < chars.len() && chars[i] != '}' {
                i += 1;
            }
            let crits: String = chars[st..i.min(chars.len())].iter().collect();
            i += 1;
            let mut m = serde_json::Map::new();
            for part in crits.split(',') {
                if let Some((k, v)) = part.split_once('=') {
                    m.insert(k.trim().to_owned(), serde_json::Value::Bool(v.trim() == "true"));
                }
            }
            out.push((id, serde_json::Value::Object(m)));
        } else {
            let st = i;
            while i < chars.len() && chars[i] != ',' {
                i += 1;
            }
            let v: String = chars[st..i].iter().collect();
            if !id.is_empty() && id != "minecraft:" {
                out.push((id, serde_json::Value::Bool(v.trim() == "true")));
            }
        }
    }
    out
}

/// Partial comparison of the game (partial lists).
pub fn nbt_matches(expected: &Nbt, actual: &Nbt) -> bool {
    match (expected, actual) {
        (Nbt::Compound(e), Nbt::Compound(_)) => e.iter().all(|(k, v)| actual.get(k).is_some_and(|a| nbt_matches(v, a))),
        (Nbt::List(e), Nbt::List(a)) => {
            if e.is_empty() {
                return a.is_empty();
            }
            // Expected list longer than the actual one: failure, even if each element finds
            // a match (the game tests the length first).
            a.len() >= e.len() && e.iter().all(|x| a.iter().any(|y| nbt_matches(x, y)))
        }
        _ => crate::items::nbt_eq(expected, actual),
    }
}

/// Equality of names ignoring case, character by character (upper then
/// lower case), like the game's player lookup by name.
pub fn java_eq_ignore_case(a: &str, b: &str) -> bool {
    let fold = |c: char| -> char {
        let u = c.to_uppercase().next().unwrap_or(c);
        u.to_lowercase().next().unwrap_or(u)
    };
    a.chars().count() == b.chars().count() && a.chars().zip(b.chars()).all(|(x, y)| x == y || fold(x) == fold(y))
}

fn wrap_degrees(v: f64) -> f64 {
    let mut w = v % 360.0;
    if w >= 180.0 {
        w -= 360.0;
    }
    if w < -180.0 {
        w += 360.0;
    }
    w
}

fn rot_test(r: &Range, v: f32) -> bool {
    let min = wrap_degrees(r.min.unwrap_or(0.0));
    let max = wrap_degrees(r.max.unwrap_or(359.0));
    let v = wrap_degrees(f64::from(v));
    if min > max { v >= min || v <= max } else { v >= min && v <= max }
}

impl Server {
    fn mode_name(&self) -> &'static str {
        match self.game_mode {
            1 => "creative",
            2 => "adventure",
            3 => "spectator",
            _ => "survival",
        }
    }

    fn sel_test(&self, s: &Sel, e: E, src: &Src) -> bool {
        if s.players_only && e != E::Player {
            return false;
        }
        if !s.names.is_empty() {
            // Name flattened in the server's language (translated default name included).
            let name = super::cmds_ents::plain(&self.ent_name(e));
            for (neg, n) in &s.names {
                if (name == *n) == *neg {
                    return false;
                }
            }
        }
        for (neg, t, is_tag) in &s.types {
            let ty = self.ent_type(e);
            let hit = if *is_tag {
                self.cmd.data.tag_members("minecraft:entity_type", t.trim_start_matches("minecraft:")).contains(&ty)
                    || self.cmd.data.tag_members("minecraft:entity_type", t).contains(&ty)
            } else {
                ty == *t
            };
            if hit == *neg {
                return false;
            }
        }
        if !s.tags.is_empty() {
            let tags = self.ent_tags(e);
            for (neg, t) in &s.tags {
                let hit = if t.is_empty() { tags.is_empty() } else { tags.contains(t) };
                if hit == *neg {
                    return false;
                }
            }
        }
        for (neg, t) in &s.teams {
            let team = self.cmd.scores.team_of(&self.ent_score_name(e)).map(|t| t.name.clone());
            let hit = if t.is_empty() { team.is_none() } else { team.as_deref() == Some(t.as_str()) };
            if hit == *neg {
                return false;
            }
        }
        for (neg, m) in &s.modes {
            if e != E::Player {
                return false;
            }
            if (self.mode_name() == m) == *neg {
                return false;
            }
        }
        if let Some(l) = &s.level
            && (e != E::Player || !l.test(f64::from(self.life.vit.xp_level)))
        {
            return false;
        }
        let [yaw, pitch] = self.ent_rot(e);
        if let Some(r) = &s.x_rot
            && !rot_test(r, pitch)
        {
            return false;
        }
        if let Some(r) = &s.y_rot
            && !rot_test(r, yaw)
        {
            return false;
        }
        for (obj, r) in &s.scores {
            match self.cmd.scores.get(&self.ent_score_name(e), obj) {
                Some(v) if r.test(f64::from(v)) => {}
                _ => return false,
            }
        }
        if !self.selector_advancements(&s.advancements, e) {
            return false; // S9
        }
        if !s.nbts.is_empty() {
            // Same NBT as the `nbt` predicate (without `id`, the player's held item).
            let n = self.w_nbt(e.into());
            for (neg, x) in &s.nbts {
                if nbt_matches(x, &n) == *neg {
                    return false;
                }
            }
        }
        let _ = src;
        true
    }

    /// Selected entities, in the game's order.
    pub fn select(&mut self, s: &Sel, src: &Src) -> Vec<E> {
        if let Some(n) = &s.name {
            return if java_eq_ignore_case(n, &self.name) { vec![E::Player] } else { Vec::new() };
        }
        if let Some(u) = s.uuid {
            return self.ent_by_uuid(u).into_iter().collect();
        }
        let base = [s.x.unwrap_or(src.pos[0]), s.y.unwrap_or(src.pos[1]), s.z.unwrap_or(src.pos[2])];
        if s.current {
            return match src.entity {
                Some(e) if self.ent_exists(e) && self.sel_test(s, e, src) && self.sel_pos_ok(s, e, base) => vec![e],
                _ => Vec::new(),
            };
        }
        let candidates = if s.players_only {
            if self.life.vit.dead && s.letter != Some('a') && s.letter != Some('p') && s.letter != Some('r') {
                Vec::new()
            } else {
                vec![E::Player]
            }
        } else {
            self.all_ents()
        };
        let mut found: Vec<E> =
            candidates.into_iter().filter(|&e| self.sel_test(s, e, src) && self.sel_pos_ok(s, e, base)).collect();
        let dist = |me: &Server, e: E| {
            let p = me.e_pos(e);
            (p[0] - base[0]).powi(2) + (p[1] - base[1]).powi(2) + (p[2] - base[2]).powi(2)
        };
        match s.sort {
            Sort::Arbitrary => {}
            Sort::Nearest => found.sort_by(|a, b| dist(self, *a).total_cmp(&dist(self, *b))),
            Sort::Furthest => found.sort_by(|a, b| dist(self, *b).total_cmp(&dist(self, *a))),
            Sort::Random => {
                // Fisher-Yates shuffle on the world's random.
                for i in (1..found.len()).rev() {
                    let j = self.life.rng.next_int_bound((i + 1) as i32) as usize;
                    found.swap(i, j);
                }
            }
        }
        found.truncate(s.limit);
        found
    }

    fn sel_pos_ok(&self, s: &Sel, e: E, base: [f64; 3]) -> bool {
        if let Some(r) = &s.distance {
            let p = self.e_pos(e);
            let d2 = (p[0] - base[0]).powi(2) + (p[1] - base[1]).powi(2) + (p[2] - base[2]).powi(2);
            if !(r.min.is_none_or(|m| d2 >= m * m) && r.max.is_none_or(|m| d2 <= m * m)) {
                return false;
            }
        }
        if s.d.iter().any(Option::is_some) {
            let mut min = [0.0; 3];
            let mut max = [0.0; 3];
            for k in 0..3 {
                let d = s.d[k].unwrap_or(0.0);
                min[k] = base[k] + d.min(0.0);
                max[k] = base[k] + d.max(0.0) + 1.0;
            }
            let b = self.e_bbox(e);
            let hit = (0..3).all(|k| b.min[k] < max[k] && b.max[k] > min[k]);
            if !hit {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_options() {
        let s = parse("@e[type=!cow,tag=a,distance=..5,limit=2,sort=nearest,scores={x=1..}]").unwrap();
        assert_eq!(s.letter, Some('e'));
        assert_eq!(s.types, vec![(true, "minecraft:cow".to_owned(), false)]);
        assert_eq!(s.tags, vec![(false, "a".to_owned())]);
        assert_eq!(s.limit, 2);
        assert_eq!(s.sort, Sort::Nearest);
        assert_eq!(s.distance.as_ref().unwrap().max, Some(5.0));
        assert_eq!(s.scores[0].0, "x");
        let n = parse("Steve").unwrap();
        assert_eq!(n.name.as_deref(), Some("Steve"));
        let s = parse("@s[nbt={OnGround:1b}]").unwrap();
        assert_eq!(s.nbts.len(), 1);
    }

    #[test]
    fn nbt_comparison() {
        let a = snbt::parse("{a:1,b:[1,2,3],c:{d:2}}").unwrap();
        assert!(nbt_matches(&snbt::parse("{b:[2]}").unwrap(), &a));
        assert!(nbt_matches(&snbt::parse("{c:{}}").unwrap(), &a));
        assert!(!nbt_matches(&snbt::parse("{a:2}").unwrap(), &a));
        assert!(!nbt_matches(&snbt::parse("{a:1b}").unwrap(), &a));
        // Expected list longer than the actual one: failure even if each element is found.
        let one = snbt::parse("{b:[1]}").unwrap();
        assert!(!nbt_matches(&snbt::parse("{b:[1,1]}").unwrap(), &one));
        assert!(nbt_matches(&snbt::parse("{b:[1]}").unwrap(), &one));
    }

    #[test]
    fn names_ignore_case() {
        assert!(java_eq_ignore_case("Steve", "sTEVE"));
        assert!(!java_eq_ignore_case("Steve", "Steve2"));
        assert!(java_eq_ignore_case("Élodie", "éLODIE"));
    }
}
