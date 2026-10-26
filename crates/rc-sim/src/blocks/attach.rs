//! Remaining attached blocks (§ 2.13, § 3.16): tripwire hooks and tripwire, vines,
//! climbing plants (weeping, twisting and cave vines), multiface blocks (lichen…),
//! and the survival rules of the small families (banners, cakes, hanging signs, corals,
//! amethysts, hanging roots, lily pad, frogspawn…).
//!
//! Candles do not fall when their support disappears: the game only tests their survival on
//! placement (quirk kept).

use super::{Dir, Kind, P, above, below, flag, rel};
use crate::server::Server;

/// Survival rule of a small family (`Kind::Support`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sup {
    /// Full block (historical "solid") below; checked from below.
    BelowSolid,
    /// Full block behind (wall banner).
    WallSolid,
    /// Centre of the underside of the block above (hanging sign under a ceiling).
    CeilingCenter,
    /// Full face of the supporting block (amethysts, 6-face orientation).
    FacingSturdy,
    /// Full bottom face of the block above (hanging roots).
    HangingRoots,
    /// Centre of the underside of the block above, out of water (spore blossom).
    SporeBlossom,
    /// Plant on a block of a tag (index in `VEG_TAGS`); checked on every shape update.
    Veg(u8),
    /// Leaf litter: full top face below; checked on every shape update.
    LeafLitter,
    LilyPad,
    Frogspawn,
    SeaPickle,
    /// Seagrass: full top face below (except tag); water tick on every shape update.
    Seagrass,
    /// Coral or fan placed (alive: dies out of water).
    CoralBelow {
        alive: bool,
    },
    /// Wall fan.
    CoralWall {
        alive: bool,
    },
    /// Live coral block.
    CoralBlock,
}

pub(crate) const VEG_TAGS: [&str; 8] = [
    "minecraft:supports_wither_rose",
    "minecraft:supports_azalea",
    "minecraft:supports_crimson_roots",
    "minecraft:supports_warped_roots",
    "minecraft:supports_crimson_fungus",
    "minecraft:supports_warped_fungus",
    "minecraft:supports_nether_sprouts",
    "minecraft:supports_dry_vegetation",
];

/// Family of a block handled here (`None`: elsewhere).
pub(crate) fn kind_of(n: &str) -> Option<Kind> {
    use Sup::*;
    let veg = |i: u8| Some(Kind::Support(Veg(i)));
    match n {
        "tripwire_hook" => return Some(Kind::TripwireHook),
        "tripwire" => return Some(Kind::Tripwire),
        "glow_lichen" | "sculk_vein" | "resin_clump" => return Some(Kind::Multiface),
        "weeping_vines" | "cave_vines" => return Some(Kind::GrowingHead { up: false }),
        "twisting_vines" => return Some(Kind::GrowingHead { up: true }),
        "weeping_vines_plant" | "cave_vines_plant" => return Some(Kind::GrowingBody { up: false }),
        "twisting_vines_plant" => return Some(Kind::GrowingBody { up: true }),
        "kelp" => return Some(Kind::GrowingHead { up: true }),
        "bamboo" => return Some(Kind::Bamboo { sapling: false }),
        "bamboo_sapling" => return Some(Kind::Bamboo { sapling: true }),
        "scaffolding" => return Some(Kind::Scaffolding),
        "kelp_plant" => return Some(Kind::GrowingBody { up: true }),
        "seagrass" => return Some(Kind::Support(Seagrass)),
        "tall_seagrass" => return Some(Kind::DoublePlant),
        "cake" => return Some(Kind::Support(BelowSolid)),
        "wither_rose" => return veg(0),
        "azalea" | "flowering_azalea" => return veg(1),
        "crimson_roots" => return veg(2),
        "warped_roots" => return veg(3),
        "crimson_fungus" => return veg(4),
        "warped_fungus" => return veg(5),
        "nether_sprouts" => return veg(6),
        "short_dry_grass" | "tall_dry_grass" => return veg(7),
        "leaf_litter" => return Some(Kind::Support(LeafLitter)),
        "lily_pad" => return Some(Kind::Support(LilyPad)),
        "frogspawn" => return Some(Kind::Support(Frogspawn)),
        "sea_pickle" => return Some(Kind::Support(SeaPickle)),
        "hanging_roots" => return Some(Kind::Support(HangingRoots)),
        "spore_blossom" => return Some(Kind::Support(SporeBlossom)),
        // Bracket fungus: holds on the full face behind it, like amethysts.
        "amethyst_cluster" | "large_amethyst_bud" | "medium_amethyst_bud" | "small_amethyst_bud" | "shelf_mushroom" => {
            return Some(Kind::Support(FacingSturdy));
        }
        _ => {}
    }
    if n == "candle_cake" || n.ends_with("_candle_cake") {
        return Some(Kind::Support(BelowSolid));
    }
    if n.ends_with("_wall_banner") {
        return Some(Kind::Support(WallSolid));
    }
    if n.ends_with("_banner") {
        return Some(Kind::Support(BelowSolid));
    }
    if n.ends_with("_hanging_sign") && !n.ends_with("_wall_hanging_sign") {
        return Some(Kind::Support(CeilingCenter));
    }
    let coral = ["tube", "brain", "bubble", "fire", "horn"];
    for c in coral {
        let dead = n.starts_with("dead_");
        let m = n.strip_prefix("dead_").unwrap_or(n);
        if m == format!("{c}_coral") || m == format!("{c}_coral_fan") {
            return Some(Kind::Support(CoralBelow { alive: !dead }));
        }
        if m == format!("{c}_coral_wall_fan") {
            return Some(Kind::Support(CoralWall { alive: !dead }));
        }
        if !dead && m == format!("{c}_coral_block") {
            return Some(Kind::Support(CoralBlock));
        }
    }
    None
}

