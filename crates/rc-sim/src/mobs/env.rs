//! The world as seen by mobs: block states, fluids, light ("magic" value),
//! visible sky, height maps, biomes, walkability for pathing.

use rc_data::flags;

use crate::server::Server;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fluid {
    None,
    Water,
    Lava,
}

impl Server {
    pub(crate) fn st(&self, p: [i32; 3]) -> u32 {
        self.state_at(p).unwrap_or(0)
    }

    pub(crate) fn bname(&self, p: [i32; 3]) -> &str {
        self.data.block_of(self.st(p)).map_or("minecraft:air", |b| b.name.as_str())
    }

    pub(crate) fn state_name(&self, s: u32) -> &str {
        self.data.block_of(s).map_or("minecraft:air", |b| b.name.as_str())
    }

    pub(crate) fn sflag(&self, p: [i32; 3], f: u16) -> bool {
        self.data.state(self.st(p)).is_some_and(|i| i.has(f))
    }

    pub(crate) fn is_air(&self, p: [i32; 3]) -> bool {
        self.sflag(p, flags::AIR)
    }

    /// The solidity test (old "solid" property).
    pub(crate) fn is_solid(&self, p: [i32; 3]) -> bool {
        self.sflag(p, flags::LEGACY_SOLID)
    }

    pub(crate) fn solid_render(&self, p: [i32; 3]) -> bool {
        self.sflag(p, flags::SOLID_RENDER)
    }

    pub(crate) fn mob_fluid(&self, p: [i32; 3]) -> Fluid {
        let Some(i) = self.data.state(self.st(p)) else { return Fluid::None };
        if i.fluid == 0 {
            return Fluid::None;
        }
        match self.bs.t.fluid_mob.get(i.fluid as usize) {
            Some(1) => Fluid::Water,
            Some(2) => Fluid::Lava,
            _ => Fluid::None,
        }
    }

    /// Fluid EXACTLY `minecraft:water` (source or captive water), without flowing water: the
    /// fluid tags of the frog and the axolotl only contain the source.
    pub(crate) fn mob_source_water(&self, p: [i32; 3]) -> bool {
        let Some(i) = self.data.state(self.st(p)) else { return false };
        i.fluid != 0 && self.bs.t.fluid_water_source.get(i.fluid as usize).copied().unwrap_or(false)
    }

    pub(crate) fn has_collision(&self, p: [i32; 3]) -> bool {
        let s = self.st(p);
        if shape_mode() == 0 {
            let mut v = Vec::new();
            self.phys.collision(s, &mut v);
            return !v.is_empty();
        }
        !self.phys.shape(s).is_empty()
    }

    /// Top of the block's collision shape (`None` if empty).
    pub(crate) fn collision_top(&self, p: [i32; 3]) -> Option<f64> {
        let s = self.st(p);
        match shape_mode() {
            0 => {
                let mut v = Vec::new();
                self.phys.collision(s, &mut v);
                v.iter().map(|b| b.max[1]).fold(None, |m, y| Some(m.map_or(y, |m: f64| m.max(y))))
            }
            1 => self.phys.collision_top(s),
            _ => self.phys.collision_top_table(s),
        }
    }

    pub(crate) fn full_collision(&self, p: [i32; 3]) -> bool {
        self.sflag(p, flags::FULL_COLLISION)
    }

    /// Stored light (sky, block) of a position.
    pub(crate) fn light(&self, p: [i32; 3]) -> (u8, u8) {
        let min_y = self.dim.height.min_y;
        let Some(Some(c)) = self.chunks.get(&(p[0] >> 4, p[2] >> 4)) else { return (15, 0) };
        let ly = p[1] - min_y;
        let si = (ly >> 4) + 1;
        if si < 0 {
            return (0, 0);
        }
        let si = si as usize;
        if si >= c.sky.len() {
            return (15, 0);
        }
        let idx = (((ly & 15) as usize) << 8) | (((p[2] & 15) as usize) << 4) | (p[0] & 15) as usize;
        let nib = |b: &[u8; rc_world::LIGHT_BYTES]| (b[idx >> 1] >> ((idx & 1) * 4)) & 15;
        let sky = match &c.sky[si] {
            Some(l) => nib(l),
            None => match c.sky[si + 1..].iter().flatten().next() {
                None => 15,
                Some(above) => {
                    let i0 = idx & 0xFF;
                    (above[i0 >> 1] >> ((i0 & 1) * 4)) & 15
                }
            },
        };
        let block = c.block.get(si).and_then(|l| l.as_ref()).map_or(0, |l| nib(l));
        (sky, block)
    }

