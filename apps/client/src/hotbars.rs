//! Saved hotbars (`docs/specs/creative-inventory.md` § 8): `hotbar.nbt` file of the game folder,
//! uncompressed NBT, stacks in persistent form (`{id, count, components}`).
//!
//! Components are converted between the client's network form of stacks and the persistent
//! form for the common types (enchantments, potions, variants, block states, names…).
//! An unsupported component is kept under a Rustcraft-specific key
//! (`rustcraft:network/<type>`, network bytes): read back identically by Rustcraft, unreadable
//! by the official game (the stack would become empty there, like any invalid stack).

use std::path::PathBuf;

use rc_data::{Registry, VersionData};
use rc_proto::Reader;
use rc_proto::nbt::Nbt;
use rc_proto::varint::{read_varint, write_varint};
use rc_proto_v777::item::ItemStack;

/// Data version of 26.3.
const DATA_VERSION: i32 = 5023;
const RAW: &str = "rustcraft:network/";

pub type Row = [Option<ItemStack>; 9];

/// Names of the registries useful for the conversion.
pub struct Names<'a> {
    pub items: &'a Registry,
    pub components: &'a Registry,
    pub potions: &'a Registry,
    pub effects: &'a Registry,
    /// Dynamic registries received from the server (a few dozen names each).
    pub paintings: &'a [String],
    pub instruments: &'a [String],
    pub enchantments: &'a [String],
    pub banner_patterns: &'a [String],
    /// Ids in the four registries above found through their name index
    /// (`RUSTCRAFT_OPT_INDEXED_NAME_LOOKUPS`), otherwise by a walk over the names.
    pub indexed: bool,
}

impl<'a> Names<'a> {
    /// The names the version data and the received registries give.
    pub fn of(data: &'a VersionData, received: &'a crate::creative::Received, indexed: bool) -> Names<'a> {
        let or_empty = |r: Option<&'a Registry>| r.unwrap_or_else(|| empty_registry());
        Names {
            items: or_empty(data.items()),
            components: or_empty(data.data_component_types()),
            potions: or_empty(data.potion_registry()),
            effects: or_empty(data.mob_effects()),
            paintings: received.names("minecraft:painting_variant"),
            instruments: received.names("minecraft:instrument"),
            enchantments: received.names("minecraft:enchantment"),
            banner_patterns: received.names("minecraft:banner_pattern"),
            indexed,
        }
    }
}

/// Stand-in for a registry the version data lack (tests on partial data).
fn empty_registry() -> &'static Registry {
    static EMPTY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Registry::new("", Vec::new()))
}

const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];
const RARITIES: [&str; 4] = ["common", "uncommon", "rare", "epic"];

fn id_of(list: &[String], name: &str) -> Option<i32> {
    let full = if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") };
    list.iter().position(|n| *n == full).map(|i| i as i32)
}

/// [`id_of`] in a built-in registry: through its name index when `indexed`.
fn registry_id_of(registry: &Registry, name: &str, indexed: bool) -> Option<i32> {
    if !indexed {
        return id_of(&registry.entries, name);
    }
    rc_data::ids::with_full_id(name, |full| registry.id(full)).map(|i| i as i32)
}

fn name_of(list: &[String], id: i32) -> Option<Nbt> {
    usize::try_from(id).ok().and_then(|i| list.get(i)).map(|n| Nbt::String(n.clone()))
}

fn vi(r: &mut Reader<'_>) -> Option<i32> {
    read_varint(r).ok()
}

fn bool_(r: &mut Reader<'_>) -> Option<bool> {
    Some(r.u8().ok()? != 0)
}

