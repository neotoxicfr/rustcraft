//! Respawning in the right dimension (S6): remembered point (bed, respawn anchor,
//! forced point) searched in its dimension, otherwise the world point in the spawn dimension
//! (the respawn point search with block use); anchor (glowstone charge, point set
//! in the Nether, explosion elsewhere); bed that explodes outside the surface (bed rule of
//! the dimension). "Invalid respawn point" explosion: blocks, fire, damage to the player.

use rc_client::Event;
use rc_physics::{Aabb, collide};

use crate::blocks::{Dir, P, flag};
use crate::dims::{OVERWORLD, Transition};
use crate::server::Server;

/// Player box (single precision of the game).
pub(crate) const PLAYER_DIMS: (f32, f32) = (0.6, 1.8);

/// Offsets around the anchor (horizontal ones, below, on top, then above).
fn anchor_offsets() -> Vec<[i32; 3]> {
    let h = [[0, 0, -1], [-1, 0, 0], [0, 0, 1], [1, 0, 0], [-1, 0, -1], [1, 0, -1], [-1, 0, 1], [1, 0, 1]];
    let mut out: Vec<[i32; 3]> = h.to_vec();
    out.extend(h.iter().map(|o| [o[0], -1, o[2]]));
    out.extend(h.iter().map(|o| [o[0], 1, o[2]]));
    out.push([0, 1, 0]);
    out
}

