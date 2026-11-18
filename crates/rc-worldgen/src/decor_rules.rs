//! Block rules coded in the game that decoration depends on: survival (`would_survive`, simple
//! feature placements), shape update on contact with a neighbour (trees).

use crate::decor_blocks::{BlockId, Blocks, Dir, State};
use crate::decor_region::{Region, add};

/// Survival family of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// Always survives.
    #[default]
    Any,
    /// Below is in `supports_vegetation`.
    Vegetation,
    /// Double plant (upper half: below = lower half of the same block).
    DoublePlant,
    DryVegetation,
    Mushroom,
    LeafLitter,
    Carpet,
    MossyCarpet,
    LilyPad,
    Seagrass,
    TallSeagrass,
    SeaPickle,
    SugarCane,
    Cactus,
    Azalea,
    SmallDripleaf,
    SporeBlossom,
    MangrovePropagule,
    NetherSprouts,
    CrimsonRoots,
    WarpedRoots,
    CrimsonFungus,
    WarpedFungus,
    SoulFire,
    Fire,
    SnowLayer,
    Bamboo,
    HangingRoots,
    Multiface,
    Vine,
    CactusFlower,
}

pub fn kind_for(name: &str) -> Kind {
    use Kind::*;
    match name {
        "short_grass" | "fern" | "bush" | "red_shrub" | "firefly_bush" | "pink_petals" | "wildflowers"
        | "sweet_berry_bush" | "dandelion" | "poppy" | "blue_orchid" | "allium" | "azure_bluet" | "red_tulip"
        | "orange_tulip" | "white_tulip" | "pink_tulip" | "oxeye_daisy" | "cornflower" | "lily_of_the_valley"
        | "torchflower" | "wither_rose" | "open_eyeblossom" | "closed_eyeblossom" | "oak_sapling"
        | "spruce_sapling" | "birch_sapling" | "jungle_sapling" | "acacia_sapling" | "cherry_sapling"
        | "dark_oak_sapling" | "pale_oak_sapling" | "poplar_sapling" | "golden_dandelion" => Vegetation,
        "tall_grass" | "large_fern" | "lilac" | "rose_bush" | "peony" | "sunflower" | "pitcher_plant" => DoublePlant,
        "dead_bush" | "short_dry_grass" | "tall_dry_grass" => DryVegetation,
        "brown_mushroom" | "red_mushroom" => Mushroom,
        "leaf_litter" => LeafLitter,
        "moss_carpet" => Carpet,
        "pale_moss_carpet" => MossyCarpet,
        "lily_pad" => LilyPad,
        "seagrass" => Seagrass,
        "tall_seagrass" => TallSeagrass,
        "sea_pickle" => SeaPickle,
        "sugar_cane" => SugarCane,
        "cactus" => Cactus,
        "azalea" | "flowering_azalea" => Azalea,
        "small_dripleaf" => SmallDripleaf,
        "spore_blossom" => SporeBlossom,
        "mangrove_propagule" => MangrovePropagule,
        "nether_sprouts" => NetherSprouts,
        "crimson_roots" => CrimsonRoots,
        "warped_roots" => WarpedRoots,
        "crimson_fungus" => CrimsonFungus,
        "warped_fungus" => WarpedFungus,
        "soul_fire" => SoulFire,
        "fire" => Fire,
        "snow" => SnowLayer,
        "bamboo" | "bamboo_sapling" => Bamboo,
        "hanging_roots" => HangingRoots,
        "glow_lichen" | "sculk_vein" | "resin_clump" => Multiface,
        "vine" => Vine,
        "cactus_flower" => CactusFlower,
        _ => Any,
    }
}