    /// Sky darkening: 15 − sky light level (day, rain, thunder).
    pub(crate) fn mob_sky_darken(&self) -> i32 {
        self.mob_sky_darken_with(sky_cache_on())
    }

    /// `mob_sky_darken`; `cache`: value kept as long as the time of day, the rain and the thunder
    /// have not moved (exact key: no moment of the tick is assumed fixed).
    pub(crate) fn mob_sky_darken_with(&self, cache: bool) -> i32 {
        if self.dim.name != "minecraft:overworld" {
            return 0;
        }
        let w = &self.life.weather;
        if !cache {
            return sky_darken_of(self.day_time(), w.rain_level, w.thunder_eff());
        }
        let key = (self.day_time(), w.rain_level.to_bits(), w.thunder_level.to_bits());
        let (k, v) = self.sky_darken.get();
        if k == Some(key) {
            return v;
        }
        let v = sky_darken_of(key.0, w.rain_level, w.thunder_eff());
        self.sky_darken.set((Some(key), v));
        v
    }
}

/// Key and value kept for `mob_sky_darken`: (time of day, rain bits, thunder bits).
pub(crate) type SkyDarkenCache = std::cell::Cell<(Option<(i64, u32, u32)>, i32)>;

/// `RUSTCRAFT_OPT_SKYDARK`, read once: 1 (default) sky darkening kept as long as its
/// inputs have not changed; 0 recomputed at each call.
fn sky_cache_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_SKYDARK").map(str::trim) != Some("0"))
}

/// Sky darkening of the overworld for time `day_time`, the rain and the effective thunder.
fn sky_darken_of(day_time: i64, rain_level: f32, thunder: f32) -> i32 {
    let t = day_time as f32;
    let keys = [(133.0f32, 1.0f32), (11867.0, 1.0), (13670.0, 0.266_666_68), (22330.0, 0.266_666_68)];
    let mut factor = 1.0;
    for k in 0..4 {
        let (t0, v0) = keys[k];
        let (t1, v1) = if k == 3 { (keys[0].0 + 24000.0, keys[0].1) } else { keys[k + 1] };
        let tt = if t < keys[0].0 { t + 24000.0 } else { t };
        if tt >= t0 && tt <= t1 {
            factor = v0 + (v1 - v0) * (tt - t0) / (t1 - t0);
            break;
        }
    }
    let mut level = 15.0 * factor;
    // Weather layers: rain weighs (rain − effective thunder), thunder its effective level
    // (thunder × rain); each blends towards 4 with its own alpha.
    let rain = rain_level - thunder;
    let lerp = |a: f32, s: f32, e: f32| s + a * (e - s);
    if rain > 0.0 {
        level = lerp(rain, level, lerp(0.3125, level, 4.0));
    }
    if thunder > 0.0 {
        level = lerp(thunder, level, lerp(0.527_343_75, level, 4.0));
    }
    (15.0 - level) as i32
}

impl Server {
    /// Maximum raw brightness.
    pub(crate) fn mob_brightness(&self, p: [i32; 3]) -> i32 {
        let (s, b) = self.light(p);
        (i32::from(s) - self.mob_sky_darken()).max(0).max(i32::from(b))
    }

    /// "Magic" light value (§ 1.7); ambient light 0 on the surface.
    pub(crate) fn magic_light(&self, p: [i32; 3]) -> f32 {
        let v = self.mob_brightness(p) as f32 / 15.0;
        let curve = v / (4.0 - 3.0 * v);
        let ambient = if self.dim.name == "minecraft:the_nether" { 0.1 } else { 0.0 };
        ambient + (1.0 - ambient) * curve
    }

