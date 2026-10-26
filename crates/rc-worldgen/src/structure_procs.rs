//! Template processors with fixed tables (`generation-decoration.md` § 10.4): ageing of the
//! stone and replacement by blackstone.

use crate::decor_blocks::{BlockId, Blocks, Dir, State};
use crate::decor_rng::DecoRng;

/// Class of a block for ageing, in the order of the tests of [`block_age`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgeClass {
    None,
    Stone,
    Stairs,
    Slab,
    Wall,
    Obsidian,
}

/// Processor tables resolved once per block set (`RUSTCRAFT_OPT_PROCS_TABLES`).
#[derive(Debug)]
pub struct ProcTables {
    pub jigsaw: Option<BlockId>,
    pub structure_void: Option<BlockId>,
    /// Per block: blackstone block that replaces it.
    blackstone_to: Vec<Option<BlockId>>,
    /// Per block: ageing class.
    age: Vec<AgeClass>,
    /// Per block: container whose loot seed is removed then drawn at placement.
    pub loot_container: Vec<bool>,
    /// Ageing output states (`cracked_stone_bricks`, `mossy_stone_bricks`,
    /// `stone_slab`, `stone_brick_slab`, `mossy_stone_brick_slab`, `mossy_stone_brick_wall`,
    /// `crying_obsidian`, `mossy_stone_brick_stairs`).
    age_out: [Option<State>; 8],
}

impl ProcTables {
    pub fn new(b: &Blocks) -> ProcTables {
        let n = b.block_count();
        let class = |bl: usize| {
            let s = b.default_state(bl as BlockId);
            let name = b.block_name(bl as BlockId);
            let n = name.strip_prefix("minecraft:").unwrap_or(name);
            let in_tag = |t: &str| b.tag(t).is_some_and(|t| b.in_tag(s, t));
            if matches!(n, "stone_bricks" | "stone" | "chiseled_stone_bricks") {
                AgeClass::Stone
            } else if in_tag("minecraft:stairs") {
                AgeClass::Stairs
            } else if in_tag("minecraft:slabs") {
                AgeClass::Slab
            } else if in_tag("minecraft:walls") {
                AgeClass::Wall
            } else if n == "obsidian" {
                AgeClass::Obsidian
            } else {
                AgeClass::None
            }
        };
        ProcTables {
            jigsaw: b.block_id("minecraft:jigsaw"),
            structure_void: b.block_id("minecraft:structure_void"),
            blackstone_to: (0..n)
                .map(|bl| {
                    let name = b.block_name(bl as BlockId);
                    let n = name.strip_prefix("minecraft:").unwrap_or(name);
                    BLACKSTONE.iter().find(|(f, _)| *f == n).and_then(|(_, to)| b.block_id(to))
                })
                .collect(),
            age: (0..n).map(class).collect(),
            loot_container: (0..n).map(|bl| crate::structure_place::is_loot_container(b.block_name(bl as BlockId))).collect(),
            age_out: [
                st(b, "cracked_stone_bricks"),
                st(b, "mossy_stone_bricks"),
                st(b, "stone_slab"),
                st(b, "stone_brick_slab"),
                st(b, "mossy_stone_brick_slab"),
                st(b, "mossy_stone_brick_wall"),
                st(b, "crying_obsidian"),
                st(b, "mossy_stone_brick_stairs"),
            ],
        }
    }
}

/// [`block_age`] through the tables: same draws, in the same order.
pub fn block_age_fast(b: &Blocks, t: &ProcTables, s: State, mossiness: f32, r: &mut DecoRng) -> Option<State> {
    let [cracked, mossy_bricks, stone_slab, brick_slab, mossy_slab, mossy_wall, crying, mossy_stairs] = t.age_out;
    let pick = |r: &mut DecoRng, non: [Option<State>; 2], mossy: [Option<State>; 2]| {
        let list = if r.next_float() < mossiness { mossy } else { non };
        list[r.next_int_bounded(2) as usize]
    };
    match t.age[b.block(s) as usize] {
        AgeClass::None => None,
        AgeClass::Stone => {
            if r.next_float() >= 0.5 {
                return None;
            }
            let non = [cracked, random_stairs(b, r, "stone_brick_stairs")];
            let mossy = [mossy_bricks, random_stairs(b, r, "mossy_stone_brick_stairs")];
            pick(r, non, mossy)
        }
        AgeClass::Stairs => {
            if r.next_float() >= 0.5 {
                return None;
            }
            pick(r, [stone_slab, brick_slab], [mossy_stairs.map(|m| b.copy_props(s, m)), mossy_slab])
        }
        AgeClass::Slab => {
            if r.next_float() < mossiness {
                mossy_slab.map(|m| b.copy_props(s, m))
            } else {
                None
            }
        }
        AgeClass::Wall => {
            if r.next_float() < mossiness {
                mossy_wall.map(|m| b.copy_props(s, m))
            } else {
                None
            }
        }
        AgeClass::Obsidian => {
            if r.next_float() < 0.15 {
                crying
            } else {
                None
            }
        }
    }
}

/// [`blackstone`] through the block table: same properties copied.
pub fn blackstone_fast(b: &Blocks, t: &ProcTables, s: State) -> State {
    let Some(to) = t.blackstone_to[b.block(s) as usize] else { return s };
    blackstone_props(b, s, b.default_state(to))
}