/// Tags used by the rules, resolved once.
#[derive(Debug, Clone, Copy)]
pub struct RuleTags {
    pub supports_vegetation: u16,
    pub supports_dry_vegetation: u16,
    pub overrides_mushroom_light: u16,
    pub supports_lily_pad: u16,
    pub cannot_support_seagrass: u16,
    pub supports_sugar_cane: u16,
    pub supports_sugar_cane_adj: u16,
    pub supports_cactus: u16,
    pub supports_azalea: u16,
    pub supports_small_dripleaf: u16,
    pub supports_mangrove_propagule: u16,
    pub supports_hanging_mangrove_propagule: u16,
    pub supports_nether_sprouts: u16,
    pub supports_crimson_roots: u16,
    pub supports_warped_roots: u16,
    pub supports_crimson_fungus: u16,
    pub supports_warped_fungus: u16,
    pub soul_fire_base: u16,
    pub cannot_support_snow: u16,
    pub support_override_snow: u16,
    pub supports_bamboo: u16,
    pub support_override_cactus_flower: u16,
    pub leaves: u16,
    pub logs: u16,
    pub replaceable_by_trees: u16,
    pub prevents_leaf_decay: u16,
    pub snow: u16,
    pub supports_cocoa: u16,
    pub base_stone_overworld: u16,
    pub replaceable_by_mushrooms: u16,
    pub fire: u16,
    pub sculk_growth_inhibitors: u16,
    pub sculk_replaceable_world_gen: u16,
    pub sculk_replaceable: u16,
    pub fences: u16,
    pub wooden_fences: u16,
    pub walls: u16,
    pub stairs: u16,
    pub fence_gates: u16,
    pub doors: u16,
    pub wall_post_override: u16,
    pub beneath_bamboo: u16,
    pub features_cannot_replace: u16,
    pub supports_chorus: u16,
    pub fluid_lily: u8,
    pub fluid_sugar_cane_adj: u8,
}

impl RuleTags {
    pub fn new_empty() -> Self {
        let m = u16::MAX;
        RuleTags {
            supports_vegetation: m,
            supports_dry_vegetation: m,
            overrides_mushroom_light: m,
            supports_lily_pad: m,
            cannot_support_seagrass: m,
            supports_sugar_cane: m,
            supports_sugar_cane_adj: m,
            supports_cactus: m,
            supports_azalea: m,
            supports_small_dripleaf: m,
            supports_mangrove_propagule: m,
            supports_hanging_mangrove_propagule: m,
            supports_nether_sprouts: m,
            supports_crimson_roots: m,
            supports_warped_roots: m,
            supports_crimson_fungus: m,
            supports_warped_fungus: m,
            soul_fire_base: m,
            cannot_support_snow: m,
            support_override_snow: m,
            supports_bamboo: m,
            support_override_cactus_flower: m,
            leaves: m,
            logs: m,
            replaceable_by_trees: m,
            prevents_leaf_decay: m,
            snow: m,
            supports_cocoa: m,
            base_stone_overworld: m,
            replaceable_by_mushrooms: m,
            fire: m,
            sculk_growth_inhibitors: m,
            sculk_replaceable_world_gen: m,
            sculk_replaceable: m,
            fences: m,
            wooden_fences: m,
            walls: m,
            stairs: m,
            fence_gates: m,
            doors: m,
            wall_post_override: m,
            beneath_bamboo: m,
            features_cannot_replace: m,
            supports_chorus: m,
            fluid_lily: 0,
            fluid_sugar_cane_adj: 0,
        }
    }