const HOOK_REACH: i32 = 42;

impl Server {
    fn name_of(&self, s: u32) -> &str {
        self.data.block_of(s).map_or("", |b| b.name.as_str())
    }

    /// State seen by the support tests: a moving block that has reached the end of its stroke
    /// (progress 1) presents the shape of the carried block, like the collision shape of the
    /// game's block entity; before that, the offset shape gives no full support.
    pub(crate) fn support_state(&self, q: P) -> u32 {
        let s = self.get(q);
        if self.v().kind(s) == Kind::MovingPiston
            && let Some(m) = self.bs.movers.get(&q)
            && m.progress >= 1.0
            && !m.source
        {
            return m.moved;
        }
        s
    }

    /// Full face `d` in the collision shape.
    pub(crate) fn collision_face_full(&self, s: u32, d: Dir) -> bool {
        let info = self.v().info(s);
        let m = self.bs.t.face_masks.get(info.collision as usize).map_or([0; 4], |m| m[d as usize]);
        m.iter().all(|&w| w == u64::MAX)
    }

    fn collision_face_empty(&self, s: u32, d: Dir) -> bool {
        let info = self.v().info(s);
        let m = self.bs.t.face_masks.get(info.collision as usize).map_or([0; 4], |m| m[d as usize]);
        m.iter().all(|&w| w == 0)
    }

    /// Attachment of a face of a multiface block (full support or full collision of the neighbour).
    pub(crate) fn multiface_can_attach(&self, p: P, d: Dir) -> bool {
        self.neighbour_accepts(rel(p, d), d)
    }

    /// The block at `q`, reached by going towards `d`, offers a full face (support or collision)
    /// on the side we come from. The vine sometimes passes a cell and a direction that do not
    /// go together (game quirk reproduced): the given cell is then tested exactly.
    fn neighbour_accepts(&self, q: P, d: Dir) -> bool {
        let ns = self.support_state(q);
        self.v().sturdy(ns, d.opposite()) || self.collision_face_full(ns, d.opposite())
    }

    // --- Survival of the small families. ---

    pub(crate) fn support_survives(&self, sup: Sup, s: u32, p: P) -> bool {
        let v = self.v();
        let b = self.support_state(below(p));
        match sup {
            Sup::BelowSolid => v.legacy_solid(b),
            Sup::WallSolid => v.legacy_solid(self.support_state(rel(p, v.facing(s).opposite()))),
            Sup::CeilingCenter => v.center_down(self.get(above(p))),
            Sup::FacingSturdy | Sup::CoralWall { .. } => {
                let f = v.facing(s);
                v.sturdy(self.support_state(rel(p, f.opposite())), f)
            }
            Sup::HangingRoots => v.sturdy(self.get(above(p)), Dir::Down),
            Sup::SporeBlossom => v.center_down(self.get(above(p))) && self.fluid_at(p).kind != 1,
            Sup::Veg(i) => self.tag(VEG_TAGS[i as usize], v.block(b)),
            Sup::LeafLitter | Sup::CoralBelow { .. } => v.sturdy(b, Dir::Up),
            Sup::LilyPad | Sup::Frogspawn => {
                let bp = below(p);
                let tag =
                    if sup == Sup::LilyPad { "minecraft:supports_lily_pad" } else { "minecraft:supports_frogspawn" };
                (self.fluid_at(bp).kind == 1 || self.tag(tag, v.block(b))) && self.fluid_at(p).is_empty()
            }
            Sup::SeaPickle => !self.collision_face_empty(b, Dir::Up) || v.sturdy(b, Dir::Up),
            Sup::Seagrass => v.sturdy(b, Dir::Up) && !self.tag_id(crate::tag_bits::TagId::CannotSupportSeagrass, v.block(b)),
            Sup::CoralBlock => true,
        }
    }

