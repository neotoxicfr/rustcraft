//! Placement by a player (`interactions.md` § 6.2): placement context (targeted cell, replacement,
//! gaze directions), placement state of each family, survival and obstruction test,
//! placement with flags 11 then the placement reaction (second half of doors, beds, double plants;
//! repeaters and pistons), interactions without an item (levers, buttons, repeaters, doors…).

use rc_data::{PropKey, flags};

use super::{Dir, Kind, P, above, below, flag, rel};
use super::rules::*;
use crate::server::Server;

/// Own sound of the powder snow bucket when it places its block (by hand or by a dispenser),
/// instead of the block's placement sound.
pub(crate) const POWDER_SNOW_BUCKET_EMPTY_SOUND: &str = "minecraft:item.bucket.empty_powder_snow";

/// The player who places a block, as the placement rules read him.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placer {
    pub yaw: f32,
    pub pitch: f32,
    pub sneaking: bool,
    /// The aiming ray started inside the clicked block (scaffolding then extends the other way).
    pub hit_inside: bool,
    /// Creative operator (permission 2 or more): may place command, structure, jigsaw and test
    /// blocks.
    pub game_master: bool,
}

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
    pub placer: Placer,
}

/// Why a block item finds no cell to go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoCell {
    /// Nothing to say to the player.
    Refused,
    /// Above the build height: the server says "too high" this many times (the game tests the
    /// height twice for a click at the ceiling).
    TooHigh(u8),
    /// Below the build height: "too low", once.
    TooLow,
}

/// Second block a placement adds right after the first (door's upper half, bed's head, tall
/// plant's top).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SecondBlock {
    pub pos: P,
    pub state: u32,
    /// The game's client places it too at the click; the small dripleaf's top is the server's
    /// alone.
    pub on_client: bool,
}

// Block placed by an item whose name differs (table shared with the client).
use rc_data::rules::placed_block as item_block;

/// Count property (1 to 4) of a block that stacks when the same item is placed again.
fn stack_prop(v: &super::table::View, s: u32) -> Option<PropKey> {
    [PropKey::Pickles, PropKey::FlowerAmount, PropKey::SegmentAmount, PropKey::Eggs].into_iter().find(|&n| v.has(s, n))
}

/// Horizontal direction a player with this yaw faces.
pub(crate) fn horizontal_facing(yaw: f32) -> Dir {
    match ((f64::from(yaw) / 90.0 + 0.5).floor() as i32) & 3 {
        0 => Dir::South,
        1 => Dir::West,
        2 => Dir::North,
        _ => Dir::East,
    }
}