    pub fn new(b: &Blocks) -> Self {
        let t = |n: &str| b.tag(n).unwrap_or(u16::MAX);
        RuleTags {
            supports_vegetation: t("supports_vegetation"),
            supports_dry_vegetation: t("supports_dry_vegetation"),
            overrides_mushroom_light: t("overrides_mushroom_light_requirement"),
            supports_lily_pad: t("supports_lily_pad"),
            cannot_support_seagrass: t("cannot_support_seagrass"),
            supports_sugar_cane: t("supports_sugar_cane"),
            supports_sugar_cane_adj: t("supports_sugar_cane_adjacently"),
            supports_cactus: t("supports_cactus"),
            supports_azalea: t("supports_azalea"),
            supports_small_dripleaf: t("supports_small_dripleaf"),
            supports_mangrove_propagule: t("supports_mangrove_propagule"),
            supports_hanging_mangrove_propagule: t("supports_hanging_mangrove_propagule"),
            supports_nether_sprouts: t("supports_nether_sprouts"),
            supports_crimson_roots: t("supports_crimson_roots"),
            supports_warped_roots: t("supports_warped_roots"),
            supports_crimson_fungus: t("supports_crimson_fungus"),
            supports_warped_fungus: t("supports_warped_fungus"),
            soul_fire_base: t("soul_fire_base_blocks"),
            cannot_support_snow: t("cannot_support_snow_layer"),
            support_override_snow: t("support_override_snow_layer"),
            supports_bamboo: t("supports_bamboo"),
            support_override_cactus_flower: t("support_override_cactus_flower"),
            leaves: t("leaves"),
            logs: t("logs"),
            replaceable_by_trees: t("replaceable_by_trees"),
            prevents_leaf_decay: t("prevents_nearby_leaf_decay"),
            snow: t("snow"),
            supports_cocoa: t("supports_cocoa"),
            base_stone_overworld: t("base_stone_overworld"),
            replaceable_by_mushrooms: t("replaceable_by_mushrooms"),
            fire: t("fire"),
            sculk_growth_inhibitors: t("sculk_growth_inhibitors"),
            sculk_replaceable_world_gen: t("sculk_replaceable_world_gen"),
            sculk_replaceable: t("sculk_replaceable"),
            fences: t("fences"),
            wooden_fences: t("wooden_fences"),
            walls: t("walls"),
            stairs: t("stairs"),
            fence_gates: t("fence_gates"),
            doors: t("doors"),
            wall_post_override: t("wall_post_override"),
            beneath_bamboo: t("beneath_bamboo_podzol_replaceable"),
            features_cannot_replace: t("features_cannot_replace"),
            supports_chorus: t("minecraft:supports_chorus_plant"),
            fluid_lily: b.fluid_tag("supports_lily_pad").unwrap_or(0),
            fluid_sugar_cane_adj: b.fluid_tag("supports_sugar_cane_adjacently").unwrap_or(0),
        }
    }
}

#[inline]
fn tag(b: &Blocks, s: State, t: u16) -> bool {
    t != u16::MAX && b.in_tag(s, t)
}

#[inline]
fn below(p: [i32; 3]) -> [i32; 3] {
    [p[0], p[1] - 1, p[2]]
}

#[inline]
fn above(p: [i32; 3]) -> [i32; 3] {
    [p[0], p[1] + 1, p[2]]
}

fn is_upper(b: &Blocks, s: State) -> bool {
    b.get_str(s, b.p.half) == Some("upper")
}

