//! `data` (get, merge, modify, remove) on block entities, entities and command storage;
//! NBT paths (`a.b[0]{x:1}`); `item` (replace); `loot` (the pack's loot tables).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_text::Component;

use super::ents::E;
use super::src::{Fail, Src};
use super::{Parsed, R, Seg, args, snbt};
use crate::server::Server;
use crate::tr;

// ---------- NBT paths ----------

#[derive(Debug, Clone)]
enum Node {
    /// `{…}` at the start: the root if it matches.
    Root(Nbt),
    Named(String),
    NamedFilter(String, Nbt),
    Index(i32),
    All,
    Match(Nbt),
}

impl Node {
    /// Tag created for a missing parent when this node follows it (list or compound).
    fn preferred_parent(&self) -> Nbt {
        match self {
            Node::Index(_) | Node::All | Node::Match(_) => Nbt::List(Vec::new()),
            _ => Nbt::Compound(Vec::new()),
        }
    }
}

/// Path read: nodes and, for each, the length of the text that ends it (error message).
struct ParsedPath<'s> {
    text: &'s str,
    nodes: Vec<(Node, usize)>,
}

impl ParsedPath<'_> {
    /// `arguments.nbtpath.nothing_found` with the start of the path up to and including node `d`.
    fn not_found(&self, d: usize) -> Fail {
        let end = self.nodes.get(d).map_or(self.text.len(), |n| n.1);
        Fail(tr!("arguments.nbtpath.nothing_found", self.text[..end].to_owned()))
    }
}

fn parse_path(s: &str) -> ParsedPath<'_> {
    let c: Vec<char> = s.chars().collect();
    // Byte position of each character (end of the text included).
    let byte_at = |i: usize| c[..i.min(c.len())].iter().map(|ch| ch.len_utf8()).sum::<usize>();
    let mut i = 0;
    let mut out: Vec<(Node, usize)> = Vec::new();
    let mut first = true;
    while i < c.len() {
        let node = match c[i] {
            '{' if first => match snbt::parse_at(&c, &mut i) {
                Ok(v) => Some(Node::Root(v)),
                Err(_) => break,
            },
            '[' => {
                i += 1;
                if c.get(i) == Some(&']') {
                    i += 1;
                    Some(Node::All)
                } else if c.get(i) == Some(&'{') {
                    let v = snbt::parse_at(&c, &mut i).ok();
                    i += 1;
                    v.map(Node::Match)
                } else {
                    let st = i;
                    while i < c.len() && c[i] != ']' {
                        i += 1;
                    }
                    let n: String = c[st..i].iter().collect();
                    i += 1;
                    Some(Node::Index(n.trim().parse().unwrap_or(0)))
                }
            }
            '.' => {
                i += 1;
                None
            }
            '"' | '\'' => {
                let mut p = snbt::P { s: &c, i };
                let name = p.quoted().unwrap_or_default();
                i = p.i;
                Some(named(&c, &mut i, name))
            }
            _ => {
                let st = i;
                while i < c.len() && !matches!(c[i], ' ' | '"' | '\'' | '[' | ']' | '.' | '{' | '}') {
                    i += 1;
                }
                if i == st {
                    break;
                }
                let name: String = c[st..i].iter().collect();
                Some(named(&c, &mut i, name))
            }
        };
        if let Some(n) = node {
            out.push((n, byte_at(i)));
        }
        first = false;
    }
    // Quirk of the game: the position of each node is recorded in a table indexed by the
    // node itself, and `[]` is a single object there; the last `[]` read therefore overwrites the position
    // of all the others ("nothing found" cites the path up to the LAST `[]`).
    if let Some(last) = out.iter().rev().find(|n| matches!(n.0, Node::All)).map(|n| n.1) {
        out.iter_mut().filter(|n| matches!(n.0, Node::All)).for_each(|n| n.1 = last);
    }
    ParsedPath { text: s, nodes: out }
}

fn named(c: &[char], i: &mut usize, name: String) -> Node {
    if c.get(*i) == Some(&'{')
        && let Ok(v) = snbt::parse_at(c, i)
    {
        return Node::NamedFilter(name, v);
    }
    Node::Named(name)
}

fn matches(f: &Nbt, v: &Nbt) -> bool {
    super::select::nbt_matches(f, v)
}

// --- Collections: typed lists and arrays (bytes, ints, longs) ---

fn coll_len(v: &Nbt) -> Option<usize> {
    match v {
        Nbt::List(l) => Some(l.len()),
        Nbt::ByteArray(a) => Some(a.len()),
        Nbt::IntArray(a) => Some(a.len()),
        Nbt::LongArray(a) => Some(a.len()),
        _ => None,
    }
}

/// Actual index (`-1`: the last) if it is within the collection.
fn resolve(i: i32, len: usize) -> Option<usize> {
    let r = if i < 0 { len as i64 + i64::from(i) } else { i64::from(i) };
    (0..len as i64).contains(&r).then_some(r as usize)
}

/// Element of a typed array (a copy: it does not exist as a tag).
fn array_elem(v: &Nbt, i: usize) -> Nbt {
    match v {
        Nbt::ByteArray(a) => Nbt::Byte(a[i] as i8),
        Nbt::IntArray(a) => Nbt::Int(a[i]),
        Nbt::LongArray(a) => Nbt::Long(a[i]),
        _ => Nbt::Int(0),
    }
}

/// Floor of a real to a 32-bit integer (saturated, NaN = 0).
fn floor_i32(x: f64) -> i32 {
    x.floor() as i32
}

/// Numeric conversions of tags: byte and short = integer truncated to their width (reals:
/// from the floor); int = saturated floor; long = truncation of a single-precision float, floor
/// of a double (asymmetry of the game reproduced).
fn byte_value(t: &Nbt) -> Option<i8> {
    Some(match *t {
        Nbt::Byte(v) => v,
        Nbt::Short(v) => v as i8,
        Nbt::Int(v) => v as i8,
        Nbt::Long(v) => v as i8,
        Nbt::Float(f) => floor_i32(f64::from(f)) as i8,
        Nbt::Double(d) => floor_i32(d) as i8,
        _ => return None,
    })
}

fn int_value(t: &Nbt) -> Option<i32> {
    Some(match *t {
        Nbt::Byte(v) => i32::from(v),
        Nbt::Short(v) => i32::from(v),
        Nbt::Int(v) => v,
        Nbt::Long(v) => v as i32,
        Nbt::Float(f) => floor_i32(f64::from(f)),
        Nbt::Double(d) => floor_i32(d),
        _ => return None,
    })
}

fn long_value(t: &Nbt) -> Option<i64> {
    Some(match *t {
        Nbt::Float(f) => f as i64,
        Nbt::Double(d) => d.floor() as i64,
        _ => t.as_i64()?,
    })
}

/// Replaces element `i`; a typed array only accepts a number (converted).
fn coll_set(v: &mut Nbt, i: usize, t: Nbt) -> bool {
    match v {
        Nbt::List(l) => l[i] = t,
        Nbt::ByteArray(a) => match byte_value(&t) {
            Some(x) => a[i] = x as u8,
            None => return false,
        },
        Nbt::IntArray(a) => match int_value(&t) {
            Some(x) => a[i] = x,
            None => return false,
        },
        Nbt::LongArray(a) => match long_value(&t) {
            Some(x) => a[i] = x,
            None => return false,
        },
        _ => return false,
    }
    true
}

