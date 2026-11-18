//! Walk over the nested data of a value, following the map of a schema.
//!
//! [`visit`] calls back on every piece of data the map knows (each with its kind), outermost
//! first, then descends into it as it is after the call: a rule that rewrites a node is applied
//! before the nodes it holds, like the game's rules (applied everywhere, top-down).
//!
//! Before any rule, [`read_as_schema`] reads the data the way the game's conversion does: typed
//! identifiers get their namespace, and an optional field whose value does not read as its node
//! (an entity list holding an unknown entity, say) is left out of the conversion. Such a field is
//! hidden from the rules under a marked key for the time of the conversion ([`restore_hidden`]).

use std::borrow::Cow;

use rc_proto::nbt::Nbt;

use crate::kind::Kind;
use crate::map::{Node, Schema, shape};

/// Prefix of a field hidden from the rules: no saved data starts a key with this control char.
const HIDDEN: &str = "\u{1}unread:";

/// Calls `on` with every piece of data of `value` (of `kind`) and their kinds, outermost first.
pub(crate) fn visit(kind: Kind, value: &mut Nbt, schema: Schema, on: &mut dyn FnMut(Kind, &mut Nbt)) {
    Walk { schema, on, hide_unread: false, hidden: 0 }.kind(kind, value);
}

/// Reads `value` (of `kind`) as the game's conversion does with `schema`, before any rule:
/// typed identifiers and choice keys get their namespace, an item stack written as a bare
/// identifier becomes one item, and optional fields that do not read are hidden from the rules.
/// Returns how many fields were hidden.
pub(crate) fn read_as_schema(kind: Kind, value: &mut Nbt, schema: Schema) -> usize {
    let mut on = |kind: Kind, node: &mut Nbt| match kind {
        k if k.is_name() => namespace_in_place(node),
        Kind::ItemStack => expand_bare_stack(node),
        Kind::Entity | Kind::BlockEntity => namespace_field(node, "id"),
        _ => {}
    };
    let mut walk = Walk { schema, on: &mut on, hide_unread: true, hidden: 0 };
    walk.kind(kind, value);
    walk.hidden
}

/// Gives back their keys to the fields [`read_as_schema`] hid. A rule that set a field of the
/// same name since wins, the way the game's rules overwrite the unread remainder.
pub(crate) fn restore_hidden(value: &mut Nbt) {
    match value {
        Nbt::Compound(entries) => {
            for (_, v) in entries.iter_mut() {
                restore_hidden(v);
            }
            let set_since: Vec<String> = entries.iter().filter(|(k, _)| !k.starts_with(HIDDEN)).map(|(k, _)| k.clone()).collect();
            entries.retain(|(k, _)| k.strip_prefix(HIDDEN).is_none_or(|name| !set_since.iter().any(|s| s == name)));
            for (k, _) in entries.iter_mut() {
                if let Some(name) = k.strip_prefix(HIDDEN) {
                    *k = name.to_owned();
                }
            }
        }
        Nbt::List(items) => items.iter_mut().for_each(restore_hidden),
        _ => {}
    }
}

struct Walk<'a> {
    schema: Schema,
    on: &'a mut dyn FnMut(Kind, &mut Nbt),
    /// Reading before the rules: optional fields that do not read are hidden.
    hide_unread: bool,
    hidden: usize,
}

