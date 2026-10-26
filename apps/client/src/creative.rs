//! Creative inventory, data: lists of the "category" tabs and of the search
//! (`docs/specs/creative-inventory.md` § 3–4), built from the sequences extracted from the jar
//! (`rc_data::CreativeData`) and from the dynamic registries received from the server.

use hashbrown::HashMap;

use rc_data::{CreativeEntry, CreativeKind, CreativeStack, VersionData};
use rc_proto::varint::write_varint;
use rc_proto_v777::item::ItemStack;

/// Dynamic registries and tags received from the server, useful for the lists.
#[derive(Default)]
pub struct Received {
    /// Painting variants (id order): name, size read from the received registry.
    paintings: Vec<(String, Option<(i32, i32)>)>,
    instruments: Vec<String>,
    /// Enchantments: name, max level read from the received registry.
    enchantments: Vec<(String, Option<i32>)>,
    banner_patterns: Vec<String>,
    /// Tags per registry: registry → (tag → ids).
    tags: HashMap<String, Vec<(String, Vec<i32>)>>,
    /// Incremented on every change (compared with the last parameter set, § 3.1).
    pub generation: u64,
}

impl Received {
    /// Registry received in configuration (content absent for a known pack).
    pub fn registry(&mut self, name: &str, entries: &[(String, Option<rc_proto::nbt::Nbt>)]) {
        let int = |n: &Option<rc_proto::nbt::Nbt>, k: &str| n.as_ref()?.get(k)?.as_i64().map(|v| v as i32);
        match name {
            "minecraft:painting_variant" => {
                self.paintings =
                    entries.iter().map(|(n, nbt)| (n.clone(), int(nbt, "width").zip(int(nbt, "height")))).collect();
            }
            "minecraft:instrument" => self.instruments = entries.iter().map(|e| e.0.clone()).collect(),
            "minecraft:enchantment" => {
                self.enchantments = entries.iter().map(|(n, nbt)| (n.clone(), int(nbt, "max_level"))).collect();
            }
            "minecraft:banner_pattern" => self.banner_patterns = entries.iter().map(|e| e.0.clone()).collect(),
            _ => return,
        }
        self.generation += 1;
    }

    /// `update_tags`: replaces the tags of the received registries.
    pub fn tags(&mut self, tags: &rc_proto_v777::play::Tags) {
        for (reg, list) in tags {
            self.tags.insert(reg.clone(), list.clone());
        }
        self.generation += 1;
    }

    fn tag(&self, registry: &str, tag: &str) -> Option<&[i32]> {
        let list = self.tags.get(registry)?;
        let t = tag.strip_prefix("minecraft:").unwrap_or(tag);
        list.iter().find(|(n, _)| n.strip_prefix("minecraft:").unwrap_or(n) == t).map(|(_, v)| &v[..])
    }

    /// Names of a received dynamic registry (id order).
    pub fn names(&self, registry: &str) -> Vec<String> {
        match registry {
            "minecraft:painting_variant" => self.paintings.iter().map(|p| p.0.clone()).collect(),
            "minecraft:instrument" => self.instruments.clone(),
            "minecraft:enchantment" => self.enchantments.iter().map(|p| p.0.clone()).collect(),
            "minecraft:banner_pattern" => self.banner_patterns.clone(),
            _ => Vec::new(),
        }
    }

    /// Item tags: (full name, ids).
    pub fn item_tags(&self) -> &[(String, Vec<i32>)] {
        self.tags.get("minecraft:item").map_or(&[], Vec::as_slice)
    }
}

/// Build parameters (§ 3.1): features, permission, registry generation.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Params {
    pub features: Vec<String>,
    pub permission: bool,
    pub generation: u64,
}

/// Built lists: per tab (registry order), list of the tab and "for the
/// search" list; then the search base (§ 4.1).
#[derive(Default)]
pub struct Lists {
    pub params: Option<Params>,
    pub tab: Vec<Vec<ItemStack>>,
    pub for_search: Vec<Vec<ItemStack>>,
    pub search: Vec<ItemStack>,
}

/// Same stack (item and components), quantity ignored.
pub fn same(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item && a.added == b.added && a.removed == b.removed
}

fn push_unique(list: &mut Vec<ItemStack>, s: &ItemStack) {
    if !list.iter().any(|x| same(x, s)) {
        list.push(s.clone());
    }
}

/// Context for resolving names to network ids.
struct Resolve<'a> {
    items: HashMap<&'a str, i32>,
    components: HashMap<&'a str, u16>,
    features: Vec<String>,
}