/// Inserts at `i`: `Err` if the index falls outside the collection; `Ok(false)` if a typed array refuses
/// a non-numeric value (without checking the index, like the game).
fn coll_add(v: &mut Nbt, i: i64, t: Nbt) -> Result<bool, ()> {
    let len = coll_len(v).ok_or(())?;
    let fits = (0..=len as i64).contains(&i);
    let i = i.max(0) as usize;
    match v {
        Nbt::List(l) => {
            if !fits {
                return Err(());
            }
            l.insert(i, t);
        }
        Nbt::ByteArray(a) => {
            let Some(x) = byte_value(&t) else { return Ok(false) };
            if !fits {
                return Err(());
            }
            a.insert(i, x as u8);
        }
        Nbt::IntArray(a) => {
            let Some(x) = int_value(&t) else { return Ok(false) };
            if !fits {
                return Err(());
            }
            a.insert(i, x);
        }
        Nbt::LongArray(a) => {
            let Some(x) = long_value(&t) else { return Ok(false) };
            if !fits {
                return Err(());
            }
            a.insert(i, x);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn coll_remove(v: &mut Nbt, i: usize) {
    match v {
        Nbt::List(l) => {
            l.remove(i);
        }
        Nbt::ByteArray(a) => {
            a.remove(i);
        }
        Nbt::IntArray(a) => {
            a.remove(i);
        }
        Nbt::LongArray(a) => {
            a.remove(i);
        }
        _ => {}
    }
}

fn coll_clear(v: &mut Nbt) {
    match v {
        Nbt::List(l) => l.clear(),
        Nbt::ByteArray(a) => a.clear(),
        Nbt::IntArray(a) => a.clear(),
        Nbt::LongArray(a) => a.clear(),
        _ => {}
    }
}

fn eq(a: &Nbt, b: &Nbt) -> bool {
    crate::items::nbt_eq(a, b)
}

// --- Reading ---

/// Children designated by a node (read only); the elements of typed arrays are copies.
fn node_get<'a>(n: &Node, v: &'a Nbt, out: &mut Vec<Cow<'a, Nbt>>) {
    match n {
        Node::Root(f) => {
            if matches!(v, Nbt::Compound(_)) && matches(f, v) {
                out.push(Cow::Borrowed(v));
            }
        }
        Node::Named(k) => out.extend(v.get(k).map(Cow::Borrowed)),
        Node::NamedFilter(k, f) => out.extend(v.get(k).filter(|x| matches(f, x)).map(Cow::Borrowed)),
        Node::Index(i) => match v {
            Nbt::List(l) => out.extend(resolve(*i, l.len()).map(|j| Cow::Borrowed(&l[j]))),
            _ => {
                if let Some(j) = coll_len(v).and_then(|len| resolve(*i, len)) {
                    out.push(Cow::Owned(array_elem(v, j)));
                }
            }
        },
        Node::All => match v {
            Nbt::List(l) => out.extend(l.iter().map(Cow::Borrowed)),
            _ => {
                for j in 0..coll_len(v).unwrap_or(0) {
                    out.push(Cow::Owned(array_elem(v, j)));
                }
            }
        },
        Node::Match(f) => {
            if let Nbt::List(l) = v {
                out.extend(l.iter().filter(|x| matches(f, x)).map(Cow::Borrowed));
            }
        }
    }
}

fn get_all<'a>(root: &'a Nbt, p: &ParsedPath<'_>) -> Result<Vec<Cow<'a, Nbt>>, usize> {
    let mut cur: Vec<Cow<'a, Nbt>> = vec![Cow::Borrowed(root)];
    for (d, (n, _)) in p.nodes.iter().enumerate() {
        let mut next = Vec::new();
        for v in &cur {
            // An array element (copy) is a number: no node finds a child in it.
            if let Cow::Borrowed(v) = v {
                node_get(n, v, &mut next);
            }
        }
        if next.is_empty() {
            return Err(d);
        }
        cur = next;
    }
    Ok(cur)
}

/// Values designated by a path (copies); empty if a node finds nothing.
pub fn path_get(root: &Nbt, path: &str) -> Vec<Nbt> {
    get_all(root, &parse_path(path)).map(|v| v.into_iter().map(Cow::into_owned).collect()).unwrap_or_default()
}

/// Values designated by a path; otherwise `arguments.nbtpath.nothing_found` with the start of the
/// path up to the node that found nothing.
pub fn path_find(root: &Nbt, path: &str) -> Result<Vec<Nbt>, Fail> {
    let p = parse_path(path);
    get_all(root, &p).map(|v| v.into_iter().map(Cow::into_owned).collect()).map_err(|d| p.not_found(d))
}

// --- Write traversal ("get or create") ---

/// Traversal of `nodes[d..counts.len()]` from `v` then `leaf` on each tag reached, in
/// the game's order. `create`: missing children are created, of the type the next node
/// requires (`Some(last)` = tag created for the last node of the path, `None` = plain read of the
/// last node); without `create`, plain read everywhere. `counts[d]` receives the number of tags
/// found by node `d`.
fn walk(
    v: &mut Nbt,
    nodes: &[(Node, usize)],
    d: usize,
    create: Option<Option<&Nbt>>,
    counts: &mut [usize],
    leaf: &mut dyn FnMut(&mut Nbt) -> Result<usize, Fail>,
) -> Result<usize, Fail> {
    if d == counts.len() {
        return leaf(v);
    }
    let Some((n, _)) = nodes.get(d) else { return leaf(v) };
    let make: Option<Nbt> = create.and_then(|last| match nodes.get(d + 1) {
        Some((next, _)) => Some(next.preferred_parent()),
        None => last.cloned(),
    });
    let mut go = |child: &mut Nbt, counts: &mut [usize]| -> Result<usize, Fail> {
        counts[d] += 1;
        walk(child, nodes, d + 1, create, counts, leaf)
    };
    match n {
        Node::Root(f) => {
            if matches!(v, Nbt::Compound(_)) && matches(f, v) {
                return go(v, counts);
            }
            Ok(0)
        }
        Node::Named(k) => {
            let Nbt::Compound(fields) = v else { return Ok(0) };
            let at = match fields.iter().position(|(x, _)| x == k) {
                Some(i) => i,
                None => {
                    let Some(m) = make else { return Ok(0) };
                    fields.push((k.clone(), m));
                    fields.len() - 1
                }
            };
            go(&mut fields[at].1, counts)
        }
        Node::NamedFilter(k, f) => {
            let Nbt::Compound(fields) = v else { return Ok(0) };
            match fields.iter().position(|(x, _)| x == k) {
                Some(i) if matches(f, &fields[i].1) => go(&mut fields[i].1, counts),
                Some(_) => Ok(0),
                None if make.is_some() => {
                    // Missing: the pattern itself is created.
                    fields.push((k.clone(), f.clone()));
                    let last = fields.len() - 1;
                    go(&mut fields[last].1, counts)
                }
                None => Ok(0),
            }
        }
        Node::Index(i) => {
            let Some(j) = coll_len(v).and_then(|len| resolve(*i, len)) else { return Ok(0) };
            match v {
                Nbt::List(l) => go(&mut l[j], counts),
                // Typed array element: a number, no child; a copy is enough.
                _ => go(&mut array_elem(v, j), counts),
            }
        }
        Node::All => {
            let Some(len) = coll_len(v) else { return Ok(0) };
            if len == 0 {
                let Some(m) = make else { return Ok(0) };
                if coll_add(v, 0, m.clone()) != Ok(true) {
                    return Ok(0);
                }
                return match v {
                    Nbt::List(l) => go(&mut l[0], counts),
                    _ => go(&mut array_elem(v, 0), counts),
                };
            }
            let mut total = 0;
            match v {
                Nbt::List(l) => {
                    for x in l.iter_mut() {
                        total += go(x, counts)?;
                    }
                }
                _ => {
                    for j in 0..len {
                        total += go(&mut array_elem(v, j), counts)?;
                    }
                }
            }
            Ok(total)
        }
        Node::Match(f) => {
            let Nbt::List(l) = v else { return Ok(0) };
            let mut total = 0;
            let mut any = false;
            for x in l.iter_mut() {
                if matches(f, x) {
                    any = true;
                    total += go(x, counts)?;
                }
            }
            if !any && make.is_some() {
                l.push(f.clone());
                if let Some(last) = l.last_mut() {
                    total += go(last, counts)?;
                }
            }
            Ok(total)
        }
    }
}

/// Traversal creating the parents (`nodes[..n-1]`), `nothing_found` error if a level is
/// empty; `last`: `Some(default)` to also get or create the last node and apply `leaf`
/// to the tags it designates, `None` to apply `leaf` to the parents.
fn walk_created(
    root: &mut Nbt,
    p: &ParsedPath<'_>,
    last: Option<&Nbt>,
    leaf: &mut dyn FnMut(&mut Nbt) -> Result<usize, Fail>,
) -> Result<usize, Fail> {
    let n = p.nodes.len();
    if n == 0 {
        return Err(Fail(tr!("arguments.nbtpath.node.invalid")));
    }
    let mut counts = vec![0usize; if last.is_some() { n } else { n - 1 }];
    let r = walk(root, &p.nodes, 0, Some(last), &mut counts, leaf);
    // An empty parent level (the last node excepted): nothing reached the leaf.
    if let Some(d) = counts[..n - 1].iter().position(|&c| c == 0) {
        return Err(p.not_found(d));
    }
    r
}

/// The last node sets its value in `parent`; number of values changed.
fn set_tag(last: &Node, parent: &mut Nbt, value: &Nbt) -> usize {
    match last {
        Node::Root(_) => 0,
        Node::Named(k) => {
            let Nbt::Compound(f) = parent else { return 0 };
            match f.iter_mut().find(|(x, _)| x == k) {
                Some((_, old)) => {
                    let changed = !eq(old, value);
                    *old = value.clone();
                    usize::from(changed)
                }
                None => {
                    f.push((k.clone(), value.clone()));
                    1
                }
            }
        }
        Node::NamedFilter(k, flt) => {
            let Nbt::Compound(f) = parent else { return 0 };
            match f.iter_mut().find(|(x, _)| x == k) {
                Some((_, old)) if matches(flt, old) && !eq(old, value) => {
                    *old = value.clone();
                    1
                }
                _ => 0,
            }
        }
        Node::Index(i) => {
            let Some(j) = coll_len(parent).and_then(|len| resolve(*i, len)) else { return 0 };
            let prev = match parent {
                Nbt::List(l) => eq(&l[j], value),
                _ => eq(&array_elem(parent, j), value),
            };
            usize::from(!prev && coll_set(parent, j, value.clone()))
        }
        Node::All => {
            let Some(size) = coll_len(parent) else { return 0 };
            if size == 0 {
                // Counted as a change even if a typed array refuses the value (like the game).
                let _ = coll_add(parent, 0, value.clone());
                return 1;
            }
            let same = match parent {
                Nbt::List(l) => l.iter().filter(|x| eq(x, value)).count(),
                _ => (0..size).filter(|&j| eq(&array_elem(parent, j), value)).count(),
            };
            let changed = size - same;
            if changed == 0 {
                return 0;
            }
            coll_clear(parent);
            if coll_add(parent, 0, value.clone()) != Ok(true) {
                return 0;
            }
            for j in 1..size {
                let _ = coll_add(parent, j as i64, value.clone());
            }
            changed
        }
        Node::Match(flt) => {
            let Nbt::List(l) = parent else { return 0 };
            if l.is_empty() {
                l.push(value.clone());
                return 1;
            }
            let mut n = 0;
            for x in l.iter_mut() {
                if matches(flt, x) && !eq(x, value) {
                    *x = value.clone();
                    n += 1;
                }
            }
            n
        }
    }
}

/// `set`: parents obtained or created, then value set by the last node; number changed.
pub fn path_set(root: &mut Nbt, path: &str, value: Nbt) -> Result<usize, Fail> {
    let p = parse_path(path);
    let Some((last, _)) = p.nodes.last().cloned() else { return Err(Fail(tr!("arguments.nbtpath.node.invalid"))) };
    walk_created(root, &p, None, &mut |parent| Ok(set_tag(&last, parent, &value)))
}

/// The last node removes what it designates in `parent`; number removed.
fn remove_tag(last: &Node, parent: &mut Nbt) -> usize {
    match last {
        Node::Root(_) => 0,
        Node::Named(k) => {
            let Nbt::Compound(f) = parent else { return 0 };
            let before = f.len();
            f.retain(|(x, _)| x != k);
            before - f.len()
        }
        Node::NamedFilter(k, flt) => {
            let Nbt::Compound(f) = parent else { return 0 };
            let before = f.len();
            f.retain(|(x, v)| !(x == k && matches(flt, v)));
            before - f.len()
        }
        Node::Index(i) => match coll_len(parent).and_then(|len| resolve(*i, len)) {
            Some(j) => {
                coll_remove(parent, j);
                1
            }
            None => 0,
        },
        Node::All => {
            let n = coll_len(parent).unwrap_or(0);
            coll_clear(parent);
            n
        }
        Node::Match(flt) => {
            let Nbt::List(l) = parent else { return 0 };
            let before = l.len();
            l.retain(|x| !matches(flt, x));
            before - l.len()
        }
    }
}

/// `remove`: parents read (no creation or error), then removal by the last node; number removed.
pub fn path_remove(root: &mut Nbt, path: &str) -> usize {
    let p = parse_path(path);
    let Some(((last, _), parents)) = p.nodes.split_last() else { return 0 };
    let mut counts = vec![0usize; parents.len()];
    walk(root, &p.nodes, 0, None, &mut counts, &mut |parent| Ok(remove_tag(last, parent))).unwrap_or(0)
}

/// Insertion into the targeted collections (obtained or created as a list) at `index` (`-1`: at the
/// end); number of collections modified.
fn path_insert(root: &mut Nbt, path: &str, index: i32, values: &[Nbt]) -> Result<usize, Fail> {
    let p = parse_path(path);
    walk_created(root, &p, Some(&Nbt::List(Vec::new())), &mut |t| {
        let Some(len) = coll_len(t) else {
            return Err(Fail(tr!("commands.data.modify.expected_list", snbt::to_snbt(t))));
        };
        let mut at = if index < 0 { len as i64 + i64::from(index) + 1 } else { i64::from(index) };
        let mut modified = false;
        for v in values {
            match coll_add(t, at, v.clone()) {
                Ok(true) => {
                    at += 1;
                    modified = true;
                }
                Ok(false) => {}
                Err(()) => return Err(Fail(tr!("commands.data.modify.invalid_index", at))),
            }
        }
        Ok(usize::from(modified))
    })
}

/// `merge` of the sub-path: each target (obtained or created as a compound) receives the sources.
fn path_merge(root: &mut Nbt, path: &str, values: &[Nbt]) -> Result<usize, Fail> {
    let p = parse_path(path);
    walk_created(root, &p, Some(&Nbt::Compound(Vec::new())), &mut |t| {
        if !matches!(t, Nbt::Compound(_)) {
            return Err(Fail(tr!("commands.data.modify.expected_object", snbt::to_snbt(t))));
        }
        let before = t.clone();
        for v in values {
            merge(t, v);
        }
        Ok(usize::from(!eq(&before, t)))
    })
}

/// Recursive merge of compounds.
pub fn merge(base: &mut Nbt, add: &Nbt) {
    let (Nbt::Compound(b), Nbt::Compound(a)) = (base, add) else { return };
    for (k, v) in a {
        match b.iter_mut().find(|(x, _)| x == k) {
            Some((_, old)) if matches!(old, Nbt::Compound(_)) && matches!(v, Nbt::Compound(_)) => merge(old, v),
            Some((_, old)) => *old = v.clone(),
            None => b.push((k.clone(), v.clone())),
        }
    }
}

/// Typed numeric value (`execute store … byte|short|int|long|float|double`).
pub fn numeric(kind: &str, v: f64) -> Nbt {
    match kind {
        "byte" => Nbt::Byte(v as i64 as i8),
        "short" => Nbt::Short(v as i64 as i16),
        "long" => Nbt::Long(v as i64),
        "float" => Nbt::Float(v as f32),
        "double" => Nbt::Double(v),
        _ => Nbt::Int(v as i64 as i32),
    }
}

// ---------- Storage ----------

/// `data/<namespace>/command_storage.dat` → full id → compound.
pub fn read_storage(world: &Path) -> BTreeMap<String, Nbt> {
    let mut out = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(world.join("data")) else { return out };
    for e in rd.flatten() {
        let ns = e.file_name().to_string_lossy().into_owned();
        let Ok(root) = crate::nbtio::read_dat(&e.path().join("command_storage.dat")) else { continue };
        if let Some(Nbt::Compound(f)) = root.get("data").and_then(|d| d.get("contents")) {
            for (k, v) in f {
                out.insert(format!("{ns}:{k}"), v.clone());
            }
        }
    }
    out
}

pub(crate) fn storage_jobs(world: &Path, st: &BTreeMap<String, Nbt>) -> Vec<crate::save::Job> {
    let mut by_ns: BTreeMap<&str, Vec<(String, Nbt)>> = BTreeMap::new();
    for (id, v) in st {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
        by_ns.entry(ns).or_default().push((path.to_owned(), v.clone()));
    }
    by_ns
        .into_iter()
        .map(|(ns, contents)| {
            let root = Nbt::Compound(vec![
                ("data".into(), Nbt::Compound(vec![("contents".into(), Nbt::Compound(contents))])),
                ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
            ]);
            crate::save::Job::Dat {
                path: world.join("data").join(ns).join("command_storage.dat"),
                nbt: crate::nbtio::encode_root(&root),
                backup: false,
            }
        })
        .collect()
}

/// Target of `data`: block, entity or storage.
#[derive(Debug, Clone)]
pub enum Acc {
    Block([i32; 3]),
    Entity(E),
    Storage(String),
}

impl Server {
    /// NBT of a block entity, `None` without a block entity.
    pub fn data_of_block(&self, p: [i32; 3]) -> Option<Nbt> {
        let mut n = self.be_get(p)?.clone();
        crate::nbtio::remove(&mut n, "keepPacked");
        Some(n)
    }

    /// Rewrites a block entity (position and id kept) and sends it to the client.
    pub fn set_block_data(&mut self, p: [i32; 3], n: Nbt) {
        self.load_block_data(p, n);
        self.send_block_entity(p);
    }

    /// Loading the data of a block entity: the given NBT replaces
    /// everything (position and id kept), then the type's own loading effects
    /// (command block: default values, switch to automatic which schedules a tick).
    pub(crate) fn load_block_data(&mut self, p: [i32; 3], mut n: Nbt) {
        let Some(old) = self.be_get(p).cloned() else { return };
        for k in ["id", "x", "y", "z"] {
            if let Some(v) = old.get(k) {
                crate::nbtio::set(&mut n, k, v.clone());
            }
        }
        if let Some(be) = self.be_get_mut(p) {
            *be = n;
        }
        self.reload_live_be(p);
        let was_auto = crate::nbtio::int(&old, "auto").is_some_and(|v| v != 0);
        self.cmdblock_loaded(p, was_auto);
        self.be_ticks_reloaded(p, &old); // G6-F01: campfire, jukebox
    }

    /// `block_entity_data` to the client (network form: without id or position).
    pub fn send_block_entity(&self, p: [i32; 3]) {
        let Some(be) = self.be_get(p) else { return };
        let Some(kind) = self
            .data
            .registry("minecraft:block_entity_type")
            .and_then(|r| r.entries.iter().position(|e| Some(e.as_str()) == be.get("id").and_then(Nbt::as_str)))
        else {
            return;
        };
        let mut nbt = be.clone();
        for k in ["id", "x", "y", "z", "keepPacked"] {
            crate::nbtio::remove(&mut nbt, k);
        }
        let _ = self.send(Event::BlockEntityData { pos: p, kind: kind as i32, nbt: Some(nbt) });
    }

    fn acc_of(&mut self, s: &Src, p: &Parsed, seg: &Seg, kind: &str, arg: &str) -> Result<Acc, Fail> {
        let a = p.arg(seg, arg).unwrap_or_default();
        Ok(match kind {
            "block" => {
                let pos = self.loaded_pos(s, &a)?;
                if self.be_get(pos).is_none() {
                    return Err(Fail(tr!("commands.data.block.invalid")));
                }
                Acc::Block(pos)
            }
            "entity" => Acc::Entity(self.arg_entity(s, &a, false)?),
            _ => Acc::Storage(args::id(&a)),
        })
    }

    fn acc_get(&self, a: &Acc) -> Nbt {
        match a {
            Acc::Block(p) => self.data_of_block(*p).unwrap_or(Nbt::Compound(Vec::new())),
            Acc::Entity(e) => self.entity_data(*e),
            Acc::Storage(id) => self.cmd.storage.get(id).cloned().unwrap_or(Nbt::Compound(Vec::new())),
        }
    }

    /// Comparable entity data (the write without id; player: held item `SelectedItem`).
    pub fn entity_data(&self, e: E) -> Nbt {
        let mut n = self.ent_nbt(e);
        crate::nbtio::remove(&mut n, "id");
        if e == E::Player
            && let Some(Some(st)) = self.slots.get(36 + self.held as usize)
        {
            crate::nbtio::set(&mut n, "SelectedItem", st.to_nbt(None));
        }
        n
    }

    fn acc_set(&mut self, a: &Acc, n: Nbt) -> Result<(), Fail> {
        match a {
            Acc::Block(p) => self.set_block_data(*p, n),
            Acc::Entity(e) => {
                if *e == E::Player {
                    return Err(Fail(tr!("commands.data.entity.invalid")));
                }
                self.ent_set_nbt(*e, &n);
            }
            Acc::Storage(id) => {
                self.cmd.storage.insert(id.clone(), n);
                self.cmd.storage_dirty = true;
            }
        }
        Ok(())
    }

    fn acc_modified(&self, a: &Acc) -> Component {
        match a {
            Acc::Block(p) => tr!("commands.data.block.modified", p[0], p[1], p[2]),
            Acc::Entity(e) => tr!("commands.data.entity.modified", self.ent_display(*e)),
            Acc::Storage(id) => tr!("commands.data.storage.modified", id.clone()),
        }
    }

    fn acc_print(&self, a: &Acc, v: &Nbt) -> Component {
        let pretty = snbt::pretty(v);
        match a {
            Acc::Block(p) => tr!("commands.data.block.query", p[0], p[1], p[2], pretty),
            Acc::Entity(e) => tr!("commands.data.entity.query", self.ent_display(*e), pretty),
            Acc::Storage(id) => tr!("commands.data.storage.query", id.clone(), pretty),
        }
    }

    /// Data source of `execute if data` (block, entity, storage).
    pub fn data_source(&mut self, s: &Src, p: &Parsed, seg: &Seg, kind: &str) -> Result<Nbt, Fail> {
        let arg = match kind {
            "block" => "source",
            "entity" => "source",
            _ => "source",
        };
        let acc = self.acc_of(s, p, seg, kind, arg)?;
        Ok(self.acc_get(&acc))
    }

    /// `execute if items`: number of matching items.
    pub fn count_items(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> Result<i32, Fail> {
        let pred = super::itempred::ItemPred::parse(&p.arg(seg, "item_predicate").unwrap_or_default());
        let range = p.arg(seg, "slots").unwrap_or_default();
        // Source like `/item … from`: entities (at least one) or loaded container; slots of the
        // range that each target owns, through the same slot table as `item` and `loot`
        // (cmds_s9b, mob equipment included).
        let kind = item_source_kind(seg);
        let h = self.item_target(s, p, seg, kind, "source")?;
        Ok(self
            .holder_slots(&h, &range)
            .into_iter()
            .flat_map(|(_, v)| v)
            .filter_map(|c| self.slot_get(c))
            .filter(|st| self.item_pred_test(&pred, st))
            .map(|st| st.count)
            .sum())
    }

    /// `execute if|unless slots`: number of slots of the range that the source owns (empty ones
    /// included), same sources as `count_items`.
    pub fn count_slots(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> Result<i32, Fail> {
        let range = p.arg(seg, "slots").unwrap_or_default();
        let kind = item_source_kind(seg);
        let h = self.item_target(s, p, seg, kind, "source")?;
        Ok(self.holder_slots(&h, &range).iter().map(|(_, v)| v.len() as i32).sum())
    }

    /// `compute float|integer <provider>` of `data modify`: value of a numeric provider
    /// (registry id or inline content) in the loot context of a command:
    /// entity and position of the source; `block`: state and block entity of the position; `entity`:
    /// the target. Random draw of the world.
    pub(crate) fn compute_number(
        &mut self,
        s: &Src,
        p: &Parsed,
        seg: &Seg,
        ctx_kind: &str,
        float: bool,
    ) -> Result<Nbt, Fail> {
        let reg = provider_registry(float);
        let text = p.arg(seg, "provider").unwrap_or_default();
        let t = text.trim();
        let provider = if t.starts_with('{') || t.starts_with('[') {
            let n = snbt::parse(t).map_err(|_| Fail(tr!("argument.resource_or_id.failed_to_parse", t.to_owned())))?;
            let v = super::cmds_s9b::nbt_to_json(&n);
            // The game decodes the content as soon as the argument is read and refuses what it cannot read.
            let exists = |r: &str, id: &str| self.life.pack.get(r, &args::id(id)).is_some();
            if !provider_direct_ok(&v, float, &exists) {
                return Err(Fail(tr!("argument.resource_or_id.failed_to_parse", t.to_owned())));
            }
            v
        } else {
            let id = args::id(t);
            if self.life.pack.get(reg, &id).is_none() {
                return Err(Fail(tr!("argument.resource_or_id.no_such_element", id, reg.to_owned())));
            }
            serde_json::Value::String(id)
        };
        let target = match ctx_kind {
            "entity" => Some(self.arg_entity(s, &p.arg(seg, "computeTarget").unwrap_or_default(), false)?),
            _ => None,
        };
        let state = match ctx_kind {
            "block" => {
                let pos = self.loaded_pos(s, &p.arg(seg, "computePos").unwrap_or_default())?;
                Some((self.get(pos), self.be_get(pos).cloned()))
            }
            _ => None,
        };
        let block = state.as_ref().map(|(st, _)| {
            let name = self.data.block_of(*st).map_or("minecraft:air", |b| b.name.as_str());
            let props = self.data.state_properties(*st).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
            (name, props)
        });
        let this_type = s.entity.map(|e| self.ent_type(e));
        let (tags, data) = (&self.block_tags, &self.data);
        let tag_fn = |tag: &str, name: &str| {
            let b = data.blocks.iter().position(|x| x.name == name).map_or(u32::MAX, |i| i as u32);
            tags.get(tag.trim_start_matches('#')).is_some_and(|v| v.binary_search(&b).is_ok())
        };
        let mut rng = self.life.rng;
        let value = {
            use crate::progress::pred::{Ctx as PCtx, Who};
            let world = self.loot_world(PCtx {
                this: s.entity.map(Who::E),
                origin: Some(s.pos),
                target: target.map(Who::E),
                block: state.as_ref().map(|(st, _)| *st),
                ..Default::default()
            });
            let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
            let ctx = crate::loot::Ctx {
                block,
                block_entity: state.as_ref().and_then(|(_, be)| be.as_ref()),
                this_type,
                world: Some(&world),
                ..Default::default()
            };
            if float {
                Nbt::Float(loot.float(&provider, &ctx, &mut rng))
            } else {
                Nbt::Int(loot.int(&provider, &ctx, &mut rng))
            }
        };
        self.life.rng = rng;
        Ok(value)
    }

    /// Commands `data`, `item`, `loot`.
    pub(crate) fn cmd_data(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let op = w.get(1).copied().unwrap_or("");
        let kind = w.get(2).copied().unwrap_or("");
        let target_arg = match kind {
            "block" => "target",
            _ => "target",
        };
        let acc = self.acc_of(s, p, seg, kind, target_arg)?;
        match op {
            "get" => {
                let data = self.acc_get(&acc);
                let Some(path) = p.arg(seg, "path") else {
                    let msg = self.acc_print(&acc, &data);
                    self.ok(s, msg);
                    return Ok(1);
                };
                let found = path_find(&data, &path)?;
                if found.len() > 1 {
                    return Err(Fail(tr!("commands.data.get.multiple")));
                }
                let v = &found[0];
                if let Some(sc) = p.arg(seg, "scale") {
                    let scale: f64 = sc.parse().unwrap_or(1.0);
                    let Some(x) = as_f64(v) else { return Err(Fail(tr!("commands.data.get.invalid", path))) };
                    let r = (x * scale).floor() as i32;
                    let sc2 = format!("{scale:.2}");
                    let msg = match &acc {
                        Acc::Block(b) => tr!("commands.data.block.get", path.clone(), b[0], b[1], b[2], sc2, r),
                        Acc::Entity(e) => tr!("commands.data.entity.get", path.clone(), self.ent_display(*e), sc2, r),
                        Acc::Storage(id) => tr!("commands.data.storage.get", path.clone(), id.clone(), sc2, r),
                    };
                    self.ok(s, msg);
                    return Ok(r);
                }
                let r = match v {
                    Nbt::List(l) => l.len() as i32,
                    Nbt::Compound(f) => f.len() as i32,
                    Nbt::String(t) => t.encode_utf16().count() as i32,
                    Nbt::ByteArray(a) => a.len() as i32,
                    Nbt::IntArray(a) => a.len() as i32,
                    Nbt::LongArray(a) => a.len() as i32,
                    n => as_f64(n).unwrap_or(0.0).floor() as i32,
                };
                let msg = self.acc_print(&acc, v);
                self.ok(s, msg);
                Ok(r)
            }
            "merge" => {
                let add = snbt::parse(&p.arg(seg, "nbt").unwrap_or_default())
                    .map_err(|_| Fail(tr!("argument.nbt.expected.compound")))?;
                let old = self.acc_get(&acc);
                let mut new = old.clone();
                merge(&mut new, &add);
                if crate::items::nbt_eq(&old, &new) {
                    return Err(Fail(tr!("commands.data.merge.failed")));
                }
                self.acc_set(&acc, new)?;
                let msg = self.acc_modified(&acc);
                self.ok(s, msg);
                Ok(1)
            }
            "remove" => {
                let mut data = self.acc_get(&acc);
                let path = p.arg(seg, "path").unwrap_or_default();
                let n = path_remove(&mut data, &path);
                if n == 0 {
                    return Err(Fail(tr!("commands.data.merge.failed")));
                }
                self.acc_set(&acc, data)?;
                let msg = self.acc_modified(&acc);
                self.ok(s, msg);
                Ok(n as i32)
            }
            "modify" => self.data_modify(s, p, seg, &acc, &w),
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    fn data_modify(&mut self, s: &Src, p: &Parsed, seg: &Seg, acc: &Acc, w: &[&str]) -> R {
        // Path: data modify <kind> <target> <targetPath> <op> [<index>] <source…>
        let opi = w.iter().position(|x| matches!(*x, "set" | "merge" | "append" | "prepend" | "insert")).unwrap_or(0);
        let op = w.get(opi).copied().unwrap_or("set");
        let src_i = if op == "insert" { opi + 2 } else { opi + 1 };
        let how = w.get(src_i).copied().unwrap_or("value");
        let values: Vec<Nbt> = match how {
            "value" => vec![
                snbt::parse(&p.arg(seg, "value").unwrap_or_default())
                    .map_err(|_| Fail(tr!("argument.nbt.invalid", "")))?,
            ],
            "from" | "string" => {
                let kind = w.get(src_i + 1).copied().unwrap_or("storage");
                let arg = match kind {
                    "block" => "source",
                    _ => "source",
                };
                let sacc = self.acc_of(s, p, seg, kind, arg)?;
                let data = self.acc_get(&sacc);
                let mut vals = match p.arg(seg, "sourcePath") {
                    Some(sp) => path_find(&data, &sp)?,
                    None => vec![data],
                };
                if how == "string" {
                    let start: Option<i32> = p.arg(seg, "start").and_then(|v| v.parse().ok());
                    let end: Option<i32> = p.arg(seg, "end").and_then(|v| v.parse().ok());
                    let mut out = Vec::new();
                    for v in vals {
                        let t = match &v {
                            Nbt::String(x) => x.clone(),
                            Nbt::Compound(_)
                            | Nbt::List(_)
                            | Nbt::ByteArray(_)
                            | Nbt::IntArray(_)
                            | Nbt::LongArray(_) => {
                                return Err(Fail(tr!("commands.data.modify.expected_value", snbt::to_snbt(&v))));
                            }
                            n => snbt::to_snbt(n),
                        };
                        let chars: Vec<char> = t.chars().collect();
                        let len = chars.len() as i32;
                        let off = |i: i32| if i >= 0 { i } else { len + i };
                        let (a, b) = (start.map_or(0, off), end.map_or(len, off));
                        if a < 0 || b > len || a > b {
                            return Err(Fail(tr!("commands.data.modify.invalid_substring", a, b)));
                        }
                        out.push(Nbt::String(chars[a as usize..b as usize].iter().collect()));
                    }
                    vals = out;
                }
                vals
            }
            "compute" => {
                let ctx_kind = w.get(src_i + 1).copied().unwrap_or("default");
                // `… compute <context> [<position>|<target>] float|integer <provider>`.
                let float = w[src_i..].contains(&"float");
                vec![self.compute_number(s, p, seg, ctx_kind, float)?]
            }
            _ => return Err(Fail(tr!("command.failed"))),
        };
        let mut data = self.acc_get(acc);
        let path = p.arg(seg, "targetPath").unwrap_or_default();
        let n = match op {
            "set" => {
                let Some(last) = values.last().cloned() else { return Err(Fail(tr!("commands.data.merge.failed"))) };
                path_set(&mut data, &path, last)?
            }
            "merge" => {
                for v in &values {
                    if !matches!(v, Nbt::Compound(_)) {
                        return Err(Fail(tr!("commands.data.modify.expected_object", snbt::to_snbt(v))));
                    }
                }
                path_merge(&mut data, &path, &values)?
            }
            "append" => path_insert(&mut data, &path, -1, &values)?,
            "prepend" => path_insert(&mut data, &path, 0, &values)?,
            _ => {
                let idx: i32 = p.arg(seg, "index").and_then(|v| v.parse().ok()).unwrap_or(0);
                path_insert(&mut data, &path, idx, &values)?
            }
        };
        if n == 0 {
            return Err(Fail(tr!("commands.data.merge.failed")));
        }
        self.acc_set(acc, data)?;
        let msg = self.acc_modified(acc);
        self.ok(s, msg);
        Ok(n as i32)
    }
}

/// Registry of a context numeric provider.
fn provider_registry(float: bool) -> &'static str {
    if float { "context_float_provider" } else { "context_int_provider" }
}

/// Numeric provider written inline, as the game's decoder accepts it: a number, or a
/// compound whose `type` is known and whose required fields are present and readable (nested
/// providers are checked in turn; conditions, targets, attributes and paths
/// are only checked for presence). `exists(registry, id)`: element of the data registry.
fn provider_direct_ok(v: &serde_json::Value, float: bool, exists: &dyn Fn(&str, &str) -> bool) -> bool {
    use serde_json::Value;
    let o = match v {
        Value::Number(_) | Value::Bool(_) => return true,
        Value::Object(o) => o,
        _ => return false,
    };
    let Some(ty) = o.get("type").and_then(Value::as_str) else { return false };
    let ty = args::id(ty);
    let Some(ty) = ty.strip_prefix("minecraft:") else { return false };
    // Nested provider: existing registry reference, or inline content.
    let holder = |x: &Value, f: bool| match x {
        Value::String(id) => exists(provider_registry(f), id),
        _ => provider_direct_ok(x, f, exists),
    };
    let req = |k: &str, f: bool| o.get(k).is_some_and(|x| holder(x, f));
    let opt = |k: &str, f: bool| o.get(k).is_none_or(|x| holder(x, f));
    let present = |k: &str| o.contains_key(k);
    let string = |k: &str| o.get(k).is_some_and(Value::is_string);
    // List of providers, or a single one (the `#…` tags of this registry do not exist).
    let set = |k: &str| match o.get(k) {
        Some(Value::Array(a)) => a.iter().all(|x| holder(x, float)),
        Some(x) => holder(x, float),
        None => false,
    };
    match ty {
        "constant" => o.get("value").is_some_and(|x| x.is_number() || x.is_boolean()),
        "abs" | "negate" => req("input", float),
        "ceil" | "cos" | "floor" | "round" | "sin" | "sqrt" | "truncate" if float => req("input", true),
        "from_int" if float => req("input", false),
        "from_float" if !float => req("input", true),
        "avg" | "max" | "min" | "mul" | "add" => set("inputs"),
        "length" if float => set("inputs"),
        "sub" | "mod" | "div" => req("left", float) && req("right", float),
        "floor_mod" | "floor_div" if !float => req("left", false) && req("right", false),
        "pow" => req("base", float) && req("exponent", float),
        "uniform" => req("min", float) && req("max", float),
        "binomial" if !float => req("n", false) && req("p", true),
        "conditional" => present("condition") && req("on_true", float) && opt("on_false", float),
        "number_dispatcher" => {
            let cases = match o.get("cases") {
                Some(Value::Array(a)) => {
                    a.iter().all(|c| c.get("condition").is_some() && c.get("value").is_some_and(|x| holder(x, float)))
                }
                _ => false,
            };
            cases && opt("default", float)
        }
        "weighted_list" => match o.get("distribution") {
            Some(Value::Array(a)) if !a.is_empty() => a.iter().all(|w| {
                w.get("data").is_some_and(|x| holder(x, float))
                    && w.get("weight").and_then(Value::as_f64).is_some_and(|x| x as i32 >= 0)
            }),
            _ => false,
        },
        "environment_attribute" => string("attribute"),
        "enchantment_level" if float => present("amount"),
        "storage" => string("storage") && string("path") && opt("fallback", float),
        "score" if !float => present("target") && string("score") && opt("fallback", false),
        _ => false,
    }
}

fn as_f64(v: &Nbt) -> Option<f64> {
    match v {
        Nbt::Float(f) => Some(f64::from(*f)),
        Nbt::Double(d) => Some(*d),
        Nbt::Byte(_) | Nbt::Short(_) | Nbt::Int(_) | Nbt::Long(_) => v.as_i64().map(|i| i as f64),
        _ => None,
    }
}

/// Slot ids designated by a slot name (the game's slot table: `contents` 0,
/// container 0-53, hotbar 0-8, inventory 9-35, Ender chest 200-226, mob inventory
/// 300-307, horse 500-514, hands 98-99, armor 100-103, body 105, saddle 106, horse chest and
/// cursor 499, crafting 500-503).
pub fn slot_ids(name: &str) -> Vec<i32> {
    const RANGES: [(&str, i32, i32); 7] = [
        ("container.", 0, 54),
        ("hotbar.", 0, 9),
        ("inventory.", 9, 27),
        ("enderchest.", 200, 27),
        ("mob.inventory.", 300, 8),
        ("horse.", 500, 15),
        ("player.crafting.", 500, 4),
    ];
    for &(prefix, offset, size) in &RANGES {
        let Some(rest) = name.strip_prefix(prefix) else { continue };
        if rest == "*" {
            return (offset..offset + size).collect();
        }
        if let Ok(i) = rest.parse::<i32>()
            && (0..size).contains(&i)
            && rest == i.to_string()
        {
            return vec![offset + i];
        }
    }
    match name {
        "contents" => vec![0],
        "weapon" | "weapon.mainhand" => vec![98],
        "weapon.offhand" => vec![99],
        "weapon.*" => vec![98, 99],
        "armor.head" => vec![103],
        "armor.chest" => vec![102],
        "armor.legs" => vec![101],
        "armor.feet" => vec![100],
        "armor.body" => vec![105],
        "armor.*" => vec![103, 102, 101, 100, 105],
        "saddle" => vec![106],
        "horse.chest" | "player.cursor" => vec![499],
        _ => Vec::new(),
    }
}

/// Kind of source of `execute if|unless items|slots`: the word that follows `items` or `slots`
/// in the path (`block` or `entity`).
fn item_source_kind(seg: &Seg) -> &str {
    let at = seg.path.iter().position(|w| w == "items" || w == "slots");
    at.and_then(|i| seg.path.get(i + 1)).map_or("", String::as_str)
}

/// Equipment slot of a mob (hand, other hand, feet, legs, chest, head, body, saddle)
/// for a slot id.
pub(crate) fn equipment_index(id: i32) -> Option<usize> {
    Some(match id {
        98 => 0,
        99 => 1,
        100..=103 => (id - 98) as usize,
        105 => 6,
        106 => 7,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        let mut n = snbt::parse("{a:{b:[1,2,3]},c:[{x:1},{x:2}]}").unwrap();
        assert_eq!(path_get(&n, "a.b[1]"), vec![Nbt::Int(2)]);
        assert_eq!(path_get(&n, "a.b[-1]"), vec![Nbt::Int(3)]);
        assert_eq!(path_get(&n, "c[{x:2}].x"), vec![Nbt::Int(2)]);
        assert_eq!(path_get(&n, "c[].x").len(), 2);
        assert_eq!(path_set(&mut n, "a.d.e", Nbt::Int(5)).unwrap(), 1);
        assert_eq!(path_get(&n, "a.d.e"), vec![Nbt::Int(5)]);
        assert_eq!(path_set(&mut n, "a.d.e", Nbt::Int(5)).unwrap(), 0);
        assert_eq!(path_remove(&mut n, "c[0]"), 1);
        assert_eq!(path_get(&n, "c[].x"), vec![Nbt::Int(2)]);
        assert_eq!(path_insert(&mut n, "a.b", 0, &[Nbt::Int(0)]).unwrap(), 1);
        assert_eq!(path_get(&n, "a.b[0]"), vec![Nbt::Int(0)]);
        let mut m = snbt::parse("{a:{x:1}}").unwrap();
        merge(&mut m, &snbt::parse("{a:{y:2},b:3}").unwrap());
        assert_eq!(path_get(&m, "a.y"), vec![Nbt::Int(2)]);
    }

    fn msg(f: Fail) -> String {
        format!("{:?}", f.0)
    }

    /// Index and "all" on typed arrays (read, set, remove, insert).
    #[test]
    fn typed_array_paths() {
        let mut n = snbt::parse("{UUID:[I;1,2,3,4],B:[B;1b],L:[L;5L]}").unwrap();
        assert_eq!(path_get(&n, "UUID[0]"), vec![Nbt::Int(1)]);
        assert_eq!(path_get(&n, "UUID[-1]"), vec![Nbt::Int(4)]);
        assert_eq!(path_get(&n, "UUID[]").len(), 4);
        assert_eq!(path_get(&n, "B[0]"), vec![Nbt::Byte(1)]);
        assert_eq!(path_set(&mut n, "UUID[0]", Nbt::Int(9)).unwrap(), 1);
        assert_eq!(path_set(&mut n, "UUID[0]", Nbt::Int(9)).unwrap(), 0);
        // Numeric conversion: truncated byte, real by floor.
        assert_eq!(path_set(&mut n, "B[0]", Nbt::Int(300)).unwrap(), 1);
        assert_eq!(path_get(&n, "B[0]"), vec![Nbt::Byte(44)]);
        assert_eq!(path_set(&mut n, "UUID[1]", Nbt::Double(-1.5)).unwrap(), 1);
        assert_eq!(path_get(&n, "UUID[1]"), vec![Nbt::Int(-2)]);
        // Long from a single-precision float: truncation (the game), not the floor.
        assert_eq!(path_set(&mut n, "L[0]", Nbt::Float(-1.5)).unwrap(), 1);
        assert_eq!(path_get(&n, "L[0]"), vec![Nbt::Long(-1)]);
        // Non-numeric value refused.
        assert_eq!(path_set(&mut n, "UUID[0]", Nbt::String("x".into())).unwrap(), 0);
        assert_eq!(path_remove(&mut n, "UUID[0]"), 1);
        assert_eq!(n.get("UUID"), Some(&Nbt::IntArray(vec![-2, 3, 4])));
        assert_eq!(path_insert(&mut n, "UUID", 0, &[Nbt::Byte(7)]).unwrap(), 1);
        assert_eq!(path_insert(&mut n, "UUID", 0, &[Nbt::String("x".into())]).unwrap(), 0);
        assert_eq!(n.get("UUID"), Some(&Nbt::IntArray(vec![7, -2, 3, 4])));
        assert_eq!(path_set(&mut n, "UUID[]", Nbt::Int(0)).unwrap(), 4);
        assert_eq!(n.get("UUID"), Some(&Nbt::IntArray(vec![0, 0, 0, 0])));
        assert_eq!(path_remove(&mut n, "UUID[]"), 4);
        assert_eq!(n.get("UUID"), Some(&Nbt::IntArray(vec![])));
        // Quirk of the game: `[]` on an empty array counts a change even if refused.
        assert_eq!(path_set(&mut n, "UUID[]", Nbt::String("x".into())).unwrap(), 1);
        assert_eq!(n.get("UUID"), Some(&Nbt::IntArray(vec![])));
    }

    /// Edge cases of `set` and the game's error messages.
    #[test]
    fn paths_set_and_messages() {
        let mut n = snbt::parse("{l:[],m:[{a:1}],e:[]}").unwrap();
        // `[]` on an empty list: one element added.
        assert_eq!(path_set(&mut n, "l[]", Nbt::Int(1)).unwrap(), 1);
        assert_eq!(n.get("l"), Some(&Nbt::List(vec![Nbt::Int(1)])));
        // `[{…}]` without a match in a non-empty list: nothing; empty list: the value added.
        assert_eq!(path_set(&mut n, "m[{a:2}]", Nbt::Int(5)).unwrap(), 0);
        assert_eq!(path_set(&mut n, "e[{a:2}]", Nbt::Int(5)).unwrap(), 1);
        assert_eq!(n.get("e"), Some(&Nbt::List(vec![Nbt::Int(5)])));
        // `name{…}` missing as the last node: nothing is created.
        assert_eq!(path_set(&mut n, "x{a:1}", Nbt::Int(1)).unwrap(), 0);
        assert!(n.get("x").is_none());
        // Parent not found: start of the path up to the empty node.
        let mut v = snbt::parse("{a:1}").unwrap();
        let e = msg(path_set(&mut v, "a.b.c", Nbt::Int(1)).unwrap_err());
        assert!(e.contains("nothing_found") && e.contains("\"a.b\""), "{e}");
        let e = msg(path_find(&snbt::parse("{a:{b:1}}").unwrap(), "a.c.d").unwrap_err());
        assert!(e.contains("\"a.c\"") && !e.contains("a.c.d"), "{e}");
        // Insertion index: the resolved index.
        let mut v = snbt::parse("{l:[1]}").unwrap();
        let e = msg(path_insert(&mut v, "l", -3, &[Nbt::Int(2)]).unwrap_err());
        assert!(e.contains("invalid_index") && (e.contains("Int(-1)") || e.contains("\"-1\"")), "{e}");
        let e = msg(path_insert(&mut v, "l", 5, &[Nbt::Int(2)]).unwrap_err());
        assert!(e.contains("invalid_index") && (e.contains("Int(5)") || e.contains("\"5\"")), "{e}");
        // `merge`: the missing target is created.
        let mut v = snbt::parse("{}").unwrap();
        assert_eq!(path_merge(&mut v, "a", &[snbt::parse("{x:1}").unwrap()]).unwrap(), 1);
        assert_eq!(path_get(&v, "a.x"), vec![Nbt::Int(1)]);
    }

    /// Quirk of the game: all the `[]` of a path share the position of the last one.
    #[test]
    fn path_all_shared_position() {
        let n = snbt::parse("{a:[]}").unwrap();
        let e = msg(path_find(&n, "a[].b[].c").unwrap_err());
        assert!(e.contains("\"a[].b[]\""), "{e}");
        let e = msg(path_find(&snbt::parse("{a:{b:1}}").unwrap(), "a.c[].d").unwrap_err());
        assert!(e.contains("\"a.c\""), "{e}");
    }

    /// Inline providers: what the game's decoder accepts or refuses.
    #[test]
    fn inline_providers() {
        let exists = |r: &str, id: &str| r == "context_int_provider" && args::id(id) == "minecraft:known";
        let ok = |s: &str, f: bool| {
            provider_direct_ok(&super::super::cmds_s9b::nbt_to_json(&snbt::parse(s).unwrap()), f, &exists)
        };
        assert!(ok("{type:\"minecraft:constant\",value:7}", false));
        assert!(ok("{type:constant,value:1b}", true));
        assert!(!ok("{type:\"minecraft:nope\"}", false));
        assert!(!ok("{}", false));
        assert!(!ok("[1]", false));
        assert!(!ok("{type:constant}", false));
        assert!(!ok("{type:uniform,min:1}", true));
        assert!(ok("{type:uniform,min:1,max:{type:abs,input:known}}", false));
        assert!(!ok("{type:uniform,min:1,max:{type:abs,input:unknown}}", false));
        // Types specific to only one of the two registries.
        assert!(ok("{type:sqrt,input:2}", true));
        assert!(!ok("{type:sqrt,input:2}", false));
        assert!(ok("{type:floor_div,left:1,right:2}", false));
        assert!(!ok("{type:floor_div,left:1,right:2}", true));
        assert!(ok("{type:binomial,n:3,p:0.5}", false));
        assert!(ok("{type:from_float,input:1.5}", false));
        assert!(!ok("{type:from_float,input:known}", false));
        assert!(ok("{type:add,inputs:[1,2]}", false));
        assert!(ok("{type:add,inputs:1}", false));
        assert!(!ok("{type:add,inputs:[1,{type:nope}]}", false));
        assert!(ok("{type:weighted_list,distribution:[{data:1,weight:2}]}", false));
        assert!(!ok("{type:weighted_list,distribution:[]}", false));
        assert!(!ok("{type:weighted_list,distribution:[{data:1,weight:-1}]}", false));
        assert!(ok("{type:conditional,condition:{condition:\"minecraft:survives_explosion\"},on_true:1}", false));
        assert!(!ok("{type:conditional,on_true:1}", false));
        assert!(ok("{type:number_dispatcher,cases:[{condition:x,value:1}],default:2}", true));
        assert!(!ok("{type:storage,storage:\"t:t\"}", false));
        assert!(ok("{type:storage,storage:\"t:t\",path:\"a\",fallback:3}", false));
        assert!(ok("{type:score,target:this,score:s}", false));
        assert!(!ok("{type:score,target:this,score:s}", true));
        assert!(!ok("{type:\"other:constant\",value:1}", false));
    }

    #[test]
    fn entity_slots() {
        assert_eq!(slot_ids("weapon.mainhand"), vec![98]);
        assert_eq!(slot_ids("armor.*"), vec![103, 102, 101, 100, 105]);
        assert_eq!(slot_ids("contents"), vec![0]);
        assert_eq!(slot_ids("container.0"), vec![0]);
        assert_eq!(slot_ids("hotbar.*").len(), 9);
        assert_eq!(slot_ids("horse.chest"), vec![499]);
        assert!(slot_ids("container.54").is_empty());
        assert_eq!([98, 99, 100, 101, 102, 103, 105, 106].map(equipment_index), [0, 1, 2, 3, 4, 5, 6, 7].map(Some));
        assert_eq!(equipment_index(104), None);
    }
}
