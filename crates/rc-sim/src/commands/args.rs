//! Argument values (text already validated by parsing): absolute, relative and local
//! coordinates, rotations, block states and predicates, stacks and item predicates, entities,
//! messages and components (selectors and scores resolved).

use rc_proto::nbt::Nbt;
use rc_text::Component;

use super::ents::E;
use super::src::{Fail, Src};
use super::{snbt, text};
use crate::player::Stack;
use crate::server::Server;
use crate::tr;

/// The tabulated sine / the tabulated cosine (the table shared by every crate), widened.
#[inline]
pub fn msin(x: f64) -> f64 {
    f64::from(rc_data::java_math::sin(x))
}

#[inline]
pub fn mcos(x: f64) -> f64 {
    f64::from(rc_data::java_math::cos(x))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum C {
    Abs(f64, bool),
    Rel(f64),
    Local(f64),
}

fn parts(text: &str) -> Vec<C> {
    text.split(' ')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if let Some(r) = s.strip_prefix('~') {
                C::Rel(if r.is_empty() { 0.0 } else { r.parse().unwrap_or(0.0) })
            } else if let Some(r) = s.strip_prefix('^') {
                C::Local(if r.is_empty() { 0.0 } else { r.parse().unwrap_or(0.0) })
            } else {
                C::Abs(s.parse().unwrap_or(0.0), s.contains('.'))
            }
        })
        .collect()
}

/// Local position `^left ^up ^forward` (rotation and anchor of the source).
fn local(s: &Server, src: &Src, l: [f64; 3]) -> [f64; 3] {
    let deg = std::f64::consts::PI / 180.0;
    let [xr, yr] = [f64::from(src.rot[0]), f64::from(src.rot[1])];
    let y_cos = mcos((yr + 90.0) * deg);
    let y_sin = msin((yr + 90.0) * deg);
    let x_cos = mcos(-xr * deg);
    let x_sin = msin(-xr * deg);
    let x_cos_up = mcos((-xr + 90.0) * deg);
    let x_sin_up = msin((-xr + 90.0) * deg);
    let fw = [y_cos * x_cos, x_sin, y_sin * x_cos];
    let up = [y_cos * x_cos_up, x_sin_up, y_sin * x_cos_up];
    let cross = [fw[1] * up[2] - fw[2] * up[1], fw[2] * up[0] - fw[0] * up[2], fw[0] * up[1] - fw[1] * up[0]];
    let left = [-cross[0], -cross[1], -cross[2]];
    let p = src.anchor(s);
    let mut out = [0.0; 3];
    for k in 0..3 {
        out[k] = p[k] + fw[k] * l[2] + up[k] * l[1] + left[k] * l[0];
    }
    out
}

/// `vec3`: reals, `+0.5` in X and Z for an absolute integer.
pub fn vec3(s: &Server, src: &Src, text: &str) -> [f64; 3] {
    let c = parts(text);
    if let Some(C::Local(_)) = c.first() {
        let l = |i: usize| match c.get(i) {
            Some(C::Local(v)) => *v,
            _ => 0.0,
        };
        return local(s, src, [l(0), l(1), l(2)]);
    }
    std::array::from_fn(|k| match c.get(k) {
        Some(C::Abs(v, dec)) => {
            if !*dec && k != 1 {
                v + 0.5
            } else {
                *v
            }
        }
        Some(C::Rel(v)) => src.pos[k] + v,
        _ => src.pos[k],
    })
}

/// `vec3` without recentring of integers.
pub fn vec3_exact(s: &Server, src: &Src, text: &str) -> [f64; 3] {
    let c = parts(text);
    if let Some(C::Local(_)) = c.first() {
        return vec3(s, src, text);
    }
    std::array::from_fn(|k| match c.get(k) {
        Some(C::Abs(v, _)) => *v,
        Some(C::Rel(v)) => src.pos[k] + v,
        _ => src.pos[k],
    })
}

