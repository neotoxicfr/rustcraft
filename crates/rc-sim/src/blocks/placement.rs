//! Placement by a player (`interactions.md` § 6.2): placement context (targeted cell, replacement,
//! gaze directions), placement state of each family, survival and obstruction test,
//! placement with flags 11 then the placement reaction (second half of doors, beds, double plants;
//! repeaters and pistons), interactions without an item (levers, buttons, repeaters, doors…).

use rc_data::flags;

use super::{Dir, Kind, P, above, below, flag, rel};
use crate::server::Server;

/// Placement context.
#[derive(Debug, Clone)]
pub(crate) struct PlaceCtx {
    pub face: Dir,
    /// Targeted point, absolute.
    pub at: [f64; 3],
    pub pos: P,
    pub replace_clicked: bool,
    pub horizontal: Dir,
    pub nearest: [Dir; 6],
    pub item: String,
    pub sneaking: bool,
}

// Block placed by an item whose name differs (table shared with the client).
use rc_data::rules::placed_block as item_block;

/// Count property (1 to 4) of a block that stacks when the same item is placed again.
fn stack_prop(v: &super::table::View, s: u32) -> Option<&'static str> {
    ["pickles", "flower_amount", "segment_amount", "eggs"].into_iter().find(|n| v.has(s, n))
}

impl Server {
    /// Horizontal direction of the player.
    pub(crate) fn look_dir(&self) -> Dir {
        match ((f64::from(self.yaw) / 90.0 + 0.5).floor() as i32) & 3 {
            0 => Dir::South,
            1 => Dir::West,
            2 => Dir::North,
            _ => Dir::East,
        }
    }

    /// Directions sorted from closest to the gaze to farthest.
    pub(crate) fn ordered_by_nearest(&self) -> [Dir; 6] {
        let pitch = self.pitch.to_radians();
        let yaw = -self.yaw.to_radians();
        let (ps, pc, ys, yc) = (pitch.sin(), pitch.cos(), yaw.sin(), yaw.cos());
        let (xp, yp, zp) = (ys > 0.0, ps < 0.0, yc > 0.0);
        let x_yaw = if xp { ys } else { -ys };
        let y_mag = if yp { -ps } else { ps };
        let z_yaw = if zp { yc } else { -yc };
        let (x_mag, z_mag) = (x_yaw * pc, z_yaw * pc);
        let ax = if xp { Dir::East } else { Dir::West };
        let ay = if yp { Dir::Up } else { Dir::Down };
        let az = if zp { Dir::South } else { Dir::North };
        let mk = |a: Dir, b: Dir, c: Dir| [a, b, c, c.opposite(), b.opposite(), a.opposite()];
        if x_yaw > z_yaw {
            if y_mag > x_mag {
                mk(ay, ax, az)
            } else if z_mag > y_mag {
                mk(ax, az, ay)
            } else {
                mk(ax, ay, az)
            }
        } else if y_mag > z_mag {
            mk(ay, az, ax)
        } else if x_mag > y_mag {
            mk(az, ax, ay)
        } else {
            mk(az, ay, ax)
        }
    }

    fn nearest_for(&self, replace_clicked: bool, face: Dir) -> [Dir; 6] {
        let mut d = self.ordered_by_nearest();
        if replace_clicked {
            return d;
        }
        let want = face.opposite();
        if let Some(i) = d.iter().position(|&x| x == want)
            && i > 0
        {
            d.copy_within(0..i, 1);
            d[0] = want;
        }
        d
    }

    /// The replacement test of a state for a given item.
    pub(crate) fn replaceable_by(&self, s: u32, ctx_item: &str, replacing_clicked: bool, face: Dir, upper: bool) -> bool {
        let v = self.v();
        let block_name = v.d.block_of(s).map_or("", |b| b.name.as_str());
        match v.kind(s) {
            Kind::Slab => {
                let t = v.get(s, "type").unwrap_or("bottom");
                if t != "double" && item_block(ctx_item) == block_name {
                    if replacing_clicked {
                        if t == "bottom" {
                            face == Dir::Up || (upper && face.horizontal())
                        } else {
                            face == Dir::Down || (!upper && face.horizontal())
                        }
                    } else {
                        true
                    }
                } else {
                    false
                }
            }
            Kind::Snow => {
                let layers = v.get_int(s, "layers");
                if item_block(ctx_item) == block_name && layers < 8 {
                    if replacing_clicked { face == Dir::Up } else { true }
                } else {
                    layers == 1
                }
            }
            Kind::Vine => self.vine_faces(s) < 5,
            Kind::Scaffolding => item_block(ctx_item) == block_name,
            Kind::Multiface => item_block(ctx_item) != block_name || Dir::ALL.iter().any(|d| !v.get_bool(s, d.name())),
            // Pitcher crop: never replaced.
            Kind::PitcherCrop => false,
            _ => {
                let same = item_block(ctx_item) == block_name;
                // Stackables (pickles, petals, wildflowers, leaf litter, turtle eggs):
                // same item, without sneaking, fewer than 4.
                if same && !self.sneaking && let Some(pr) = stack_prop(&v, s) {
                    return v.get_int(s, pr) < 4;
                }
                v.replaceable(s) && !same
            }
        }
    }

    /// Right click on a block with the main hand (§ 6.2): interaction or placement.
    pub(crate) fn use_item_on_block(&mut self, clicked: P, face: Dir, at: [f32; 3]) {
        let held = self.slots.get(36 + self.held as usize).cloned().flatten();
        let has_item = held.is_some();
        let state = self.get(clicked);
        // Hanging sign under another: the block lets it through, the item is placed.
        let suppress = (self.sneaking && has_item)
            || held.as_ref().is_some_and(|h| self.hanging_sign_chains(clicked, face, &h.id));
        // S6: respawn anchor (charge, point, explosion).
        if !suppress && self.game_mode != 3 && self.use_anchor(clicked, held.as_ref().map(|s| s.id.as_str())) {
            return;
        }
        // Item on the block first (shears on a pumpkin), then interaction without an item
        // (including berry picking).
        if !suppress && self.game_mode != 3 && self.carve_pumpkin(clicked, state, face) {
            return;
        }
        // Item on the block: cake, pot, composter, cauldrons, TNT…
        let mut empty_hand = true;
        if !suppress && self.game_mode != 3 {
            match self.block_use_item_on(clicked, state, face, at) {
                super::use_block::ItemOn::Done => return,
                super::use_block::ItemOn::Pass => empty_hand = false,
                super::use_block::ItemOn::TryEmptyHand => {}
            }
        }
        if !suppress
            && empty_hand
            && self.game_mode != 3
            && (self.harvest_use(clicked, state)
                || self.use_without_item(clicked, state)
                || self.block_use_without_item(clicked, state, face, at))
        {
            return;
        }
        let Some(stack) = held else { return };
        // Adventure: any item use on a block requires `can_place_on` to accept the clicked
        // block.
        if self.game_mode == 2 && !self.adventure_can_place_on(Some(&stack), self.get(clicked)) {
            return;
        }
        // Bone meal, axe/hoe/shovel (transformers), wax.
        if self.item_use_on(36 + self.held as usize, clicked, face) {
            return;
        }
        // S6: flint and steel, fire charge, Eye of Ender on a frame.
        if self.use_igniter(&stack.id, clicked, face) {
            return;
        }
        if stack.id == "minecraft:ender_eye" && self.use_ender_eye(clicked) {
            self.award_stat("minecraft:used", &stack.id, 1);
            return;
        }
        // Lily pad, frogspawn: nothing on a block, placement goes through their ray
        // (`use_item`, `fluid_use.rs`).
        if matches!(stack.id.as_str(), "minecraft:lily_pad" | "minecraft:frogspawn") {
            return;
        }
        let at = [
            f64::from(clicked[0]) + f64::from(at[0]),
            f64::from(clicked[1]) + f64::from(at[1]),
            f64::from(clicked[2]) + f64::from(at[2]),
        ];
        // Entities placed by their item: painting, frames, armor stand, cushion.
        if self.deco_use_on(36 + self.held as usize, &stack, clicked, face, at) {
            return;
        }
        // Failed placement of a consumable block item (carrot, berries…): it gets eaten as in the air.
        if !self.place_item(&stack.id, clicked, face, at)
            && self.bs.t.block_id(item_block(&stack.id)) != u16::MAX
            && self.comp_json(&stack, "minecraft:consumable").is_some()
        {
            self.on_use_item(0);
        }
    }