    pub(crate) fn can_see_sky(&self, p: [i32; 3]) -> bool {
        self.light(p).0 >= 15
    }

    /// Rain (not snow) falling on this block, according to biome and altitude;
    /// never in the desert nor where it snows.
    pub(crate) fn is_raining_at(&self, p: [i32; 3]) -> bool {
        self.bs_raining_at(p)
    }

    /// Mob in water or in the rain (at its feet or at the top of its box).
    pub(crate) fn mob_in_water_or_rain(&self, m: &super::mob::Mob) -> bool {
        m.in_water || self.is_raining_at(m.block_pos()) || {
            let top = super::mth::block_of([m.pos[0], m.bbox().max[1], m.pos[2]]);
            self.is_raining_at(top)
        }
    }

    /// Height of a height map (network identifier: 1 surface, 4 motion blocking).
    pub(crate) fn surface_y(&self, x: i32, z: i32, map: i32) -> i32 {
        let min_y = self.dim.height.min_y;
        let Some(Some(c)) = self.chunks.get(&(x >> 4, z >> 4)) else { return min_y };
        let Some((_, longs)) = c.heightmaps.iter().find(|(k, _)| *k == map) else { return min_y };
        let h = self.dim.height.sections as u32 * 16;
        let bits = 32 - h.leading_zeros();
        let per = 64 / bits as usize;
        let i = ((z & 15) * 16 + (x & 15)) as usize;
        let Some(l) = longs.get(i / per) else { return min_y };
        let v = ((*l as u64) >> ((i % per) * bits as usize)) & ((1u64 << bits) - 1);
        min_y + v as i32
    }

    /// Biome (registry index) at a position.
    pub(crate) fn biome_at(&self, p: [i32; 3]) -> Option<usize> {
        let min_y = self.dim.height.min_y;
        let c = self.chunks.get(&(p[0] >> 4, p[2] >> 4))?.as_ref()?;
        let ly = (p[1] - min_y).clamp(0, self.dim.height.sections as i32 * 16 - 1);
        let s = c.sections.get((ly >> 4) as usize)?;
        let i = ((((ly & 15) >> 2) * 4 + ((p[2] & 15) >> 2)) * 4 + ((p[0] & 15) >> 2)) as usize;
        Some(s.biomes.get(i) as usize)
    }

    pub(crate) fn biome_name(&self, p: [i32; 3]) -> String {
        self.biome_str(p).to_owned()
    }

    /// Like `biome_name`, without copy (spawn attempts).
    pub(crate) fn biome_str(&self, p: [i32; 3]) -> &str {
        let id = self.biome_at(p).unwrap_or(0);
        self.regs.entries("minecraft:worldgen/biome").get(id).map_or("", String::as_str)
    }

    /// Biome in a tag (`#minecraft:…`).
    pub(crate) fn biome_tag(&self, p: [i32; 3], tag: &str) -> bool {
        let Some(id) = self.biome_at(p) else { return false };
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:worldgen/biome")
            .and_then(|(_, t)| t.iter().find(|(n, _)| n == tag))
            .is_some_and(|(_, ids)| ids.contains(&(id as i32)))
    }

    /// Block in a tag (`#minecraft:…`, sorted block identifiers).
    pub(crate) fn btag(&self, p: [i32; 3], tag: &str) -> bool {
        let block = self.data.state(self.st(p)).map_or(u32::MAX, |s| u32::from(s.block));
        self.tag_u32(tag, block, crate::tag_bits::mode())
    }

    /// Like `btag`, for a tag named in the code.
    pub(crate) fn btag_id(&self, p: [i32; 3], t: crate::tag_bits::TagId) -> bool {
        let block = self.data.state(self.st(p)).map_or(u32::MAX, |s| u32::from(s.block));
        self.tag_id_u32(t, block, crate::tag_bits::mode())
    }

    /// Block (index) in the tag named in the code `t`.
    pub(crate) fn tag_id(&self, t: crate::tag_bits::TagId, block: u16) -> bool {
        self.tag_id_u32(t, u32::from(block), crate::tag_bits::mode())
    }