    pub(crate) fn support_update_shape(&mut self, sup: Sup, s: u32, p: P, dir: Dir) -> u32 {
        let v = self.v();
        let check = match sup {
            Sup::BelowSolid | Sup::CoralBelow { .. } => dir == Dir::Down,
            Sup::WallSolid | Sup::FacingSturdy | Sup::CoralWall { .. } => dir == v.facing(s).opposite(),
            Sup::CeilingCenter | Sup::HangingRoots | Sup::SporeBlossom => dir == Dir::Up,
            Sup::Veg(_) | Sup::LeafLitter | Sup::LilyPad | Sup::Frogspawn | Sup::SeaPickle | Sup::Seagrass => true,
            Sup::CoralBlock => false,
        };
        if check && !self.support_survives(sup, s, p) {
            return self.air();
        }
        if sup == Sup::Seagrass {
            self.schedule_fluid_tick(p, 1, 5);
        }
        if matches!(sup, Sup::CoralBelow { alive: true } | Sup::CoralWall { alive: true } | Sup::CoralBlock) {
            self.coral_try_schedule_death(s, p);
        }
        s
    }

    // --- Corals (§ 2.9). ---

    fn coral_has_water(&self, s: u32, p: P) -> bool {
        if self.v().get_bool(s, "waterlogged") {
            return true;
        }
        Dir::ALL.iter().any(|&d| self.fluid_at(rel(p, d)).kind == 1)
    }

    pub(crate) fn coral_try_schedule_death(&mut self, s: u32, p: P) {
        if !self.coral_has_water(s, p) {
            let delay = 60 + self.bs.rng.next_int_bound(40);
            let b = self.v().block(s);
            self.schedule_tick(p, b, delay);
        }
    }

    pub(crate) fn coral_tick(&mut self, sup: Sup, s: u32, p: P) {
        if self.coral_has_water(s, p) {
            return;
        }
        let v = self.v();
        let dead_name = format!("minecraft:dead_{}", self.name_of(s).strip_prefix("minecraft:").unwrap_or(""));
        let dead = self.bs.t.block_id(&dead_name);
        if dead == u16::MAX {
            return;
        }
        let mut ns = v.default_state(dead);
        if sup != Sup::CoralBlock {
            ns = v.set_bool(ns, "waterlogged", false);
        }
        if matches!(sup, Sup::CoralWall { .. }) {
            ns = v.set_dir(ns, "facing", v.facing(s));
        }
        self.set_block(p, ns, flag::CLIENTS);
    }

    // --- Note block: instrument from the block above (heads) or below. ---

    /// Rank of the instrument given by state `s` (game order).
    fn instrument_of(&self, s: u32) -> usize {
        usize::from(self.data.instrument.get(s as usize).copied().unwrap_or(0))
    }

    /// Ranks 20 and above (heads, custom head) play above the block.
    pub(crate) fn note_with_instrument(&self, s: u32, p: P) -> u32 {
        const FIRST_HEAD: usize = 20;
        let a = self.instrument_of(self.get(above(p)));
        let i = if a >= FIRST_HEAD {
            a
        } else {
            let b = self.instrument_of(self.support_state(below(p)));
            if b >= FIRST_HEAD { 0 } else { b }
        };
        let v = self.v();
        let name =
            v.d.block_of(s)
                .and_then(|b| {
                    b.properties.iter().map(|&pi| &v.d.properties[pi as usize]).find(|pr| pr.name == "instrument")
                })
                .and_then(|pr| pr.values.get(i).cloned());
        match name {
            Some(n) => v.set(s, "instrument", &n),
            None => s,
        }
    }

    // --- Tripwire hooks and tripwire (§ 3.16). ---

    pub(crate) fn hook_survives(&self, s: u32, p: P) -> bool {
        let f = self.v().facing(s);
        f.horizontal() && self.v().sturdy(self.support_state(rel(p, f.opposite())), f)
    }