    /// Interactions without an item; true if the action is consumed.
    fn use_without_item(&mut self, p: P, s: u32) -> bool {
        // W8a: signs (applicator held, then editor or refusal of the waxed sign).
        if self.sign_use(p) {
            return true;
        }
        let may_build = self.game_mode != 2;
        let v = self.v();
        match v.kind(s) {
            Kind::Lever => {
                self.lever_pull(p, s);
                true
            }
            Kind::Button { .. } => {
                self.button_press(p, s, Some(crate::mobs::mob::Ent::Player));
                true
            }
            Kind::Repeater if may_build => {
                self.repeater_use(p, s);
                true
            }
            Kind::Comparator if may_build => {
                self.comparator_use(p, s);
                true
            }
            Kind::Wire if may_build => self.wire_use(s, p),
            Kind::Door { .. } | Kind::Trapdoor { .. } | Kind::FenceGate => self.openable_use(p, s),
            Kind::NoteBlock => {
                let ns = self.v().cycle(s, "note");
                self.set_block(p, ns, flag::ALL);
                self.note_play(p, ns, Some(crate::mobs::mob::Ent::Player));
                self.award_stat("minecraft:custom", "minecraft:tune_noteblock", 1); // G6-F01
                true
            }
            // Inversion: flag 2, `block_change` event, signal recomputed at once.
            Kind::DaylightDetector if may_build => {
                let ns = self.v().cycle(s, "inverted");
                self.set_block(p, ns, flag::CLIENTS);
                self.game_event_block("minecraft:block_change", p, Some(crate::mobs::mob::Ent::Player), Some(ns));
                self.daylight_signal(p);
                true
            }
            _ => false,
        }
    }

    /// Placement of a block item.
    pub(crate) fn place_item(&mut self, item: &str, clicked: P, face: Dir, at: [f64; 3]) -> bool {
        self.place_item_from(36 + self.held as usize, item, clicked, face, at, false)
    }

    /// Placement of a block item held in slot `slot` (main or off hand); `by_use`:
    /// placement called by the use of the item (lily pad) — neither "used" statistic nor
    /// "item used on block" trigger, only "block placed".
    pub(crate) fn place_item_from(
        &mut self,
        slot: usize,
        item: &str,
        clicked: P,
        face: Dir,
        at: [f64; 3],
        by_use: bool,
    ) -> bool {
        let block_name = item_block(item).to_owned();
        let tb = self.tb();
        let block = tb.block_id(&block_name);
        if block == u16::MAX {
            return false;
        }
        let cs = self.get(clicked);
        let upper = at[1] - f64::from(clicked[1]) > 0.5;
        let replace_clicked = self.replaceable_by(cs, item, true, face, upper);
        let pos = if replace_clicked { clicked } else { rel(clicked, face) };
        if !self.in_build_height(pos[1]) {
            // Failed placement out of height (top face at the ceiling, bottom at the floor): the game
            // sends "too high" twice (two successive tests), "too low" once.
            if pos[1] < self.dim.height.min_y {
                self.build_limit_message(false);
            } else {
                self.build_limit_message(true);
                self.build_limit_message(true);
            }
            return false;
        }
        if !replace_clicked && !self.replaceable_by(self.get(pos), item, false, face, upper) {
            return false;
        }
        let scaffold = self.bs.t.kind.get(block as usize) == Some(&Kind::Scaffolding);
        let (pos, face, at, replace_clicked) = if scaffold {
            match self.scaffold_target(block, item, (pos, replace_clicked), face, at) {
                Some(t) => t,
                None => return false,
            }
        } else {
            (pos, face, at, replace_clicked)
        };
        let ctx = PlaceCtx {
            face,
            at,
            pos,
            replace_clicked,
            horizontal: self.look_dir(),
            nearest: self.nearest_for(replace_clicked, face),
            item: item.to_owned(),
            sneaking: self.sneaking,
        };
        let Some(state) = self.placement_state(block, &ctx) else { return false };
        let state = self.placement_random(state, pos);
        // Scaffolding is placed without a survival test (it will fall by itself if too far).
        if (!scaffold && !self.can_place_at(state, pos)) || self.obstructed(state, pos) {
            return false;
        }
        if !self.set_block(pos, state, flag::ALL | 8) {
            return false;
        }
        let mut placed = self.get(pos);
        let stack = self.slots[slot].clone();
        if self.v().block(placed) == self.v().block(state) {
            // Order of the game: `block_state` properties of the stack (flag 2), block entity data and
            // components, placement reaction, trigger.
            if let Some(st) = &stack {
                placed = self.apply_block_state_component(pos, placed, st);
                self.placed_components(pos, st);
            }
            self.set_placed_by(pos, placed);
            if let Some(st) = &stack {
                if by_use {
                    self.trig_location("minecraft:placed_block", pos, Some(st));
                } else {
                    self.s9_block_placed(clicked, pos, st); // S9
                }
            }
        }
        // Successful placement: `block_place` event (player, placed state re-read).
        let placed = self.get(pos);
        self.game_event_block("minecraft:block_place", pos, Some(crate::mobs::mob::Ent::Player), Some(placed));
        if self.game_mode != 1 {
            if let Some(s) = &mut self.slots[slot] {
                s.count -= 1;
                if s.count <= 0 {
                    self.slots[slot] = None;
                }
            }
            // Powder snow bucket: the empty bucket stays in hand.
            if item == "minecraft:powder_snow_bucket" {
                self.slots[slot] = Some(crate::player::Stack { id: "minecraft:bucket".into(), count: 1, components: None });
            }
            self.send_slot(slot);
        }
        // "Used" statistic: any successful use on a block (not placement by use).
        if !by_use && let Some(st) = &stack {
            self.award_stat("minecraft:used", &st.id, 1);
        }
        true
    }