impl Resolve<'_> {
    fn enabled(&self, need: &[String]) -> bool {
        // Received list empty: base set.
        need.iter().all(|f| {
            if self.features.is_empty() { f == "minecraft:vanilla" } else { self.features.iter().any(|x| x == f) }
        })
    }

    fn stack(&self, item: &str, added: Vec<(u16, Box<[u8]>)>) -> Option<ItemStack> {
        Some(ItemStack { item: *self.items.get(item)?, count: 1, added, removed: Vec::new() })
    }

    fn fixed(&self, s: &CreativeStack, rec: &Received) -> Option<ItemStack> {
        if !self.enabled(&s.features) {
            return None;
        }
        let mut added = Vec::with_capacity(s.added.len() + 1);
        for (t, b) in &s.added {
            added.push((*self.components.get(t.as_str())?, b.clone().into_boxed_slice()));
        }
        if !s.banner.is_empty() {
            let mut b = Vec::new();
            write_varint(&mut b, s.banner.len() as i32);
            for (p, c) in &s.banner {
                let id = rec.banner_patterns.iter().position(|n| n == p)?;
                write_varint(&mut b, id as i32 + 1);
                write_varint(&mut b, i32::from(*c));
            }
            added.push((*self.components.get("minecraft:banner_patterns")?, b.into_boxed_slice()));
        }
        let removed = s.removed.iter().map(|t| self.components.get(t.as_str()).copied()).collect::<Option<Vec<_>>>()?;
        let mut st = self.stack(&s.item, added)?;
        st.removed = removed;
        Some(st)
    }

    fn holder(&self, component: &str, id: usize) -> Option<(u16, Box<[u8]>)> {
        let mut b = Vec::new();
        write_varint(&mut b, id as i32 + 1);
        Some((*self.components.get(component)?, b.into_boxed_slice()))
    }
}

/// Sequence generated for a dynamic registry tag (§ 3.2): (stack, visibility).
fn dynamic(
    reg: &str,
    tab: &str,
    data: &rc_data::CreativeData,
    rec: &Received,
    res: &Resolve<'_>,
) -> Vec<(ItemStack, u8)> {
    let mut out = Vec::new();
    match reg {
        "minecraft:painting_variant" => {
            let placeable = rec.tag("minecraft:painting_variant", "minecraft:placeable").unwrap_or(&[]);
            let want = tab != "minecraft:op_blocks";
            let mut list: Vec<(usize, i32, i32)> = rec
                .paintings
                .iter()
                .enumerate()
                .filter(|(i, _)| placeable.contains(&(*i as i32)) == want)
                .filter_map(|(i, (name, size))| {
                    let (w, h) = size.or_else(|| {
                        data.paintings.iter().find(|p| &p.0 == name).map(|p| (i32::from(p.1), i32::from(p.2)))
                    })?;
                    Some((i, w, h))
                })
                .collect();
            list.sort_by_key(|&(_, w, h)| (w * h, w));
            for (i, _, _) in list {
                if let Some(c) = res.holder("minecraft:painting/variant", i)
                    && let Some(s) = res.stack("minecraft:painting", vec![c])
                {
                    out.push((s, 0));
                }
            }
        }
        "minecraft:instrument" => {
            for &i in rec.tag("minecraft:instrument", "minecraft:goat_horns").unwrap_or(&[]) {
                if let Some(c) = res.holder("minecraft:instrument", i as usize)
                    && let Some(s) = res.stack("minecraft:goat_horn", vec![c])
                {
                    out.push((s, 0));
                }
            }
        }
        "minecraft:enchantment" => {
            let Some(&ty) = res.components.get("minecraft:stored_enchantments") else { return out };
            let book = |id: usize, level: i32| {
                let mut b = Vec::new();
                write_varint(&mut b, 1);
                write_varint(&mut b, id as i32);
                write_varint(&mut b, level);
                res.stack("minecraft:enchanted_book", vec![(ty, b.into_boxed_slice())])
            };
            let levels = |name: &str, nbt_max: Option<i32>| {
                let base = data.enchantments.iter().find(|e| e.0 == name);
                let min = base.map_or(1, |e| i32::from(e.1)).max(1);
                let max = nbt_max.or_else(|| base.map(|e| i32::from(e.2))).unwrap_or(1);
                (min, max.max(min))
            };
            for (i, (name, max)) in rec.enchantments.iter().enumerate() {
                if let Some(s) = book(i, levels(name, *max).1) {
                    out.push((s, 1));
                }
            }
            for (i, (name, max)) in rec.enchantments.iter().enumerate() {
                let (lo, hi) = levels(name, *max);
                for l in lo..=hi {
                    if let Some(s) = book(i, l) {
                        out.push((s, 2));
                    }
                }
            }
        }
        _ => {}
    }
    out
}