    /// Recomputation of a hook and its line.
    pub(crate) fn hook_calculate(
        &mut self,
        p: P,
        state: u32,
        destroying: bool,
        can_update: bool,
        wire_source: i32,
        wire_source_state: Option<u32>,
    ) {
        let v = self.v();
        if !v.has(state, "facing") {
            return;
        }
        let dir = v.facing(state);
        let was_attached = v.get_bool(state, "attached");
        let was_powered = v.get_bool(state, "powered");
        let block = v.block(state);
        let mut attached = !destroying;
        let mut powered = false;
        let mut receiver = 0;
        let mut wires: [Option<u32>; HOOK_REACH as usize] = [None; HOOK_REACH as usize];
        for i in 1..HOOK_REACH {
            let tp = super::rel_n(p, dir, i);
            let mut ws = self.get(tp);
            let k = v.kind(ws);
            if k == Kind::TripwireHook {
                if v.facing(ws) == dir.opposite() {
                    receiver = i;
                }
                break;
            }
            if k != Kind::Tripwire && i != wire_source {
                wires[i as usize] = None;
                attached = false;
            } else {
                if i == wire_source {
                    ws = wire_source_state.unwrap_or(ws);
                }
                let armed = !v.get_bool(ws, "disarmed");
                let wp = v.get_bool(ws, "powered");
                powered |= armed && wp;
                wires[i as usize] = Some(ws);
                if i == wire_source {
                    self.schedule_tick(p, block, 10);
                    attached &= armed;
                }
            }
        }
        attached &= receiver > 1;
        powered &= attached;
        let def = v.default_state(block);
        let new = v.set_bool(v.set_bool(def, "attached", attached), "powered", powered);
        if receiver > 0 {
            let tp = super::rel_n(p, dir, receiver);
            let opp = dir.opposite();
            self.set_update(tp, v.set_dir(new, "facing", opp));
            self.hook_notify(tp, block, opp);
            if self.v().kind(self.get(p)) != Kind::TripwireHook {
                self.hook_on_removed(new, p);
                return;
            }
            self.hook_emit(tp, attached, powered, was_attached, was_powered);
        }
        self.hook_emit(p, attached, powered, was_attached, was_powered);
        if !destroying {
            self.set_update(p, v.set_dir(new, "facing", dir));
            if can_update {
                self.hook_notify(p, block, dir);
            }
        }
        if was_attached != attached {
            for i in 1..receiver {
                let tp = super::rel_n(p, dir, i);
                if let Some(wd) = wires[i as usize] {
                    let cur = self.get(tp);
                    if matches!(self.v().kind(cur), Kind::Tripwire | Kind::TripwireHook) {
                        let w =
                            if self.v().has(wd, "attached") { self.v().set_bool(wd, "attached", attached) } else { wd };
                        self.set_update(tp, w);
                    }
                }
            }
        }
    }

    /// Sound and event of the hook at `p` (only one, by priority: on, off, attached,
    /// detached); only detaching draws a random number (pitch of the sound), before the
    /// sound seed.
    fn hook_emit(&mut self, p: P, attached: bool, powered: bool, was_attached: bool, was_powered: bool) {
        let (sound, pitch, event) = if powered && !was_powered {
            ("minecraft:block.tripwire.click_on", 0.6, "minecraft:block_activate")
        } else if !powered && was_powered {
            ("minecraft:block.tripwire.click_off", 0.5, "minecraft:block_deactivate")
        } else if attached && !was_attached {
            ("minecraft:block.tripwire.attach", 0.7, "minecraft:block_attach")
        } else if !attached && was_attached {
            let pitch = 1.2 / (self.bs.rng.next_float() * 0.2 + 0.9);
            ("minecraft:block.tripwire.detach", pitch, "minecraft:block_detach")
        } else {
            return;
        };
        self.block_sound(p, sound, 0.4, pitch);
        self.game_event_block(event, p, None, None);
    }

    fn hook_notify(&mut self, p: P, block: u16, facing: Dir) {
        self.update_neighbors_at(p, block);
        self.update_neighbors_at(rel(p, facing.opposite()), block);
    }

    pub(crate) fn hook_on_removed(&mut self, state: u32, p: P) {
        let v = self.v();
        let attached = v.get_bool(state, "attached");
        let powered = v.get_bool(state, "powered");
        if attached || powered {
            self.hook_calculate(p, state, true, false, -1, None);
        }
        if powered {
            let f = v.facing(state);
            self.hook_notify(p, v.block(state), f);
        }
    }

    pub(crate) fn hook_tick(&mut self, state: u32, p: P) {
        self.hook_calculate(p, state, false, true, -1, None);
    }

    /// Tripwire: updates the hooks to the south and west again.
    pub(crate) fn tripwire_update_source(&mut self, p: P, state: u32) {
        for d in [Dir::South, Dir::West] {
            for i in 1..HOOK_REACH {
                let tp = super::rel_n(p, d, i);
                let s = self.get(tp);
                let k = self.v().kind(s);
                if k == Kind::TripwireHook {
                    if self.v().facing(s) == d.opposite() {
                        self.hook_calculate(tp, s, false, true, i, Some(state));
                    }
                    break;
                }
                if k != Kind::Tripwire {
                    break;
                }
            }
        }
    }

    pub(crate) fn tripwire_connects(&self, ns: u32, d: Dir) -> bool {
        match self.v().kind(ns) {
            Kind::TripwireHook => self.v().facing(ns) == d.opposite(),
            Kind::Tripwire => true,
            _ => false,
        }
    }

    pub(crate) fn tripwire_placement(&self, def: u32, p: P) -> u32 {
        let mut s = def;
        for d in Dir::HORIZONTAL {
            let c = self.tripwire_connects(self.get(rel(p, d)), d);
            s = self.v().set_bool(s, d.name(), c);
        }
        s
    }

    fn tripwire_box_has_entity(&self, s: u32, p: P) -> bool {
        let (lo, hi) = if self.v().get_bool(s, "attached") { (1.0, 2.5) } else { (0.0, 8.0) };
        let b = rc_physics::Aabb {
            min: [f64::from(p[0]), f64::from(p[1]) + lo / 16.0, f64::from(p[2])],
            max: [f64::from(p[0]) + 1.0, f64::from(p[1]) + hi / 16.0, f64::from(p[2]) + 1.0],
        };
        self.count_entities_in(&b, false) > 0
    }