/// Yaw to look at the bottom centre of a block from `pos`, computed like the game: normalised vector
/// (zero below 1e-5), the game's approximate `atan2`, degrees in double, brought into [-180, 180[.
pub(crate) fn look_at_yaw(pos: [f64; 3], at: P) -> f32 {
    let d = [f64::from(at[0]) + 0.5 - pos[0], f64::from(at[1]) - pos[1], f64::from(at[2]) + 0.5 - pos[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let (x, z) = if l < f64::from(1.0e-5f32) { (0.0, 0.0) } else { (d[0] / l, d[2] / l) };
    let a = crate::mobs::mth::atan2(z, x) * 180.0 / f64::from(std::f32::consts::PI) - 90.0;
    let mut w = a % 360.0;
    if w >= 180.0 {
        w -= 360.0;
    }
    if w < -180.0 {
        w += 360.0;
    }
    w as f32
}

/// Condition of a bed rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BedWhen {
    Always,
    WhenDark,
    Never,
}

impl BedWhen {
    fn parse(v: Option<&serde_json::Value>, default: BedWhen) -> BedWhen {
        match v.and_then(serde_json::Value::as_str) {
            Some("always") => BedWhen::Always,
            Some("when_dark") => BedWhen::WhenDark,
            Some("never") => BedWhen::Never,
            _ => default,
        }
    }
}

/// Bed rule of a dimension (attribute `gameplay/bed_rule` or `gameplay/straw_bed_rule`).
#[derive(Clone, Debug)]
pub(crate) struct SleepRule {
    pub can_sleep: BedWhen,
    pub can_set_spawn: BedWhen,
    pub destroy_on_use: bool,
    pub destroy_on_leave: bool,
    /// Error message (JSON text), shown above the bar.
    pub error: Option<serde_json::Value>,
}

impl Server {
    /// Bed rule of a dimension; attribute absent: game default (bed: sleep
    /// when it is dark, point always set; straw: point never set, destroyed on waking).
    pub(crate) fn bed_rule(&self, dim: &str, straw: bool) -> SleepRule {
        let key = if straw { "minecraft:gameplay/straw_bed_rule" } else { "minecraft:gameplay/bed_rule" };
        let r = self.dim_attribute(dim, key);
        let (set_default, leave_default) = if straw { (BedWhen::Never, true) } else { (BedWhen::Always, false) };
        let no_sleep = || serde_json::json!({ "translate": "block.minecraft.bed.no_sleep" });
        let Some(r) = r else {
            return SleepRule {
                can_sleep: BedWhen::WhenDark,
                can_set_spawn: set_default,
                destroy_on_use: false,
                destroy_on_leave: leave_default,
                error: Some(no_sleep()),
            };
        };
        SleepRule {
            can_sleep: BedWhen::parse(r.get("can_sleep"), BedWhen::WhenDark),
            can_set_spawn: BedWhen::parse(r.get("can_set_spawn"), set_default),
            destroy_on_use: r.get("destroy_on_use").and_then(serde_json::Value::as_bool).unwrap_or(false),
            destroy_on_leave: r.get("destroy_on_leave").and_then(serde_json::Value::as_bool).unwrap_or(false),
            error: r.get("error_message").cloned(),
        }
    }

    /// A bed rule condition, in the active dimension.
    pub(crate) fn bed_when(&self, w: BedWhen) -> bool {
        match w {
            BedWhen::Always => true,
            BedWhen::WhenDark => self.is_dark_outside(),
            BedWhen::Never => false,
        }
    }

    fn anchor_works(&self, dim: &str) -> bool {
        self.dim_attribute(dim, "minecraft:gameplay/respawn_anchor_works").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// Block dangerous for a player.
    fn dangerous(&self, p: P) -> bool {
        let s = self.get(p);
        let v = self.v();
        let name = self.data.block_of(s).map(|b| b.name.as_str()).unwrap_or("");
        self.tag_id(crate::tag_bits::TagId::Fire, v.block(s))
            || matches!(
                name,
                "minecraft:lava"
                    | "minecraft:magma_block"
                    | "minecraft:lava_cauldron"
                    | "minecraft:wither_rose"
                    | "minecraft:sweet_berry_bush"
                    | "minecraft:cactus"
                    | "minecraft:powder_snow"
            )
            || (name.ends_with("campfire") && v.get_bool(s, "lit"))
    }

    /// Top of the collision shape of a block excluding climbables (empty: `None`).
    fn shape_top(&self, p: P) -> Option<(f64, f64)> {
        let s = self.get(p);
        let v = self.v();
        let climb = self.tag_id(crate::tag_bits::TagId::Climbable, v.block(s));
        let open_trap =
            v.get(s, "open") == Some("true") && self.data.block_of(s).is_some_and(|b| b.name.ends_with("trapdoor"));
        if climb || open_trap {
            return None;
        }
        let mut boxes = Vec::new();
        self.phys.collision(s, &mut boxes);
        if boxes.is_empty() {
            return None;
        }
        let lo = boxes.iter().map(|b| b.min[1]).fold(f64::MAX, f64::min);
        let hi = boxes.iter().map(|b| b.max[1]).fold(f64::MIN, f64::max);
        Some((lo, hi))
    }

    /// The search for a safe dismount point for a player.
    pub(crate) fn safe_dismount(&self, p: P, check_danger: bool) -> Option<[f64; 3]> {
        self.safe_dismount_sized(p, check_danger, PLAYER_DIMS, true)
    }

    /// Same search for a `(width, height)` box of an entity type; blocks
    /// forbidden at spawn only count for a player.
    pub(crate) fn safe_dismount_sized(&self, p: P, check_danger: bool, dims: (f32, f32), player: bool) -> Option<[f64; 3]> {
        if check_danger && self.dangerous(p) {
            return None;
        }
        let floor = match self.shape_top(p) {
            Some((_, hi)) => hi,
            None => match self.shape_top([p[0], p[1] - 1, p[2]]) {
                Some((_, hi)) if hi >= 1.0 => hi - 1.0,
                _ => f64::NEG_INFINITY,
            },
        };
        if floor.is_infinite() || floor >= 1.0 {
            return None;
        }
        if check_danger && floor <= 0.0 && self.dangerous([p[0], p[1] - 1, p[2]]) {
            return None;
        }
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]) + floor, f64::from(p[2]) + 0.5];
        // Half-width computed in single precision, like the entity type's box.
        let (w, h) = (f64::from(dims.0 / 2.0), f64::from(dims.1));
        let b = Aabb {
            min: [pos[0] - w, pos[1], pos[2] - w],
            max: [pos[0] + w, pos[1] + h, pos[2] + w],
        };
        if !collide::is_free(&self.world(), &b) {
            return None;
        }
        let invalid = |q: P| self.tag_id(crate::tag_bits::TagId::InvalidSpawnInside, self.v().block(self.get(q)));
        if player && (invalid(p) || invalid([p[0], p[1] + 1, p[2]])) {
            return None;
        }
        // Box inside the world border.
        self.box_in_border(&b).then_some(pos)
    }

    fn anchor_stand_up(&self, p: P) -> Option<[f64; 3]> {
        for danger in [true, false] {
            for o in anchor_offsets() {
                if let Some(pos) = self.safe_dismount([p[0] + o[0], p[1] + o[1], p[2] + o[2]], danger) {
                    return Some(pos);
                }
            }
        }
        None
    }

    /// The search for a wake-up position near a bed (player).
    pub(crate) fn bed_stand_up(&self, p: P, forward: Dir, yaw: f32) -> Option<[f64; 3]> {
        self.bed_stand_up_sized(p, forward, yaw, PLAYER_DIMS, true)
    }

    /// Same search for the box of another entity type (villager).
    pub(crate) fn bed_stand_up_sized(
        &self,
        p: P,
        forward: Dir,
        yaw: f32,
        dims: (f32, f32),
        player: bool,
    ) -> Option<[f64; 3]> {
        let right = forward.clockwise();
        let r = right.step();
        // Side chosen according to the yaw (the game's tabulated sine, in single precision).
        let facing_yaw = {
            let rad = yaw * (std::f64::consts::PI / 180.0) as f32;
            r[0] as f32 * -crate::mobs::mth::sin(rad) + r[2] as f32 * crate::mobs::mth::cos(rad) > 0.0
        };
        let side = if facing_yaw { right.opposite() } else { right };
        let (f, s) = (forward.step(), side.step());
        let surround = vec![
            [s[0], s[2]],
            [s[0] - f[0], s[2] - f[2]],
            [s[0] - f[0] * 2, s[2] - f[2] * 2],
            [-f[0] * 2, -f[2] * 2],
            [-s[0] - f[0] * 2, -s[2] - f[2] * 2],
            [-s[0] - f[0], -s[2] - f[2]],
            [-s[0], -s[2]],
            [-s[0] + f[0], -s[2] + f[2]],
            [f[0], f[2]],
            [s[0] + f[0], s[2] + f[2]],
        ];
        let above = vec![[0, 0], [-f[0], -f[2]]];
        let at = |base: P, offs: &[[i32; 2]], danger: bool| {
            offs.iter().find_map(|o| self.safe_dismount_sized([base[0] + o[0], base[1], base[2] + o[1]], danger, dims, player))
        };
        let bunk = self.data.block_of(self.get([p[0], p[1] - 1, p[2]])).is_some_and(|b| b.name.ends_with("_bed"));
        if bunk {
            let below = [p[0], p[1] - 1, p[2]];
            return at(p, &surround, true)
                .or_else(|| at(below, &surround, true))
                .or_else(|| at(p, &above, true))
                .or_else(|| at(p, &surround, false))
                .or_else(|| at(below, &surround, false))
                .or_else(|| at(p, &above, false));
        }
        let mut all = surround;
        all.extend(above);
        at(p, &all, true).or_else(|| at(p, &all, false))
    }

    /// The respawn point search with block use in the active dimension.
    fn respawn_at(&mut self, p: P, yaw: f32, pitch: f32, forced: bool, consume: bool) -> Option<([f64; 3], f32, f32)> {
        self.load_now(&[(p[0] >> 4, p[2] >> 4)]);
        let s = self.get(p);
        let v = self.v();
        let name = self.data.block_of(s).map(|b| b.name.clone()).unwrap_or_default();
        let dim = self.dim.name.clone();
        if name == "minecraft:respawn_anchor" && (forced || v.get_int(s, "charges") > 0) && self.anchor_works(&dim) {
            let found = self.anchor_stand_up(p);
            if !forced && consume && found.is_some() {
                let ns = v.set_int(s, "charges", v.get_int(s, "charges") - 1);
                self.set_block(p, ns, flag::ALL);
            }
            return found.map(|q| (q, look_at_yaw(q, p), 0.0));
        }
        let straw = name == "minecraft:straw_bed";
        if name.ends_with("_bed") && self.bed_when(self.bed_rule(&dim, straw).can_set_spawn) {
            let forward = v.get_dir(s, "facing").unwrap_or(Dir::North);
            return self.bed_stand_up(p, forward, yaw).map(|q| (q, look_at_yaw(q, p), 0.0));
        }
        if !forced {
            return None;
        }
        let free = |q: P| {
            let st = self.get(q);
            !self.v().legacy_solid(st) && self.fluid_of(st).is_empty()
        };
        (free(p) && free([p[0], p[1] + 1, p[2]]))
            .then(|| ([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.1, f64::from(p[2]) + 0.5], yaw, pitch))
    }

    /// Respawn point; `consume`: the anchor's charge
    /// is taken.
    pub(crate) fn respawn_transition(&mut self, consume: bool) -> Transition {
        let mut missing = false;
        if let Some((dim, p, yaw, pitch, forced)) = self.life.vit.respawn.clone()
            && self.world_dimensions().contains(&dim)
        {
            if let Some((pos, y, pi)) = self.in_world(&dim, |s| s.respawn_at(p, yaw, pitch, forced, consume)) {
                return Transition {
                    dim,
                    pos,
                    yaw: y,
                    pitch: pi,
                    relative: 0,
                    sound: false,
                    ticket: false,
                    missing_respawn: false,
                };
            }
            missing = true;
            self.life.vit.respawn = None;
        }
        let dim = self.level.spawn_dimension.clone();
        let dim = if self.world_dimensions().contains(&dim) { dim } else { OVERWORLD.to_owned() };
        let (pos, yaw, pitch) = self.in_world(&dim, |s| s.world_spawn_point());
        Transition { dim, pos, yaw, pitch, relative: 0, sound: false, ticket: false, missing_respawn: missing }
    }

    /// Respawn after death: destination dimension made active.
    pub(crate) fn respawn_point_dims(&mut self) -> ([f64; 3], f32, f32, Option<&'static str>) {
        let from = self.dim.name.clone();
        let t = self.respawn_transition(true);
        self.flush_block_changes();
        self.enter_world(&t.dim);
        self.forget_world_entities(&from);
        // Anchor used: depletion sound, to the respawning player
        // only.
        if let Some((dim, p, ..)) = self.life.vit.respawn.clone()
            && dim == self.dim.name
            && self.data.block_of(self.get(p)).is_some_and(|b| b.name == "minecraft:respawn_anchor")
        {
            let at = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
            self.play_sound_personal("minecraft:block.respawn_anchor.deplete", 4, at, 1.0, 1.0);
        }
        (t.pos, t.yaw, t.pitch, t.missing_respawn.then_some("block.minecraft.spawn.not_valid"))
    }

    // --- Respawn anchor and exploding beds. ---

    /// Right click on an anchor (item use on a respawn anchor then use without an item); true if
    /// the action is taken.
    pub(crate) fn use_anchor(&mut self, p: P, held: Option<&str>) -> bool {
        let s = self.get(p);
        if self.data.block_of(s).is_none_or(|b| b.name != "minecraft:respawn_anchor") {
            return false;
        }
        let v = self.v();
        let charges = v.get_int(s, "charges");
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        if held == Some("minecraft:glowstone") && charges < 4 {
            let ns = v.set_int(s, "charges", charges + 1);
            self.set_block(p, ns, flag::ALL);
            // Charge: "block changed" game event (source the player, new state).
            let src = Some(crate::mobs::recent::vibration::Src::Living(crate::mobs::mob::Ent::Player));
            self.game_event_src("minecraft:block_change", at, src, Some(ns));
            self.play_sound("minecraft:block.respawn_anchor.charge", 4, at, 1.0, 1.0);
            let slot = 36 + self.held as usize;
            if self.game_mode != 1
                && let Some(st) = &mut self.slots[slot]
            {
                st.count -= 1;
                if st.count <= 0 {
                    self.slots[slot] = None;
                }
            }
            self.broadcast_menus();
            return true;
        }
        if charges == 0 {
            return false;
        }
        let dim = self.dim.name.clone();
        if !self.anchor_works(&dim) {
            self.anchor_explode(p);
            return true;
        }
        let same = self.life.vit.respawn.as_ref().is_some_and(|r| r.0 == dim && r.1 == p);
        if !same {
            self.life.vit.respawn = Some((dim, p, 0.0, 0.0, false));
            let _ = self.send(Event::Chat {
                component: rc_text::Component::translate("block.minecraft.set_spawn", Vec::new()),
                tag: rc_client::ChatTag::System,
            });
            self.play_sound("minecraft:block.respawn_anchor.set_spawn", 4, at, 1.0, 1.0);
        }
        true
    }

    fn anchor_explode(&mut self, p: P) {
        self.remove_block(p, false);
        let water_flows = |s: &Server, q: P| {
            let f = s.fluid_at(q);
            if f.kind != 1 {
                return false;
            }
            if f.source {
                return true;
            }
            f.amount >= 2 && s.fluid_at([q[0], q[1] - 1, q[2]]).kind != 1
        };
        let in_water = Dir::HORIZONTAL.iter().any(|d| water_flows(self, crate::portal_shape::add(p, d.step(), 1)))
            || self.fluid_at([p[0], p[1] + 1, p[2]]).kind == 1;
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.respawn_explosion(c, 5.0, if in_water { Some(p) } else { None });
    }

    /// Straw bed destroyed (on waking or use): break sound "on waking", then air; the
    /// shape update removes the other half (particles 2014, no sound).
    pub(crate) fn straw_bed_break(&mut self, p: P) {
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.play_sound("minecraft:block.straw_bed.break_leave", 4, at, 1.0, 1.0);
        let air = self.air();
        self.set_block(p, air, flag::ALL);
    }

    /// Bed used where the rule destroys it (head `head`): the bed explodes, the straw bed
    /// breaks without an explosion.
    pub(crate) fn bed_destroy_on_use(&mut self, head: P) {
        let s = self.get(head);
        if self.data.block_of(s).is_some_and(|b| b.name == "minecraft:straw_bed") {
            self.straw_bed_break(head);
            return;
        }
        self.remove_block(head, false);
        let facing = self.v().get_dir(s, "facing").unwrap_or(Dir::North);
        let foot = crate::portal_shape::add(head, facing.opposite().step(), 1);
        if self.v().block(self.get(foot)) == self.v().block(s) {
            self.remove_block(foot, false);
        }
        let c = [f64::from(head[0]) + 0.5, f64::from(head[1]) + 0.5, f64::from(head[2]) + 0.5];
        self.respawn_explosion(c, 5.0, None);
    }

    /// Explosion of a bed or an anchor (no author, damage
    /// `bad_respawn_point`, radius 5, fire, "block" interaction) through the common explosion.
    fn respawn_explosion(&mut self, c: [f64; 3], radius: f32, water_at: Option<P>) {
        self.game_event_src("minecraft:explode", c, None, None);
        self.explode_with(
            None,
            None,
            None,
            c,
            radius,
            true,
            "minecraft:bad_respawn_point",
            crate::mobs::explode::Blast::Respawn(water_at),
        );
    }
}