/// Could the block `s` stay at `p`?
pub fn can_survive(r: &Region, s: State, p: [i32; 3]) -> bool {
    let b = r.b;
    let t = &b.rules;
    let bl = b.block(s);
    let veg = |q: State| tag(b, q, t.supports_vegetation);
    match b.kind(bl) {
        Kind::Any => true,
        Kind::Vegetation => veg(r.get_p(below(p))),
        Kind::DoublePlant => {
            if is_upper(b, s) {
                let d = r.get_p(below(p));
                b.is(d, bl) && !is_upper(b, d)
            } else {
                veg(r.get_p(below(p)))
            }
        }
        Kind::DryVegetation => tag(b, r.get_p(below(p)), t.supports_dry_vegetation),
        Kind::Mushroom => {
            let d = r.get_p(below(p));
            // Zero light during generation: the light condition is always true.
            tag(b, d, t.overrides_mushroom_light) || b.solid_render(d)
        }
        Kind::LeafLitter => b.sturdy(r.get_p(below(p)), Dir::Up),
        Kind::Carpet => !b.is_air(r.get_p(below(p))),
        Kind::MossyCarpet => {
            let d = r.get_p(below(p));
            if b.get_bool(s, b.p.bottom).unwrap_or(true) {
                !b.is_air(d)
            } else {
                b.is(d, bl) && b.get_bool(d, b.p.bottom).unwrap_or(false)
            }
        }
        Kind::LilyPad => {
            let d = r.get_p(below(p));
            (crate::decor_pred::fluid_bits(b, d) & t.fluid_lily != 0 || tag(b, d, t.supports_lily_pad))
                && b.fluid(r.get_p(p)) == 0
        }
        Kind::Seagrass => {
            let d = r.get_p(below(p));
            b.sturdy(d, Dir::Up) && !tag(b, d, t.cannot_support_seagrass)
        }
        Kind::TallSeagrass => {
            if is_upper(b, s) {
                let d = r.get_p(below(p));
                b.is(d, bl) && !is_upper(b, d)
            } else {
                let d = r.get_p(below(p));
                let here = r.get_p(p);
                b.sturdy(d, Dir::Up)
                    && !tag(b, d, t.cannot_support_seagrass)
                    && b.fluid(here) == 1
                    && b.facts(here).fluid_amount == 8
            }
        }
        Kind::SeaPickle => {
            let d = r.get_p(below(p));
            b.facts(d).coll_up_nonempty() || b.sturdy(d, Dir::Up)
        }
        Kind::SugarCane => {
            let d = r.get_p(below(p));
            if b.is(d, bl) {
                return true;
            }
            if tag(b, d, t.supports_sugar_cane) {
                let q = below(p);
                for dir in Dir::NESW {
                    let n = r.get_p(add(q, dir));
                    if crate::decor_pred::fluid_bits(b, n) & t.fluid_sugar_cane_adj != 0
                        || tag(b, n, t.supports_sugar_cane_adj)
                    {
                        return true;
                    }
                }
            }
            false
        }
        Kind::Cactus => {
            for dir in Dir::NESW {
                let n = r.get_p(add(p, dir));
                if b.legacy_solid(n) || b.fluid(n) == 2 {
                    return false;
                }
            }
            let d = r.get_p(below(p));
            (b.is(d, bl) || tag(b, d, t.supports_cactus)) && !b.has_flag(r.get_p(above(p)), rc_data::flags::LIQUID)
        }
        Kind::Azalea => tag(b, r.get_p(below(p)), t.supports_azalea),
        Kind::SmallDripleaf => {
            if is_upper(b, s) {
                let d = r.get_p(below(p));
                b.is(d, bl) && !is_upper(b, d)
            } else {
                let d = r.get_p(below(p));
                tag(b, d, t.supports_small_dripleaf) || (b.water_source(r.get_p(p)) && veg(d))
            }
        }
        Kind::SporeBlossom => {
            let u = r.get_p(above(p));
            b.facts(u).center_down() && b.fluid(r.get_p(p)) != 1
        }
        Kind::MangrovePropagule => {
            if b.get_bool(s, b.p.hanging) == Some(true) {
                tag(b, r.get_p(above(p)), t.supports_hanging_mangrove_propagule)
            } else {
                tag(b, r.get_p(below(p)), t.supports_mangrove_propagule)
            }
        }
        Kind::NetherSprouts => tag(b, r.get_p(below(p)), t.supports_nether_sprouts),
        Kind::CrimsonRoots => tag(b, r.get_p(below(p)), t.supports_crimson_roots),
        Kind::WarpedRoots => tag(b, r.get_p(below(p)), t.supports_warped_roots),
        Kind::CrimsonFungus => tag(b, r.get_p(below(p)), t.supports_crimson_fungus),
        Kind::WarpedFungus => tag(b, r.get_p(below(p)), t.supports_warped_fungus),
        Kind::SoulFire => tag(b, r.get_p(below(p)), t.soul_fire_base),
        Kind::Fire => b.sturdy(r.get_p(below(p)), Dir::Up),
        Kind::SnowLayer => {
            let d = r.get_p(below(p));
            if tag(b, d, t.cannot_support_snow) {
                false
            } else if tag(b, d, t.support_override_snow) {
                true
            } else {
                b.facts(d).coll_faces >> Dir::Up as u8 & 1 != 0
                    || (b.is(d, b.k.snow) && b.get_int(d, b.p.layers) == Some(8))
            }
        }
        Kind::Bamboo => tag(b, r.get_p(below(p)), t.supports_bamboo),
        Kind::HangingRoots => b.sturdy(r.get_p(above(p)), Dir::Down),
        Kind::Multiface => {
            let mut any = false;
            for d in Dir::ALL {
                if multiface_has(b, s, d) {
                    if !multiface_can_attach(r, p, d) {
                        return false;
                    }
                    any = true;
                }
            }
            any
        }
        Kind::Vine => {
            // A vine survives if at least one face remains attachable. Up: the game looks at the
            // UP face of the block above (direction Down passed for this block; quirk
            // reproduced, no effect on a cube).
            if vine_has(b, s, Dir::Up) && can_attach_to(b, r.get_p(above(p)), Dir::Down) {
                return true;
            }
            for d in [Dir::North, Dir::South, Dir::West, Dir::East] {
                if vine_has(b, s, d) && vine_can_attach(r, p, d) {
                    return true;
                }
            }
            false
        }
        Kind::CactusFlower => {
            let d = r.get_p(below(p));
            tag(b, d, t.support_override_cactus_flower) || b.facts(d).center_up()
        }
    }
}