    /// Properties of the stack's `block_state` component applied to the placed state (unknown
    /// values ignored); rewritten with flag 2 if it changes. Returns the final state.
    fn apply_block_state_component(&mut self, pos: P, placed: u32, st: &crate::player::Stack) -> u32 {
        let Some(rc_proto::nbt::Nbt::Compound(props)) = crate::items::component(st, "minecraft:block_state") else {
            return placed;
        };
        let v = self.v();
        let mut s = placed;
        for (k, val) in props {
            if let Some(val) = val.as_str() {
                s = v.set(s, k, val);
            }
        }
        if s != placed {
            self.set_block(pos, s, flag::CLIENTS);
        }
        s
    }

    /// Cell targeted by scaffolding: off scaffolding, refusal at distance 7; on
    /// scaffolding, the first replaceable at the end of the column (upwards, or at
    /// the horizontal of the gaze when clicking the top; clicked face when sneaking), at most
    /// 7 horizontal steps. Returns the cell, the face, the targeted point and "clicked cell replaced".
    #[allow(clippy::type_complexity)]
    fn scaffold_target(
        &self,
        block: u16,
        item: &str,
        (pos, replace_clicked): (P, bool),
        face: Dir,
        at: [f64; 3],
    ) -> Option<(P, Dir, [f64; 3], bool)> {
        let v = self.v();
        if v.block(self.get(pos)) != block {
            return (self.scaffold_distance(pos) != 7).then_some((pos, face, at, replace_clicked));
        }
        let dir = if self.sneaking {
            // Ray that started inside the clicked block: opposite face.
            if self.hit_inside { face.opposite() } else { face }
        } else if face == Dir::Up {
            self.look_dir()
        } else {
            Dir::Up
        };
        let mut horizontal = 0;
        let mut q = rel(pos, dir);
        while horizontal < 7 {
            if !self.in_build_height(q[1]) {
                if q[1] >= self.top_y() {
                    self.build_limit_message(true);
                }
                return None;
            }
            let s = self.get(q);
            if v.block(s) != block {
                if self.replaceable_by(s, item, false, dir, false) {
                    let st = dir.step();
                    let hit = [0, 1, 2].map(|i| f64::from(q[i]) + 0.5 + f64::from(st[i]) * 0.5);
                    return Some((q, dir, hit, true));
                }
                return None;
            }
            q = rel(q, dir);
            if dir.horizontal() {
                horizontal += 1;
            }
        }
        None
    }

    pub(crate) fn obstructed(&self, state: u32, p: P) -> bool {
        let info = self.v().info(state);
        let me = rc_physics::Aabb {
            min: [self.pos[0] - 0.3, self.pos[1], self.pos[2] - 0.3],
            max: [self.pos[0] + 0.3, self.pos[1] + if self.sneaking { 1.5 } else { 1.8 }, self.pos[2] + 0.3],
        };
        if self.game_mode == 3 {
            return false;
        }
        self.data.shape(info.collision).iter().any(|b| {
            (0..3).all(|i| {
                let lo = b.min[i] + f64::from(p[i]);
                let hi = b.max[i] + f64::from(p[i]);
                hi > me.min[i] && lo < me.max[i]
            })
        })
    }

    /// The survival test of the block at placement (attached families included).
    pub(crate) fn can_place_at(&self, s: u32, p: P) -> bool {
        let v = self.v();
        let b = self.get(below(p));
        match v.kind(s) {
            Kind::Torch | Kind::RedstoneTorch => v.center_up(b),
            Kind::WallTorch | Kind::RedstoneWallTorch | Kind::Ladder => self.wall_attach(p, v.facing(s)),
            Kind::Lever | Kind::Button { .. } | Kind::Grindstone => {
                let c = super::redstone::connected_dir(&v, s);
                self.face_attach(p, c.opposite())
            }
            Kind::Repeater | Kind::Comparator => v.rigid_up(b),
            Kind::Wire => v.sturdy(b, Dir::Up) || v.block(b) == self.bs.t.id.hopper,
            Kind::Plate { .. } | Kind::WeightedPlate { .. } => v.rigid_up(b) || v.center_up(b),
            Kind::Door { .. } => {
                if v.get(s, "half") == Some("upper") {
                    v.block(b) == v.block(s)
                } else {
                    v.sturdy(b, Dir::Up)
                }
            }
            Kind::Rail | Kind::PoweredRail | Kind::DetectorRail => v.rigid_up(b),
            Kind::Lantern => {
                if v.get_bool(s, "hanging") {
                    v.center_down(self.get(above(p)))
                } else {
                    v.center_up(b)
                }
            }
            Kind::StandingSign => v.legacy_solid(b),
            Kind::WallSign => v.legacy_solid(self.get(rel(p, v.facing(s).opposite()))),
            Kind::Cocoa => self.tag_id(crate::tag_bits::TagId::SupportsCocoa, v.block(self.get(rel(p, v.facing(s))))),
            _ => self.can_survive(s, p),
        }
    }