/// Persistent value of a component (network → NBT); `None`: not supported.
fn to_persistent(ty: &str, b: &[u8], n: &Names<'_>) -> Option<Nbt> {
    let mut r = Reader::new(b);
    let r = &mut r;
    let v = match ty.strip_prefix("minecraft:")? {
        "damage" | "repair_cost" | "ominous_bottle_amplifier" | "max_damage" | "max_stack_size" => Nbt::Int(vi(r)?),
        "dyed_color" => Nbt::Int(i32::from_be_bytes(r.take(4).ok()?.try_into().ok()?)),
        "rarity" => Nbt::String(RARITIES.get(vi(r)? as usize)?.to_string()),
        "enchantment_glint_override" => Nbt::Byte(i8::from(bool_(r)?)),
        "creative_slot_lock" | "unbreakable" | "glider" | "intangible_projectile" => Nbt::Compound(Vec::new()),
        "custom_name" | "item_name" => rc_proto::nbt::read(r).ok()??,
        "enchantments" | "stored_enchantments" => {
            let k = vi(r)?;
            let mut m = Vec::new();
            for _ in 0..k {
                let (e, l) = (vi(r)?, vi(r)?);
                m.push((n.enchantments.get(e as usize)?.clone(), Nbt::Int(l)));
            }
            Nbt::Compound(m)
        }
        "potion_contents" => {
            let potion = if bool_(r)? { Some(vi(r)?) } else { None };
            if bool_(r)? || vi(r)? != 0 || bool_(r)? {
                return None;
            }
            let mut m = Vec::new();
            if let Some(p) = potion {
                m.push(("potion".into(), name_of(&n.potions.entries, p)?));
            }
            Nbt::Compound(m)
        }
        "painting/variant" => name_of(n.paintings, vi(r)? - 1)?,
        "instrument" => name_of(n.instruments, vi(r)? - 1)?,
        "fireworks" => {
            let d = vi(r)?;
            if vi(r)? != 0 {
                return None;
            }
            Nbt::Compound(vec![
                ("flight_duration".into(), Nbt::Byte(d as i8)),
                ("explosions".into(), Nbt::List(vec![])),
            ])
        }
        "block_state" => {
            let k = vi(r)?;
            let mut m = Vec::new();
            for _ in 0..k {
                let key = rc_proto::read_str(r, 32767).ok()?.to_owned();
                let val = rc_proto::read_str(r, 32767).ok()?.to_owned();
                m.push((key, Nbt::String(val)));
            }
            Nbt::Compound(m)
        }
        "tooltip_display" => {
            let hide = bool_(r)?;
            let k = vi(r)?;
            let mut l = Vec::new();
            for _ in 0..k {
                l.push(name_of(&n.components.entries, vi(r)?)?);
            }
            Nbt::Compound(vec![
                ("hide_tooltip".into(), Nbt::Byte(i8::from(hide))),
                ("hidden_components".into(), Nbt::List(l)),
            ])
        }
        "banner_patterns" => {
            let k = vi(r)?;
            let mut l = Vec::new();
            for _ in 0..k {
                let p = vi(r)? - 1;
                let c = vi(r)?;
                l.push(Nbt::Compound(vec![
                    ("pattern".into(), name_of(n.banner_patterns, p)?),
                    ("color".into(), Nbt::String(DYES.get(c as usize)?.to_string())),
                ]));
            }
            Nbt::List(l)
        }
        "suspicious_stew_effects" => {
            let k = vi(r)?;
            let mut l = Vec::new();
            for _ in 0..k {
                let e = vi(r)?;
                let d = vi(r)?;
                l.push(Nbt::Compound(vec![
                    ("id".into(), name_of(&n.effects.entries, e)?),
                    ("duration".into(), Nbt::Int(d)),
                ]));
            }
            Nbt::List(l)
        }
        _ => return None,
    };
    (r.rest().is_empty()).then_some(v)
}

fn str_of(v: &Nbt) -> Option<&str> {
    v.as_str()
}