fn blackstone_props(b: &Blocks, s: State, mut out: State) -> State {
    for p in ["facing", "half", "type"] {
        if let Some(pr) = b.prop(p)
            && let Some(v) = b.get_str(s, pr)
            && b.has(out, pr)
        {
            let v = v.to_owned();
            out = b.with_str(out, pr, &v);
        }
    }
    out
}

const BLACKSTONE: [(&str, &str); 23] = [
    ("cobblestone", "blackstone"),
    ("mossy_cobblestone", "blackstone"),
    ("stone", "polished_blackstone"),
    ("stone_bricks", "polished_blackstone_bricks"),
    ("mossy_stone_bricks", "polished_blackstone_bricks"),
    ("cobblestone_stairs", "blackstone_stairs"),
    ("mossy_cobblestone_stairs", "blackstone_stairs"),
    ("stone_stairs", "polished_blackstone_stairs"),
    ("stone_brick_stairs", "polished_blackstone_brick_stairs"),
    ("mossy_stone_brick_stairs", "polished_blackstone_brick_stairs"),
    ("cobblestone_slab", "blackstone_slab"),
    ("mossy_cobblestone_slab", "blackstone_slab"),
    ("smooth_stone_slab", "polished_blackstone_slab"),
    ("stone_slab", "polished_blackstone_slab"),
    ("stone_brick_slab", "polished_blackstone_brick_slab"),
    ("mossy_stone_brick_slab", "polished_blackstone_brick_slab"),
    ("stone_brick_wall", "polished_blackstone_brick_wall"),
    ("mossy_stone_brick_wall", "polished_blackstone_brick_wall"),
    ("cobblestone_wall", "blackstone_wall"),
    ("mossy_cobblestone_wall", "blackstone_wall"),
    ("chiseled_stone_bricks", "chiseled_polished_blackstone"),
    ("cracked_stone_bricks", "cracked_polished_blackstone_bricks"),
    ("iron_bars", "iron_chain"),
];

fn st(b: &Blocks, n: &str) -> Option<State> {
    b.block_id(n).map(|id| b.default_state(id))
}

fn random_stairs(b: &Blocks, r: &mut DecoRng, n: &str) -> Option<State> {
    let s = st(b, n)?;
    let facing = [Dir::North, Dir::East, Dir::South, Dir::West][r.next_int_bounded(4) as usize];
    let half = ["top", "bottom"][r.next_int_bounded(2) as usize];
    let s = b.with_dir(s, b.prop("facing")?, facing);
    Some(b.with_str(s, b.prop("half")?, half))
}

/// Ageing (`block_age`): new state, or `None` if unchanged.
pub fn block_age(b: &Blocks, s: State, mossiness: f32, r: &mut DecoRng) -> Option<State> {
    let name = b.block_name(b.block(s)).to_owned();
    let n = name.strip_prefix("minecraft:").unwrap_or(&name);
    let in_tag = |t: &str| b.tag(t).is_some_and(|t| b.in_tag(s, t));
    let pick = |r: &mut DecoRng, non: [Option<State>; 2], mossy: [Option<State>; 2]| {
        let list = if r.next_float() < mossiness { mossy } else { non };
        list[r.next_int_bounded(2) as usize]
    };
    if matches!(n, "stone_bricks" | "stone" | "chiseled_stone_bricks") {
        if r.next_float() >= 0.5 {
            return None;
        }
        let non = [st(b, "cracked_stone_bricks"), random_stairs(b, r, "stone_brick_stairs")];
        let mossy = [st(b, "mossy_stone_bricks"), random_stairs(b, r, "mossy_stone_brick_stairs")];
        return pick(r, non, mossy);
    }
    if in_tag("minecraft:stairs") {
        if r.next_float() >= 0.5 {
            return None;
        }
        let non = [st(b, "stone_slab"), st(b, "stone_brick_slab")];
        let mossy = [st(b, "mossy_stone_brick_stairs").map(|m| b.copy_props(s, m)), st(b, "mossy_stone_brick_slab")];
        return pick(r, non, mossy);
    }
    if in_tag("minecraft:slabs") {
        return if r.next_float() < mossiness {
            st(b, "mossy_stone_brick_slab").map(|m| b.copy_props(s, m))
        } else {
            None
        };
    }
    if in_tag("minecraft:walls") {
        return if r.next_float() < mossiness {
            st(b, "mossy_stone_brick_wall").map(|m| b.copy_props(s, m))
        } else {
            None
        };
    }
    if n == "obsidian" {
        return if r.next_float() < 0.15 { st(b, "crying_obsidian") } else { None };
    }
    None
}

/// Replacement by blackstone (`blackstone_replace`), orientation, half and type kept.
pub fn blackstone(b: &Blocks, s: State) -> State {
    let name = b.block_name(b.block(s)).to_owned();
    let n = name.strip_prefix("minecraft:").unwrap_or(&name);
    let Some((_, to)) = BLACKSTONE.iter().find(|(f, _)| *f == n) else { return s };
    let Some(out) = st(b, to) else { return s };
    blackstone_props(b, s, out)
}