    fn tripwire_check_pressed(&mut self, p: P, pressed: bool) {
        let mut s = self.get(p);
        let was = self.v().get_bool(s, "powered");
        let b = self.v().block(s);
        if pressed != was {
            s = self.v().set_bool(s, "powered", pressed);
            self.set_update(p, s);
            self.tripwire_update_source(p, s);
        }
        if pressed {
            self.schedule_tick(p, b, 10);
        } else if was {
            self.schedule_tick(p, b, 1);
        }
    }

    pub(crate) fn tripwire_tick(&mut self, s: u32, p: P) {
        if self.v().get_bool(s, "powered") {
            let hit = self.tripwire_box_has_entity(s, p);
            self.tripwire_check_pressed(p, hit);
        }
    }

    /// Player in the tripwire.
    pub(crate) fn tripwire_entity_inside(&mut self, p: P) {
        let s = self.get(p);
        if self.v().kind(s) == Kind::Tripwire
            && !self.v().get_bool(s, "powered")
            && !self.has_scheduled_tick(p, self.v().block(s))
            && self.tripwire_box_has_entity(s, p)
        {
            self.tripwire_check_pressed(p, true);
        }
    }

    pub(crate) fn hook_placement(&self, def: u32, p: P, nearest: &[Dir; 6]) -> Option<u32> {
        let v = self.v();
        let base = v.set_bool(v.set_bool(def, "powered", false), "attached", false);
        for &d in nearest {
            if d.horizontal() {
                let s = v.set_dir(base, "facing", d.opposite());
                if self.hook_survives(s, p) {
                    return Some(s);
                }
            }
        }
        None
    }

    // --- Vines (§ 2.10). ---

    fn vine_face_supported(&self, p: P, d: Dir) -> bool {
        if d == Dir::Down {
            return false;
        }
        if self.multiface_can_attach(p, d) {
            return true;
        }
        if d.axis() == 1 {
            return false;
        }
        let a = self.get(above(p));
        self.v().kind(a) == Kind::Vine && self.v().get_bool(a, d.name())
    }

    fn vine_updated(&self, s: u32, p: P) -> u32 {
        let v = self.v();
        let mut s = s;
        if v.get_bool(s, "up") {
            // The game passes the Down direction here for the block above: it is the TOP face
            // of that block that must be full (quirk reproduced; no effect on a cube).
            s = v.set_bool(s, "up", self.neighbour_accepts(above(p), Dir::Down));
        }
        let mut above_state = None;
        for d in Dir::HORIZONTAL {
            if v.get_bool(s, d.name()) {
                let mut ok = self.multiface_can_attach(p, d);
                if !ok {
                    let a = *above_state.get_or_insert_with(|| self.get(above(p)));
                    ok = v.kind(a) == Kind::Vine && v.get_bool(a, d.name());
                }
                s = v.set_bool(s, d.name(), ok);
            }
        }
        s
    }

    pub(crate) fn vine_faces(&self, s: u32) -> usize {
        ["up", "north", "east", "south", "west"].iter().filter(|f| self.v().get_bool(s, f)).count()
    }

    pub(crate) fn vine_survives(&self, s: u32, p: P) -> bool {
        self.vine_faces(self.vine_updated(s, p)) > 0
    }

    pub(crate) fn vine_update_shape(&mut self, s: u32, p: P, dir: Dir) -> u32 {
        if dir == Dir::Down {
            return s;
        }
        let n = self.vine_updated(s, p);
        if self.vine_faces(n) == 0 { self.air() } else { n }
    }

    pub(crate) fn vine_placement(&self, block: u16, p: P, nearest: &[Dir; 6]) -> Option<u32> {
        let v = self.v();
        let cur = self.get(p);
        let clicked_vine = v.block(cur) == block;
        let result = if clicked_vine { cur } else { v.default_state(block) };
        for &d in nearest {
            if d != Dir::Down {
                let occupied = clicked_vine && v.get_bool(cur, d.name());
                if !occupied && self.vine_face_supported(p, d) {
                    return Some(v.set_bool(result, d.name(), true));
                }
            }
        }
        clicked_vine.then_some(result)
    }