    /// Placement state of a block.
    pub(crate) fn placement_state(&self, block: u16, ctx: &PlaceCtx) -> Option<u32> {
        let v = self.v();
        let def = v.default_state(block);
        let kind = self.bs.t.kind.get(block as usize).copied().unwrap_or(Kind::Other);
        let name = v.d.blocks[block as usize].name.as_str();
        let p = ctx.pos;
        let water = {
            let f = self.fluid_at(p);
            f.kind == 1 && f.source
        };
        let wl = |s: u32| if v.has(s, "waterlogged") { v.set_bool(s, "waterlogged", water) } else { s };
        let upper_click = !(ctx.face == Dir::Down || (ctx.face != Dir::Up && ctx.at[1] - f64::from(p[1]) > 0.5));
        let half_bottom = ctx.face != Dir::Down && (ctx.face == Dir::Up || ctx.at[1] - f64::from(p[1]) <= 0.5);
        let _ = upper_click;
        // Game master blocks: only an operator in creative.
        if matches!(
            name,
            "minecraft:command_block"
                | "minecraft:repeating_command_block"
                | "minecraft:chain_command_block"
                | "minecraft:structure_block"
                | "minecraft:jigsaw"
                | "minecraft:test_block"
                | "minecraft:test_instance_block"
        ) && !self.can_use_game_master_blocks()
        {
            return None;
        }
        if let Some(r) = self.head_or_banner_placement(block, name, ctx) {
            return r;
        }
        if kind == Kind::Support(super::attach::Sup::CeilingCenter) {
            return self.hanging_sign_placement(block, name, ctx);
        }
        // Stacking on the same block: one more (4 at most), rest of the state kept.
        let cur = self.get(p);
        if v.block(cur) == block
            && let Some(pr) = stack_prop(&v, cur)
        {
            return Some(v.set_int(cur, pr, (v.get_int(cur, pr) + 1).min(4)));
        }
        // Kelp and seagrass: only in full water.
        let water_full = || {
            let f = self.fluid_at(p);
            f.kind == 1 && f.amount == 8
        };
        let s = match kind {
            Kind::GrowingHead { up } => {
                if self.is_kelp(def) && !water_full() {
                    return None;
                }
                // Against the same plant in the growth direction: a piece of stem, not a head.
                let (head, body) = self.growing_parts(def);
                let next = v.block(self.get(rel(p, if up { Dir::Up } else { Dir::Down })));
                if next == head || next == body {
                    return Some(v.default_state(body));
                }
                def
            }
            Kind::Support(super::attach::Sup::Seagrass) if !water_full() => return None,
            // Propagule placed: adult (age 4), never hanging.
            Kind::Sapling if block == self.bs.t.id.mangrove_propagule => {
                v.set_int(self.generic_placement(def, name, ctx), "age", 4)
            }
            // Pitcher crop: default state, without upper half at placement.
            Kind::PitcherCrop => def,
            // Campfire: turned like the gaze, put out in water, signal on a hay bale.
            Kind::Campfire => {
                let f = self.fluid_at(p);
                let wet = f.kind == 1 && f.source;
                let hay = v.block(self.get(below(p))) == self.bs.t.block_id("minecraft:hay_block");
                let s = v.set_bool(v.set_bool(def, "waterlogged", wet), "lit", !wet);
                v.set_dir(v.set_bool(s, "signal_fire", hay), "facing", ctx.horizontal)
            }
            // Chorus plant: connected to its neighbours from placement.
            Kind::ChorusPlant => self.chorus_plant_state(p),
            Kind::HugeMushroom => self.huge_mushroom_placement(def, p),
            Kind::PaleMossCarpet => self.moss_carpet_sides(def, p, true),
            // Shelf fungus: first horizontal direction of the gaze, turned to the
            // opposite, which holds against the full face behind it.
            Kind::Support(super::attach::Sup::FacingSturdy) if name == "minecraft:shelf_mushroom" => {
                return ctx.nearest.iter().filter(|d| d.horizontal()).find_map(|&d| {
                    let s = v.set_dir(def, "facing", d.opposite());
                    self.can_place_at(s, p).then_some(s)
                });
            }
            // Jigsaw block: front = clicked face; top = upwards, or opposite to the horizontal
            // gaze when the front is vertical.
            _ if name == "minecraft:jigsaw" => {
                let front = ctx.face;
                let top = if front.axis() == 1 { ctx.horizontal.opposite() } else { Dir::Up };
                v.set(def, "orientation", &format!("{}_{}", front.name(), top.name()))
            }
            // Corals and fans: waterlogged in full water; the fan becomes wall (wall variant)
            // according to the gaze order, the top excluded.
            Kind::Support(super::attach::Sup::CoralBelow { .. }) => {
                let wet = water_full();
                let standing = v.set_bool(def, "waterlogged", wet);
                if !name.ends_with("_coral_fan") {
                    return Some(standing);
                }
                let wall = self.bs.t.block_id(&name.replace("_coral_fan", "_coral_wall_fan"));
                let wall_state = (wall != u16::MAX)
                    .then(|| {
                        let wdef = v.set_bool(v.default_state(wall), "waterlogged", wet);
                        ctx.nearest.iter().filter(|d| d.horizontal()).find_map(|&d| {
                            let s = v.set_dir(wdef, "facing", d.opposite());
                            self.can_place_at(s, p).then_some(s)
                        })
                    })
                    .flatten();
                for &d in &ctx.nearest {
                    if d == Dir::Up {
                        continue;
                    }
                    let cand = if d == Dir::Down { Some(standing) } else { wall_state };
                    if let Some(s) = cand
                        && self.can_place_at(s, p)
                    {
                        return Some(s);
                    }
                }
                return None;
            }
            Kind::Wire => self.wire_placement(p),
            Kind::Repeater => {
                let s = v.set_dir(def, "facing", ctx.horizontal.opposite());
                let locked = self.repeater_locked(p, s);
                v.set_bool(s, "locked", locked)
            }
            Kind::Comparator => v.set_dir(def, "facing", ctx.horizontal.opposite()),
            Kind::Torch | Kind::RedstoneTorch | Kind::StandingSign => {
                return self.standing_or_wall(block, ctx);
            }
            Kind::Lever | Kind::Button { .. } | Kind::Grindstone => {
                for &d in &ctx.nearest {
                    let s = if d.axis() == 1 {
                        v.set_dir(
                            v.set(def, "face", if d == Dir::Up { "ceiling" } else { "floor" }),
                            "facing",
                            ctx.horizontal,
                        )
                    } else {
                        v.set_dir(v.set(def, "face", "wall"), "facing", d.opposite())
                    };
                    if self.can_place_at(s, p) {
                        return Some(s);
                    }
                }
                return None;
            }
            Kind::Lamp => v.set_bool(def, "lit", self.has_neighbor_signal(p)),
            Kind::Piston { .. } => v.set_bool(v.set_dir(def, "facing", ctx.nearest[0].opposite()), "extended", false),
            Kind::Observer => v.set_dir(def, "facing", ctx.nearest[0]),
            Kind::Dispenser | Kind::Dropper | Kind::Crafter => {
                if kind == Kind::Crafter {
                    let f = ctx.nearest[0].opposite();
                    let o = match f {
                        Dir::Up => format!("up_{}", ctx.horizontal.name()),
                        Dir::Down => format!("down_{}", ctx.horizontal.opposite().name()),
                        d => format!("{}_up", d.name()),
                    };
                    v.set(def, "orientation", &o)
                } else {
                    v.set_dir(def, "facing", ctx.nearest[0].opposite())
                }
            }
            // G6-F01: shelf placed powered if a neighbour gives power (chain at placement).
            Kind::Shelf => {
                let s = self.generic_placement(def, name, ctx);
                v.set_bool(s, "powered", self.has_neighbor_signal(p))
            }
            Kind::Hopper => {
                let d = ctx.face.opposite();
                v.set_bool(v.set_dir(def, "facing", if d.axis() == 1 { Dir::Down } else { d }), "enabled", true)
            }
            Kind::Door { .. } => {
                if p[1] >= self.top_y() - 1
                    || !self.replaceable_by(self.get(above(p)), &ctx.item, false, ctx.face, false)
                {
                    return None;
                }
                let powered = self.has_neighbor_signal(p) || self.has_neighbor_signal(above(p));
                let s = v.set_dir(def, "facing", ctx.horizontal);
                let s = v.set(s, "hinge", self.door_hinge(ctx));
                let s = v.set_bool(v.set_bool(s, "powered", powered), "open", powered);
                v.set(s, "half", "lower")
            }
            Kind::Trapdoor { .. } => {
                let mut s = if !ctx.replace_clicked && ctx.face.horizontal() {
                    let s = v.set_dir(def, "facing", ctx.face);
                    v.set(s, "half", if ctx.at[1] - f64::from(p[1]) > 0.5 { "top" } else { "bottom" })
                } else {
                    let s = v.set_dir(def, "facing", ctx.horizontal.opposite());
                    v.set(s, "half", if ctx.face == Dir::Up { "bottom" } else { "top" })
                };
                if self.has_neighbor_signal(p) {
                    s = v.set_bool(v.set_bool(s, "open", true), "powered", true);
                }
                wl(s)
            }
            Kind::FenceGate => {
                let open = self.has_neighbor_signal(p);
                let d = ctx.horizontal;
                let side = |dd: Dir| self.is_wall(self.get(rel(p, dd)));
                let in_wall = if d.axis() == 2 {
                    side(Dir::West) || side(Dir::East)
                } else {
                    side(Dir::North) || side(Dir::South)
                };
                let s = v.set_dir(def, "facing", d);
                let s = v.set_bool(v.set_bool(s, "open", open), "powered", open);
                v.set_bool(s, "in_wall", in_wall)
            }
            Kind::Fence => {
                let mut s = def;
                for d in Dir::HORIZONTAL {
                    let q = rel(p, d);
                    let qs = self.get(q);
                    s = v.set_bool(s, d.name(), self.fence_connects(def, qs, v.sturdy(qs, d.opposite()), d.opposite()));
                }
                wl(s)
            }
            Kind::Pane => {
                let mut s = def;
                for d in Dir::HORIZONTAL {
                    let q = rel(p, d);
                    let qs = self.get(q);
                    s = v.set_bool(s, d.name(), self.pane_attaches(qs, v.sturdy(qs, d.opposite())));
                }
                wl(s)
            }
            Kind::Wall => {
                let c = [Dir::North, Dir::East, Dir::South, Dir::West].map(|d| {
                    let qs = self.get(rel(p, d));
                    self.wall_connects(qs, v.sturdy(qs, d.opposite()), d.opposite())
                });
                let a = above(p);
                self.wall_shape(wl(def), a, self.get(a), c)
            }
            Kind::Stairs => {
                let s = v.set_dir(def, "facing", ctx.horizontal);
                let s = v.set(s, "half", if half_bottom { "bottom" } else { "top" });
                let s = wl(s);
                let sh = self.stairs_shape(s, p);
                v.set(s, "shape", sh)
            }
            Kind::Slab => {
                let cur = self.get(p);
                if v.block(cur) == block {
                    v.set_bool(v.set(cur, "type", "double"), "waterlogged", false)
                } else {
                    wl(v.set(def, "type", if half_bottom { "bottom" } else { "top" }))
                }
            }
            Kind::Bed => {
                let d = ctx.horizontal;
                let head = rel(p, d);
                if !self.replaceable_by(self.get(head), &ctx.item, false, ctx.face, false) || !Self::block_in_border(self.border_bounds(), head) {
                    return None;
                }
                v.set_dir(def, "facing", d)
            }
            Kind::DoublePlant => {
                if p[1] >= self.top_y() - 1
                    || !self.replaceable_by(self.get(above(p)), &ctx.item, false, ctx.face, false)
                {
                    return None;
                }
                // Small dripleaf: turned towards the player, waterlogged in any water.
                if v.has(def, "facing") {
                    let s = v.set_dir(def, "facing", ctx.horizontal.opposite());
                    v.set_bool(s, "waterlogged", self.fluid_at(p).kind == 1)
                } else {
                    def
                }
            }
            // Leaves placed: persistent, distance computed from the neighbours.
            Kind::Leaves => self.leaf_update_distance(wl(v.set_bool(def, "persistent", true)), p),
            Kind::Chest => {
                let facing = ctx.horizontal.opposite();
                let mut s = wl(v.set_dir(def, "facing", facing));
                let partner = |d: Dir| {
                    let q = self.get(rel(p, d));
                    (self.chest_connects(def, q) && v.get(q, "type") == Some("single")).then(|| v.facing(q))
                };
                let mut joined = false;
                if ctx.face.horizontal()
                    && ctx.sneaking
                    && let Some(f) = partner(ctx.face.opposite())
                    && f.axis() != ctx.face.axis()
                {
                    s = v.set_dir(s, "facing", f);
                    let t = if f.counter_clockwise() == ctx.face.opposite() { "right" } else { "left" };
                    s = v.set(s, "type", t);
                    joined = true;
                }
                if !joined && !ctx.sneaking {
                    if partner(facing.clockwise()) == Some(facing) {
                        s = v.set(s, "type", "left");
                    } else if partner(facing.counter_clockwise()) == Some(facing) {
                        s = v.set(s, "type", "right");
                    }
                }
                // Copper joined to another variant: the placed chest takes the less oxidized.
                self.least_oxidized_chest(s, p)
            }
            Kind::Ladder => {
                if !ctx.replace_clicked {
                    let r = self.get(rel(ctx.pos, ctx.face.opposite()));
                    if v.block(r) == block && v.facing(r) == ctx.face {
                        return None;
                    }
                }
                for &d in &ctx.nearest {
                    if d.horizontal() {
                        let s = v.set_dir(def, "facing", d.opposite());
                        if self.wall_attach(p, d.opposite()) {
                            return Some(wl(s));
                        }
                    }
                }
                return None;
            }
            Kind::Lantern => {
                for &d in &ctx.nearest {
                    if d.axis() == 1 {
                        let s = v.set_bool(def, "hanging", d == Dir::Up);
                        if self.can_place_at(s, p) {
                            return Some(wl(s));
                        }
                    }
                }
                return None;
            }
            Kind::Snow => {
                let cur = self.get(p);
                if v.block(cur) == block {
                    let l = v.get_int(cur, "layers");
                    v.set_int(cur, "layers", (l + 1).min(8))
                } else {
                    def
                }
            }
            Kind::Anvil => v.set_dir(def, "facing", ctx.horizontal.clockwise()),
            Kind::Cocoa => {
                for &d in &ctx.nearest {
                    if d.horizontal() {
                        let s = v.set_dir(def, "facing", d);
                        if self.can_place_at(s, p) {
                            return Some(s);
                        }
                    }
                }
                return None;
            }
            Kind::NoteBlock => self.note_with_instrument(def, p),
            // Bell: top or bottom face → floor or ceiling turned like the player; side
            // face → wall (double if both sides of the axis are full), otherwise fallback floor
            // (full below) or ceiling; each variant must hold, otherwise no placement.
            Kind::Bell => {
                let f = ctx.face;
                if f.axis() == 1 {
                    let s = v.set(def, "attachment", if f == Dir::Down { "ceiling" } else { "floor" });
                    let s = v.set_dir(s, "facing", ctx.horizontal);
                    return self.bell_survives(s, p).then_some(s);
                }
                let (a, b) = if f.axis() == 0 { (Dir::West, Dir::East) } else { (Dir::North, Dir::South) };
                let double = v.sturdy(self.get(rel(p, a)), b) && v.sturdy(self.get(rel(p, b)), a);
                let s = v.set_dir(def, "facing", f.opposite());
                let s = v.set(s, "attachment", if double { "double_wall" } else { "single_wall" });
                if self.bell_survives(s, p) {
                    return Some(s);
                }
                let floor = v.sturdy(self.get(below(p)), Dir::Up);
                let s = v.set(s, "attachment", if floor { "floor" } else { "ceiling" });
                return self.bell_survives(s, p).then_some(s);
            }
            // G5-F09: big dripleaf placed on a leaf or a stem: same orientation.
            Kind::BigDripleaf => {
                let s = self.generic_placement(def, name, ctx);
                let b = self.get(below(p));
                if matches!(v.kind(b), Kind::BigDripleaf | Kind::BigDripleafStem) {
                    v.set_dir(s, "facing", v.facing(b))
                } else {
                    s
                }
            }
            Kind::Rail | Kind::PoweredRail | Kind::DetectorRail => {
                let ew = ctx.horizontal.axis() == 0;
                let s = v.set(def, "shape", if ew { "east_west" } else { "north_south" });
                v.set_bool(s, "waterlogged", self.fluid_at(p).kind == 1)
            }
            Kind::TripwireHook => return self.hook_placement(def, p, &ctx.nearest),
            Kind::Bamboo { .. } => return self.bamboo_placement(p),
            Kind::Scaffolding => self.scaffold_placement(def, p),
            Kind::Tripwire => self.tripwire_placement(def, p),
            Kind::Vine => return self.vine_placement(block, p, &ctx.nearest),
            Kind::Multiface => {
                let old = self.get(p);
                for &d in &ctx.nearest {
                    let has = v.block(old) == block && v.get_bool(old, d.name());
                    if !has && self.multiface_can_attach(p, d) {
                        let base = if v.block(old) == block {
                            old
                        } else {
                            let f = self.fluid_of(old);
                            let w = f.kind == 1 && f.source;
                            if v.has(def, "waterlogged") { v.set_bool(def, "waterlogged", w) } else { def }
                        };
                        return Some(v.set_bool(base, d.name(), true));
                    }
                }
                return None;
            }
            _ => self.generic_placement(def, name, ctx),
        };
        Some(s)
    }