// ---------------------------------------------------------------- Multiface and vines

pub fn face_prop(b: &Blocks, d: Dir) -> crate::decor_blocks::Prop {
    match d {
        Dir::Down => b.p.down,
        Dir::Up => b.p.up,
        Dir::North => b.p.north,
        Dir::South => b.p.south,
        Dir::West => b.p.west,
        Dir::East => b.p.east,
    }
}

pub fn multiface_has(b: &Blocks, s: State, d: Dir) -> bool {
    b.get_bool(s, face_prop(b, d)) == Some(true)
}

/// Attachable face: full face (support or collision shape) of the neighbour turned towards us.
pub fn multiface_can_attach(r: &Region, p: [i32; 3], d: Dir) -> bool {
    can_attach_to(r.b, r.get_p(add(p, d)), d)
}

/// The neighbour (in direction d) has a full face turned towards us.
pub fn can_attach_to(b: &Blocks, n: State, d: Dir) -> bool {
    let f = b.facts(n);
    let bit = 1u8 << d.opposite() as u8;
    (f.sup_faces | f.coll_faces) & bit != 0
}

pub fn vine_has(b: &Blocks, s: State, d: Dir) -> bool {
    b.get_bool(s, face_prop(b, d)) == Some(true)
}

/// Vine attachable on face d: full face of the neighbour (collision shape) turned towards us,
/// or, for the top, in addition a vine above.
pub fn vine_can_attach(r: &Region, p: [i32; 3], d: Dir) -> bool {
    vine_can_support_at_face(r, p, d)
}

/// The support test of a vine on a face.
fn vine_can_support_at_face(r: &Region, p: [i32; 3], d: Dir) -> bool {
    if d == Dir::Down {
        return false;
    }
    let b = r.b;
    if can_attach_to(b, r.get_p(add(p, d)), d) {
        return true;
    }
    if d.axis() == 1 {
        return false;
    }
    let up = r.get_p(add(p, Dir::Up));
    b.is(up, b.k.vine) && vine_has(b, up, d)
}

/// The state update of a vine.
pub fn vine_updated(r: &Region, mut s: State, p: [i32; 3]) -> State {
    let b = r.b;
    let up_p = add(p, Dir::Up);
    if vine_has(b, s, Dir::Up) {
        // UP face of the block above (direction Down passed by the game; quirk reproduced).
        s = b.with_bool(s, b.p.up, can_attach_to(b, r.get_p(up_p), Dir::Down));
    }
    for d in Dir::NESW {
        if vine_has(b, s, d) {
            let mut ok = vine_can_support_at_face(r, p, d);
            if !ok {
                let above = r.get_p(up_p);
                ok = b.is(above, b.k.vine) && vine_has(b, above, d);
            }
            s = b.with_bool(s, face_prop(b, d), ok);
        }
    }
    s
}

