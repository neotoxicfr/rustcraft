//! Disk compound a chunk keeps to rewrite it (`anvil-world.md` § 7), without holding a second copy
//! of what memory already holds.
//!
//! The save writes, at each height, the section compound read (unknown keys and key order kept),
//! its containers re-encoded when they changed, and the current light. Memory already holds the
//! containers and the light, so a section keeps only what memory cannot give back:
//! - a section memory rebuilds byte for byte (usual keys only, palette entries written the way our
//!   encoder or the game writes them) keeps only the order of its keys ([`RebuiltSection`]);
//! - any other keeps its compound, minus its light arrays and the packed words moved into memory;
//! - the light read stays shared with memory ([`DiskSection::sky`], free while memory keeps the same
//!   layer) and is written when memory has none, like the old compound did;
//! - `sections`, `block_entities` and complete `Heightmaps` stay in the root compound as empty
//!   values, so that the save writes the keys in the same order.

use rc_data::VersionData;
use rc_proto::nbt::Nbt;
use rc_world::{Container, LightLayer, Section};

use crate::states::States;

/// Which parts of the disk compound a chunk leaves to memory (one switch each, read once).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiskKeeping {
    /// Sections taken out of the root compound and kept per height, light arrays shared with
    /// memory, packed words moved into memory, complete height maps kept once
    /// (`RUSTCRAFT_OPT_SLIM_DISK_COMPOUND`).
    pub slim: bool,
    /// A read section memory rebuilds exactly keeps only its key order (on top of `slim`,
    /// `RUSTCRAFT_OPT_DROP_REBUILDABLE_SECTIONS`).
    pub drop_rebuildable: bool,
    /// The block entity list lives in memory only (`RUSTCRAFT_OPT_NO_DUPLICATE_BLOCK_ENTITIES`).
    pub no_duplicate_block_entities: bool,
    /// A generated chunk keeps no encoded section, the save writes them from memory (on top of
    /// `slim`, `RUSTCRAFT_OPT_LAZY_GENERATED_SECTION_NBT`).
    pub lazy_generated_sections: bool,
}

#[cfg(test)]
impl DiskKeeping {
    /// Everything kept as read (all the switches at 0).
    pub(crate) const OLD: Self = Self {
        slim: false,
        drop_rebuildable: false,
        no_duplicate_block_entities: false,
        lazy_generated_sections: false,
    };
    /// Everything memory holds left to memory.
    pub(crate) const SLIMMEST: Self =
        Self { slim: true, drop_rebuildable: true, no_duplicate_block_entities: true, lazy_generated_sections: true };
}

impl DiskKeeping {
    pub(crate) fn from_switches() -> Self {
        let slim = rc_data::opt_switch!("RUSTCRAFT_OPT_SLIM_DISK_COMPOUND");
        Self {
            slim,
            drop_rebuildable: slim && rc_data::opt_switch!("RUSTCRAFT_OPT_DROP_REBUILDABLE_SECTIONS"),
            no_duplicate_block_entities: rc_data::opt_switch!("RUSTCRAFT_OPT_NO_DUPLICATE_BLOCK_ENTITIES"),
            lazy_generated_sections: slim && rc_data::opt_switch!("RUSTCRAFT_OPT_LAZY_GENERATED_SECTION_NBT"),
        }
    }
}

/// `RUSTCRAFT_OPT_ENCODE_WITHOUT_COPIES=0`: the save copies the kept compound (root, sections, block
/// entities, height maps) instead of moving the parts into the written compound and back.
pub(crate) fn encode_without_copies() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_ENCODE_WITHOUT_COPIES")
}

/// A key of a section compound the save can rebuild from memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionKey {
    Y,
    BlockStates,
    Biomes,
    BlockLight,
    SkyLight,
}

impl SectionKey {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Y => "Y",
            Self::BlockStates => "block_states",
            Self::Biomes => "biomes",
            Self::BlockLight => "BlockLight",
            Self::SkyLight => "SkyLight",
        }
    }

    fn of(name: &str) -> Option<Self> {
        [Self::Y, Self::BlockStates, Self::Biomes, Self::BlockLight, Self::SkyLight]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

/// Key order of a compound written from memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyOrder {
    /// Our encoder's insertion order.
    Ours,
    /// The game's: it keeps a compound in a hash table and writes it in table order.
    Game,
}