    /// General rules of the usual properties (axis, orientation, half, waterlogged).
    fn generic_placement(&self, def: u32, name: &str, ctx: &PlaceCtx) -> u32 {
        let v = self.v();
        let p = ctx.pos;
        let mut s = def;
        if v.has(s, "axis") {
            let a = match ctx.face.axis() {
                0 => "x",
                1 => "y",
                _ => "z",
            };
            s = v.set(s, "axis", a);
        }
        let six = |s: u32| {
            v.d.block_of(s).is_some_and(|b| {
                b.properties.iter().any(|&pi| {
                    let pr = &v.d.properties[pi as usize];
                    pr.name == "facing" && pr.values.len() == 6
                })
            })
        };
        if v.has(s, "facing") {
            // End rod against a rod turned towards the clicked face: placed head to tail.
            let rod_flip = name == "minecraft:end_rod" && {
                let behind = self.get(rel(p, ctx.face.opposite()));
                v.block(behind) == v.block(s) && v.facing(behind) == ctx.face
            };
            let f = if rod_flip {
                ctx.face.opposite()
            } else if six(s) {
                if name.ends_with("shulker_box")
                    || name.ends_with("end_rod")
                    || name.ends_with("lightning_rod")
                    || name.ends_with("amethyst_cluster")
                    || name.ends_with("_bud")
                {
                    ctx.face
                } else {
                    ctx.nearest[0].opposite()
                }
            } else {
                ctx.horizontal.opposite()
            };
            s = v.set_dir(s, "facing", f);
        }
        if v.has(s, "rotation") {
            let r = (((self.yaw + 180.0) * 16.0 / 360.0 + 0.5).floor() as i32) & 15;
            s = v.set_int(s, "rotation", r);
        }
        if v.has(s, "half") && v.get(s, "half").is_some_and(|h| h == "bottom" || h == "top") {
            let bottom = ctx.face != Dir::Down && (ctx.face == Dir::Up || ctx.at[1] - f64::from(p[1]) <= 0.5);
            s = v.set(s, "half", if bottom { "bottom" } else { "top" });
        }
        if v.has(s, "waterlogged") {
            let f = self.fluid_at(p);
            s = v.set_bool(s, "waterlogged", f.kind == 1 && f.source);
        }
        s
    }