/// Directions sorted from the closest to the gaze to the farthest.
pub(crate) fn directions_by_gaze(yaw: f32, pitch: f32) -> [Dir; 6] {
    let pitch = pitch.to_radians();
    let yaw = -yaw.to_radians();
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

/// Directions by gaze, the face against which the block goes first when the clicked block stays.
fn nearest_for(placer: &Placer, replace_clicked: bool, face: Dir) -> [Dir; 6] {
    let mut d = directions_by_gaze(placer.yaw, placer.pitch);
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

impl Server {
    /// Horizontal direction of the player.
    pub(crate) fn look_dir(&self) -> Dir {
        horizontal_facing(self.yaw)
    }

    /// The player as the placement rules read him.
    pub(crate) fn placer(&self) -> Placer {
        Placer {
            yaw: self.yaw,
            pitch: self.pitch,
            sneaking: self.sneaking,
            hit_inside: self.hit_inside,
            game_master: self.can_use_game_master_blocks(),
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
        // G4-F07a: lead on a fence (sneaking: the block did not react): knot.
        if stack.id == crate::mobs::leash::LEAD
            && self.btag_id(clicked, crate::tag_bits::TagId::Fences)
            && self.leash_bind_player_mobs(clicked)
        {
            self.award_stat("minecraft:used", &stack.id, 1);
            return;
        }
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
        let v = self.view();
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
                let ns = self.view().cycle(s, PropKey::Note);
                self.set_block(p, ns, flag::ALL);
                self.note_play(p, ns, Some(crate::mobs::mob::Ent::Player));
                self.award_stat("minecraft:custom", "minecraft:tune_noteblock", 1); // G6-F01
                true
            }
            // Inversion: flag 2, `block_change` event, signal recomputed at once.
            Kind::DaylightDetector if may_build => {
                let ns = self.view().cycle(s, PropKey::Inverted);
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
        let block = self.table().block_id(item_block(item));
        if block == u16::MAX {
            return false;
        }
        let ctx = match self.placement_target(block, item, clicked, face, at, &self.placer()) {
            Ok(ctx) => ctx,
            Err(no_cell) => {
                self.say_build_limit(no_cell);
                return false;
            }
        };
        let pos = ctx.pos;
        let Some(state) = self.placement_state(block, &ctx) else { return false };
        let state = self.placement_random(state, pos);
        if !self.placement_holds(block, state, pos) || self.obstructed(state, pos) {
            return false;
        }
        if !self.set_block(pos, state, flag::ALL | 8) {
            return false;
        }
        let mut placed = self.get(pos);
        let stack = self.slots[slot].clone();
        if self.view().block(placed) == self.view().block(state) {
            // Order of the game: `block_state` properties of the stack (flag 2), block entity data and
            // components, placement reaction, trigger.
            if let Some(st) = &stack {
                placed = self.apply_block_state_component(pos, placed, st);
                self.placed_components(pos, st);
            }
            self.set_placed_by(pos, placed, stack.as_ref());
            if let Some(st) = &stack {
                if by_use {
                    self.trig_location("minecraft:placed_block", pos, Some(st));
                } else {
                    self.s9_block_placed(clicked, pos, st); // S9
                }
            }
        }
        // Successful placement: sound, then `block_place` event (player, placed state re-read).
        let placed = self.get(pos);
        self.place_sound_for_others(pos, placed, item);
        self.game_event_block("minecraft:block_place", pos, Some(crate::mobs::mob::Ent::Player), Some(placed));
        if self.game_mode != 1 {
            if let Some(s) = &mut self.slots[slot] {
                s.count -= 1;
                if s.count <= 0 {
                    self.slots[slot] = None;
                }
            }
            // Powder snow bucket: the empty bucket stays in hand.
            if crate::placement_prediction::solid_bucket_sound(item).is_some() {
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

    /// Placement sound of a block item, from the placed state's sound set (the powder snow bucket
    /// has its own). The acting player's client plays it, so it goes to the other players only.
    fn place_sound_for_others(&self, pos: P, placed: u32, item: &str) {
        let Some(set) = self.data.state(placed).and_then(|i| self.data.sound_types.get(usize::from(i.sound))) else {
            return;
        };
        let name = crate::placement_prediction::solid_bucket_sound(item).unwrap_or(&set.place);
        self.block_sound_for_others(pos, name, (set.volume + 1.0) / 2.0, set.pitch * 0.8);
    }

    /// Properties of the stack's `block_state` component applied to the placed state (unknown
    /// values ignored); rewritten with flag 2 if it changes. Returns the final state.
    fn apply_block_state_component(&mut self, pos: P, placed: u32, st: &crate::player::Stack) -> u32 {
        let Some(rc_proto::nbt::Nbt::Compound(props)) = crate::items::component(st, "minecraft:block_state") else {
            return placed;
        };
        let v = self.view();
        let mut s = placed;
        for (k, val) in props {
            if let Some(val) = val.as_str() {
                s = v.set_named(s, k, val);
            }
        }
        if s != placed {
            self.set_block(pos, s, flag::CLIENTS);
        }
        s
    }

    /// Messages of a block item that found no cell (build height).
    fn say_build_limit(&self, no_cell: NoCell) {
        match no_cell {
            NoCell::Refused => {}
            NoCell::TooHigh(times) => (0..times).for_each(|_| self.build_limit_message(true)),
            NoCell::TooLow => self.build_limit_message(false),
        }
    }

    pub(crate) fn obstructed(&self, state: u32, p: P) -> bool {
        let info = self.view().info(state);
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

    /// Second halves, repeaters and pistons.
    fn set_placed_by(&mut self, p: P, s: u32, stack: Option<&crate::player::Stack>) {
        self.wither_check_spawn(p); // F16: wither skeleton skull that completes a wither
        let v = self.view();
        match v.kind(s) {
            Kind::Door { .. } | Kind::DoublePlant | Kind::Bed => {
                if let Some(second) = self.placed_second_block(p, s) {
                    self.set_update(second.pos, second.state);
                }
            }
            Kind::Repeater | Kind::Comparator => self.diode_placed_by_player(p, s),
            Kind::Piston { .. } => self.piston_placed_by_player(p, s),
            Kind::TripwireHook => self.hook_calculate(p, s, false, false, -1, None),
            Kind::PaleMossCarpet => self.moss_carpet_placed(s, p),
            // A crafter placed powered crafts in 4 ticks, as on a rising edge.
            Kind::Crafter if self.view().get_bool(s, PropKey::Triggered) => {
                let crafter = self.view().block(s);
                self.schedule_tick(p, crafter, 4);
            }
            // S5: command blocks (a stack carrying block entity data keeps its own settings).
            _ => self.cmdblock_placed(p, s, stack.is_some_and(has_be_data)),
        }
        self.sign_placed_by(p); // W8a: editor of the placed sign
        self.tech_placed_by(p, s); // G6-F14: author of the structure block
        self.dried_ghast_placed(p, s); // G6-F01: placement sound of the dried ghast
    }

    /// Random part of placement states (age of climbing plants, scheduled death of a coral
    /// block out of water), drawn at the same moment as the game.
    fn placement_random(&mut self, state: u32, p: P) -> u32 {
        match self.view().kind(state) {
            Kind::GrowingHead { .. } => self.growing_placement(state),
            Kind::Support(super::attach::Sup::CoralBlock) => {
                if !Dir::ALL.iter().any(|&d| self.fluid_at(rel(p, d)).kind == 1) {
                    let delay = 60 + self.bs.rng.next_int_bound(40);
                    let b = self.view().block(state);
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
            let honey = self.view().get_int(s, PropKey::HoneyLevel);
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
        let v = self.view();
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
                if v.get(s, PropKey::Half) == Some("upper") {
                    let lp = below(p);
                    let ls = self.get(lp);
                    if v.block(ls) == v.block(s) && v.get(ls, PropKey::Half) == Some("lower") {
                        let f = self.fluid_of(ls);
                        let rest = if f.is_empty() { self.air() } else { self.legacy_block(f) };
                        self.set_block(lp, rest, flag::ALL | flag::NO_DROPS);
                        self.level_event(2001, lp, ls as i32);
                    }
                }
            }
            Kind::Bed if creative => {
                if v.get(s, PropKey::Part) == Some("foot") {
                    let hp = rel(p, v.facing(s));
                    let hs = self.get(hp);
                    if v.block(hs) == v.block(s) && v.get(hs, PropKey::Part) == Some("head") {
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

/// The stack carries block entity data (`block_entity_data` component).
fn has_be_data(st: &crate::player::Stack) -> bool {
    st.components.as_ref().is_some_and(|c| c.get("minecraft:block_entity_data").is_some())
}

/// Placement rules of block items (cell, placement state, survival, second block), shared by
/// the server and the client's placement prediction (one source, read through [`BlockReader`]).
pub(crate) trait PlacementRules: BlockReader {
    /// The replacement test of a state for a given item (`sneaking`: the placer sneaks, which
    /// stops a stackable block from taking one more).
    fn replaceable_by(&self, s: u32, ctx_item: &str, replacing_clicked: bool, face: Dir, upper: bool, sneaking: bool) -> bool {
        let v = self.view();
        let block_name = v.data.block_of(s).map_or("", |b| b.name.as_str());
        match v.kind(s) {
            Kind::Slab => {
                let t = v.get(s, PropKey::Type).unwrap_or("bottom");
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
                let layers = v.get_int(s, PropKey::Layers);
                if item_block(ctx_item) == block_name && layers < 8 {
                    if replacing_clicked { face == Dir::Up } else { true }
                } else {
                    layers == 1
                }
            }
            Kind::Vine => self.vine_faces(s) < 5,
            Kind::Scaffolding => item_block(ctx_item) == block_name,
            Kind::Multiface => item_block(ctx_item) != block_name || Dir::ALL.iter().any(|d| !v.get_bool(s, d.key())),
            // Pitcher crop: never replaced.
            Kind::PitcherCrop => false,
            _ => {
                let same = item_block(ctx_item) == block_name;
                // Stackables (pickles, petals, wildflowers, leaf litter, turtle eggs):
                // same item, without sneaking, fewer than 4.
                if same && !sneaking && let Some(pr) = stack_prop(&v, s) {
                    return v.get_int(s, pr) < 4;
                }
                v.replaceable(s) && !same
            }
        }
    }

    /// Cell, face and context where block item `item` (placing `block`) goes when `placer`
    /// clicks face `face` of `clicked` at point `at`: the clicked block if the item replaces it,
    /// otherwise the cell against the face, which must be in the build height and replaceable;
    /// scaffolding then looks for the end of its column.
    fn placement_target(&self, block: u16, item: &str, clicked: P, face: Dir, at: [f64; 3], placer: &Placer) -> Result<PlaceCtx, NoCell> {
        let upper = at[1] - f64::from(clicked[1]) > 0.5;
        let replace_clicked = self.replaceable_by(self.get(clicked), item, true, face, upper, placer.sneaking);
        let pos = if replace_clicked { clicked } else { rel(clicked, face) };
        if !self.in_build_height(pos[1]) {
            // Click on the top face at the ceiling, or on the bottom face at the floor: the game
            // tests the height twice above, once below.
            return Err(if pos[1] < self.min_y() { NoCell::TooLow } else { NoCell::TooHigh(2) });
        }
        if !replace_clicked && !self.replaceable_by(self.get(pos), item, false, face, upper, placer.sneaking) {
            return Err(NoCell::Refused);
        }
        let scaffold = self.table().kind.get(block as usize) == Some(&Kind::Scaffolding);
        let (pos, face, at, replace_clicked) =
            if scaffold { self.scaffold_target(block, item, (pos, replace_clicked), face, at, placer)? } else { (pos, face, at, replace_clicked) };
        Ok(PlaceCtx {
            face,
            at,
            pos,
            replace_clicked,
            horizontal: horizontal_facing(placer.yaw),
            nearest: nearest_for(placer, replace_clicked, face),
            item: item.to_owned(),
            placer: *placer,
        })
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
        placer: &Placer,
    ) -> Result<(P, Dir, [f64; 3], bool), NoCell> {
        let v = self.view();
        if v.block(self.get(pos)) != block {
            return if self.scaffold_distance(pos) != 7 { Ok((pos, face, at, replace_clicked)) } else { Err(NoCell::Refused) };
        }
        let dir = if placer.sneaking {
            // Ray that started inside the clicked block: opposite face.
            if placer.hit_inside { face.opposite() } else { face }
        } else if face == Dir::Up {
            horizontal_facing(placer.yaw)
        } else {
            Dir::Up
        };
        let mut horizontal = 0;
        let mut q = rel(pos, dir);
        while horizontal < 7 {
            if !self.in_build_height(q[1]) {
                return Err(if q[1] >= self.top_y() { NoCell::TooHigh(1) } else { NoCell::Refused });
            }
            let s = self.get(q);
            if v.block(s) != block {
                if self.replaceable_by(s, item, false, dir, false, placer.sneaking) {
                    let st = dir.step();
                    let hit = [0, 1, 2].map(|i| f64::from(q[i]) + 0.5 + f64::from(st[i]) * 0.5);
                    return Ok((q, dir, hit, true));
                }
                return Err(NoCell::Refused);
            }
            q = rel(q, dir);
            if dir.horizontal() {
                horizontal += 1;
            }
        }
        Err(NoCell::Refused)
    }

    /// The placed `state` of `block` holds at `p` (scaffolding is placed without that test: it
    /// falls by itself when too far from a support).
    fn placement_holds(&self, block: u16, state: u32, p: P) -> bool {
        self.table().kind.get(block as usize) == Some(&Kind::Scaffolding) || self.can_place_at(state, p)
    }

    /// Second block the placement of `s` at `p` adds right after it, before anything else
    /// reacts: the upper half of a door or of a tall plant (waterlogged in any water), the head
    /// of a bed.
    fn placed_second_block(&self, p: P, s: u32) -> Option<SecondBlock> {
        let v = self.view();
        let (pos, state) = match v.kind(s) {
            Kind::Door { .. } => (above(p), v.set(s, PropKey::Half, "upper")),
            Kind::DoublePlant => {
                let a = above(p);
                let up = v.set(s, PropKey::Half, "upper");
                let up = if v.has(up, PropKey::Waterlogged) { v.set_bool(up, PropKey::Waterlogged, self.fluid_at(a).kind == 1) } else { up };
                (a, up)
            }
            Kind::Bed => (rel(p, v.facing(s)), v.set(s, PropKey::Part, "head")),
            _ => return None,
        };
        let on_client = self.state_name(s) != "minecraft:small_dripleaf";
        Some(SecondBlock { pos, state, on_client })
    }

    /// The survival test of the block at placement (attached families included).
    fn can_place_at(&self, s: u32, p: P) -> bool {
        let v = self.view();
        let b = self.get(below(p));
        match v.kind(s) {
            Kind::Torch | Kind::RedstoneTorch => v.center_up(b),
            Kind::WallTorch | Kind::RedstoneWallTorch | Kind::Ladder => self.wall_attach(p, v.facing(s)),
            Kind::Lever | Kind::Button { .. } | Kind::Grindstone => {
                let c = super::redstone::connected_dir(&v, s);
                self.face_attach(p, c.opposite())
            }
            Kind::Repeater | Kind::Comparator => v.rigid_up(b),
            Kind::Wire => v.sturdy(b, Dir::Up) || v.block(b) == self.table().id.hopper,
            Kind::Plate { .. } | Kind::WeightedPlate { .. } => v.rigid_up(b) || v.center_up(b),
            Kind::Door { .. } => {
                if v.get(s, PropKey::Half) == Some("upper") {
                    v.block(b) == v.block(s)
                } else {
                    v.sturdy(b, Dir::Up)
                }
            }
            Kind::Rail | Kind::PoweredRail | Kind::DetectorRail => v.rigid_up(b),
            Kind::Lantern => {
                if v.get_bool(s, PropKey::Hanging) {
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
    fn placement_state(&self, block: u16, ctx: &PlaceCtx) -> Option<u32> {
        let v = self.view();
        let def = v.default_state(block);
        let kind = self.table().kind.get(block as usize).copied().unwrap_or(Kind::Other);
        let name = v.data.blocks[block as usize].name.as_str();
        let p = ctx.pos;
        let water = {
            let f = self.fluid_at(p);
            f.kind == 1 && f.source
        };
        let wl = |s: u32| if v.has(s, PropKey::Waterlogged) { v.set_bool(s, PropKey::Waterlogged, water) } else { s };
        let half_bottom = ctx.face != Dir::Down && (ctx.face == Dir::Up || ctx.at[1] - f64::from(p[1]) <= 0.5);
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
        ) && !ctx.placer.game_master
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
            Kind::Sapling if block == self.table().id.mangrove_propagule => {
                v.set_int(self.generic_placement(def, name, ctx), PropKey::Age, 4)
            }
            // Pitcher crop: default state, without upper half at placement.
            Kind::PitcherCrop => def,
            // Campfire: turned like the gaze, put out in water, signal on a hay bale.
            Kind::Campfire => {
                let f = self.fluid_at(p);
                let wet = f.kind == 1 && f.source;
                let hay = v.block(self.get(below(p))) == self.table().block_id("minecraft:hay_block");
                let s = v.set_bool(v.set_bool(def, PropKey::Waterlogged, wet), PropKey::Lit, !wet);
                v.set_dir(v.set_bool(s, PropKey::SignalFire, hay), PropKey::Facing, ctx.horizontal)
            }
            // Chorus plant: connected to its neighbours from placement.
            Kind::ChorusPlant => self.chorus_plant_state(p),
            Kind::HugeMushroom => self.huge_mushroom_placement(def, p),
            Kind::PaleMossCarpet => self.moss_carpet_sides(def, p, true),
            // Shelf fungus: first horizontal direction of the gaze, turned to the
            // opposite, which holds against the full face behind it.
            Kind::Support(super::attach::Sup::FacingSturdy) if name == "minecraft:shelf_mushroom" => {
                return ctx.nearest.iter().filter(|d| d.horizontal()).find_map(|&d| {
                    let s = v.set_dir(def, PropKey::Facing, d.opposite());
                    self.can_place_at(s, p).then_some(s)
                });
            }
            // Jigsaw block: front = clicked face; top = upwards, or opposite to the horizontal
            // gaze when the front is vertical.
            _ if name == "minecraft:jigsaw" => {
                let front = ctx.face;
                let top = if front.axis() == 1 { ctx.horizontal.opposite() } else { Dir::Up };
                v.set(def, PropKey::Orientation, &format!("{}_{}", front.name(), top.name()))
            }
            // Corals and fans: waterlogged in full water; the fan becomes wall (wall variant)
            // according to the gaze order, the top excluded.
            Kind::Support(super::attach::Sup::CoralBelow { .. }) => {
                let wet = water_full();
                let standing = v.set_bool(def, PropKey::Waterlogged, wet);
                if !name.ends_with("_coral_fan") {
                    return Some(standing);
                }
                let wall = self.table().block_id(&name.replace("_coral_fan", "_coral_wall_fan"));
                let wall_state = (wall != u16::MAX)
                    .then(|| {
                        let wdef = v.set_bool(v.default_state(wall), PropKey::Waterlogged, wet);
                        ctx.nearest.iter().filter(|d| d.horizontal()).find_map(|&d| {
                            let s = v.set_dir(wdef, PropKey::Facing, d.opposite());
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
                let s = v.set_dir(def, PropKey::Facing, ctx.horizontal.opposite());
                let locked = self.repeater_locked(p, s);
                v.set_bool(s, PropKey::Locked, locked)
            }
            Kind::Comparator => v.set_dir(def, PropKey::Facing, ctx.horizontal.opposite()),
            Kind::Torch | Kind::RedstoneTorch | Kind::StandingSign => {
                return self.standing_or_wall(block, ctx);
            }
            Kind::Lever | Kind::Button { .. } | Kind::Grindstone => {
                for &d in &ctx.nearest {
                    let s = if d.axis() == 1 {
                        v.set_dir(
                            v.set(def, PropKey::Face, if d == Dir::Up { "ceiling" } else { "floor" }),
                            PropKey::Facing,
                            ctx.horizontal,
                        )
                    } else {
                        v.set_dir(v.set(def, PropKey::Face, "wall"), PropKey::Facing, d.opposite())
                    };
                    if self.can_place_at(s, p) {
                        return Some(s);
                    }
                }
                return None;
            }
            Kind::Lamp => v.set_bool(def, PropKey::Lit, self.has_neighbor_signal(p)),
            Kind::Piston { .. } => {
                v.set_bool(v.set_dir(def, PropKey::Facing, ctx.nearest[0].opposite()), PropKey::Extended, false)
            }
            Kind::Observer => v.set_dir(def, PropKey::Facing, ctx.nearest[0]),
            Kind::Dispenser | Kind::Dropper | Kind::Crafter => {
                if kind == Kind::Crafter {
                    let f = ctx.nearest[0].opposite();
                    let o = match f {
                        Dir::Up => format!("up_{}", ctx.horizontal.name()),
                        Dir::Down => format!("down_{}", ctx.horizontal.opposite().name()),
                        d => format!("{}_up", d.name()),
                    };
                    // Placed already powered: `triggered` (it then crafts once, `set_placed_by`).
                    v.set_bool(v.set(def, PropKey::Orientation, &o), PropKey::Triggered, self.has_neighbor_signal(p))
                } else {
                    v.set_dir(def, PropKey::Facing, ctx.nearest[0].opposite())
                }
            }
            // G6-F01: shelf placed powered if a neighbour gives power (chain at placement).
            Kind::Shelf => {
                let s = self.generic_placement(def, name, ctx);
                v.set_bool(s, PropKey::Powered, self.has_neighbor_signal(p))
            }
            Kind::Hopper => {
                let d = ctx.face.opposite();
                let facing = if d.axis() == 1 { Dir::Down } else { d };
                v.set_bool(v.set_dir(def, PropKey::Facing, facing), PropKey::Enabled, true)
            }
            Kind::Door { .. } => {
                if p[1] >= self.top_y() - 1
                    || !self.replaceable_by(self.get(above(p)), &ctx.item, false, ctx.face, false, ctx.placer.sneaking)
                {
                    return None;
                }
                let powered = self.has_neighbor_signal(p) || self.has_neighbor_signal(above(p));
                let s = v.set_dir(def, PropKey::Facing, ctx.horizontal);
                let s = v.set(s, PropKey::Hinge, self.door_hinge(ctx));
                let s = v.set_bool(v.set_bool(s, PropKey::Powered, powered), PropKey::Open, powered);
                v.set(s, PropKey::Half, "lower")
            }
            Kind::Trapdoor { .. } => {
                let mut s = if !ctx.replace_clicked && ctx.face.horizontal() {
                    let s = v.set_dir(def, PropKey::Facing, ctx.face);
                    v.set(s, PropKey::Half, if ctx.at[1] - f64::from(p[1]) > 0.5 { "top" } else { "bottom" })
                } else {
                    let s = v.set_dir(def, PropKey::Facing, ctx.horizontal.opposite());
                    v.set(s, PropKey::Half, if ctx.face == Dir::Up { "bottom" } else { "top" })
                };
                if self.has_neighbor_signal(p) {
                    s = v.set_bool(v.set_bool(s, PropKey::Open, true), PropKey::Powered, true);
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
                let s = v.set_dir(def, PropKey::Facing, d);
                let s = v.set_bool(v.set_bool(s, PropKey::Open, open), PropKey::Powered, open);
                v.set_bool(s, PropKey::InWall, in_wall)
            }
            Kind::Fence => {
                let mut s = def;
                for d in Dir::HORIZONTAL {
                    let q = rel(p, d);
                    let qs = self.get(q);
                    s = v.set_bool(s, d.key(), self.fence_connects(def, qs, v.sturdy(qs, d.opposite()), d.opposite()));
                }
                wl(s)
            }
            Kind::Pane => {
                let mut s = def;
                for d in Dir::HORIZONTAL {
                    let q = rel(p, d);
                    let qs = self.get(q);
                    s = v.set_bool(s, d.key(), self.pane_attaches(qs, v.sturdy(qs, d.opposite())));
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
                let s = v.set_dir(def, PropKey::Facing, ctx.horizontal);
                let s = v.set(s, PropKey::Half, if half_bottom { "bottom" } else { "top" });
                let s = wl(s);
                let sh = self.stairs_shape(s, p);
                v.set(s, PropKey::Shape, sh)
            }
            Kind::Slab => {
                let cur = self.get(p);
                if v.block(cur) == block {
                    v.set_bool(v.set(cur, PropKey::Type, "double"), PropKey::Waterlogged, false)
                } else {
                    wl(v.set(def, PropKey::Type, if half_bottom { "bottom" } else { "top" }))
                }
            }
            Kind::Bed => {
                let d = ctx.horizontal;
                let head = rel(p, d);
                if !self.replaceable_by(self.get(head), &ctx.item, false, ctx.face, false, ctx.placer.sneaking) || !Server::block_in_border(self.border_bounds(), head) {
                    return None;
                }
                v.set_dir(def, PropKey::Facing, d)
            }
            Kind::DoublePlant => {
                if p[1] >= self.top_y() - 1
                    || !self.replaceable_by(self.get(above(p)), &ctx.item, false, ctx.face, false, ctx.placer.sneaking)
                {
                    return None;
                }
                // Small dripleaf: turned towards the player, waterlogged in any water.
                if v.has(def, PropKey::Facing) {
                    let s = v.set_dir(def, PropKey::Facing, ctx.horizontal.opposite());
                    v.set_bool(s, PropKey::Waterlogged, self.fluid_at(p).kind == 1)
                } else {
                    def
                }
            }
            // Leaves placed: persistent, distance computed from the neighbours.
            Kind::Leaves => self.leaf_update_distance(wl(v.set_bool(def, PropKey::Persistent, true)), p),
            Kind::Chest => {
                let facing = ctx.horizontal.opposite();
                let mut s = wl(v.set_dir(def, PropKey::Facing, facing));
                let partner = |d: Dir| {
                    let q = self.get(rel(p, d));
                    (self.chest_connects(def, q) && v.get(q, PropKey::Type) == Some("single")).then(|| v.facing(q))
                };
                let mut joined = false;
                if ctx.face.horizontal()
                    && ctx.placer.sneaking
                    && let Some(f) = partner(ctx.face.opposite())
                    && f.axis() != ctx.face.axis()
                {
                    s = v.set_dir(s, PropKey::Facing, f);
                    let t = if f.counter_clockwise() == ctx.face.opposite() { "right" } else { "left" };
                    s = v.set(s, PropKey::Type, t);
                    joined = true;
                }
                if !joined && !ctx.placer.sneaking {
                    if partner(facing.clockwise()) == Some(facing) {
                        s = v.set(s, PropKey::Type, "left");
                    } else if partner(facing.counter_clockwise()) == Some(facing) {
                        s = v.set(s, PropKey::Type, "right");
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
                        let s = v.set_dir(def, PropKey::Facing, d.opposite());
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
                        let s = v.set_bool(def, PropKey::Hanging, d == Dir::Up);
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
                    let l = v.get_int(cur, PropKey::Layers);
                    v.set_int(cur, PropKey::Layers, (l + 1).min(8))
                } else {
                    def
                }
            }
            Kind::Anvil => v.set_dir(def, PropKey::Facing, ctx.horizontal.clockwise()),
            Kind::Cocoa => {
                for &d in &ctx.nearest {
                    if d.horizontal() {
                        let s = v.set_dir(def, PropKey::Facing, d);
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
                    let s = v.set(def, PropKey::Attachment, if f == Dir::Down { "ceiling" } else { "floor" });
                    let s = v.set_dir(s, PropKey::Facing, ctx.horizontal);
                    return self.bell_survives(s, p).then_some(s);
                }
                let (a, b) = if f.axis() == 0 { (Dir::West, Dir::East) } else { (Dir::North, Dir::South) };
                let double = v.sturdy(self.get(rel(p, a)), b) && v.sturdy(self.get(rel(p, b)), a);
                let s = v.set_dir(def, PropKey::Facing, f.opposite());
                let s = v.set(s, PropKey::Attachment, if double { "double_wall" } else { "single_wall" });
                if self.bell_survives(s, p) {
                    return Some(s);
                }
                let floor = v.sturdy(self.get(below(p)), Dir::Up);
                let s = v.set(s, PropKey::Attachment, if floor { "floor" } else { "ceiling" });
                return self.bell_survives(s, p).then_some(s);
            }
            // G5-F09: big dripleaf placed on a leaf or a stem: same orientation.
            Kind::BigDripleaf => {
                let s = self.generic_placement(def, name, ctx);
                let b = self.get(below(p));
                if matches!(v.kind(b), Kind::BigDripleaf | Kind::BigDripleafStem) {
                    v.set_dir(s, PropKey::Facing, v.facing(b))
                } else {
                    s
                }
            }
            Kind::Rail | Kind::PoweredRail | Kind::DetectorRail => {
                let ew = ctx.horizontal.axis() == 0;
                let s = v.set(def, PropKey::Shape, if ew { "east_west" } else { "north_south" });
                v.set_bool(s, PropKey::Waterlogged, self.fluid_at(p).kind == 1)
            }
            // G6-F09a: speleothems (direction from the gaze, thickness, merging) and active sulfur.
            Kind::Speleothem { .. } => return self.speleothem_placement(def, p, ctx.placer.pitch, ctx.placer.sneaking, water),
            Kind::PotentSulfur => self.potent_sulfur_state(def, p).0,
            Kind::TripwireHook => return self.hook_placement(def, p, &ctx.nearest),
            Kind::Bamboo { .. } => return self.bamboo_placement(p),
            Kind::Scaffolding => self.scaffold_placement(def, p),
            Kind::Tripwire => self.tripwire_placement(def, p),
            Kind::Vine => return self.vine_placement(block, p, &ctx.nearest),
            Kind::Multiface => {
                let old = self.get(p);
                for &d in &ctx.nearest {
                    let has = v.block(old) == block && v.get_bool(old, d.key());
                    if !has && self.multiface_can_attach(p, d) {
                        let base = if v.block(old) == block {
                            old
                        } else {
                            let f = self.fluid_of(old);
                            let w = f.kind == 1 && f.source;
                            if v.has(def, PropKey::Waterlogged) { v.set_bool(def, PropKey::Waterlogged, w) } else { def }
                        };
                        return Some(v.set_bool(base, d.key(), true));
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
        let v = self.view();
        let p = ctx.pos;
        let mut s = def;
        if v.has(s, PropKey::Axis) {
            let a = match ctx.face.axis() {
                0 => "x",
                1 => "y",
                _ => "z",
            };
            s = v.set(s, PropKey::Axis, a);
        }
        let six = |s: u32| {
            v.data.block_of(s).is_some_and(|b| {
                b.properties.iter().any(|&pi| {
                    let pr = &v.data.properties[pi as usize];
                    pr.name == "facing" && pr.values.len() == 6
                })
            })
        };
        if v.has(s, PropKey::Facing) {
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
            s = v.set_dir(s, PropKey::Facing, f);
        }
        if v.has(s, PropKey::Rotation) {
            let r = (((ctx.placer.yaw + 180.0) * 16.0 / 360.0 + 0.5).floor() as i32) & 15;
            s = v.set_int(s, PropKey::Rotation, r);
        }
        if v.has(s, PropKey::Half) && v.get(s, PropKey::Half).is_some_and(|h| h == "bottom" || h == "top") {
            let bottom = ctx.face != Dir::Down && (ctx.face == Dir::Up || ctx.at[1] - f64::from(p[1]) <= 0.5);
            s = v.set(s, PropKey::Half, if bottom { "bottom" } else { "top" });
        }
        if v.has(s, PropKey::Waterlogged) {
            let f = self.fluid_at(p);
            s = v.set_bool(s, PropKey::Waterlogged, f.kind == 1 && f.source);
        }
        s
    }

    /// Item with a wall variant (torches, signs…): first state that holds in the order of the
    /// gaze.
    fn standing_or_wall(&self, block: u16, ctx: &PlaceCtx) -> Option<u32> {
        let v = self.view();
        let name = v.data.blocks[block as usize].name.clone();
        // `wall_` before the last word: torch → wall_torch, oak_sign → oak_wall_sign.
        let bare = name.strip_prefix("minecraft:").unwrap_or(&name);
        let wall_name = match bare.rsplit_once('_') {
            Some((a, b)) => format!("minecraft:{a}_wall_{b}"),
            None => format!("minecraft:wall_{bare}"),
        };
        let wall = self.table().block_id(&wall_name);
        let standing = self.generic_placement(v.default_state(block), &name, ctx);
        let wall_state = (wall != u16::MAX)
            .then(|| {
                let wdef = v.default_state(wall);
                ctx.nearest.iter().filter(|d| d.horizontal()).find_map(|&d| {
                    let s = v.set_dir(wdef, PropKey::Facing, d.opposite());
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
                let s = if v.has(s, PropKey::Waterlogged) {
                    let f = self.fluid_at(ctx.pos);
                    v.set_bool(s, PropKey::Waterlogged, f.kind == 1 && f.source)
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
        let v = self.view();
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
        let wall = self.table().block_id(&wall_name);
        let p = ctx.pos;
        let powered = |s: u32| {
            if v.has(s, PropKey::Powered) { v.set_bool(s, PropKey::Powered, self.has_neighbor_signal(p)) } else { s }
        };
        let yaw = if head { ctx.placer.yaw } else { ctx.placer.yaw + 180.0 };
        let segment = ((yaw / 22.5 + 0.5).floor() as i32) & 15;
        let standing = powered(v.set_int(v.default_state(block), PropKey::Rotation, segment));
        let standing_ok = head || v.legacy_solid(self.get(below(p)));
        let wall_state = (wall != u16::MAX)
            .then(|| {
                let wdef = powered(v.default_state(wall));
                ctx.nearest.iter().filter(|d| d.horizontal()).find_map(|&d| {
                    let s = v.set_dir(wdef, PropKey::Facing, d.opposite());
                    let behind = self.get(rel(p, d));
                    let ok = if head {
                        !self.replaceable_by(behind, &ctx.item, false, ctx.face, false, ctx.placer.sneaking)
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
        let v = self.view();
        let p = ctx.pos;
        let water = self.fluid_at(p);
        let water = water.kind == 1 && water.source;
        // Wall variant: first horizontal direction of the gaze outside the axis of the
        // clicked face, oriented opposite, held by one side of its axis.
        let wall = self.table().block_id(&name.replace("_hanging_sign", "_wall_hanging_sign"));
        let wall_state = (wall != u16::MAX)
            .then(|| {
                let wdef = v.default_state(wall);
                ctx.nearest.iter().filter(|d| d.horizontal() && d.axis() != ctx.face.axis()).find_map(|&d| {
                    let s = v.set_dir(wdef, PropKey::Facing, d.opposite());
                    self.wall_hanging_sign_holds(s, p).then(|| v.set_bool(s, PropKey::Waterlogged, water))
                })
            })
            .flatten();
        // Ceiling sign: hung in the middle under an incomplete bottom face (or when
        // sneaking), except under another sign of the same axis; otherwise turned according to
        // the gaze (16 notches), aligned on the axis when it is not.
        let up = above(p);
        let up_s = self.get(up);
        let dir = ctx.horizontal;
        let mut middle = !self.collision_face_full(up_s, Dir::Down) || ctx.placer.sneaking;
        if !ctx.placer.sneaking && self.tag("minecraft:all_hanging_signs", v.block(up_s)) {
            let up_dir = if v.has(up_s, PropKey::Facing) {
                Some(v.facing(up_s))
            } else if v.has(up_s, PropKey::Rotation) {
                match v.get_int(up_s, PropKey::Rotation) {
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
            ((ctx.placer.yaw + 180.0) * 16.0 / 360.0 + 0.5).floor() as i32 & 15
        } else {
            // Notch of a direction: south 0, west 4, north 8, east 12.
            match dir.opposite() {
                Dir::South => 0,
                Dir::West => 4,
                Dir::North => 8,
                _ => 12,
            }
        };
        let ceiling = v.set_int(v.set_bool(v.default_state(block), PropKey::Attached, middle), PropKey::Rotation, rotation);
        let ceiling = v.set_bool(ceiling, PropKey::Waterlogged, water);
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
        let v = self.view();
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
        let v = self.view();
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
            matches!(v.kind(s), Kind::Door { .. }) && v.get(s, PropKey::Half) == Some("lower")
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
}

impl<W: BlockReader + ?Sized> PlacementRules for W {}