/// How a rebuilt section writes one of its containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContainerWriting {
    /// As memory holds it (palette in its order, unused entries included, packed words as they
    /// are), keys and palette entries in the given order: the bytes read, while it is unchanged.
    AsInMemory(KeyOrder),
    /// Our disk encoding (palette compacted in order of first appearance), as after a change.
    Reencoded,
}

/// A section the save rebuilds entirely from memory: its keys in write order and how its
/// containers are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RebuiltSection {
    pub(crate) keys: [Option<SectionKey>; 5],
    pub(crate) blocks: ContainerWriting,
    pub(crate) biomes: ContainerWriting,
}

impl RebuiltSection {
    /// A section written by our encoder: `Y`, then the containers.
    pub(crate) fn ours(blocks: ContainerWriting, biomes: ContainerWriting) -> Self {
        use SectionKey::*;
        Self { keys: [Some(Y), Some(BlockStates), Some(Biomes), None, None], blocks, biomes }
    }

    fn only_y() -> Self {
        Self {
            keys: [Some(SectionKey::Y), None, None, None, None],
            blocks: ContainerWriting::Reencoded,
            biomes: ContainerWriting::Reencoded,
        }
    }

    fn has(&self, key: SectionKey) -> bool {
        self.keys.contains(&Some(key))
    }

    /// Adds `key` at the end, like a save writing a key the compound lacked.
    fn append(&mut self, key: SectionKey) {
        if let Some(slot) = self.keys.iter_mut().find(|k| k.is_none()) {
            *slot = Some(key);
        }
    }
}

/// What the save writes at one height of the chunk, besides what memory holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum SectionForm {
    /// No section at this height on disk: written only once it holds blocks or light.
    #[default]
    Absent,
    /// Rebuilt from memory, nothing kept but the key order.
    Rebuilt(RebuiltSection),
    /// The compound read, with empty values where memory gives the content back: light arrays
    /// (`ByteArray []`), packed words moved into memory ([`moved_words`]), and a container to
    /// re-encode from memory (`{}`). None of them can be read from a stored chunk: a light array
    /// or a container that empty makes it corrupt, and words are read as a long array only.
    Kept(Box<Nbt>),
}

/// One height of the chunk (`sections + 2`, bottom to top: the light layers below and above the
/// world have one too) as the save rewrites it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskSection {
    pub(crate) form: SectionForm,
    /// Light layers last read or written: written again when memory has none.
    pub(crate) sky: KeptLight,
    pub(crate) block: KeptLight,
}

/// A light layer last read or written, kept for a save where memory has none.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum KeptLight {
    #[default]
    None,
    /// All its bytes equal (0 under the terrain, 0xff in full light). Named instead of holding
    /// the shared sheet: every loader thread counting references on the one shared sheet made
    /// them wait for each other (reading 5 µs slower per chunk on 32 threads).
    Uniform(u8),
    /// Its own sheet, the very one of memory while memory keeps it.
    Own(std::sync::Arc<[u8; rc_world::LIGHT_BYTES]>),
}

impl KeptLight {
    pub(crate) fn of(layer: &LightLayer) -> Self {
        let Some(l) = layer else { return Self::None };
        // A sheet held by memory alone is not one of the shared uniform sheets: no need to look.
        if std::sync::Arc::strong_count(l) == 1 {
            return Self::Own(l.clone());
        }
        // Compared 16 bytes at a time: a lit sheet is told apart within a few words.
        let word = u128::from_ne_bytes([l[0]; 16]);
        if l.as_chunks::<16>().0.iter().all(|w| u128::from_ne_bytes(*w) == word) {
            Self::Uniform(l[0])
        } else {
            Self::Own(l.clone())
        }
    }