    /// Item with a wall variant (torches, signs…): first state that holds in the order of the
    /// gaze.
    fn standing_or_wall(&self, block: u16, ctx: &PlaceCtx) -> Option<u32> {
        let v = self.v();
        let name = v.d.blocks[block as usize].name.clone();
        let wall_name = match name.rsplit_once('_') {
            Some((a, b)) => format!("{a}_wall_{b}"),
            None => format!("{name}_wall"),
        };
        let wall = self.bs.t.block_id(&wall_name);
        let standing = self.generic_placement(v.default_state(block), &name, ctx);
        let wall_state = (wall != u16::MAX)
            .then(|| {
                let wdef = v.default_state(wall);
                ctx.nearest.iter().filter(|d| d.horizontal()).find_map(|&d| {
                    let s = v.set_dir(wdef, "facing", d.opposite());
                    self.can_place_at(s, ctx.pos).then_some(s)
                })
            })
            .flatten();
        for &d in &ctx.nearest {
            if d == Dir::Up {
                continue;
            }
            let cand = if d == Dir::Down { Some(standing) } else { wall_state };
            if let Some(s) = cand
                && self.can_place_at(s, ctx.pos)
            {
                let s = if v.has(s, "waterlogged") {
                    let f = self.fluid_at(ctx.pos);
                    v.set_bool(s, "waterlogged", f.kind == 1 && f.source)
                } else {
                    s
                };
                return Some(s);
            }
        }
        None
    }

