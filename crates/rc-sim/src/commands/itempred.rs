//! Item predicate of commands (`item_predicate`, `commands.md`): `type[conditions]` read
//! once (the text is already validated by parsing), then applied to each stack.
//!
//! Conditions: `a,b` (all), `a|b` (any), `!a` (negation); a test is `component=value`
//! (equality with the effective component: the stack's, otherwise the item's), `predicate~value`
//! (partial component predicate), or `component` alone (presence, item defaults included).
//! `count` is a pseudo-component: `count=..`, `count~..` compare the quantity; `count` alone
//! is always true.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use super::cmds_s9b::nbt_to_json;
use super::snbt;
use crate::player::Stack;
use crate::server::Server;

const COUNT: &str = "minecraft:count";

#[derive(Debug, Clone, PartialEq)]
enum Head {
    Any,
    Item(String),
    Tag(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Test {
    /// `count` alone: always true.
    Always,
    /// Quantity bounds (`count=`, `count~`).
    Count(Value),
    /// Presence of the effective component.
    Has(String),
    /// Equality of the effective component.
    Eq(String, Want),
    /// Partial predicate (`~`).
    Pred(String, Value),
}

/// Expected value of an equality: typed components go through their decoder (numbers brought back
/// to their type, comparison by value); those carrying raw NBT are compared tag by
/// tag, type included (`{a:1}` ≠ `{a:1b}`, `"stone"` ≠ `"minecraft:stone"`).
#[derive(Debug, Clone, PartialEq)]
enum Want {
    Typed(Value),
    Raw(Nbt),
}

/// Components whose value is a free NBT compound.
const RAW_NBT: [&str; 4] =
    ["minecraft:custom_data", "minecraft:entity_data", "minecraft:block_entity_data", "minecraft:bucket_entity_data"];

/// NBT compound of a raw component; the text form (quoted SNBT) is read.
fn raw_compound(n: Nbt) -> Option<Nbt> {
    match n {
        Nbt::String(s) => snbt::parse(&s).ok(),
        o => Some(o),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Term {
    neg: bool,
    test: Test,
}

/// Predicate read: type then groups of alternatives (all groups must pass).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemPred {
    head: Head,
    all: Vec<Vec<Term>>,
}

fn id_char(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | ':' | '/')
}

struct Rd<'a> {
    c: &'a [char],
    i: usize,
}

impl Rd<'_> {
    fn ws(&mut self) {
        while self.c.get(self.i).is_some_and(|c| c.is_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, ch: char) -> bool {
        self.ws();
        if self.c.get(self.i) == Some(&ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn id(&mut self) -> String {
        self.ws();
        let st = self.i;
        while self.c.get(self.i).copied().is_some_and(id_char) {
            self.i += 1;
        }
        super::args::id(&self.c[st..self.i].iter().collect::<String>())
    }

    fn nbt(&mut self) -> Option<Nbt> {
        self.ws();
        snbt::parse_at(self.c, &mut self.i).ok()
    }

    fn value(&mut self) -> Value {
        self.nbt().map(|n| nbt_to_json(&n)).unwrap_or(Value::Null)
    }

    fn term(&mut self) -> Term {
        let neg = self.eat('!');
        let id = self.id();
        let test = if self.eat('=') {
            if id == COUNT {
                Test::Count(self.value())
            } else if RAW_NBT.contains(&id.as_str()) {
                let v = self.nbt().and_then(raw_compound).unwrap_or(Nbt::Compound(Vec::new()));
                Test::Eq(id, Want::Raw(v))
            } else {
                let v = self.value();
                Test::Eq(id, Want::Typed(v))
            }
        } else if self.eat('~') {
            let v = self.value();
            if id == COUNT { Test::Count(v) } else { Test::Pred(id, v) }
        } else if id == COUNT {
            Test::Always
        } else {
            Test::Has(id)
        };
        Term { neg, test }
    }
}

impl ItemPred {
    pub fn parse(text: &str) -> ItemPred {
        let chars: Vec<char> = text.chars().collect();
        let mut r = Rd { c: &chars, i: 0 };
        let head = if r.eat('*') {
            Head::Any
        } else if r.eat('#') {
            Head::Tag(r.id())
        } else {
            Head::Item(r.id())
        };
        let mut all = Vec::new();
        if r.eat('[') {
            r.ws();
            if r.c.get(r.i) != Some(&']') {
                loop {
                    let mut alts = vec![r.term()];
                    while r.eat('|') {
                        alts.push(r.term());
                    }
                    all.push(alts);
                    if !r.eat(',') {
                        break;
                    }
                }
            }
        }
        ItemPred { head, all }
    }
}

impl Server {
    /// Stack matching the read predicate.
    pub fn item_pred_test(&self, p: &ItemPred, s: &Stack) -> bool {
        let ok = match &p.head {
            Head::Any => true,
            Head::Item(id) => *id == s.id,
            Head::Tag(t) => self.life.items.has_tag(t, &s.id),
        };
        ok && p.all.iter().all(|alts| alts.iter().any(|t| self.item_term(t, s) != t.neg))
    }

    fn item_term(&self, t: &Term, s: &Stack) -> bool {
        match &t.test {
            Test::Always => true,
            Test::Count(v) => crate::progress::pred::ints(Some(v), i64::from(s.count)),
            Test::Has(id) => self.p_component(s, id).is_some(),
            Test::Eq(id, Want::Typed(want)) => {
                self.p_component(s, id).is_some_and(|have| crate::progress::pred::json_eq_nbt(want, &have))
            }
            Test::Eq(id, Want::Raw(want)) => self
                .p_component(s, id)
                .and_then(raw_compound)
                .is_some_and(|have| crate::items::nbt_eq(want, &have)),
            Test::Pred(id, v) => self.p_partial(id, v, s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_conditions() {
        let p = ItemPred::parse("*[damage=0|!count=2, enchantments~{levels:{sharpness:1}},food]");
        assert_eq!(p.head, Head::Any);
        assert_eq!(p.all.len(), 3);
        assert_eq!(p.all[0].len(), 2);
        assert_eq!(p.all[0][0].test, Test::Eq("minecraft:damage".into(), Want::Typed(Value::from(0))));
        assert!(p.all[0][1].neg);
        assert_eq!(p.all[0][1].test, Test::Count(Value::from(2)));
        assert!(matches!(&p.all[1][0].test, Test::Pred(k, _) if k == "minecraft:enchantments"));
        assert_eq!(p.all[2][0].test, Test::Has("minecraft:food".into()));
        // SNBT value with an inner comma: a single test.
        let p = ItemPred::parse("stick[custom_data={a:1,b:2}]");
        assert_eq!(p.head, Head::Item("minecraft:stick".into()));
        assert_eq!(p.all.len(), 1);
        let p = ItemPred::parse("#minecraft:logs");
        assert_eq!(p.head, Head::Tag("minecraft:logs".into()));
        assert!(p.all.is_empty());
    }
}