impl Lists {
    /// Rebuilds the lists if the parameters changed (§ 3.1); true if rebuilt.
    pub fn update(&mut self, data: &VersionData, rec: &Received, params: Params) -> bool {
        if self.params.as_ref() == Some(&params) {
            return false;
        }
        let names = data.registry("minecraft:item").map(|r| &r.entries[..]).unwrap_or(&[]);
        let comps = data.registry("minecraft:data_component_type").map(|r| &r.entries[..]).unwrap_or(&[]);
        let res = Resolve {
            items: names.iter().enumerate().map(|(i, n)| (n.as_str(), i as i32)).collect(),
            components: comps.iter().enumerate().map(|(i, n)| (n.as_str(), i as u16)).collect(),
            features: params.features.clone(),
        };
        let tabs = &data.creative.tabs;
        self.tab = vec![Vec::new(); tabs.len()];
        self.for_search = vec![Vec::new(); tabs.len()];
        for (k, t) in tabs.iter().enumerate() {
            if t.kind != CreativeKind::Category || (t.name == "minecraft:op_blocks" && !params.permission) {
                continue;
            }
            let (tab, search) = (&mut self.tab[k], &mut self.for_search[k]);
            let mut add = |s: &ItemStack, v: u8| {
                if v != 2 {
                    push_unique(tab, s);
                }
                if v != 1 {
                    push_unique(search, s);
                }
            };
            for e in &t.entries {
                match e {
                    CreativeEntry::Stack(s) => {
                        if let Some(st) = res.fixed(s, rec) {
                            add(&st, s.visibility);
                        }
                    }
                    CreativeEntry::Dynamic(reg) => {
                        for (st, v) in dynamic(reg, &t.name, &data.creative, rec, &res) {
                            add(&st, v);
                        }
                    }
                }
            }
        }
        self.search.clear();
        for (k, t) in tabs.iter().enumerate() {
            if t.kind != CreativeKind::Search {
                for s in &self.for_search[k] {
                    push_unique(&mut self.search, s);
                }
            }
        }
        self.params = Some(params);
        true
    }
}

/// Search index (§ 4.1): lowercase tooltip lines, id, tags.
#[derive(Default)]
pub struct SearchIndex {
    pub lines: Vec<Vec<String>>,
    pub ids: Vec<String>,
    pub tags: Vec<Vec<String>>,
}

fn split(q: &str) -> Option<(&str, &str)> {
    q.split_once(':').map(|(a, b)| (a.trim(), b.trim()))
}

fn ns_path(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

impl SearchIndex {
    /// Indices of the stacks kept by the query (list order, § 4.2) and, for a
    /// `#` query, the visible tags among `all_tags`.
    pub fn query(&self, query: &str, all_tags: &[(String, Vec<i32>)]) -> (Vec<usize>, Vec<String>) {
        let q = query.to_lowercase();
        let n = self.ids.len();
        if q.is_empty() {
            return ((0..n).collect(), Vec::new());
        }
        if let Some(t) = q.strip_prefix('#') {
            let matches = |tag: &str| {
                let (ns, path) = ns_path(tag);
                match split(t) {
                    None => path.contains(t),
                    Some((a, b)) => ns.contains(a) && path.contains(b),
                }
            };
            let hits = (0..n).filter(|&i| self.tags[i].iter().any(|g| matches(g))).collect();
            let visible = all_tags.iter().map(|(g, _)| g.clone()).filter(|g| matches(g)).collect();
            return (hits, visible);
        }
        let in_lines = |i: usize, s: &str| self.lines[i].iter().any(|l| l.contains(s));
        let hits = (0..n)
            .filter(|&i| match split(&q) {
                None => in_lines(i, &q),
                Some((a, b)) => {
                    let (ns, path) = ns_path(&self.ids[i]);
                    ns.contains(a) && (path.contains(b) || in_lines(i, b))
                }
            })
            .collect();
        (hits, Vec::new())
    }
}

/// Item tags by item id (full names `minecraft:…`).
pub fn tags_by_item(rec: &Received) -> HashMap<i32, Vec<String>> {
    let mut out: HashMap<i32, Vec<String>> = HashMap::new();
    for (tag, ids) in rec.item_tags() {
        let full = if tag.contains(':') { tag.clone() } else { format!("minecraft:{tag}") };
        for &i in ids {
            out.entry(i).or_default().push(full.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> SearchIndex {
        SearchIndex {
            lines: vec![vec!["diamond sword".into()], vec!["stone".into()], vec!["diamond".into()]],
            ids: vec!["minecraft:diamond_sword".into(), "minecraft:stone".into(), "other:gem".into()],
            tags: vec![vec!["minecraft:swords".into()], vec!["minecraft:stone_bricks".into()], vec![]],
        }
    }

    #[test]
    fn query_by_name_namespace_and_tag() {
        let idx = index();
        assert_eq!(idx.query("", &[]).0, vec![0, 1, 2]);
        assert_eq!(idx.query("Diamond", &[]).0, vec![0, 2]);
        // With ":": namespace, then path or tooltip line.
        assert_eq!(idx.query("minecraft:sto", &[]).0, vec![1]);
        assert_eq!(idx.query("other:", &[]).0, vec![2]);
        let tags = vec![("minecraft:swords".to_owned(), vec![]), ("minecraft:logs".to_owned(), vec![])];
        let (hits, visible) = idx.query("#sword", &tags);
        assert_eq!(hits, vec![0]);
        assert_eq!(visible, vec!["minecraft:swords".to_owned()]);
        assert_eq!(idx.query("#minecraft:stone", &tags).0, vec![1]);
    }

    #[test]
    fn dedupe_on_item_and_components() {
        let a = ItemStack { item: 3, count: 1, added: vec![], removed: vec![] };
        let mut b = a.clone();
        b.added.push((1, vec![2].into_boxed_slice()));
        let mut l = Vec::new();
        push_unique(&mut l, &a);
        push_unique(&mut l, &b);
        push_unique(&mut l, &a);
        assert_eq!(l.len(), 2);
    }
}