    /// Heads and banners (item with wall variant, attached at the bottom): in the order of the gaze
    /// (top excluded), at the bottom the standing variant — head turned like the gaze (without a half turn),
    /// banner facing the player and placed on a solid block —, otherwise the wall variant — head
    /// in front of a non-replaceable block, banner against a solid block. `None`: other family.
    fn head_or_banner_placement(&self, block: u16, name: &str, ctx: &PlaceCtx) -> Option<Option<u32>> {
        let v = self.v();
        let n = name.trim_start_matches("minecraft:");
        let (wall_name, head) = match n {
            "skeleton_skull" | "wither_skeleton_skull" => {
                (format!("minecraft:{}", n.replace("_skull", "_wall_skull")), true)
            }
            "zombie_head" | "player_head" | "creeper_head" | "dragon_head" | "piglin_head" => {
                (format!("minecraft:{}", n.replace("_head", "_wall_head")), true)
            }
            _ if n.ends_with("_banner") && !n.ends_with("_wall_banner") => {
                (format!("minecraft:{}", n.replace("_banner", "_wall_banner")), false)
            }
            _ => return None,
        };
        let wall = self.bs.t.block_id(&wall_name);
        let p = ctx.pos;
        let powered =
            |s: u32| if v.has(s, "powered") { v.set_bool(s, "powered", self.has_neighbor_signal(p)) } else { s };
        let yaw = if head { self.yaw } else { self.yaw + 180.0 };
        let segment = ((yaw / 22.5 + 0.5).floor() as i32) & 15;
        let standing = powered(v.set_int(v.default_state(block), "rotation", segment));
        let standing_ok = head || v.legacy_solid(self.get(below(p)));
        let wall_state = (wall != u16::MAX)
            .then(|| {
                let wdef = powered(v.default_state(wall));
                ctx.nearest.iter().filter(|d| d.horizontal()).find_map(|&d| {
                    let s = v.set_dir(wdef, "facing", d.opposite());
                    let behind = self.get(rel(p, d));
                    let ok = if head {
                        !self.replaceable_by(behind, &ctx.item, false, ctx.face, false)
                    } else {
                        v.legacy_solid(behind)
                    };
                    ok.then_some(s)
                })
            })
            .flatten();
        for &d in &ctx.nearest {
            if d == Dir::Up {
                continue;
            }
            let cand = if d == Dir::Down { standing_ok.then_some(standing) } else { wall_state };
            if let Some(s) = cand
                && self.can_place_at(s, p)
            {
                return Some(Some(s));
            }
        }
        Some(None)
    }

    /// Hanging sign (item with wall variant, attached at the top): in the order of the gaze,
    /// bottom excluded, at the top the ceiling sign if the block above holds it, otherwise the wall
    /// variant if one side of its axis holds it.
    fn hanging_sign_placement(&self, block: u16, name: &str, ctx: &PlaceCtx) -> Option<u32> {
        let v = self.v();
        let p = ctx.pos;
        let water = self.fluid_at(p);
        let water = water.kind == 1 && water.source;
        // Wall variant: first horizontal direction of the gaze outside the axis of the
        // clicked face, oriented opposite, held by one side of its axis.
        let wall = self.bs.t.block_id(&name.replace("_hanging_sign", "_wall_hanging_sign"));
        let wall_state = (wall != u16::MAX)
            .then(|| {
                let wdef = v.default_state(wall);
                ctx.nearest.iter().filter(|d| d.horizontal() && d.axis() != ctx.face.axis()).find_map(|&d| {
                    let s = v.set_dir(wdef, "facing", d.opposite());
                    self.wall_hanging_sign_holds(s, p).then(|| v.set_bool(s, "waterlogged", water))
                })
            })
            .flatten();
        // Ceiling sign: hung in the middle under an incomplete bottom face (or when
        // sneaking), except under another sign of the same axis; otherwise turned according to
        // the gaze (16 notches), aligned on the axis when it is not.
        let up = above(p);
        let up_s = self.get(up);
        let dir = ctx.horizontal;
        let mut middle = !self.collision_face_full(up_s, Dir::Down) || ctx.sneaking;
        if !ctx.sneaking && self.tag("minecraft:all_hanging_signs", v.block(up_s)) {
            let up_dir = if v.has(up_s, "facing") {
                Some(v.facing(up_s))
            } else if v.has(up_s, "rotation") {
                match v.get_int(up_s, "rotation") {
                    0 => Some(Dir::North),
                    4 => Some(Dir::East),
                    8 => Some(Dir::South),
                    12 => Some(Dir::West),
                    _ => None,
                }
            } else {
                None
            };
            if up_dir.is_some_and(|d| d.axis() == dir.axis()) {
                middle = false;
            }
        }
        let rotation = if middle {
            ((self.yaw + 180.0) * 16.0 / 360.0 + 0.5).floor() as i32 & 15
        } else {
            // Notch of a direction: south 0, west 4, north 8, east 12.
            match dir.opposite() {
                Dir::South => 0,
                Dir::West => 4,
                Dir::North => 8,
                _ => 12,
            }
        };
        let ceiling = v.set_int(v.set_bool(v.default_state(block), "attached", middle), "rotation", rotation);
        let ceiling = v.set_bool(ceiling, "waterlogged", water);
        for &d in &ctx.nearest {
            if d == Dir::Down {
                continue;
            }
            if d == Dir::Up {
                if self.can_survive(ceiling, p) {
                    return Some(ceiling);
                }
            } else if let Some(s) = wall_state {
                return Some(s);
            }
        }
        None
    }

    /// Wall hanging sign: one of the two sides of its axis is a full face, or another
    /// wall sign of the same axis.
    fn wall_hanging_sign_holds(&self, s: u32, p: P) -> bool {
        let v = self.v();
        let f = v.facing(s);
        let attach = |side: Dir| {
            let q = self.get(rel(p, side));
            if self.tag("minecraft:wall_hanging_signs", v.block(q)) {
                v.facing(q).axis() == f.axis()
            } else {
                v.sturdy(q, side.opposite())
            }
        };
        attach(f.clockwise()) || attach(f.counter_clockwise())
    }