/// Network value of a component (NBT → network); `None`: unreadable.
fn to_network(ty: &str, v: &Nbt, n: &Names<'_>) -> Option<Vec<u8>> {
    let mut w = Vec::new();
    let int = |v: &Nbt| v.as_i64().map(|x| x as i32);
    match ty.strip_prefix("minecraft:")? {
        "damage" | "repair_cost" | "ominous_bottle_amplifier" | "max_damage" | "max_stack_size" => {
            write_varint(&mut w, int(v)?);
        }
        "dyed_color" => w.extend_from_slice(&int(v)?.to_be_bytes()),
        "rarity" => write_varint(&mut w, RARITIES.iter().position(|x| Some(*x) == str_of(v))? as i32),
        "enchantment_glint_override" => w.push(u8::from(int(v)? != 0)),
        "creative_slot_lock" | "unbreakable" | "glider" | "intangible_projectile" => {}
        "custom_name" | "item_name" => rc_proto::nbt::write(&mut w, Some(v)),
        "enchantments" | "stored_enchantments" => {
            let Nbt::Compound(m) = v else { return None };
            write_varint(&mut w, m.len() as i32);
            for (e, l) in m {
                write_varint(&mut w, id_of(n.enchantments, e)?);
                write_varint(&mut w, int(l)?);
            }
        }
        "potion_contents" => {
            let p = match v {
                Nbt::String(s) => Some(s.as_str()),
                Nbt::Compound(m) => {
                    if m.iter().any(|(k, _)| k != "potion") {
                        return None;
                    }
                    v.get("potion").and_then(Nbt::as_str)
                }
                _ => return None,
            };
            match p {
                Some(p) => {
                    w.push(1);
                    write_varint(&mut w, registry_id_of(n.potions, p, n.indexed)?);
                }
                None => w.push(0),
            }
            w.extend_from_slice(&[0, 0, 0]);
        }
        "painting/variant" => write_varint(&mut w, id_of(n.paintings, str_of(v)?)? + 1),
        "instrument" => write_varint(&mut w, id_of(n.instruments, str_of(v)?)? + 1),
        "fireworks" => {
            if v.get("explosions").is_some_and(|e| !matches!(e, Nbt::List(l) if l.is_empty())) {
                return None;
            }
            write_varint(&mut w, v.get("flight_duration").and_then(int).unwrap_or(0));
            write_varint(&mut w, 0);
        }
        "block_state" => {
            let Nbt::Compound(m) = v else { return None };
            write_varint(&mut w, m.len() as i32);
            for (k, x) in m {
                rc_proto::write_str(&mut w, k);
                rc_proto::write_str(&mut w, str_of(x)?);
            }
        }
        "tooltip_display" => {
            w.push(u8::from(v.get("hide_tooltip").and_then(int).unwrap_or(0) != 0));
            let empty = Vec::new();
            let l = match v.get("hidden_components") {
                Some(Nbt::List(l)) => l,
                None => &empty,
                _ => return None,
            };
            write_varint(&mut w, l.len() as i32);
            for c in l {
                write_varint(&mut w, registry_id_of(n.components, str_of(c)?, n.indexed)?);
            }
        }
        "banner_patterns" => {
            let Nbt::List(l) = v else { return None };
            write_varint(&mut w, l.len() as i32);
            for x in l {
                write_varint(&mut w, id_of(n.banner_patterns, x.get("pattern").and_then(Nbt::as_str)?)? + 1);
                let c = x.get("color").and_then(Nbt::as_str).unwrap_or("white");
                write_varint(&mut w, DYES.iter().position(|d| *d == c)? as i32);
            }
        }
        "suspicious_stew_effects" => {
            let Nbt::List(l) = v else { return None };
            write_varint(&mut w, l.len() as i32);
            for x in l {
                write_varint(&mut w, registry_id_of(n.effects, x.get("id").and_then(Nbt::as_str)?, n.indexed)?);
                write_varint(&mut w, x.get("duration").and_then(int).unwrap_or(160));
            }
        }
        _ => return None,
    }
    Some(w)
}

/// Stack → persistent compound (`{}` if empty).
pub fn stack_to_nbt(s: Option<&ItemStack>, n: &Names<'_>) -> Nbt {
    let Some(s) = s.filter(|s| s.count > 0) else { return Nbt::Compound(Vec::new()) };
    let Some(id) = usize::try_from(s.item).ok().and_then(|i| n.items.entries.get(i)) else {
        return Nbt::Compound(Vec::new());
    };
    let mut out = vec![("id".to_owned(), Nbt::String(id.clone())), ("count".to_owned(), Nbt::Int(s.count))];
    let mut comps = Vec::new();
    for (ty, b) in &s.added {
        let Some(name) = n.components.entries.get(usize::from(*ty)) else { continue };
        match to_persistent(name, b, n) {
            Some(v) => comps.push((name.clone(), v)),
            None => comps.push((format!("{RAW}{name}"), Nbt::ByteArray(b.to_vec()))),
        }
    }
    for ty in &s.removed {
        if let Some(name) = n.components.entries.get(usize::from(*ty)) {
            comps.push((format!("!{name}"), Nbt::Compound(Vec::new())));
        }
    }
    if !comps.is_empty() {
        out.push(("components".to_owned(), Nbt::Compound(comps)));
    }
    Nbt::Compound(out)
}