    /// Block in tag `tag`, at level `mode` of `RUSTCRAFT_OPT_TAG_BITS`; a block outside
    /// the table goes through the sorted list (the only one that can hold an unknown identifier).
    pub(crate) fn tag_u32(&self, tag: &str, block: u32, mode: u8) -> bool {
        if mode >= 1
            && let Some(v) = self.tag_bits.has(tag, block)
        {
            return v;
        }
        self.block_tags.get(tag).is_some_and(|v| v.binary_search(&block).is_ok())
    }

    /// Block in the tag named `t`, at level `mode` of `RUSTCRAFT_OPT_TAG_BITS`.
    pub(crate) fn tag_id_u32(&self, t: crate::tag_bits::TagId, block: u32, mode: u8) -> bool {
        if mode >= 2
            && let Some(v) = self.tag_bits.has_id(t, block)
        {
            return v;
        }
        self.tag_u32(t.name(), block, mode)
    }

    /// Block tag `tag` of the block named `name` (full name, `minecraft:` included), for
    /// the conditions of loot tables: name resolved by the block table
    /// (`RUSTCRAFT_OPT_BLOCKIDS`, default) instead of a walk over all blocks. A closure
    /// that borrows the rest of the server goes through `TagBits::name_in_tag`.
    #[allow(dead_code)] // replaces the closures copied from the loot tables (owning missions)
    pub(crate) fn block_tag_by_name(&self, tag: &str, name: &str) -> bool {
        self.block_tag_by_name_with(tag, name, block_ids_on())
    }

    pub(crate) fn block_tag_by_name_with(&self, tag: &str, name: &str, fast: bool) -> bool {
        if !fast {
            let id = self.data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            return id.is_some_and(|id| self.block_tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()));
        }
        // Exact full name, like the comparison of the old walk.
        self.tag_bits.block_of_name(name).is_some_and(|b| self.tag_u32(tag, u32::from(b), crate::tag_bits::mode()))
    }

    /// Item in an item tag.
    pub(crate) fn item_tag(&self, item: &str, tag: &str) -> bool {
        self.life.items.has_tag(tag, item)
    }

    /// Walkable on foot, rules of the game's block classes.
    pub(crate) fn pathfindable_land(&self, p: [i32; 3]) -> bool {
        self.land_ok(self.st(p), mob_states_on())
    }

    /// Swimmable: by default, the cell holds water;
    /// blocks that override the rule (stairs, fences, walls, doors…) refuse it.
    pub(crate) fn pathfindable_water(&self, p: [i32; 3]) -> bool {
        self.water_ok(self.st(p), mob_states_on())
    }

    /// On-foot rule of state `s`: `mob_states` bit (`table`) or rule by name.
    pub(crate) fn land_ok(&self, s: u32, table: bool) -> bool {
        match self.mob_states.bits(s) {
            Some(b) if table => b & MOB_LAND != 0,
            _ => land_rule(&self.data, s),
        }
    }

    /// Swimming rule of state `s`: `mob_states` bit (`table`) or rule by name.
    pub(crate) fn water_ok(&self, s: u32, table: bool) -> bool {
        match self.mob_states.bits(s) {
            Some(b) if table => b & MOB_WATER != 0,
            _ => water_rule(&self.data, &self.bs.t.fluid_mob, s),
        }
    }
}

/// `RUSTCRAFT_OPT_MOBSTATES`, read once: 1 (default) walkability rules read from the per-state
/// bits of `Server::mob_states`, suffocation read from the collision table; 0 old path
/// (names and properties compared at each call, tag looked up by its name).
pub(crate) fn mob_states_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_MOBSTATES").map(str::trim) != Some("0"))
}

/// `RUSTCRAFT_OPT_BLOCKIDS`, read once: 1 (default) block name resolved by the block table;
/// 0 walk over all blocks at each call.
fn block_ids_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_BLOCKIDS").map(str::trim) != Some("0"))
}

/// Bit of `MobStates`: walkable on foot.
pub(crate) const MOB_LAND: u8 = 1;
/// Bit of `MobStates`: swimmable.
pub(crate) const MOB_WATER: u8 = 2;