    fn door_hinge(&self, ctx: &PlaceCtx) -> &'static str {
        let v = self.v();
        let p = ctx.pos;
        let d = ctx.horizontal;
        let ap = above(p);
        let (l, r) = (d.counter_clockwise(), d.clockwise());
        let full = |q: P| v.full_collision(self.get(q));
        let bal = -(i32::from(full(rel(p, l))) + i32::from(full(rel(ap, l))))
            + i32::from(full(rel(p, r)))
            + i32::from(full(rel(ap, r)));
        let lower_door = |q: P| {
            let s = self.get(q);
            matches!(v.kind(s), Kind::Door { .. }) && v.get(s, "half") == Some("lower")
        };
        let (dl, dr) = (lower_door(rel(p, l)), lower_door(rel(p, r)));
        if (!dl || dr) && bal <= 0 {
            if (!dr || dl) && bal >= 0 {
                let (sx, sz) = (d.step_x(), d.step_z());
                let cx = ctx.at[0] - f64::from(p[0]);
                let cz = ctx.at[2] - f64::from(p[2]);
                if (sx >= 0 || cz >= 0.5) && (sx <= 0 || cz <= 0.5) && (sz >= 0 || cx <= 0.5) && (sz <= 0 || cx >= 0.5)
                {
                    "left"
                } else {
                    "right"
                }
            } else {
                "left"
            }
        } else {
            "right"
        }
    }

    /// Second halves, repeaters and pistons.
    fn set_placed_by(&mut self, p: P, s: u32) {
        self.wither_check_spawn(p); // F16: wither skeleton skull that completes a wither
        let v = self.v();
        match v.kind(s) {
            Kind::Door { .. } => {
                let up = v.set(s, "half", "upper");
                self.set_update(above(p), up);
            }
            Kind::DoublePlant => {
                let up = v.set(s, "half", "upper");
                let a = above(p);
                // Water at `a`, source or flowing (as the game does for the upper half).
                let up = if v.has(up, "waterlogged") {
                    v.set_bool(up, "waterlogged", self.fluid_at(a).kind == 1)
                } else {
                    up
                };
                self.set_update(a, up);
            }
            Kind::Bed => {
                let head = v.set(s, "part", "head");
                let hp = rel(p, v.facing(s));
                self.set_update(hp, head);
            }
            Kind::Repeater | Kind::Comparator => self.diode_placed_by_player(p, s),
            Kind::Piston { .. } => self.piston_placed_by_player(p, s),
            Kind::TripwireHook => self.hook_calculate(p, s, false, false, -1, None),
            Kind::PaleMossCarpet => self.moss_carpet_placed(s, p),
            _ => self.cmdblock_placed(p, s), // S5: command blocks
        }
        self.sign_placed_by(p); // W8a: editor of the placed sign
        self.dried_ghast_placed(p, s); // G6-F01: placement sound of the dried ghast
    }

    /// Random part of placement states (age of climbing plants, scheduled death of a coral
    /// block out of water), drawn at the same moment as the game.
    fn placement_random(&mut self, state: u32, p: P) -> u32 {
        match self.v().kind(state) {
            Kind::GrowingHead { .. } => self.growing_placement(state),
            Kind::Support(super::attach::Sup::CoralBlock) => {
                if !Dir::ALL.iter().any(|&d| self.fluid_at(rel(p, d)).kind == 1) {
                    let delay = 60 + self.bs.rng.next_int_bound(40);
                    let b = self.v().block(state);
                    self.schedule_tick(p, b, delay);
                }
                state
            }
            _ => state,
        }
    }

    /// The reaction to destruction by a player in creative: a non-empty Shulker box
    /// (at the center of the block) or a beehive with bees or honey (at the corner, rule `block_drops`)
    /// drops anyway, with its components (contents, bees…) and the default pickup delay;
    /// the normal loot, on the other hand, stays suppressed.
    fn creative_container_drop(&mut self, p: P, s: u32) {
        let name = self.state_name(s).to_owned();
        let shulker = name.ends_with("shulker_box");
        let hive = name == "minecraft:beehive" || name == "minecraft:bee_nest";
        if !shulker && !hive {
            return;
        }
        let Some(be) = self.block_entity(p).cloned() else { return };
        let mut stack = crate::player::Stack { id: name, count: 1, components: None };
        let at = if shulker {
            if self.be_item_counts(p).is_empty() {
                return;
            }
            [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
        } else {
            if !self.rule("minecraft:block_drops", true) {
                return;
            }
            let honey = self.v().get_int(s, "honey_level");
            let bees = matches!(be.get("bees"), Some(rc_proto::nbt::Nbt::List(l)) if !l.is_empty());
            if !bees && honey <= 0 {
                return;
            }
            let state = vec![("honey_level".to_owned(), rc_proto::nbt::Nbt::String(honey.to_string()))];
            crate::items::set_component(&mut stack, "minecraft:block_state", rc_proto::nbt::Nbt::Compound(state));
            [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
        };
        for (k, v) in crate::loot::be::collect(&be) {
            crate::items::set_component(&mut stack, &k, v);
        }
        self.drop_default_entity(at, stack);
    }

    /// Creation of an item entity (world, position, stack) + default pickup delay: velocity drawn at
    /// ±0.1 horizontally and 0.2 upwards, pickup possible after 10 ticks.
    fn drop_default_entity(&mut self, at: [f64; 3], stack: crate::player::Stack) {
        let r = &mut self.life.rng;
        let vel = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item(at, vel, stack, 10);
    }

    /// Break by a player (§ 6.1): linked halves then removal.
    pub(crate) fn player_destroy(&mut self, p: P) -> bool {
        let s = self.get(p);
        self.rec_player_will_destroy(p, s); // F14: creaking heart (protector undone)
        let v = self.v();
        let creative = self.game_mode == 1;
        // Door: the lower half also goes without loot when the held tool is not the right one for a
        // block that requires one; the double plant only does so in creative.
        let wrong_tool = !creative && {
            let needs = self.data.needs_tool.get(s as usize).copied().unwrap_or(false);
            let held = self.slots[36 + self.held as usize].clone();
            needs && !self.tool_correct(held.as_ref(), s)
        };
        match v.kind(s) {
            Kind::Door { .. } | Kind::DoublePlant | Kind::PitcherCrop
                if creative || (wrong_tool && matches!(v.kind(s), Kind::Door { .. })) =>
            {
                // The lower half disappears without loot when the upper one is broken.
                if v.get(s, "half") == Some("upper") {
                    let lp = below(p);
                    let ls = self.get(lp);
                    if v.block(ls) == v.block(s) && v.get(ls, "half") == Some("lower") {
                        let f = self.fluid_of(ls);
                        let rest = if f.is_empty() { self.air() } else { self.legacy_block(f) };
                        self.set_block(lp, rest, flag::ALL | flag::NO_DROPS);
                        self.level_event(2001, lp, ls as i32);
                    }
                }
            }
            Kind::Bed if creative => {
                if v.get(s, "part") == Some("foot") {
                    let hp = rel(p, v.facing(s));
                    let hs = self.get(hp);
                    if v.block(hs) == v.block(s) && v.get(hs, "part") == Some("head") {
                        let air = self.air();
                        self.set_block(hp, air, 35);
                        self.level_event(2001, hp, hs as i32);
                    }
                }
            }
            // Fire put out by hand: extinguishing sound for everyone.
            Kind::Fire | Kind::SoulFire => self.level_event(1009, p, 0),
            Kind::PistonHead if creative => {
                let bp = rel(p, v.facing(s).opposite());
                if self.piston_head_fitting(s, self.get(bp)) {
                    self.destroy_block(bp, false, 512);
                }
            }
            _ => {}
        }
        if creative {
            self.creative_container_drop(p, s);
        }
        let s = self.get(p);
        let f = self.fluid_of(s);
        let rest = if f.is_empty() || f.kind == 0 { self.air() } else { self.legacy_block(f) };
        let _ = flags::AIR;
        self.set_block(p, rest, flag::ALL)
    }
}