    fn vine_can_spread(&self, p: P) -> bool {
        let mut n = 4;
        for x in -4..=4 {
            for y in -1..=1 {
                for z in -4..=4 {
                    if self.v().kind(self.get([p[0] + x, p[1] + y, p[2] + z])) == Kind::Vine {
                        n -= 1;
                        if n < 0 {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    fn vine_copy_random(&mut self, from: u32, to: u32) -> u32 {
        let mut to = to;
        for d in Dir::HORIZONTAL {
            if self.bs.rng.next_int_bound(2) == 0 && self.v().get_bool(from, d.name()) {
                to = self.v().set_bool(to, d.name(), true);
            }
        }
        to
    }

    fn horizontal_any(&self, s: u32) -> bool {
        Dir::HORIZONTAL.iter().any(|d| self.v().get_bool(s, d.name()))
    }

    pub(crate) fn vine_random_tick(&mut self, s: u32, p: P) {
        if !self.rule("minecraft:spread_vines", true) || self.bs.rng.next_int_bound(4) != 0 {
            return;
        }
        let v = self.v();
        let b = v.block(s);
        let def = v.default_state(b);
        let d = Dir::from_index(self.bs.rng.next_int_bound(6) as usize);
        let ap = above(p);
        if d.horizontal() && !v.get_bool(s, d.name()) {
            if !self.vine_can_spread(p) {
                return;
            }
            let tp = rel(p, d);
            let edge = self.get(tp);
            if v.is_air(edge) {
                let (cw, ccw) = (d.clockwise(), d.counter_clockwise());
                let cw_face = v.get_bool(s, cw.name());
                let ccw_face = v.get_bool(s, ccw.name());
                let cwp = rel(tp, cw);
                let ccwp = rel(tp, ccw);
                if cw_face && self.multiface_can_attach(tp, cw) {
                    self.set_block(tp, v.set_bool(def, cw.name(), true), flag::CLIENTS);
                } else if ccw_face && self.multiface_can_attach(tp, ccw) {
                    self.set_block(tp, v.set_bool(def, ccw.name(), true), flag::CLIENTS);
                } else {
                    let opp = d.opposite();
                    // Corner: the game tests the starting side cell itself (face turned
                    // towards `d`), then the block above the targeted cell itself.
                    if cw_face && v.is_air(self.get(cwp)) && self.neighbour_accepts(rel(p, cw), opp) {
                        self.set_block(cwp, v.set_bool(def, opp.name(), true), flag::CLIENTS);
                    } else if ccw_face && v.is_air(self.get(ccwp)) && self.neighbour_accepts(rel(p, ccw), opp) {
                        self.set_block(ccwp, v.set_bool(def, opp.name(), true), flag::CLIENTS);
                    } else if self.bs.rng.next_float() < 0.05 && self.neighbour_accepts(above(tp), Dir::Up) {
                        self.set_block(tp, v.set_bool(def, "up", true), flag::CLIENTS);
                    }
                }
            } else if self.multiface_can_attach(p, d) {
                self.set_block(p, v.set_bool(s, d.name(), true), flag::CLIENTS);
            }
            return;
        }
        if d == Dir::Up && p[1] < self.top_y() - 1 {
            if self.vine_face_supported(p, Dir::Up) {
                self.set_block(p, v.set_bool(s, "up", true), flag::CLIENTS);
                return;
            }
            if v.is_air(self.get(ap)) {
                if !self.vine_can_spread(p) {
                    return;
                }
                let mut a = s;
                for h in Dir::HORIZONTAL {
                    if self.bs.rng.next_int_bound(2) == 0 || !self.neighbour_accepts(rel(ap, h), h) {
                        a = self.v().set_bool(a, h.name(), false);
                    }
                }
                if self.horizontal_any(a) {
                    self.set_block(ap, a, flag::CLIENTS);
                }
                return;
            }
        }
        if p[1] > self.dim.height.min_y {
            let bp = below(p);
            let bs = self.get(bp);
            if v.is_air(bs) || v.block(bs) == b {
                let before = if v.is_air(bs) { def } else { bs };
                let after = self.vine_copy_random(s, before);
                if before != after && self.horizontal_any(after) {
                    self.set_block(bp, after, flag::CLIENTS);
                }
            }
        }
    }

    // --- Multiface blocks (glow lichen, sculk vein, resin clump). ---

    pub(crate) fn multiface_update_shape(&mut self, s: u32, p: P, dir: Dir) -> u32 {
        let v = self.v();
        if !v.get_bool(s, dir.name()) || self.multiface_can_attach(p, dir) {
            return s;
        }
        let n = v.set_bool(s, dir.name(), false);
        // No face left: air (destruction leaves the water of a waterlogged block).
        if Dir::ALL.iter().any(|d| v.get_bool(n, d.name())) { n } else { self.air() }
    }

    pub(crate) fn multiface_survives(&self, s: u32, p: P) -> bool {
        let mut any = false;
        for d in Dir::ALL {
            if self.v().get_bool(s, d.name()) {
                if !self.multiface_can_attach(p, d) {
                    return false;
                }
                any = true;
            }
        }
        any
    }

    // --- Climbing plants (weeping, twisting and cave vines). ---

    pub(crate) fn growing_parts(&self, s: u32) -> (u16, u16) {
        let n = self.name_of(s);
        let base = n.strip_suffix("_plant").unwrap_or(n);
        (self.bs.t.block_id(base), self.bs.t.block_id(&format!("{base}_plant")))
    }

    pub(crate) fn growing_survives(&self, s: u32, p: P, up: bool) -> bool {
        let g = if up { Dir::Up } else { Dir::Down };
        let ap = rel(p, g.opposite());
        let a = self.support_state(ap);
        let (head, body) = self.growing_parts(s);
        let ab = self.v().block(a);
        if self.is_kelp(s) && self.tag_id(crate::tag_bits::TagId::CannotSupportKelp, ab) {
            return false;
        }
        ab == head || ab == body || self.v().sturdy(a, g)
    }

    pub(crate) fn growing_update_shape(&mut self, s: u32, p: P, dir: Dir, ns: u32, head: bool, up: bool) -> u32 {
        let v = self.v();
        let g = if up { Dir::Up } else { Dir::Down };
        let (hb, bb) = self.growing_parts(s);
        let nb = v.block(ns);
        let cave = self.name_of(s).starts_with("minecraft:cave_vines");
        if head {
            if dir == g.opposite() {
                if !self.growing_survives(s, p, up) {
                    self.schedule_tick(p, hb, 1);
                } else {
                    let ng = v.block(self.get(rel(p, g)));
                    if ng == hb || ng == bb {
                        return self.growing_to_body(s, bb, cave);
                    }
                }
            }
            if dir == g && (nb == hb || nb == bb) {
                return self.growing_to_body(s, bb, cave);
            }
            if self.is_kelp(s) {
                self.schedule_fluid_tick(p, 1, 5);
            }
            s
        } else {
            if dir == g.opposite() && !self.growing_survives(s, p, up) {
                self.schedule_tick(p, bb, 1);
            }
            if dir == g && nb != bb && nb != hb {
                let age = self.bs.rng.next_int_bound(25);
                let mut h = v.set_int(v.default_state(hb), "age", age);
                if cave {
                    h = v.set_bool(h, "berries", v.get_bool(s, "berries"));
                }
                return h;
            }
            if self.is_kelp(s) {
                self.schedule_fluid_tick(p, 1, 5);
            }
            s
        }
    }

    pub(crate) fn is_kelp(&self, s: u32) -> bool {
        self.name_of(s).starts_with("minecraft:kelp")
    }

    fn growing_to_body(&self, head: u32, body: u16, cave: bool) -> u32 {
        let v = self.v();
        let b = v.default_state(body);
        if cave { v.set_bool(b, "berries", v.get_bool(head, "berries")) } else { b }
    }

    pub(crate) fn growing_tick(&mut self, s: u32, p: P, up: bool) {
        if !self.growing_survives(s, p, up) {
            self.destroy_block(p, true, 512);
        }
    }

    pub(crate) fn growing_random_tick(&mut self, s: u32, p: P, up: bool) {
        let v = self.v();
        let kelp = self.is_kelp(s);
        let chance = if kelp { 0.14 } else { 0.1 };
        if v.get_int(s, "age") < 25 && self.bs.rng.next_double() < chance {
            let gp = rel(p, if up { Dir::Up } else { Dir::Down });
            let into = self.get(gp);
            let ok = if kelp { v.kind(into) == Kind::Water } else { v.is_air(into) };
            if ok {
                let mut n = v.cycle(s, "age");
                if self.name_of(s) == "minecraft:cave_vines" {
                    n = v.set_bool(n, "berries", self.bs.rng.next_float() < 0.11);
                }
                self.set_update(gp, n);
            }
        }
    }

    /// Placement of a climbing plant head.
    pub(crate) fn growing_placement(&mut self, def: u32) -> u32 {
        let age = self.bs.rng.next_int_bound(25);
        self.v().set_int(def, "age", age)
    }

    // --- Bamboo (§ 2.4). ---

    fn bamboo_id(&self) -> u16 {
        self.bs.t.block_id("minecraft:bamboo")
    }

    pub(crate) fn bamboo_survives(&self, p: P) -> bool {
        self.tag_id(crate::tag_bits::TagId::SupportsBamboo, self.v().block(self.get(below(p))))
    }

    pub(crate) fn bamboo_update_shape(&mut self, s: u32, p: P, dir: Dir, ns: u32, sapling: bool) -> u32 {
        let v = self.v();
        let bamboo = self.bamboo_id();
        if sapling {
            if !self.bamboo_survives(p) {
                return self.air();
            }
            if dir == Dir::Up && v.block(ns) == bamboo {
                return v.default_state(bamboo);
            }
            return s;
        }
        if !self.bamboo_survives(p) {
            self.schedule_tick(p, v.block(s), 1);
        }
        if dir == Dir::Up && v.block(ns) == bamboo && v.get_int(ns, "age") > v.get_int(s, "age") {
            return v.cycle(s, "age");
        }
        s
    }

    pub(crate) fn bamboo_tick(&mut self, p: P) {
        if !self.bamboo_survives(p) {
            self.destroy_block(p, true, 512);
        }
    }

    fn bamboo_run(&self, p: P, up: bool) -> i32 {
        let bamboo = self.bamboo_id();
        let mut h = 0;
        while h < 16 && self.v().block(self.get(super::rel_n(p, if up { Dir::Up } else { Dir::Down }, h + 1))) == bamboo
        {
            h += 1;
        }
        h
    }

    pub(crate) fn bamboo_random_tick(&mut self, s: u32, p: P, sapling: bool) {
        let v = self.v();
        let ap = above(p);
        if sapling {
            if self.bs.rng.next_int_bound(3) == 0 && v.is_air(self.get(ap)) && self.raw_brightness(ap) >= 9 {
                let st = v.set(v.default_state(self.bamboo_id()), "leaves", "small");
                self.set_update(ap, st);
            }
            return;
        }
        if v.get_int(s, "stage") == 0
            && self.bs.rng.next_int_bound(3) == 0
            && v.is_air(self.get(ap))
            && self.raw_brightness(ap) >= 9
        {
            let height = self.bamboo_run(p, false) + 1;
            if height < 16 {
                self.bamboo_grow(s, p, height);
            }
        }
    }

    fn bamboo_grow(&mut self, s: u32, p: P, height: i32) {
        let v = self.v();
        let bamboo = self.bamboo_id();
        let bp = below(p);
        let bs = self.get(bp);
        let bbp = below(bp);
        let bbs = self.get(bbp);
        let mut leaves = "none";
        if height >= 1 {
            if v.block(bs) != bamboo || v.get(bs, "leaves") == Some("none") {
                leaves = "small";
            } else {
                leaves = "large";
                if v.block(bbs) == bamboo {
                    self.set_update(bp, v.set(bs, "leaves", "small"));
                    self.set_update(bbp, v.set(bbs, "leaves", "none"));
                }
            }
        }
        let age = if v.get_int(s, "age") != 1 && v.block(bbs) != bamboo { 0 } else { 1 };
        let stage = if (height < 11 || self.bs.rng.next_float() >= 0.25) && height != 15 { 0 } else { 1 };
        let mut n = v.set_int(v.default_state(bamboo), "age", age);
        n = v.set(n, "leaves", leaves);
        n = v.set_int(n, "stage", stage);
        self.set_update(above(p), n);
    }

    /// Bamboo placement by a player.
    pub(crate) fn bamboo_placement(&self, p: P) -> Option<u32> {
        let v = self.v();
        if !self.fluid_at(p).is_empty() {
            return None;
        }
        let bamboo = self.bamboo_id();
        let b = self.get(below(p));
        if !self.tag_id(crate::tag_bits::TagId::SupportsBamboo, v.block(b)) {
            return None;
        }
        let def = v.default_state(bamboo);
        if v.block(b) == self.bs.t.block_id("minecraft:bamboo_sapling") {
            return Some(v.set_int(def, "age", 0));
        }
        if v.block(b) == bamboo {
            return Some(v.set_int(def, "age", i32::from(v.get_int(b, "age") > 0)));
        }
        let a = self.get(above(p));
        if v.block(a) == bamboo {
            Some(v.set_int(def, "age", v.get_int(a, "age")))
        } else {
            Some(v.default_state(self.bs.t.block_id("minecraft:bamboo_sapling")))
        }
    }

    // --- Scaffolding. ---

    pub(crate) fn scaffold_distance(&self, p: P) -> i32 {
        let v = self.v();
        let me = self.bs.t.block_id("minecraft:scaffolding");
        let bp = below(p);
        let b = self.get(bp);
        let mut d = 7;
        if v.block(b) == me {
            d = v.get_int(b, "distance");
        } else if v.sturdy(self.support_state(bp), Dir::Up) {
            return 0;
        }
        for h in Dir::HORIZONTAL {
            let q = self.get(rel(p, h));
            if v.block(q) == me {
                d = d.min(v.get_int(q, "distance") + 1);
                if d == 1 {
                    break;
                }
            }
        }
        d
    }

    fn scaffold_state(&self, s: u32, p: P) -> u32 {
        let v = self.v();
        let d = self.scaffold_distance(p);
        let bottom = d > 0 && v.block(self.get(below(p))) != v.block(s);
        v.set_bool(v.set_int(s, "distance", d), "bottom", bottom)
    }

    pub(crate) fn scaffold_placement(&self, def: u32, p: P) -> u32 {
        let s = self.scaffold_state(def, p);
        let f = self.fluid_at(p);
        self.v().set_bool(s, "waterlogged", f.kind == 1 && f.source)
    }

    pub(crate) fn scaffold_tick(&mut self, s: u32, p: P) {
        let n = self.scaffold_state(s, p);
        if self.v().get_int(n, "distance") == 7 {
            if self.v().get_int(s, "distance") == 7 {
                self.spawn_falling_block(p, n);
            } else {
                self.destroy_block(p, true, 512);
            }
        } else if n != s {
            self.set_update(p, n);
        }
    }
}