fn vine_has_faces(b: &Blocks, s: State) -> bool {
    [Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East].iter().any(|d| vine_has(b, s, *d))
}

/// Shape update of a block on contact with a neighbour (direction d towards the neighbour).
pub fn update_shape(r: &Region, s: State, p: [i32; 3], d: Dir, n: State) -> State {
    let b = r.b;
    let bl = b.block(s);
    let air = b.default_state(b.k.air);
    if bl == b.k.grass_block || bl == b.k.podzol || bl == b.k.mycelium {
        return if d == Dir::Up { b.with_bool(s, b.p.snowy, tag(b, n, b.rules.snow)) } else { s };
    }
    if bl == b.k.vine {
        if d == Dir::Down {
            return s;
        }
        let u = vine_updated(r, s, p);
        return if vine_has_faces(b, u) { u } else { air };
    }
    if bl == b.k.cocoa {
        if b.get_dir(s, b.p.facing) == Some(d) && !tag(b, r.get_p(add(p, d)), b.rules.supports_cocoa) {
            return air;
        }
        return s;
    }
    if bl == b.k.pale_hanging_moss {
        let below_is = b.is(r.get_p(add(p, Dir::Down)), bl);
        return b.with_bool(s, b.p.tip, !below_is);
    }
    match b.kind(bl) {
        Kind::DoublePlant | Kind::TallSeagrass | Kind::SmallDripleaf => {
            let lower = !is_upper(b, s);
            if d.axis() != 1 || lower != (d == Dir::Up) || (b.is(n, bl) && is_upper(b, n) != !lower) {
                if lower && d == Dir::Down && !can_survive(r, s, p) { air } else { s }
            } else {
                air
            }
        }
        Kind::Vegetation
        | Kind::DryVegetation
        | Kind::Mushroom
        | Kind::LeafLitter
        | Kind::Carpet
        | Kind::Seagrass
        | Kind::SeaPickle
        | Kind::Azalea
        | Kind::NetherSprouts
        | Kind::CrimsonRoots
        | Kind::WarpedRoots
        | Kind::CrimsonFungus
        | Kind::WarpedFungus
        | Kind::SnowLayer
        | Kind::LilyPad
        | Kind::MangrovePropagule => {
            if can_survive(r, s, p) {
                s
            } else {
                air
            }
        }
        Kind::Multiface => {
            if !Dir::ALL.iter().any(|dd| multiface_has(b, s, *dd)) {
                return air;
            }
            if multiface_has(b, s, d) && !can_attach_to(b, n, d) {
                return b.with_bool(s, face_prop(b, d), false);
            }
            s
        }
        _ => s,
    }
}

/// Block that [`update_shape`] always returns unchanged (last case of its rule): same lists as
/// its branches, in the same order; the shape equivalence proof checks it state by state.
pub fn shape_inert(b: &Blocks, bl: BlockId) -> bool {
    let k = &b.k;
    if [k.grass_block, k.podzol, k.mycelium, k.vine, k.cocoa, k.pale_hanging_moss].contains(&bl) {
        return false;
    }
    !matches!(
        b.kind(bl),
        Kind::DoublePlant
            | Kind::TallSeagrass
            | Kind::SmallDripleaf
            | Kind::Vegetation
            | Kind::DryVegetation
            | Kind::Mushroom
            | Kind::LeafLitter
            | Kind::Carpet
            | Kind::Seagrass
            | Kind::SeaPickle
            | Kind::Azalea
            | Kind::NetherSprouts
            | Kind::CrimsonRoots
            | Kind::WarpedRoots
            | Kind::CrimsonFungus
            | Kind::WarpedFungus
            | Kind::SnowLayer
            | Kind::LilyPad
            | Kind::MangrovePropagule
            | Kind::Multiface
    )
}

#[allow(dead_code)]
fn unused(_: BlockId) {}