    /// The bytes to write, if any.
    pub(crate) fn bytes(&self) -> Option<Vec<u8>> {
        match self {
            Self::None => None,
            Self::Uniform(b) => Some(vec![*b; rc_world::LIGHT_BYTES]),
            Self::Own(l) => Some(l.to_vec()),
        }
    }
}

/// Slot of `key` in the game's hash table of a compound of `len` keys (Java string hash, spread
/// and masked; 16 slots, doubled while more than three quarters would be used).
fn game_slot(key: &str, len: usize) -> usize {
    let mut slots = 16;
    while len > slots * 3 / 4 {
        slots *= 2;
    }
    let step = |h: i32, c: u16| h.wrapping_mul(31).wrapping_add(i32::from(c));
    // ASCII (every key the game writes here): one UTF-16 unit per byte, no re-encoding.
    let h = if key.is_ascii() {
        key.bytes().fold(0, |h, b| step(h, u16::from(b)))
    } else {
        key.encode_utf16().fold(0, step)
    } as u32;
    ((h ^ (h >> 16)) as usize) & (slots - 1)
}

/// Sorts `items`, given in the game's insertion order, into the order the game writes them
/// (table slot, ties in insertion order).
fn into_game_order<T>(items: &mut [T], key: impl Fn(&T) -> &str) {
    let len = items.len();
    if len > 16 {
        items.sort_by_key(|i| game_slot(key(i), len));
        return;
    }
    // A handful of keys: each hashed once, then a stable insertion sort on the slots.
    let mut slots = [0usize; 16];
    for (slot, item) in slots.iter_mut().zip(items.iter()) {
        *slot = game_slot(key(item), len);
    }
    for i in 1..len {
        let mut j = i;
        while j > 0 && slots[j - 1] > slots[j] {
            slots.swap(j - 1, j);
            items.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// The game's order of a container's keys (checked against the table by
/// `game_order_of_known_compounds`): fixed, so not worked out per section.
const GAME_CONTAINER_KEYS: [&str; 2] = ["data", "palette"];
/// Keys of a palette entry compound: `id` then `properties`, in our order and in the game's alike
/// (checked by `game_order_of_known_compounds`).
const ENTRY_KEYS: [&str; 2] = ["id", "properties"];

/// Inner keys of a disk container in `order` (`palette`, then `data` when it has packed words),
/// and how many.
fn container_keys(with_words: bool, order: KeyOrder) -> ([&'static str; 2], usize) {
    match (with_words, order) {
        (true, KeyOrder::Game) => (GAME_CONTAINER_KEYS, 2),
        (with_words, _) => (["palette", "data"], 1 + usize::from(with_words)),
    }
}

/// Game write order of a block's properties (inserted by name from the last, then laid out by
/// table slot), as positions in their definition order.
type PropertyOrder = [u8; 16];

thread_local! {
    /// [`PropertyOrder`] per block, worked out at the first state of the block met on this thread
    /// (sorting and hashing the names for every palette entry cost a read 5 µs per chunk), for the
    /// data of the given address.
    static GAME_PROPERTY_ORDERS: std::cell::RefCell<(usize, Vec<Option<PropertyOrder>>)> =
        const { std::cell::RefCell::new((0, Vec::new())) };
}

fn game_property_order(data: &VersionData, block: usize) -> PropertyOrder {
    GAME_PROPERTY_ORDERS.with_borrow_mut(|(of_data, orders)| {
        let here = std::ptr::from_ref(data) as usize;
        if *of_data != here || orders.len() != data.blocks.len() {
            *of_data = here;
            orders.clear();
            orders.resize(data.blocks.len(), None);
        }
        *orders[block].get_or_insert_with(|| {
            // (position in the definition order, name)
            let mut order: Vec<(u8, &str)> = data.blocks[block]
                .properties
                .iter()
                .take(16)
                .enumerate()
                .map(|(i, &p)| (i as u8, data.properties[p as usize].name.as_str()))
                .collect();
            order.sort_by(|a, b| b.1.cmp(a.1));
            into_game_order(&mut order, |p| p.1);
            let mut out = [0u8; 16];
            for (o, (i, _)) in out.iter_mut().zip(&order) {
                *o = *i;
            }
            out
        })
    })
}

/// Properties of a block state in write order: ours (definition order), or the game's.
/// `None`: more than 16 properties.
fn ordered_properties(data: &VersionData, state: u32, order: KeyOrder) -> Option<([(&str, &str); 16], usize)> {
    let mut props = [("", ""); 16];
    let mut n = 0;
    for p in data.state_properties(state) {
        *props.get_mut(n)? = p;
        n += 1;
    }
    if order == KeyOrder::Game {
        let block = data.state(state)?.block as usize;
        let game = game_property_order(data, block);
        let definition = props;
        for (slot, &from) in props[..n].iter_mut().zip(&game[..n]) {
            *slot = definition[from as usize];
        }
    }
    Some((props, n))
}

/// Disk palette entry of a block state with its keys in `order` (only the properties move: the
/// entry's own keys are in the same order both ways, [`ENTRY_KEYS`]).
pub(crate) fn block_entry(data: &VersionData, states: &States, state: u32, order: KeyOrder) -> Nbt {
    let mut entry = states.palette_entry(data, state);
    if order == KeyOrder::Game
        && let Some(Nbt::Compound(props)) = crate::nbtio::get_mut(&mut entry, "properties")
    {
        props.sort_by(|a, b| b.0.cmp(&a.0));
        into_game_order(props, |p| p.0.as_str());
    }
    entry
}

/// Is `read` the entry [`block_entry`] gives for `state` in `order`? Compared without building it.
fn block_entry_matches(read: &Nbt, data: &VersionData, state: u32, order: KeyOrder) -> bool {
    let Some(info) = data.block_of(state) else { return read.as_str() == Some("minecraft:air") };
    if state == info.default_state || info.properties.is_empty() {
        return read.as_str() == Some(info.name.as_str());
    }
    let (Nbt::Compound(fields), Some((props, n))) = (read, ordered_properties(data, state, order)) else {
        return false;
    };
    let props_match = |v: &Nbt| {
        matches!(v, Nbt::Compound(p) if p.len() == n
            && p.iter().zip(&props[..n]).all(|((k, v), (want_k, want_v))| k == want_k && v.as_str() == Some(want_v)))
    };
    fields.len() == 2
        && fields.iter().zip(ENTRY_KEYS).all(|((k, v), want)| {
            k == want && if want == "id" { v.as_str() == Some(info.name.as_str()) } else { props_match(v) }
        })
}

/// Disk palette entry of a biome: its name.
pub(crate) fn biome_name(states: &States, biome: u32) -> &str {
    states.biome_names.get(biome as usize).map_or("minecraft:plains", String::as_str)
}

/// Left as a container's `data` where its packed words were moved into memory: an empty list,
/// which no stored container holds as its words (they are read as a long array only).
pub(crate) fn moved_words() -> Nbt {
    Nbt::List(Vec::new())
}

fn is_moved_words(v: &Nbt) -> bool {
    matches!(v, Nbt::List(l) if l.is_empty())
}

/// Palette and packed words of a container as memory holds it; `None` without palette (direct
/// values: never read from disk).
fn palette_and_words(c: &Container) -> Option<(&[u32], Option<&[u64]>)> {
    match c {
        Container::Single(v) => Some((std::slice::from_ref(v), None)),
        Container::Packed { palette: Some(p), data, .. } => Some((p, Some(data))),
        Container::Packed { palette: None, .. } => None,
    }
}

/// Disk container `{palette, data?}` written exactly as memory holds `c`, keys in `order`.
/// `None`: a container without palette.
fn container_as_in_memory(c: &Container, order: KeyOrder, entry: impl Fn(u32) -> Nbt) -> Option<Nbt> {
    let (palette, words) = palette_and_words(c)?;
    let (keys, n) = container_keys(words.is_some(), order);
    let fields = keys[..n]
        .iter()
        .map(|&k| {
            let v = match words {
                Some(w) if k == "data" => Nbt::LongArray(w.iter().map(|&x| x as i64).collect()),
                _ => Nbt::List(palette.iter().map(|&v| entry(v)).collect()),
            };
            (k.to_owned(), v)
        })
        .collect();
    Some(Nbt::Compound(fields))
}

/// Does the disk container `read` give back exactly `c` written as in memory in `order`? Its
/// packed words may have been moved into `c` ([`moved_words`] left).
fn container_matches(read: &Nbt, c: &Container, order: KeyOrder, entry_matches: impl Fn(&Nbt, u32) -> bool) -> bool {
    let (Nbt::Compound(fields), Some((palette, words))) = (read, palette_and_words(c)) else { return false };
    let (keys, n) = container_keys(words.is_some(), order);
    fields.len() == n
        && fields.iter().zip(&keys[..n]).all(|((k, v), &want)| {
            k == want
                && match (want, v, words) {
                    ("palette", Nbt::List(l), _) => {
                        l.len() == palette.len() && l.iter().zip(palette).all(|(e, &id)| entry_matches(e, id))
                    }
                    ("data", v, Some(_)) if is_moved_words(v) => true,
                    ("data", Nbt::LongArray(l), Some(w)) => {
                        l.len() == w.len() && l.iter().zip(w).all(|(&a, &b)| a as u64 == b)
                    }
                    _ => false,
                }
        })
}

/// The order in which `read` writes `c` as memory holds it, if it does.
fn writing_of(
    read: &Nbt,
    c: &Container,
    entry_matches: impl Fn(&Nbt, u32, KeyOrder) -> bool,
) -> Option<ContainerWriting> {
    [KeyOrder::Ours, KeyOrder::Game]
        .into_iter()
        .find(|&o| container_matches(read, c, o, |e, id| entry_matches(e, id, o)))
        .map(ContainerWriting::AsInMemory)
}

/// Keys of a section read at height `y` when memory (`section`, `None` outside the world's
/// sections) gives every value back exactly.
fn rebuildable(
    read: &Nbt,
    y: i32,
    section: Option<&Section>,
    data: &VersionData,
    states: &States,
) -> Option<RebuiltSection> {
    let Nbt::Compound(fields) = read else { return None };
    let mut r = RebuiltSection { keys: [None; 5], ..RebuiltSection::only_y() };
    for (k, v) in fields {
        let key = SectionKey::of(k).filter(|&key| !r.has(key))?;
        match key {
            SectionKey::Y => i8::try_from(y).is_ok_and(|b| *v == Nbt::Byte(b)).then_some(())?,
            SectionKey::BlockLight | SectionKey::SkyLight => matches!(v, Nbt::ByteArray(_)).then_some(())?,
            SectionKey::BlockStates => {
                r.blocks = writing_of(v, &section?.blocks, |e, id, o| block_entry_matches(e, data, id, o))?;
            }
            SectionKey::Biomes => {
                r.biomes = writing_of(v, &section?.biomes, |e, id, _| e.as_str() == Some(biome_name(states, id)))?;
            }
        }
        r.append(key);
    }
    Some(r)
}

/// `s` with its light arrays emptied (memory and [`DiskSection`] hold the layers).
pub(crate) fn without_light(mut s: Nbt) -> Nbt {
    if let Nbt::Compound(fields) = &mut s {
        for (k, v) in fields {
            if matches!(k.as_str(), "BlockLight" | "SkyLight") && matches!(v, Nbt::ByteArray(_)) {
                *v = Nbt::ByteArray(Vec::new());
            }
        }
    }
    s
}

/// Form of a section read at height `y`, its light already read into memory and its packed words
/// possibly moved there: rebuilt when memory gives it back exactly and `drop_rebuildable`, kept
/// (light arrays emptied) otherwise.
pub(crate) fn read_section_form(
    s: Nbt,
    y: i32,
    section: Option<&Section>,
    drop_rebuildable: bool,
    data: &VersionData,
    states: &States,
) -> SectionForm {
    if drop_rebuildable && let Some(r) = rebuildable(&s, y, section, data, states) {
        return SectionForm::Rebuilt(r);
    }
    SectionForm::Kept(Box::new(without_light(s)))
}

/// Takes the list at `key` out of `root`, an empty list left in its place (same key order).
pub(crate) fn take_list(root: &mut Nbt, key: &str) -> Vec<Nbt> {
    match crate::nbtio::get_mut(root, key) {
        Some(Nbt::List(l)) => std::mem::take(l),
        _ => Vec::new(),
    }
}

/// Empties the compound at `key` of `root` in place (same key order).
pub(crate) fn empty_compound(root: &mut Nbt, key: &str) {
    if let Some(Nbt::Compound(c)) = crate::nbtio::get_mut(root, key) {
        *c = Vec::new();
    }
}

/// What the save writes for a container of a section: re-encoded from memory, or as memory holds it.
fn container_to_write(
    c: &Container,
    writing: ContainerWriting,
    entry: impl Fn(u32, KeyOrder) -> Nbt,
    encode: impl Fn(&Container) -> Nbt,
) -> Nbt {
    match writing {
        ContainerWriting::AsInMemory(order) => {
            container_as_in_memory(c, order, |v| entry(v, order)).unwrap_or_else(|| encode(c))
        }
        ContainerWriting::Reencoded => encode(c),
    }
}

/// What the save does to one section this time.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SectionWrite {
    pub blocks_reencoded: bool,
    pub biomes_reencoded: bool,
    /// Light keys written (in place, or added when missing).
    pub block_light: bool,
    pub sky_light: bool,
    /// Written at all: something besides its `Y`.
    pub written: bool,
}

/// Section compound at height `y` as read, rebuilt from memory before the save's own changes;
/// the light values and the containers about to be re-encoded are left empty (the save writes
/// them). `None`: no section at this height on disk.
pub(crate) fn section_as_read(
    form: &SectionForm,
    y: i32,
    section: Option<&Section>,
    reencoding: SectionWrite,
    data: &VersionData,
    states: &States,
) -> Option<Nbt> {
    let block = |v: u32, o: KeyOrder| block_entry(data, states, v, o);
    let biome = |v: u32, _: KeyOrder| Nbt::String(biome_name(states, v).to_owned());
    let blocks = |c: &Container| crate::chunk::encode_blocks(c, data, states);
    let biomes = |c: &Container| crate::chunk::encode_biomes(c, states);
    match form {
        SectionForm::Absent => None,
        SectionForm::Rebuilt(r) => {
            let fields = r.keys.iter().flatten().map(|&key| {
                let v = match (key, section) {
                    (SectionKey::Y, _) => Nbt::Byte(y as i8),
                    (SectionKey::BlockStates, Some(s)) if !reencoding.blocks_reencoded => {
                        container_to_write(&s.blocks, r.blocks, block, blocks)
                    }
                    (SectionKey::Biomes, Some(s)) if !reencoding.biomes_reencoded => {
                        container_to_write(&s.biomes, r.biomes, biome, biomes)
                    }
                    (SectionKey::BlockStates | SectionKey::Biomes, _) => Nbt::Compound(Vec::new()),
                    (SectionKey::BlockLight | SectionKey::SkyLight, _) => Nbt::ByteArray(Vec::new()),
                };
                (key.name().to_owned(), v)
            });
            Some(Nbt::Compound(fields.collect()))
        }
        SectionForm::Kept(kept) => {
            let mut s = (**kept).clone();
            if let (Nbt::Compound(fields), Some(sec)) = (&mut s, section) {
                for (k, v) in fields {
                    match k.as_str() {
                        "block_states" if !reencoding.blocks_reencoded => fill_from_memory(v, &sec.blocks, blocks),
                        "biomes" if !reencoding.biomes_reencoded => fill_from_memory(v, &sec.biomes, biomes),
                        _ => {}
                    }
                }
            }
            Some(s)
        }
    }
}

/// A kept container: re-encoded when left empty, its moved packed words put back.
fn fill_from_memory(v: &mut Nbt, c: &Container, encode: impl Fn(&Container) -> Nbt) {
    match v {
        Nbt::Compound(f) if f.is_empty() => *v = encode(c),
        Nbt::Compound(f) => {
            if let (Some((_, slot)), Container::Packed { data, .. }) = (f.iter_mut().find(|(k, _)| k == "data"), c)
                && is_moved_words(slot)
            {
                *slot = Nbt::LongArray(data.iter().map(|&w| w as i64).collect());
            }
        }
        _ => {}
    }
}

/// Form of a section once the save wrote it, mirroring the save: containers re-encoded and light
/// keys written in place, or added at the end when missing.
pub(crate) fn form_after_save(form: SectionForm, y: i32, w: SectionWrite, drop_rebuildable: bool) -> SectionForm {
    if !w.written {
        return SectionForm::Absent;
    }
    match form {
        SectionForm::Absent => SectionForm::Rebuilt(rebuilt_after_save(RebuiltSection::only_y(), w)),
        SectionForm::Rebuilt(r) => SectionForm::Rebuilt(rebuilt_after_save(r, w)),
        SectionForm::Kept(mut kept) => {
            let reencode = || Nbt::Compound(Vec::new());
            let light = || Nbt::ByteArray(Vec::new());
            for (key, done, v) in [
                ("block_states", w.blocks_reencoded, reencode()),
                ("biomes", w.biomes_reencoded, reencode()),
                ("BlockLight", w.block_light, light()),
                ("SkyLight", w.sky_light, light()),
            ] {
                if done {
                    crate::nbtio::set(&mut kept, key, v);
                }
            }
            match drop_rebuildable.then(|| rebuilt_kept(&kept, y)).flatten() {
                Some(r) => SectionForm::Rebuilt(r),
                None => SectionForm::Kept(kept),
            }
        }
    }
}

fn rebuilt_after_save(mut r: RebuiltSection, w: SectionWrite) -> RebuiltSection {
    for (key, done) in [
        (SectionKey::BlockStates, w.blocks_reencoded),
        (SectionKey::Biomes, w.biomes_reencoded),
        (SectionKey::BlockLight, w.block_light),
        (SectionKey::SkyLight, w.sky_light),
    ] {
        if done && !r.has(key) {
            r.append(key);
        }
    }
    if w.blocks_reencoded {
        r.blocks = ContainerWriting::Reencoded;
    }
    if w.biomes_reencoded {
        r.biomes = ContainerWriting::Reencoded;
    }
    r
}

/// A kept section left with nothing but the usual keys, its containers to re-encode: rebuilt.
fn rebuilt_kept(kept: &Nbt, y: i32) -> Option<RebuiltSection> {
    let Nbt::Compound(fields) = kept else { return None };
    let mut r = RebuiltSection { keys: [None; 5], ..RebuiltSection::only_y() };
    for (k, v) in fields {
        let key = SectionKey::of(k).filter(|&key| !r.has(key))?;
        let usual = match key {
            SectionKey::Y => *v == Nbt::Byte(y as i8),
            SectionKey::BlockLight | SectionKey::SkyLight => matches!(v, Nbt::ByteArray(_)),
            SectionKey::BlockStates | SectionKey::Biomes => matches!(v, Nbt::Compound(c) if c.is_empty()),
        };
        usual.then_some(())?;
        r.append(key);
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's order of the keys of a section, of a container and of a stair state, as read in
    /// the files of an official world.
    #[test]
    fn game_order_of_known_compounds() {
        let mut section = ["block_states", "biomes", "BlockLight", "SkyLight", "Y"];
        into_game_order(&mut section, |k| k);
        assert_eq!(section, ["block_states", "SkyLight", "biomes", "BlockLight", "Y"]);
        let mut container = ["palette", "data"];
        into_game_order(&mut container, |k| k);
        assert_eq!(container, GAME_CONTAINER_KEYS);
        let mut entry = ["id", "properties"];
        into_game_order(&mut entry, |k| k);
        assert_eq!(entry, ENTRY_KEYS);
        let mut stairs = ["waterlogged", "shape", "half", "facing"];
        into_game_order(&mut stairs, |k| k);
        assert_eq!(stairs, ["waterlogged", "half", "shape", "facing"]);
        let mut dripstone = ["waterlogged", "vertical_direction", "thickness"];
        into_game_order(&mut dripstone, |k| k);
        assert_eq!(dripstone, ["waterlogged", "vertical_direction", "thickness"]);
    }
}