/// Persistent compound → stack; unreadable or empty: `None`.
pub fn stack_from_nbt(v: &Nbt, n: &Names<'_>) -> Option<ItemStack> {
    let item = registry_id_of(n.items, v.get("id")?.as_str()?, n.indexed)?;
    let count = v.get("count").and_then(Nbt::as_i64).unwrap_or(1) as i32;
    if count <= 0 {
        return None;
    }
    let component = |name: &str| registry_id_of(n.components, name, n.indexed).map(|id| id as u16);
    let mut s = ItemStack { item, count, added: Vec::new(), removed: Vec::new() };
    if let Some(Nbt::Compound(m)) = v.get("components") {
        for (k, x) in m {
            if let Some(t) = k.strip_prefix('!') {
                s.removed.push(component(t)?);
            } else if let Some(t) = k.strip_prefix(RAW) {
                let Nbt::ByteArray(b) = x else { return None };
                s.added.push((component(t)?, b.clone().into_boxed_slice()));
            } else {
                let b = to_network(k, x, n)?;
                s.added.push((component(k)?, b.into_boxed_slice()));
            }
        }
    }
    Some(s)
}

/// What the rows of the hotbars tab read besides the saved file and the version data, which never
/// change during a session.
pub struct HotbarSources<'a> {
    pub data: &'a VersionData,
    pub received: &'a crate::creative::Received,
    /// The bound keys named on the paper of an empty row.
    pub options: &'a crate::options::Options,
    /// Language of the key names; `None` without a renderer (raw key names then).
    pub language: Option<&'a rc_text::Lang>,
    /// `RUSTCRAFT_OPT_INDEXED_NAME_LOOKUPS`.
    pub indexed: bool,
}

/// What the rows of the hotbars tab were made from besides the saved file, compared whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabRowsInputs {
    /// Generation of the registries received from the server: the names of the saved stacks.
    received_registries: u64,
    /// Revision of the settings: the keys named on the paper of an empty row.
    settings: u64,
    /// Identity of the language of the key names, `None` without a renderer.
    language: Option<u64>,
}

impl HotbarSources<'_> {
    pub fn names(&self) -> Names<'_> {
        Names::of(self.data, self.received, self.indexed)
    }

    /// Name of the key bound to `option` (`key.hotbar.1`…), translated when there is a renderer.
    pub fn key_text(&self, option: &str) -> String {
        let name = self.options.s(&format!("key_{option}")).to_owned();
        match self.language {
            Some(lang) => crate::options::key_label(lang, &name),
            None => name,
        }
    }

    /// "Creative slot lock" paper named `inventory.hotbarInfo`, shown in an empty row `r` (§ 3.4).
    pub fn locked_paper(&self, r: usize) -> Option<ItemStack> {
        let id = |n: &str| crate::stack::component_type_id(self.data, n);
        let names = self.data.items().map_or(&[][..], |items| &items.entries);
        let item = crate::item_ids::item_id(self.data, names, "minecraft:paper", self.indexed)?;
        let name = Nbt::Compound(vec![
            ("translate".into(), Nbt::String("inventory.hotbarInfo".into())),
            (
                "with".into(),
                Nbt::List(vec![
                    Nbt::String(self.key_text("key.saveToolbarActivator")),
                    Nbt::String(self.key_text(&format!("key.hotbar.{}", r + 1))),
                ]),
            ),
        ]);
        let mut b = Vec::new();
        rc_proto::nbt::write(&mut b, Some(&name));
        Some(ItemStack {
            item,
            count: 1,
            added: vec![
                (id("minecraft:creative_slot_lock")?, Vec::new().into_boxed_slice()),
                (id("minecraft:custom_name")?, b.into_boxed_slice()),
            ],
            removed: Vec::new(),
        })
    }

    fn rows_inputs(&self) -> TabRowsInputs {
        TabRowsInputs {
            received_registries: self.received.generation,
            settings: self.options.revision(),
            language: self.language.map(|lang| lang.generation),
        }
    }
}

/// `RUSTCRAFT_OPT_HOTBAR_ROWS_ON_DEMAND` (on; `=0`: the rows of the hotbars tab made again at every
/// tick of the open creative screen and at every call, old path).
pub fn rows_on_demand() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_HOTBAR_ROWS_ON_DEMAND")
}