/// `block_pos`: integer position (relative ones rounded down).
pub fn block_pos(s: &Server, src: &Src, text: &str) -> [i32; 3] {
    let c = parts(text);
    if let Some(C::Local(_)) = c.first() {
        let p = vec3(s, src, text);
        return [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32];
    }
    std::array::from_fn(|k| {
        match c.get(k) {
            Some(C::Abs(v, _)) => *v,
            Some(C::Rel(v)) => src.pos[k] + v,
            _ => src.pos[k],
        }
        .floor() as i32
    })
}

/// `column_pos`: integer X Z.
pub fn column(src: &Src, text: &str) -> [i32; 2] {
    let c = parts(text);
    let g = |i: usize, base: f64| match c.get(i) {
        Some(C::Abs(v, _)) => *v,
        Some(C::Rel(v)) => base + v,
        _ => base,
    };
    [g(0, src.pos[0]).floor() as i32, g(1, src.pos[2]).floor() as i32]
}

/// `vec2`: real X Z (`+0.5` if an absolute integer).
pub fn vec2(src: &Src, text: &str) -> [f64; 2] {
    let c = parts(text);
    let g = |i: usize, base: f64| match c.get(i) {
        Some(C::Abs(v, dec)) => {
            if *dec {
                *v
            } else {
                v + 0.5
            }
        }
        Some(C::Rel(v)) => base + v,
        _ => base,
    };
    [g(0, src.pos[0]), g(1, src.pos[2])]
}

/// `rotation` ("yaw pitch") → [yaw, pitch].
pub fn rotation(src: &Src, text: &str) -> [f32; 2] {
    let c = parts(text);
    let g = |i: usize, base: f32| match c.get(i) {
        Some(C::Abs(v, _)) => *v as f32,
        Some(C::Rel(v)) => base + *v as f32,
        _ => base,
    };
    [g(0, src.rot[1]), g(1, src.rot[0])]
}

/// Rotation: relative components? (yaw, pitch).
pub fn rotation_relative(text: &str) -> [bool; 2] {
    let c = parts(text);
    [matches!(c.first(), Some(C::Rel(_))), matches!(c.get(1), Some(C::Rel(_)))]
}

/// Coordinates: relative components (x, y, z)? (`~`, `^` counts as non-relative).
pub fn relative_flags(text: &str) -> [bool; 3] {
    let c = parts(text);
    [0, 1, 2].map(|i| matches!(c.get(i), Some(C::Rel(_))))
}

pub fn is_local(text: &str) -> bool {
    matches!(parts(text).first(), Some(C::Local(_)))
}

/// `angle` (yaw only).
pub fn angle(src: &Src, text: &str) -> f32 {
    match parts(text).first() {
        Some(C::Abs(v, _)) => *v as f32,
        Some(C::Rel(v)) => src.rot[1] + *v as f32,
        _ => src.rot[1],
    }
}

/// Full identifier: a missing or empty namespace (`stone`, `:stone`) is `minecraft`, like the game.
pub fn id(s: &str) -> String {
    match s.split_once(':') {
        None => format!("minecraft:{s}"),
        Some(("", path)) => format!("minecraft:{path}"),
        Some(_) => s.to_owned(),
    }
}

/// Block state read: state, optional compound.
pub struct BlockInput {
    pub state: u32,
    pub nbt: Option<Nbt>,
    /// Properties given explicitly.
    pub props: Vec<(String, String)>,
}