/// Mob rules resolved once per state (names and properties: nothing depends on the
/// tags, so the table is valid after a reload of the data packs).
#[derive(Default)]
pub(crate) struct MobStates {
    bits: Box<[u8]>,
}

impl MobStates {
    pub(crate) fn new(d: &rc_data::VersionData, fluid_mob: &[u8]) -> MobStates {
        let bits = (0..d.states.len() as u32)
            .map(|s| (u8::from(land_rule(d, s)) * MOB_LAND) | (u8::from(water_rule(d, fluid_mob, s)) * MOB_WATER))
            .collect();
        MobStates { bits }
    }

    /// Bits of the state, `None` outside the table.
    pub(crate) fn bits(&self, s: u32) -> Option<u8> {
        self.bits.get(s as usize).copied()
    }

}

fn state_name(d: &rc_data::VersionData, s: u32) -> &str {
    d.block_of(s).map_or("minecraft:air", |b| b.name.as_str())
}

/// On-foot rule by name (old path, and table construction).
fn land_rule(d: &rc_data::VersionData, s: u32) -> bool {
    let name = state_name(d, s).trim_start_matches("minecraft:");
    let prop = |k: &str| d.state_properties(s).find(|(n, _)| *n == k).map(|(_, v)| v.to_owned());
    if name.ends_with("_door") || name.ends_with("_fence_gate") || name.ends_with("_trapdoor") {
        return prop("open").as_deref() == Some("true");
    }
    if name == "water" {
        return true;
    }
    if name == "lava" {
        return false;
    }
    if name == "powder_snow" {
        return true;
    }
    if name == "snow" {
        return prop("layers").and_then(|v| v.parse::<i32>().ok()).is_some_and(|l| l < 5);
    }
    if never_pathfindable(name, true) {
        return false;
    }
    !d.state(s).is_some_and(|i| i.has(flags::FULL_COLLISION))
}

/// Swimming rule by name (old path, and table construction); `fluid_mob`: kind
/// of fluid as seen by mobs (`Table::fluid_mob`).
fn water_rule(d: &rc_data::VersionData, fluid_mob: &[u8], s: u32) -> bool {
    let name = state_name(d, s).trim_start_matches("minecraft:");
    let prop = |k: &str| d.state_properties(s).find(|(n, _)| *n == k).map(|(_, v)| v.to_owned());
    if name.ends_with("_door") || name.ends_with("_fence_gate") {
        return false;
    }
    if name.ends_with("_trapdoor") {
        return prop("waterlogged").as_deref() == Some("true");
    }
    match name {
        "water" | "powder_snow" => return true,
        "lava" | "snow" => return false,
        _ => {}
    }
    if never_pathfindable(name, false) {
        return false;
    }
    d.state(s).is_some_and(|i| i.fluid != 0 && fluid_mob.get(i.fluid as usize) == Some(&1))
}

/// `RUSTCRAFT_OPT_SHAPEREF`, read once: 0 old path (shape copied into a fresh `Vec` at
/// each call), 1 shape borrowed and traversed, 2 (default) top read from the per-state table.
fn shape_mode() -> u8 {
    static M: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *M.get_or_init(|| match rc_data::env_str!("RUSTCRAFT_OPT_SHAPEREF").map(str::trim) {
        Some("0") => 0,
        Some("1") => 1,
        _ => 2,
    })
}