/// Saved hotbars, read lazily on first access.
#[derive(Default)]
pub struct Hotbars {
    raw: Option<Nbt>,
    /// The file read and written: `hotbar.nbt` of the game folder unless a test gives its own.
    file: Option<PathBuf>,
    /// Rows of the hotbars tab and what they were made from; forgotten when a row is saved.
    tab_rows: Option<(TabRowsInputs, Vec<Option<ItemStack>>)>,
    /// Times [`Self::kept_tab_rows`] made the rows (proofs).
    #[cfg(test)]
    tab_rows_made: usize,
}

impl Hotbars {
    /// Hotbars saved in `file` instead of the game folder's.
    #[cfg(test)]
    pub fn at(file: PathBuf) -> Hotbars {
        Hotbars { file: Some(file), ..Hotbars::default() }
    }

    fn path(&self) -> PathBuf {
        self.file.clone().unwrap_or_else(|| crate::servers::game_dir().join("hotbar.nbt"))
    }

    fn raw(&mut self) -> &Nbt {
        let path = if self.raw.is_none() { Some(self.path()) } else { None };
        self.raw.get_or_insert_with(|| {
            path.and_then(|p| std::fs::read(p).ok())
                .and_then(|b| rc_proto::nbt::read_named(&mut Reader::new(&b)).ok().flatten())
                .map_or(Nbt::Compound(Vec::new()), |(_, v)| v)
        })
    }

    /// Contents of the hotbars tab: 9 rows of 9, each saved row or the locked paper of an empty
    /// one (§ 3.4), made now.
    pub fn make_tab_rows(&mut self, sources: &HotbarSources<'_>) -> Vec<Option<ItemStack>> {
        let mut out = Vec::with_capacity(81);
        for r in 0..9 {
            if self.is_empty(r) {
                let mut row = vec![None; 9];
                row[r] = sources.locked_paper(r);
                out.extend(row);
            } else {
                out.extend(self.row(r, &sources.names()));
            }
        }
        out
    }

    /// [`Self::make_tab_rows`], made again only when what they read changed or a row was saved.
    pub fn kept_tab_rows(&mut self, sources: &HotbarSources<'_>) -> &[Option<ItemStack>] {
        let inputs = sources.rows_inputs();
        if self.tab_rows.as_ref().is_none_or(|(made_from, _)| *made_from != inputs) {
            let rows = self.make_tab_rows(sources);
            self.tab_rows = Some((inputs, rows));
            #[cfg(test)]
            {
                self.tab_rows_made += 1;
            }
        }
        self.tab_rows.as_ref().map_or(&[], |(_, rows)| rows)
    }

    /// Hotbar `i` (0..9) decoded; list of size ≠ 9: empty hotbar.
    pub fn row(&mut self, i: usize, n: &Names<'_>) -> Row {
        let mut out: Row = Default::default();
        if let Some(Nbt::List(l)) = self.raw().get(&i.to_string())
            && l.len() == 9
        {
            for (o, x) in out.iter_mut().zip(l) {
                *o = stack_from_nbt(x, n);
            }
        }
        out
    }

    /// Empty hotbar: 9 empty compounds (or absent).
    pub fn is_empty(&mut self, i: usize) -> bool {
        match self.raw().get(&i.to_string()) {
            Some(Nbt::List(l)) if l.len() == 9 => l.iter().all(|x| matches!(x, Nbt::Compound(m) if m.is_empty())),
            _ => true,
        }
    }