/// Reading of `id[properties]{nbt}`.
fn split_block(text: &str) -> (String, Vec<(String, String)>, Option<Nbt>, bool) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let tag = chars.first() == Some(&'#');
    if tag {
        i += 1;
    }
    let st = i;
    while i < chars.len() && !matches!(chars[i], '[' | '{') {
        i += 1;
    }
    let name = id(&chars[st..i].iter().collect::<String>());
    let mut props = Vec::new();
    if chars.get(i) == Some(&'[') {
        let end = chars[i..].iter().position(|&c| c == ']').map_or(chars.len(), |p| i + p);
        let inner: String = chars[i + 1..end].iter().collect();
        for kv in inner.split(',') {
            if let Some((k, v)) = kv.split_once('=') {
                props.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
        i = (end + 1).min(chars.len());
    }
    let nbt = if chars.get(i) == Some(&'{') { snbt::parse_at(&chars, &mut i).ok() } else { None };
    (name, props, nbt, tag)
}

/// A block predicate (`block_predicate`) read once for a whole `fill` or `clone`, where
/// `block_matches` reads its text again for every block: its blocks resolved to identifiers
/// (a tag: the members found under its short and its full name, like `block_matches`), its
/// properties kept, and the answer for each block state kept from its first test. The data part
/// (`{…}`) is still compared block by block.
pub(crate) struct BlockPred {
    blocks: Vec<u32>,
    props: Vec<(String, String)>,
    nbt: Option<Nbt>,
    /// Answer per block state: 0 not tested yet, 1 matches, 2 does not (≈ 30 KB, one byte
    /// per state, cheaper than a map for a box that walks many blocks).
    by_state: Vec<u8>,
}

impl BlockPred {
    pub(crate) fn new(sv: &Server, text: &str) -> BlockPred {
        let (name, props, nbt, tag) = split_block(text);
        let members: Vec<&String> = if tag {
            let short = name.trim_start_matches("minecraft:");
            let data = &sv.cmd.data;
            let full = data.tag_members("minecraft:block", &name);
            data.tag_members("minecraft:block", short).iter().chain(full).collect()
        } else {
            vec![&name]
        };
        let mut blocks: Vec<u32> = members.into_iter().filter_map(|m| sv.data.block_id(m)).collect();
        blocks.sort_unstable();
        blocks.dedup();
        BlockPred { blocks, props, nbt, by_state: vec![0; sv.data.states.len()] }
    }

    /// The part of the test that only reads the state: its block, then its properties (a
    /// property the block does not have never matches).
    fn state_matches(&mut self, sv: &Server, state: u32) -> bool {
        let Some(known) = self.by_state.get_mut(state as usize) else { return false };
        if *known == 0 {
            let block = sv.data.state(state).map(|s| u32::from(s.block));
            let v = sv.view();
            let ok = block.is_some_and(|b| self.blocks.binary_search(&b).is_ok())
                && self.props.iter().all(|(k, val)| v.get(state, k) == Some(val.as_str()));
            *known = if ok { 1 } else { 2 };
        }
        *known == 1
    }
}

impl Server {
    pub fn block_input(&self, text: &str) -> Option<BlockInput> {
        let (name, props, nbt, _) = split_block(text);
        let t = self.table();
        let b = t.block_id(&name);
        if b == u16::MAX {
            return None;
        }
        let v = self.view();
        let mut s = v.default_state(b);
        for (k, val) in &props {
            s = v.set(s, k, val);
        }
        Some(BlockInput { state: s, nbt, props })
    }

    /// Block predicate (`block_predicate`) applied to a position.
    pub fn block_matches(&self, text: &str, p: [i32; 3]) -> bool {
        let (name, props, nbt, tag) = split_block(text);
        let state = self.get(p);
        let bname = self.data.block_of(state).map_or("", |b| b.name.as_str()).to_owned();
        let ok_block = if tag {
            let tagname = name.trim_start_matches("minecraft:");
            self.cmd.data.tag_members("minecraft:block", tagname).contains(&bname)
                || self.cmd.data.tag_members("minecraft:block", &name).contains(&bname)
        } else {
            bname == name
        };
        if !ok_block {
            return false;
        }
        let v = self.view();
        for (k, val) in &props {
            if v.get(state, k) != Some(val.as_str()) {
                return false;
            }
        }
        if let Some(n) = nbt {
            // The live data: a furnace's NBT lags until the save.
            let Some(be) = self.live_block_data(p) else { return false };
            return super::select::nbt_matches(&n, &be);
        }
        true
    }

    /// `block_matches` with a predicate read once (`BlockPred`).
    pub(crate) fn block_pred_matches(&self, pred: &mut BlockPred, p: [i32; 3]) -> bool {
        let state = self.get(p);
        if !pred.state_matches(self, state) {
            return false;
        }
        match &pred.nbt {
            // The live data: a furnace's NBT lags until the save.
            Some(n) => self.live_block_data(p).is_some_and(|be| super::select::nbt_matches(n, &be)),
            None => true,
        }
    }

    /// Item stack (`item_stack`): identifier and added or removed components.
    pub fn item_input(&self, text: &str, count: i32) -> Option<Stack> {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() && chars[i] != '[' {
            i += 1;
        }
        let name = id(&chars[..i].iter().collect::<String>());
        let mut comps: Vec<(String, Nbt)> = Vec::new();
        if chars.get(i) == Some(&'[') {
            i += 1;
            loop {
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                if i >= chars.len() || chars[i] == ']' {
                    break;
                }
                let removed = chars[i] == '!';
                if removed {
                    i += 1;
                }
                let st = i;
                while i < chars.len() && !matches!(chars[i], '=' | ',' | ']') {
                    i += 1;
                }
                let key = id(chars[st..i].iter().collect::<String>().trim());
                if removed {
                    comps.push((format!("!{key}"), Nbt::Compound(Vec::new())));
                } else if chars.get(i) == Some(&'=') {
                    i += 1;
                    let v = snbt::parse_at(&chars, &mut i).ok()?;
                    comps.push((key, v));
                }
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                if chars.get(i) == Some(&',') {
                    i += 1;
                }
            }
        }
        // Components equal to the item's default removed from the patch, as at stack creation.
        let mut st = Stack { id: name, count, components: (!comps.is_empty()).then_some(Nbt::Compound(comps)) };
        crate::items::normalize_patch(&mut st);
        Some(st)
    }

    /// Item predicate (`item_predicate`) on a stack; for several stacks, read the
    /// predicate once (`ItemPred::parse`) and call `item_pred_test`.
    pub fn item_matches(&self, text: &str, s: &Stack) -> bool {
        self.item_pred_test(&super::itempred::ItemPred::parse(text), s)
    }

    /// Entities of an `entity` argument; error "no entity found".
    pub fn arg_entities(&mut self, src: &Src, text: &str, players: bool) -> Result<Vec<E>, Fail> {
        let Some(sel) = super::select::parse(text) else { return Err(Fail(tr!("argument.entity.invalid"))) };
        let found = self.select(&sel, src);
        if found.is_empty() {
            return Err(Fail(if players {
                tr!("argument.entity.notfound.player")
            } else {
                tr!("argument.entity.notfound.entity")
            }));
        }
        Ok(found)
    }

    /// A single entity (`single`).
    pub fn arg_entity(&mut self, src: &Src, text: &str, players: bool) -> Result<E, Fail> {
        let v = self.arg_entities(src, text, players)?;
        Ok(v[0])
    }

    /// Entities, empty list allowed.
    pub fn arg_entities_opt(&mut self, src: &Src, text: &str) -> Vec<E> {
        super::select::parse(text).map(|s| self.select(&s, src)).unwrap_or_default()
    }

    /// Does an `entity` argument find at least one entity? (`arg_entities_opt` not empty.)
    pub fn arg_entities_any(&mut self, src: &Src, text: &str) -> bool {
        super::select::parse(text).is_some_and(|s| self.select_finds_any(&s, src))
    }

    /// Player profiles (`game_profile`): selector or name (online player only).
    pub fn arg_profiles(&mut self, src: &Src, text: &str) -> Result<Vec<E>, Fail> {
        if text.starts_with('@') {
            let v = self.arg_entities_opt(src, text);
            if v.is_empty() {
                return Err(Fail(tr!("argument.entity.notfound.player")));
            }
            return Ok(v);
        }
        // Cache of known profiles: name ignoring case.
        if super::select::java_eq_ignore_case(text, &self.name) {
            Ok(vec![E::Player])
        } else {
            Err(Fail(tr!("argument.player.unknown")))
        }
    }

    /// Score holders: `*` (all known), selector, or raw name.
    pub fn arg_score_holders(&mut self, src: &Src, text: &str) -> Result<Vec<String>, Fail> {
        if text == "*" {
            let all: Vec<String> = self.cmd.scores.holders();
            if all.is_empty() {
                return Err(Fail(tr!("argument.scoreHolder.empty")));
            }
            return Ok(all);
        }
        if text.starts_with('@') {
            let v = self.arg_entities_opt(src, text);
            if v.is_empty() {
                return Err(Fail(tr!("argument.entity.notfound.entity")));
            }
            return Ok(v.into_iter().map(|e| self.ent_score_name(e)).collect());
        }
        Ok(vec![self.score_holder_name(text)])
    }

    /// Named holder: `#name` as is; UUID of a present entity → that entity; name of an
    /// online player (case ignored) → that player; otherwise the raw text.
    pub fn score_holder_name(&self, text: &str) -> String {
        if text.starts_with('#') {
            return text.to_owned();
        }
        if let Some(u) = snbt::parse_uuid(text).filter(|_| text.contains('-')) {
            return self.ent_by_uuid(u).map_or_else(|| text.to_owned(), |e| self.ent_score_name(e));
        }
        if super::select::java_eq_ignore_case(text, &self.name) { self.name.clone() } else { text.to_owned() }
    }

    /// Message: selectors replaced by the names (if the source has the right to use selectors).
    pub fn arg_message(&mut self, src: &Src, text: &str) -> Component {
        if src.level < 2 || !text.contains('@') {
            return Component::text(text);
        }
        let chars: Vec<char> = text.chars().collect();
        let mut out: Vec<Component> = Vec::new();
        let mut lit_start = 0;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '@' && matches!(chars.get(i + 1), Some('p' | 'a' | 'r' | 's' | 'e' | 'n')) {
                // End of the selector: optional options in brackets.
                let mut j = i + 2;
                if chars.get(j) == Some(&'[') {
                    let mut depth = 0;
                    while j < chars.len() {
                        match chars[j] {
                            '[' | '{' => depth += 1,
                            ']' | '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    j += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                }
                if i > lit_start {
                    out.push(Component::text(chars[lit_start..i].iter().collect::<String>()));
                }
                let sel: String = chars[i..j].iter().collect();
                let ents = self.arg_entities_opt(src, &sel);
                let names: Vec<Component> = ents.into_iter().map(|e| self.ent_display(e)).collect();
                out.push(text::list(names));
                i = j;
                lit_start = j;
            } else {
                i += 1;
            }
        }
        if lit_start < chars.len() {
            out.push(Component::text(chars[lit_start..].iter().collect::<String>()));
        }
        text::join(out)
    }

    /// Text component (`component`): SNBT → component, server contents resolved.
    pub fn arg_component(&mut self, src: &Src, text: &str, target: Option<E>) -> Result<Component, Fail> {
        let n = snbt::parse(text).map_err(|_| Fail(tr!("argument.component.invalid", "")))?;
        let n = self.resolve_component(src, &n, target, 0)?;
        rc_text::from_nbt(&n).ok_or_else(|| Fail(tr!("argument.component.invalid", "")))
    }

    /// Resolution of the `selector`, `score`, `nbt` contents (the resolution of text components):
    /// resolved content, then children added, then the component's style applied on top.
    pub fn resolve_component(&mut self, src: &Src, n: &Nbt, target: Option<E>, depth: u32) -> Result<Nbt, Fail> {
        if depth > 100 {
            return Ok(n.clone());
        }
        match n {
            Nbt::List(l) => {
                let mut out = Vec::new();
                for e in l {
                    out.push(self.resolve_component(src, e, target, depth + 1)?);
                }
                Ok(Nbt::List(out))
            }
            Nbt::Compound(f) => {
                let mut f = f.clone();
                for key in ["extra", "with"] {
                    if let Some(i) = f.iter().position(|(k, _)| k == key) {
                        let v = self.resolve_component(src, &f[i].1, target, depth + 1)?;
                        f[i].1 = v;
                    }
                }
                let get = |k: &str| f.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
                // Explicit type, otherwise the first content present in the game's reading order.
                let kind = get("type").and_then(|t| t.as_str().map(str::to_owned)).or_else(|| {
                    [("text", "text"), ("translate", "translatable"), ("keybind", "keybind")]
                        .into_iter()
                        .chain([("score", "score"), ("selector", "selector"), ("nbt", "nbt")])
                        .find(|(k, _)| f.iter().any(|(x, _)| x == k))
                        .map(|(_, t)| t.to_owned())
                });
                let mut c = match kind.as_deref() {
                    Some("selector") => {
                        let sel = get("selector").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
                        let sep = self.resolve_separator(src, get("separator"), target, depth)?;
                        let ents = self.arg_entities_opt(src, &sel);
                        let names: Vec<Component> = ents.into_iter().map(|e| self.ent_display(e)).collect();
                        text::to_nbt(&match sep {
                            Some(sep) => text::list_sep(names, sep),
                            None => text::list(names),
                        })
                    }
                    Some("score") => {
                        let score = get("score").unwrap_or(Nbt::Compound(Vec::new()));
                        let name = score.get("name").and_then(Nbt::as_str).unwrap_or("").to_owned();
                        let obj = score.get("objective").and_then(Nbt::as_str).unwrap_or("").to_owned();
                        self.score_text(src, &name, &obj, target)?
                    }
                    Some("nbt") => self.nbt_text(src, &f, target, depth)?,
                    _ => return Ok(Nbt::Compound(f)),
                };
                if let Some(Nbt::List(extra)) = get("extra") {
                    for e in extra {
                        append_child(&mut c, e);
                    }
                }
                let style: Vec<(String, Nbt)> = f
                    .iter()
                    .filter(|(k, _)| {
                        !matches!(
                            k.as_str(),
                            "selector"
                                | "score"
                                | "nbt"
                                | "separator"
                                | "type"
                                | "source"
                                | "entity"
                                | "block"
                                | "storage"
                                | "interpret"
                                | "plain"
                                | "extra"
                                | "text"
                                | "translate"
                                | "fallback"
                                | "with"
                                | "keybind"
                        )
                    })
                    .cloned()
                    .collect();
                apply_style(&mut c, &style);
                Ok(c)
            }
            o => Ok(o.clone()),
        }
    }

    /// Separator given (resolved); `None` = default separator of the content.
    fn resolve_separator(
        &mut self,
        src: &Src,
        sep: Option<Nbt>,
        target: Option<E>,
        depth: u32,
    ) -> Result<Option<Component>, Fail> {
        let Some(sep) = sep else { return Ok(None) };
        let r = self.resolve_component(src, &sep, target, depth + 1)?;
        Ok(rc_text::from_nbt(&r))
    }

    /// `score` content: holder (selector with at most one entity, otherwise the text; `*` = the
    /// target of the text), value formatted by the score's format, otherwise the objective's.
    /// A simple name is also read as a selector: UUID = the entity, name = the online player
    /// (case ignored), otherwise the raw text (`#name` and `*` are not valid names).
    fn score_text(&mut self, src: &Src, name: &str, obj: &str, target: Option<E>) -> Result<Nbt, Fail> {
        let mut holder = if name.starts_with('@') { name.to_owned() } else { self.score_holder_name(name) };
        if name.starts_with('@') && super::select::parse(name).is_some() {
            let ents = self.arg_entities_opt(src, name);
            if ents.len() > 1 {
                return Err(Fail(tr!("argument.entity.toomany")));
            }
            if let Some(e) = ents.first() {
                holder = self.ent_score_name(*e);
            }
        }
        if holder == "*"
            && let Some(e) = target
        {
            holder = self.ent_score_name(e);
        }
        let empty = Nbt::String(String::new());
        let Some(o) = self.cmd.scores.objective(obj) else { return Ok(empty) };
        let Some(sc) = self.cmd.scores.score(&holder, obj) else { return Ok(empty) };
        let value = sc.value;
        Ok(match sc.format.as_ref().or(o.format.as_ref()) {
            Some(f) => match f.get("type").and_then(Nbt::as_str) {
                Some("blank") => empty,
                Some("fixed") => f.get("value").cloned().unwrap_or(empty),
                _ => {
                    let mut c = vec![("text".to_owned(), Nbt::String(value.to_string()))];
                    if let Nbt::Compound(x) = f {
                        c.extend(x.iter().filter(|(k, _)| k != "type").cloned());
                    }
                    Nbt::Compound(c)
                }
            },
            None => Nbt::String(value.to_string()),
        })
    }

    /// Tags the path of an `nbt` content designates in its source (entities, loaded block,
    /// storage). A storage and a block entity are read in place: only the tags reached are
    /// copied (`RUSTCRAFT_OPT_DATABORROW`; the former path copies the whole source first).
    fn nbt_text_elements(
        &mut self,
        src: &Src,
        source: Option<&str>,
        text_of: &dyn Fn(&str) -> String,
        path: &str,
    ) -> Vec<Nbt> {
        use super::cmds_data::path_get;
        let borrow = self.cmd.paths.data_borrow;
        match source {
            Some("entity") => {
                let ents = self.arg_entities_opt(src, &text_of("entity"));
                ents.into_iter().flat_map(|e| path_get(&self.w_nbt(e.into()), path)).collect()
            }
            Some("block") => {
                let p = block_pos(self, src, &text_of("block"));
                if !self.is_loaded(p) {
                    return Vec::new();
                }
                if borrow {
                    self.block_data_view(p).map(|d| path_get(&d, path)).unwrap_or_default()
                } else {
                    self.data_of_block(p).map(|d| path_get(&d, path)).unwrap_or_default()
                }
            }
            Some("storage") => {
                let key = id(&text_of("storage"));
                let empty = Nbt::Compound(Vec::new());
                if borrow {
                    path_get(self.cmd.storage.get(&key).unwrap_or(&empty), path)
                } else {
                    path_get(&self.cmd.storage.get(&key).cloned().unwrap_or(empty), path)
                }
            }
            _ => Vec::new(),
        }
    }

    /// `nbt` content: tags designated by the path in the source (entities, loaded block,
    /// storage), written as SNBT (coloured, or raw with `plain`) or read as components
    /// (`interpret`), joined by the separator (`, ` without style by default).
    fn nbt_text(&mut self, src: &Src, f: &[(String, Nbt)], target: Option<E>, depth: u32) -> Result<Nbt, Fail> {
        let get = |k: &str| f.iter().find(|(x, _)| x == k).map(|(_, v)| v);
        let flag = |k: &str| get(k).and_then(Nbt::as_i64).is_some_and(|v| v != 0);
        let (interpret, plain) = (flag("interpret"), flag("plain"));
        if interpret && plain {
            return Err(Fail(tr!("argument.component.invalid", "'interpret' and 'plain' flags can't be both on")));
        }
        let path = get("nbt").and_then(Nbt::as_str).unwrap_or("").to_owned();
        let source = get("source").and_then(Nbt::as_str).map(str::to_owned).or_else(|| {
            ["entity", "block", "storage"].into_iter().find(|k| get(k).is_some()).map(str::to_owned)
        });
        let text_of = |k: &str| get(k).and_then(Nbt::as_str).unwrap_or("").to_owned();
        let elements = self.nbt_text_elements(src, source.as_deref(), &text_of, &path);
        let sep = match self.resolve_separator(src, get("separator").cloned(), target, depth)? {
            Some(s) => text::to_nbt(&s),
            None => Nbt::String(", ".into()),
        };
        let mut parts: Vec<Nbt> = Vec::new();
        for tag in &elements {
            if interpret {
                // Tag unreadable as a component: ignored.
                if rc_text::from_nbt(tag).is_some()
                    && let Ok(r) = self.resolve_component(src, tag, target, depth + 1)
                {
                    parts.push(r);
                }
            } else {
                parts.push(text::to_nbt(&if plain { snbt::pretty_plain(tag) } else { snbt::pretty(tag) }));
            }
        }
        // The first element carries the following ones (separator then element) as children.
        let mut it = parts.into_iter();
        let Some(mut first) = it.next() else { return Ok(Nbt::String(String::new())) };
        for p in it {
            append_child(&mut first, sep.clone());
            append_child(&mut first, p);
        }
        Ok(first)
    }
}

/// NBT component reduced to a compound (text only, or list: first element then the others
/// as children).
fn into_compound(c: Nbt) -> Vec<(String, Nbt)> {
    match c {
        Nbt::Compound(f) => f,
        Nbt::List(l) => {
            let mut it = l.into_iter();
            let mut base = it.next().map_or_else(|| vec![("text".into(), Nbt::String(String::new()))], into_compound);
            for e in it {
                push_extra(&mut base, e);
            }
            base
        }
        Nbt::String(s) => vec![("text".into(), Nbt::String(s))],
        o => vec![("text".into(), Nbt::String(snbt::to_snbt(&o)))],
    }
}

fn push_extra(f: &mut Vec<(String, Nbt)>, child: Nbt) {
    match f.iter_mut().find(|(k, _)| k == "extra") {
        Some((_, Nbt::List(l))) => l.push(child),
        Some((_, v)) => *v = Nbt::List(vec![child]),
        None => f.push(("extra".into(), Nbt::List(vec![child]))),
    }
}

/// Adds a child at the end of a component.
fn append_child(c: &mut Nbt, child: Nbt) {
    let mut f = into_compound(std::mem::replace(c, Nbt::Compound(Vec::new())));
    push_extra(&mut f, child);
    *c = Nbt::Compound(f);
}

/// Style of the original component applied to the resolved content: its fields take precedence.
fn apply_style(c: &mut Nbt, style: &[(String, Nbt)]) {
    if style.is_empty() {
        return;
    }
    let mut f = into_compound(std::mem::replace(c, Nbt::Compound(Vec::new())));
    for (k, v) in style {
        match f.iter_mut().find(|(x, _)| x == k) {
            Some((_, old)) => *old = v.clone(),
            None => f.push((k.clone(), v.clone())),
        }
    }
    *c = Nbt::Compound(f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components() {
        assert_eq!(parts("~ ~1 5"), vec![C::Rel(0.0), C::Rel(1.0), C::Abs(5.0, false)]);
        assert_eq!(parts("^ ^ ^2"), vec![C::Local(0.0), C::Local(0.0), C::Local(2.0)]);
        assert_eq!(id("stone"), "minecraft:stone");
        let (n, p, nbt, tag) = split_block("oak_stairs[facing=east,half=top]{a:1}");
        assert_eq!(n, "minecraft:oak_stairs");
        assert_eq!(p.len(), 2);
        assert!(nbt.is_some());
        assert!(!tag);
        assert!((msin(std::f64::consts::FRAC_PI_2) - 1.0).abs() < 1e-6);
    }
}