/// Blocks whose walkability rule is "never" (on foot and swimming); slabs
/// are only refused on foot (`slab`): swimming they follow the water.
fn never_pathfindable(name: &str, slab: bool) -> bool {
    const NEVER_SUFFIX: [&str; 12] = [
        "_bed",
        "_slab",
        "_stairs",
        "_fence",
        "_wall",
        "_pane",
        "_skull",
        "_head",
        "_wall_hanging_sign",
        "cauldron",
        "anvil",
        "campfire",
    ];
    const NEVER: [&str; 42] = [
        "azalea",
        "flowering_azalea",
        "bamboo",
        "bell",
        "brewing_stand",
        "cactus",
        "cake",
        "chain",
        "iron_chain",
        "chest",
        "trapped_chest",
        "ender_chest",
        "chorus_plant",
        "cocoa",
        "composter",
        "conduit",
        "iron_bars",
        "decorated_pot",
        "dragon_egg",
        "dried_ghast",
        "enchanting_table",
        "end_portal_frame",
        "farmland",
        "flower_pot",
        "grindstone",
        "heavy_core",
        "hopper",
        "lantern",
        "soul_lantern",
        "lectern",
        "mud",
        "dirt_path",
        "respawn_anchor",
        "end_rod",
        "lightning_rod",
        "sculk_sensor",
        "calibrated_sculk_sensor",
        "sea_pickle",
        "soul_sand",
        "pointed_dripstone",
        "sulfur_spike",
        "stonecutter",
    ];
    const PISTONS: [&str; 4] = ["piston", "sticky_piston", "piston_head", "moving_piston"];
    NEVER_SUFFIX.iter().any(|x| (slab || *x != "_slab") && name.ends_with(x))
        || NEVER.contains(&name)
        || PISTONS.contains(&name)
        || name.starts_with("potted_")
        || name.ends_with("candle_cake")
        || name.ends_with("_chain")
        || name.ends_with("_lantern")
        // Copper families (oxidation stages, waxed): lightning rods and bars.
        || name.ends_with("lightning_rod")
        || name.ends_with("copper_bars")
}

impl Server {
    /// Block that suffocates: by default a block of the `causes_suffocation` tag with a
    /// full collision shape; never for glass (stained, tinted), foliage, mangrove root
    /// blocks, copper grates and moving piston; always for farmland,
    /// soul sand, the path and mud; piston only when retracted; Shulker box
    /// only when closed (lid not tracked by the server: held as closed).
    pub(crate) fn suffocating(&self, s: u32) -> bool {
        self.suffocating_with(s, mob_states_on())
    }

    /// `suffocating` read from the collision table (`table`, same rules, tag applied at
    /// each reload) or computed by name.
    pub(crate) fn suffocating_with(&self, s: u32, table: bool) -> bool {
        if table && (s as usize) < self.data.states.len() {
            return self.phys.suffocating(s);
        }
        let name = self.state_name(s).trim_start_matches("minecraft:");
        let never = name == "glass"
            || name == "tinted_glass"
            || name.ends_with("_stained_glass")
            || name.ends_with("leaves")
            || name == "mangrove_roots"
            || name.ends_with("copper_grate")
            || name == "moving_piston";
        if never {
            return false;
        }
        if matches!(name, "farmland" | "soul_sand" | "dirt_path" | "mud") {
            return true;
        }
        if matches!(name, "piston" | "sticky_piston") {
            return !self.v().get_bool(s, "extended");
        }
        if name.ends_with("shulker_box") {
            return true;
        }
        self.tag_id(crate::tag_bits::TagId::CausesSuffocation, self.v().block(s))
            && self.data.state(s).is_some_and(|i| i.has(flags::FULL_COLLISION))
    }

    /// A suffocating block whose collision shape touches the eye box
    /// (`width × 0.8` per side, 1e-6 high, centered on the eyes).
    pub(crate) fn in_wall(&self, eye: [f64; 3], width: f32) -> bool {
        let h = f64::from(width * 0.8) / 2.0;
        let b = rc_physics::Aabb {
            min: [eye[0] - h, eye[1] - 0.5e-6, eye[2] - h],
            max: [eye[0] + h, eye[1] + 0.5e-6, eye[2] + h],
        };
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.floor() as i32);
        let w = self.world();
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let s = self.st([x, y, z]);
                    if s == 0 || !self.suffocating(s) {
                        continue;
                    }
                    // Eye box reduced to the cell: only this block's shape counts.
                    let c = [f64::from(x), f64::from(y), f64::from(z)];
                    let part = rc_physics::Aabb {
                        min: std::array::from_fn(|k| b.min[k].max(c[k])),
                        max: std::array::from_fn(|k| b.max[k].min(c[k] + 1.0)),
                    };
                    if (0..3).all(|k| part.min[k] < part.max[k]) && !rc_physics::is_free(&w, &part) {
                        return true;
                    }
                }
            }
        }
        false
    }
}