impl Walk<'_> {
    fn kind(&mut self, kind: Kind, value: &mut Nbt) {
        (self.on)(kind, value);
        self.node(shape(kind, self.schema), value);
    }

    fn node(&mut self, node: Node, value: &mut Nbt) {
        match node {
            Node::Of(kind) => self.kind(kind, value),
            Node::List(inner) => {
                if let Nbt::List(items) = value {
                    for item in items {
                        self.node(*inner, item);
                    }
                }
            }
            Node::Fields(fields) => self.fields(fields, value, true),
            Node::Required(fields) => self.fields(fields, value, false),
            Node::Values(inner) => {
                if let Nbt::Compound(entries) = value {
                    for (_, v) in entries {
                        self.node(*inner, v);
                    }
                }
            }
            Node::KeysOf(kind) => self.keys(Some(kind), value, Node::Plain),
            Node::NamespacedKeys(inner) => self.keys(None, value, *inner),
            Node::Either(options) => {
                let schema = self.schema;
                // Before the rules the whole value must read; afterwards unread parts are hidden
                // already and the form decides.
                let chosen = if self.hide_unread {
                    options.iter().find(|o| reads(**o, value, schema))
                } else {
                    options.iter().find(|o| fits_form(**o, value, schema))
                };
                if let Some(chosen) = chosen {
                    self.node(*chosen, value);
                }
            }
            Node::All(parts) => {
                for part in parts {
                    self.node(*part, value);
                }
            }
            Node::Choice { key, namespaced, cases, .. } => {
                let Some(id) = value.get(key).and_then(Nbt::as_str) else { return };
                if let Some(case) = choice_case(id, namespaced, cases) {
                    self.node(case, value);
                }
            }
            Node::PackedCriterion => self.criterion(value),
            Node::Namespaced | Node::Text | Node::Plain => {}
        }
    }

    /// The listed fields of a compound; an `optional` one that does not read is hidden when
    /// reading before the rules.
    fn fields(&mut self, fields: &[(&'static str, Node)], value: &mut Nbt, optional: bool) {
        let Nbt::Compound(entries) = value else { return };
        let schema = self.schema;
        for (name, inner) in fields {
            let Some((key, v)) = entries.iter_mut().find(|(k, _)| k == name) else { continue };
            if self.hide_unread && optional && !reads(*inner, v, schema) {
                *key = format!("{HIDDEN}{key}");
                self.hidden += 1;
                continue;
            }
            self.node(*inner, v);
        }
    }

    /// The keys of a compound, each an identifier (of `kind`, or of none), then its values.
    fn keys(&mut self, kind: Option<Kind>, value: &mut Nbt, values: Node) {
        let Nbt::Compound(entries) = value else { return };
        for (key, v) in entries.iter_mut() {
            let mut name = Nbt::String(std::mem::take(key));
            match kind {
                Some(kind) => (self.on)(kind, &mut name),
                None => namespace_in_place(&mut name),
            }
            if let Nbt::String(s) = name {
                *key = s;
            }
            self.node(values, v);
        }
    }

    /// A packed criterion: its id visited as the identifier its type names, then packed again.
    fn criterion(&mut self, objective: &mut Nbt) {
        let Some(packed) = objective.get("CriteriaName").and_then(Nbt::as_str) else { return };
        let Some((stat_type, id)) = unpack_criterion(packed) else { return };
        let mut name = Nbt::String(id);
        if let Some(kind) = criterion_kind(&stat_type) {
            self.kind(kind, &mut name);
        }
        let Nbt::String(id) = name else { return };
        let repacked = format!("{}:{}", pack_with_dot(&stat_type), pack_with_dot(&id));
        if let Nbt::Compound(entries) = objective
            && let Some((_, v)) = entries.iter_mut().find(|(k, _)| k == "CriteriaName")
        {
            *v = Nbt::String(repacked);
        }
    }
}

/// The fields `id` gives in a choice, if the choice has some for it.
fn choice_case(id: &str, namespaced: bool, cases: &[(&'static str, Node)]) -> Option<Node> {
    let name = choice_name(id, namespaced);
    cases.iter().find(|(case, _)| *case == listed_name(&name, namespaced)).map(|&(_, node)| node)
}

/// A choice knows `id`: it has fields for it, or knows it holds nothing nested.
fn choice_knows(id: &str, namespaced: bool, cases: &[(&'static str, Node)], plain: &[&str]) -> bool {
    let name = choice_name(id, namespaced);
    let listed = listed_name(&name, namespaced);
    cases.iter().any(|(case, _)| *case == listed) || plain.contains(&listed)
}

fn choice_name(id: &str, namespaced: bool) -> Cow<'_, str> {
    if namespaced { namespaced_id(id) } else { Cow::Borrowed(id) }
}

/// Namespaced choices list the game's identifiers without their `minecraft:` prefix.
fn listed_name(name: &str, namespaced: bool) -> &str {
    if namespaced { name.strip_prefix("minecraft:").unwrap_or(name) } else { name }
}

/// `value` has the form `node` expects (compound, list or text, and a known choice): what picks
/// an alternative of [`Node::Either`] once unread parts are hidden.
fn fits_form(node: Node, value: &Nbt, schema: Schema) -> bool {
    match node {
        Node::Of(Kind::ItemStack) => true,
        Node::Of(kind) => fits_form(shape(kind, schema), value, schema),
        Node::List(_) => is_list(value),
        Node::Values(_) | Node::KeysOf(_) | Node::NamespacedKeys(_) | Node::Required(_) => matches!(value, Nbt::Compound(_)),
        Node::Namespaced | Node::Text => matches!(value, Nbt::String(_)),
        Node::Either(options) => options.iter().any(|o| fits_form(*o, value, schema)),
        Node::All(parts) => parts.iter().all(|p| fits_form(*p, value, schema)),
        Node::Choice { key, namespaced, cases, plain } => {
            value.get(key).and_then(Nbt::as_str).is_some_and(|id| choice_knows(id, namespaced, cases, plain))
        }
        Node::Fields(_) | Node::PackedCriterion | Node::Plain => true,
    }
}

/// The whole of `value` reads as `node`, as the game reads it before converting: lists need
/// every element to read, required fields to be there, choices a known key. Optional fields
/// never fail: one that does not read is just left out.
fn reads(node: Node, value: &Nbt, schema: Schema) -> bool {
    match node {
        // A stack is read from anything: a bare identifier becomes one item, any other value
        // an item without fields.
        Node::Of(Kind::ItemStack) => true,
        Node::Of(kind) => reads(shape(kind, schema), value, schema),
        Node::List(inner) => match value {
            Nbt::List(items) => items.iter().all(|i| reads(*inner, i, schema)),
            // Number arrays read as lists of numbers.
            Nbt::ByteArray(_) | Nbt::IntArray(_) | Nbt::LongArray(_) => {
                array_is_empty(value) || reads(*inner, &Nbt::Int(0), schema)
            }
            _ => false,
        },
        Node::Required(fields) => fields.iter().all(|(name, inner)| value.get(name).is_some_and(|v| reads(*inner, v, schema))),
        Node::Values(inner) | Node::NamespacedKeys(inner) => match value {
            Nbt::Compound(entries) => entries.iter().all(|(_, v)| reads(*inner, v, schema)),
            _ => false,
        },
        Node::KeysOf(_) => matches!(value, Nbt::Compound(_)),
        Node::Namespaced | Node::Text => matches!(value, Nbt::String(_)),
        Node::Either(options) => options.iter().any(|o| reads(*o, value, schema)),
        Node::All(parts) => parts.iter().all(|p| reads(*p, value, schema)),
        Node::Choice { key, namespaced, cases, plain } => {
            let Some(id) = value.get(key).and_then(Nbt::as_str) else { return false };
            choice_knows(id, namespaced, cases, plain)
                && choice_case(id, namespaced, cases).is_none_or(|case| reads(case, value, schema))
        }
        Node::Fields(_) | Node::PackedCriterion | Node::Plain => true,
    }
}

fn is_list(value: &Nbt) -> bool {
    matches!(value, Nbt::List(_) | Nbt::ByteArray(_) | Nbt::IntArray(_) | Nbt::LongArray(_))
}

fn array_is_empty(value: &Nbt) -> bool {
    match value {
        Nbt::ByteArray(a) => a.is_empty(),
        Nbt::IntArray(a) => a.is_empty(),
        Nbt::LongArray(a) => a.is_empty(),
        _ => false,
    }
}

/// The kind of identifier a criterion of `stat_type` counts.
fn criterion_kind(stat_type: &str) -> Option<Kind> {
    match stat_type {
        "minecraft:mined" => Some(Kind::BlockName),
        "minecraft:crafted" | "minecraft:used" | "minecraft:broken" | "minecraft:picked_up" | "minecraft:dropped" => {
            Some(Kind::ItemName)
        }
        "minecraft:killed" | "minecraft:killed_by" => Some(Kind::EntityName),
        _ => None,
    }
}

/// A packed criterion `ns.type:ns.id` as two identifiers; `None` for a special one (no `:`, or
/// parts that are not identifiers), which the game keeps as it is.
fn unpack_criterion(packed: &str) -> Option<(String, String)> {
    let (stat_type, id) = packed.split_once(':')?;
    Some((dotted_identifier(stat_type)?, dotted_identifier(id)?))
}

/// An identifier written with `.` between namespace and path (`minecraft.stone`); no `.`
/// means the `minecraft` namespace.
fn dotted_identifier(text: &str) -> Option<String> {
    let (namespace, path) = match text.split_once('.') {
        Some(("", path)) => ("minecraft", path),
        Some((namespace, path)) => (namespace, path),
        None => ("minecraft", text),
    };
    (valid_namespace(namespace) && valid_path(path)).then(|| format!("{namespace}:{path}"))
}

/// An identifier packed with `.` instead of `:`, or the text as it is when not an identifier.
fn pack_with_dot(identifier: &str) -> Cow<'_, str> {
    match parse_identifier(identifier) {
        Some((namespace, path)) => Cow::Owned(format!("{namespace}.{path}")),
        None => Cow::Borrowed(identifier),
    }
}

/// `id` with the `minecraft:` namespace added when it has none, as the game reads a typed
/// identifier; text that is not a valid identifier is kept as it is.
pub(crate) fn namespaced_id(id: &str) -> Cow<'_, str> {
    match parse_identifier(id) {
        Some((namespace, path)) if id.len() == namespace.len() + 1 + path.len() => Cow::Borrowed(id),
        Some((namespace, path)) => Cow::Owned(format!("{namespace}:{path}")),
        None => Cow::Borrowed(id),
    }
}

/// [`namespaced_id`] of a string value, in place.
pub(crate) fn namespace_in_place(value: &mut Nbt) {
    if let Nbt::String(s) = value
        && let Cow::Owned(full) = namespaced_id(s)
    {
        *s = full;
    }
}

/// Namespace and path of an identifier (`minecraft` when there is no `:` or nothing before it).
fn parse_identifier(id: &str) -> Option<(&str, &str)> {
    let (namespace, path) = match id.split_once(':') {
        Some(("", path)) => ("minecraft", path),
        Some((namespace, path)) => (namespace, path),
        None => ("minecraft", id),
    };
    (valid_namespace(namespace) && valid_path(path)).then_some((namespace, path))
}

fn valid_path(path: &str) -> bool {
    path.bytes().all(|c| matches!(c, b'_' | b'-' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.'))
}

fn valid_namespace(namespace: &str) -> bool {
    namespace != ".." && namespace.bytes().all(|c| matches!(c, b'_' | b'-' | b'a'..=b'z' | b'0'..=b'9' | b'.'))
}

/// An item stack written as its identifier, read as `{id, count: 1}`.
fn expand_bare_stack(stack: &mut Nbt) {
    if let Nbt::String(id) = stack {
        let id = std::mem::take(id);
        *stack = Nbt::Compound(vec![("id".to_owned(), Nbt::String(id)), ("count".to_owned(), Nbt::Int(1))]);
    }
}

fn namespace_field(value: &mut Nbt, field: &str) {
    if let Nbt::Compound(entries) = value
        && let Some((_, v)) = entries.iter_mut().find(|(k, _)| k == field)
    {
        namespace_in_place(v);
    }
}