    /// Saving of hotbar `i` then rewriting of the file.
    pub fn save(&mut self, i: usize, row: &[Option<ItemStack>], n: &Names<'_>) -> std::io::Result<()> {
        let list = Nbt::List(row.iter().map(|s| stack_to_nbt(s.as_ref(), n)).collect());
        self.raw();
        self.tab_rows = None;
        let Some(Nbt::Compound(m)) = &mut self.raw else { return Ok(()) };
        m.retain(|(k, _)| k != "DataVersion" && *k != i.to_string());
        m.insert(0, ("DataVersion".into(), Nbt::Int(DATA_VERSION)));
        m.push((i.to_string(), list));
        let mut w = Vec::new();
        rc_proto::nbt::write_named(&mut w, "", self.raw.as_ref().expect("read"));
        let p = self.path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_round_trip() {
        let items = vec!["minecraft:air".to_owned(), "minecraft:painting".into(), "minecraft:enchanted_book".into()];
        let comps = vec![
            "minecraft:painting/variant".to_owned(),
            "minecraft:stored_enchantments".into(),
            "minecraft:lore".into(),
        ];
        let (items, comps) = (Registry::new("minecraft:item", items), Registry::new("components", comps));
        for indexed in [false, true] {
            let n = Names {
                items: &items,
                components: &comps,
                potions: empty_registry(),
                effects: empty_registry(),
                paintings: &["minecraft:kebab".into(), "minecraft:aztec".into()],
                instruments: &[],
                enchantments: &["minecraft:sharpness".into()],
                banner_patterns: &[],
                indexed,
            };
            let s = ItemStack {
                item: 2,
                count: 1,
                added: vec![(1, vec![1, 0, 5].into_boxed_slice()), (2, vec![9, 9].into_boxed_slice())],
                removed: vec![],
            };
            let nbt = stack_to_nbt(Some(&s), &n);
            assert_eq!(
                nbt.get("components")
                    .and_then(|c| c.get("minecraft:stored_enchantments"))
                    .and_then(|e| e.get("minecraft:sharpness")),
                Some(&Nbt::Int(5))
            );
            assert_eq!(stack_from_nbt(&nbt, &n), Some(s));
            let p = ItemStack { item: 1, count: 1, added: vec![(0, vec![2].into_boxed_slice())], removed: vec![] };
            let nbt = stack_to_nbt(Some(&p), &n);
            assert_eq!(
                nbt.get("components").and_then(|c| c.get("minecraft:painting/variant")),
                Some(&Nbt::String("minecraft:aztec".into()))
            );
            assert_eq!(stack_from_nbt(&nbt, &n), Some(p));
            assert_eq!(stack_to_nbt(None, &n), Nbt::Compound(vec![]));
        }
    }

    /// The game's data, the received registries and the settings of the hotbars tab proofs.
    struct TabRowsWorld {
        data: VersionData,
        received: crate::creative::Received,
        options: crate::options::Options,
        english: rc_text::Lang,
        french: rc_text::Lang,
    }

    impl TabRowsWorld {
        fn new() -> Option<TabRowsWorld> {
            let data = rc_testdata::version_data()?;
            let mut options = crate::options::Options::default();
            // A mouse button: its name is translated, unlike a letter key.
            options.set("key_key.saveToolbarActivator", crate::options::Val::S("key.mouse.left".into()));
            let english = br#"{"key.mouse.left": "Left Button"}"#;
            let french = r#"{"key.mouse.left": "Bouton gauche"}"#;
            let mut world = TabRowsWorld {
                data,
                received: crate::creative::Received::default(),
                options,
                english: rc_text::Lang::load(english, None, None),
                french: rc_text::Lang::load(english, Some(french.as_bytes()), None),
            };
            world.enchantments(&["minecraft:sharpness", "minecraft:unbreaking"]);
            Some(world)
        }

        fn enchantments(&mut self, names: &[&str]) {
            let entries: Vec<(String, Option<Nbt>)> = names.iter().map(|n| ((*n).to_owned(), None)).collect();
            self.received.registry("minecraft:enchantment", &entries);
        }

        fn sources<'a>(&'a self, language: Option<&'a rc_text::Lang>, indexed: bool) -> HotbarSources<'a> {
            HotbarSources { data: &self.data, received: &self.received, options: &self.options, language, indexed }
        }

        fn stack(&self, item: &str, component: Option<(&str, Vec<u8>)>) -> Option<ItemStack> {
            let item = self.data.registry_id("minecraft:item", item).expect("item") as i32;
            let added = component
                .map(|(name, bytes)| {
                    (crate::stack::component_type_id(&self.data, name).expect("component"), bytes.into_boxed_slice())
                })
                .into_iter()
                .collect();
            Some(ItemStack { item, count: 1, added, removed: Vec::new() })
        }

        /// A row holding a stone, a sharpness III book and a stack of 5 arrows.
        fn row(&self) -> Vec<Option<ItemStack>> {
            let mut row = vec![None; 9];
            row[0] = self.stack("minecraft:stone", None);
            row[4] = self.stack("minecraft:enchanted_book", Some(("minecraft:stored_enchantments", vec![1, 0, 3])));
            row[8] = self.stack("minecraft:arrow", None).map(|s| ItemStack { count: 5, ..s });
            row
        }

        /// Hotbars saved in a test folder of their own, rows `saved` filled.
        fn hotbars(&self, folder: &str, saved: &[usize]) -> Hotbars {
            let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("{folder}-{}", std::process::id())));
            let mut hotbars = Hotbars::at(dir.join("hotbar.nbt"));
            let names = Names::of(&self.data, &self.received, true);
            for &r in saved {
                hotbars.save(r, &self.row(), &names).expect("hotbar.nbt written");
            }
            hotbars
        }
    }

    /// The kept rows of the hotbars tab are the rows made at every tick, over 100 ticks during
    /// which a renderer appears, the language changes, a key is bound again, a row is saved, the
    /// server sends other enchantments and the renderer goes; each change shows in the rows, and
    /// the rows are made again at these changes only.
    #[test]
    fn kept_tab_rows_are_the_rows_made_at_every_tick() {
        let Some(mut world) = TabRowsWorld::new() else { return };
        for indexed in [false, true] {
            let mut hotbars = world.hotbars("rc-hotbar-tab-rows", &[0, 4, 8]);
            let (mut changes, mut previous_rows) = (0, Vec::new());
            for tick in 0..100 {
                match tick {
                    40 => {
                        world.options.set("key_key.hotbar.4", crate::options::Val::S("key.keyboard.z".into()));
                    }
                    55 => {
                        let names = Names::of(&world.data, &world.received, indexed);
                        hotbars.save(2, &world.row(), &names).expect("hotbar.nbt written");
                    }
                    75 => world.enchantments(&["minecraft:unbreaking", "minecraft:sharpness"]),
                    _ => {}
                }
                let language = match tick {
                    10..25 | 70..85 => Some(&world.english),
                    25..70 => Some(&world.french),
                    _ => None,
                };
                let sources = world.sources(language, indexed);
                let kept = hotbars.kept_tab_rows(&sources).to_vec();
                let made = hotbars.make_tab_rows(&sources);
                assert_eq!(kept, made, "tick {tick}, indexed {indexed}");
                assert_eq!(made.len(), 81);
                changes += usize::from(tick > 0 && made != previous_rows);
                previous_rows = made;
            }
            let makes = hotbars.tab_rows_made;
            // Renderer at 10, French at 25, key at 40, saved row at 55, English at 70, registries
            // at 75, no renderer at 85.
            assert_eq!(changes, 7, "indexed {indexed}: every change shows in the rows");
            assert_eq!(makes, 8, "indexed {indexed}: rows made at the start and at each change only");
            world.enchantments(&["minecraft:sharpness", "minecraft:unbreaking"]);
            world.options.set("key_key.hotbar.4", crate::options::Val::S("key.keyboard.4".into()));
        }
    }

    /// Micro-bench (ignored): `cargo test --release -p rc-app bench_hotbar_tab_rows -- --ignored
    /// --nocapture`. Rows of the hotbars tab made at every tick against kept, with 0, 1 and 9 saved
    /// rows.
    #[test]
    #[ignore = "micro-bench"]
    fn bench_hotbar_tab_rows() {
        let Some(world) = TabRowsWorld::new() else { return };
        let sources = world.sources(Some(&world.english), true);
        let walk = world.sources(Some(&world.english), false);
        for saved in [0, 1, 9] {
            let mut hotbars = world.hotbars("rc-hotbar-tab-rows-bench", &(0..saved).collect::<Vec<_>>());
            let time = |f: &mut dyn FnMut()| {
                let n = 2_000;
                (0..5)
                    .map(|_| {
                        let start = std::time::Instant::now();
                        for _ in 0..n {
                            f();
                        }
                        start.elapsed().as_nanos() as f64 / f64::from(n)
                    })
                    .fold(f64::MAX, f64::min)
            };
            let made_walk = time(&mut || drop(std::hint::black_box(hotbars.make_tab_rows(&walk))));
            let made = time(&mut || drop(std::hint::black_box(hotbars.make_tab_rows(&sources))));
            let kept = time(&mut || {
                std::hint::black_box(hotbars.kept_tab_rows(&sources).len());
            });
            println!("{saved} saved rows: made (walks) {made_walk:>9.0} ns, made (indexes) {made:>9.0} ns, kept {kept:>6.1} ns");
        }
    }
}
