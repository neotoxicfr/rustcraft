//! W8c: player item uses in singleplayer — totem (protection against death), shield
//! (blocking hits, wear, disabling), buckets (empty, pick up), equipment worn by right
//! click. Rules taken from game 26.3 (`docs/specs/integrated-server.md`), in our own words.

use rc_client::{EntityEvent, Event};
use serde_json::Value;

use crate::player::Stack;
use crate::server::Server;

/// Source of the damage in progress on the player, set by the caller just before `hurt` and
/// consumed by it: position of the direct entity (shield angle), living direct
/// entity (block retaliation), piercing arrow (never blocked), seconds of shield
/// disabling by the direct attacker's weapon.
#[derive(Debug, Clone, Copy, Default)]
pub struct DmgSrc {
    pub pos: Option<[f64; 3]>,
    pub direct_living: bool,
    pub piercing_arrow: bool,
    pub disable_secs: f32,
    /// Position of the living direct attacker (pushback of the defender on partial block).
    pub attacker_pos: Option<[f64; 3]>,
}

/// Use duration of an item that blocks (shield): one game hour.
pub(crate) const BLOCKING_USE_TICKS: i32 = 72000;

/// Use duration of a bundle (emptying).
pub(crate) const BUNDLE_USE_TICKS: i32 = 200;

/// Faces in game order (down, up, north, south, west, east).
const FACE_DIRS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

fn face_of(n: [i32; 3]) -> usize {
    FACE_DIRS.iter().position(|d| *d == n).unwrap_or(1)
}

/// Previous rank in a sequence of `len` elements, like the game's traversal: before the
/// first, the last; without a current rank, the last too.
fn previous_index(len: usize, cur: Option<usize>) -> usize {
    match cur {
        Some(c) if c > 0 => c - 1,
        _ => len.saturating_sub(1),
    }
}

/// Does a set of damage types (`#tag`, id or list) contain `ty`?
fn holder_set_has(s: &Server, set: &Value, ty: &str) -> bool {
    match set {
        Value::String(x) => match x.strip_prefix('#') {
            Some(tag) => s.dmg_tag(ty, tag),
            None => x == ty,
        },
        Value::Array(a) => a.iter().any(|v| holder_set_has(s, v, ty)),
        _ => false,
    }
}

/// Test of a slot against a list of boxes: fraction of the segment and
/// normal of the face hit.
fn clip_boxes(boxes: &[rc_data::Aabb], cell: [i32; 3], from: [f64; 3], to: [f64; 3]) -> Option<(f64, [i32; 3])> {
    if boxes.is_empty() {
        return None;
    }
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 1.0e-7 {
        return None;
    }
    let p = [from[0] + d[0] * 0.001, from[1] + d[1] * 0.001, from[2] + d[2] * 0.001];
    let c = cell.map(f64::from);
    if boxes.iter().any(|b| (0..3).all(|k| p[k] - c[k] >= b.min[k] && p[k] - c[k] < b.max[k])) {
        // Inner origin: face opposite to the direction closest to the movement.
        let cand = [(1usize, -d[1], -1), (1, d[1], 1), (2, -d[2], -1), (2, d[2], 1), (0, -d[0], -1), (0, d[0], 1)];
        let mut top = cand[0];
        for c in &cand[1..] {
            if c.1 > top.1 {
                top = *c;
            }
        }
        let mut n = [0; 3];
        n[top.0] = -top.2;
        return Some((0.001, n));
    }
    let mut best = 1.0f64;
    let mut normal = [0; 3];
    let mut hit = false;
    for b in boxes {
        let bb = rc_data::Aabb {
            min: std::array::from_fn(|k| b.min[k] + c[k]),
            max: std::array::from_fn(|k| b.max[k] + c[k]),
        };
        for a in 0..3 {
            let (b1, b2) = ((a + 1) % 3, (a + 2) % 3);
            let plane = if d[a] > 1.0e-7 {
                bb.min[a]
            } else if d[a] < -1.0e-7 {
                bb.max[a]
            } else {
                continue;
            };
            let s = (plane - from[a]) / d[a];
            let pb = from[b1] + s * d[b1];
            let pc = from[b2] + s * d[b2];
            if 0.0 < s
                && s < best
                && bb.min[b1] - 1.0e-7 < pb
                && pb < bb.max[b1] + 1.0e-7
                && bb.min[b2] - 1.0e-7 < pc
                && pc < bb.max[b2] + 1.0e-7
            {
                best = s;
                hit = true;
                normal = [0; 3];
                normal[a] = if d[a] > 0.0 { -1 } else { 1 };
            }
        }
    }
    hit.then_some((best, normal))
}

impl Server {
    /// Effective component of a stack, in the reports' format: the stack's, otherwise the item's
    /// (unless it is removed from the stack).
    pub(crate) fn comp_json(&self, s: &Stack, name: &str) -> Option<Value> {
        if let Some(n) = crate::items::component(s, name) {
            return Some(crate::commands::cmds_s9b::nbt_to_json(n));
        }
        self.life.items.default_component(s, name)
    }

    // ---------- Start and end of use ----------

    /// Does the stack emit use vibrations? Yes by default (component common to all
    /// items); no if `use_effects.interact_vibrations` is false (spears) or if the component
    /// was removed from the stack; an empty stack has no component.
    pub(crate) fn use_vibrates(&self, s: Option<&Stack>) -> bool {
        s.filter(|s| s.count > 0).is_some_and(|s| {
            self.comp_json(s, "minecraft:use_effects")
                .is_some_and(|u| u["interact_vibrations"].as_bool() != Some(false))
        })
    }

    /// Use vibration `ev` of the player if the stack `s` emits it.
    pub(crate) fn use_vibration(&mut self, s: Option<&Stack>, ev: &str) {
        if self.use_vibrates(s) {
            let pos = self.pos;
            self.game_event_at(ev, pos, Some(crate::mobs::mob::Ent::Player));
        }
    }

    /// Start of a prolonged use of hand `hand` for `ticks`: vibration
    /// `item_interact_start` (except silent item), only if no use was in progress.
    pub(crate) fn start_using(&mut self, hand: i32, ticks: i32) {
        let was = self.life.vit.using.is_some();
        self.life.vit.using = Some((hand, ticks));
        if was {
            return;
        }
        let s = self.slots[self.hand_slot(hand)].clone();
        self.life.vit.use_item = s.as_ref().map(|s| s.id.clone()).unwrap_or_default();
        self.life.vit.use_vibrations = self.use_vibrates(s.as_ref());
        if self.life.vit.use_vibrations {
            let pos = self.pos;
            self.game_event_at("minecraft:item_interact_start", pos, Some(crate::mobs::mob::Ent::Player));
        }
    }

    /// End of a prolonged use (released, completed, interrupted): vibration `item_interact_finish`
    /// if a use was in progress and its item vibrates.
    pub(crate) fn stop_using(&mut self) {
        if self.life.vit.using.take().is_some() && self.life.vit.use_vibrations {
            let pos = self.pos;
            self.game_event_at("minecraft:item_interact_finish", pos, Some(crate::mobs::mob::Ent::Player));
        }
    }

    /// The slot `slot` has just been emptied by the use (item thrown, broken, last copy):
    /// if it is the hand in use, the use continues on an empty stack, which no longer vibrates.
    pub(crate) fn use_stack_emptied(&mut self, slot: usize) {
        let using = self.life.vit.using;
        if self.slots[slot].is_none() && using.is_some_and(|(h, _)| self.hand_slot(h) == slot) {
            self.life.vit.use_item.clear();
            self.life.vit.use_vibrations = false;
        }
    }

    // ---------- Totem ----------

    /// Protection against death: the first hand (main
    /// then off) holding a protective item consumes it; health to 1, item's effects,
    /// event 35. True if the player is saved.
    pub(crate) fn death_protection(&mut self, ty: &str) -> bool {
        if self.dmg_tag(ty, "minecraft:bypasses_invulnerability") {
            return false;
        }
        for hand in [0, 1] {
            let slot = self.hand_slot(hand);
            let Some(s) = self.slots[slot].clone() else { continue };
            let Some(prot) = self.comp_json(&s, "minecraft:death_protection") else { continue };
            let mut rest = s.clone();
            rest.count -= 1;
            self.slots[slot] = (rest.count > 0).then_some(rest);
            self.award_stat("minecraft:used", &s.id, 1);
            self.trig_used_totem(&s);
            self.use_vibration(Some(&s), "minecraft:item_interact_finish");
            self.life.vit.health = 1.0f32.min(self.max_health());
            let effects = prot["death_effects"].as_array().cloned().unwrap_or_default();
            self.run_consume_effects(&effects);
            let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: 35 }));
            self.broadcast_menus();
            return true;
        }
        false
    }

    // ---------- Shield ----------

    /// Item that is blocking right now: use in progress of an item
    /// `blocks_attacks` for at least its raise delay; slot and component.
    pub(crate) fn blocking_item(&self) -> Option<(usize, i32, Value)> {
        let (hand, left) = self.life.vit.using?;
        let slot = self.hand_slot(hand);
        let s = self.slots[slot].as_ref()?;
        let ba = self.comp_json(s, "minecraft:blocks_attacks")?;
        let delay = ba["block_delay_seconds"].as_f64().unwrap_or(0.0) as f32;
        let elapsed = BLOCKING_USE_TICKS - left;
        (elapsed >= (delay * 20.0).round() as i32).then_some((slot, hand, ba))
    }

    /// Damage parried by the raised item: horizontal angle between the gaze
    /// and the source, component's reductions, wear, retaliation and disabling.
    pub(crate) fn apply_item_blocking(&mut self, ty: &str, damage: f32, src: &DmgSrc) -> f32 {
        if damage <= 0.0 {
            return 0.0;
        }
        let Some((slot, _hand, ba)) = self.blocking_item() else { return 0.0 };
        if ba.get("bypassed_by").is_some_and(|b| holder_set_has(self, b, ty)) {
            return 0.0;
        }
        if src.piercing_arrow {
            return 0.0;
        }
        let angle = match src.pos {
            Some(p) => {
                let view = crate::mobs::cubes::view_vector(0.0, self.yaw);
                let (x, z) = (p[0] - self.pos[0], p[2] - self.pos[2]);
                let l = (x * x + z * z).sqrt();
                let (nx, nz) = if l < 1.0e-5 { (0.0, 0.0) } else { (x / l, z / l) };
                (nx * view[0] + nz * view[2]).acos()
            }
            None => std::f64::consts::PI,
        };
        let default_red = [serde_json::json!({ "base": 0.0, "factor": 1.0 })];
        let reds = ba.get("damage_reductions").and_then(Value::as_array).map_or(&default_red[..], Vec::as_slice);
        let mut blocked = 0.0f32;
        for r in reds {
            let limit =
                (std::f64::consts::PI / 180.0) as f32 * r["horizontal_blocking_angle"].as_f64().unwrap_or(90.0) as f32;
            if angle > f64::from(limit) {
                continue;
            }
            if r.get("type").is_some_and(|t| !holder_set_has(self, t, ty)) {
                continue;
            }
            let base = r["base"].as_f64().unwrap_or(0.0) as f32;
            let factor = r["factor"].as_f64().unwrap_or(0.0) as f32;
            blocked += (base + factor * damage).clamp(0.0, damage);
        }
        let blocked = blocked.clamp(0.0, damage);
        // Wear: statistic at each hit received with the shield raised.
        let s = self.slots[slot].clone();
        if let Some(s) = &s {
            self.award_stat("minecraft:used", &s.id, 1);
        }
        let f = ba.get("item_damage");
        let num = |k: &str, d: f64| f.and_then(|f| f[k].as_f64()).unwrap_or(d) as f32;
        let (threshold, base, factor) = (num("threshold", 1.0), num("base", 0.0), num("factor", 1.0));
        let wear = if blocked < threshold { 0 } else { (base + factor * blocked).floor() as i32 };
        if wear > 0 {
            self.hurt_item(slot, wear);
        }
        if blocked > 0.0 && !self.dmg_tag(ty, "minecraft:is_projectile") && src.direct_living {
            let full = blocked >= damage;
            // Attacker's retaliation: specific to the species (hoglin, zoglin,
            // ravager), otherwise the default pushback on partial block.
            if let Some(mut a) = self.prog.attacker.take() {
                self.blocked_by_item(&mut a, full);
                if a.kind == crate::mobs::species::Kind::Ravager {
                    self.prog.riposte = Some((a.id, a.ill.stun_tick, a.rng));
                }
                self.prog.attacker = Some(a);
            } else if !full && let Some(a) = src.attacker_pos {
                self.player_knockback(0.5, self.pos[0] - a[0], self.pos[2] - a[2]);
            }
            // Disabling (`disable`): cooldown on the item, use stopped, sound.
            let ticks = if src.disable_secs > 0.0 {
                let secs = src.disable_secs * ba["disable_cooldown_scale"].as_f64().unwrap_or(1.0) as f32;
                if secs > 0.0 { (secs * 20.0).round() as i32 } else { 0 }
            } else {
                0
            };
            if ticks > 0 {
                if let Some(s) = &s {
                    let group = self
                        .comp_json(s, "minecraft:use_cooldown")
                        .and_then(|c| c["cooldown_group"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| s.id.clone());
                    self.add_cooldown(&group, ticks);
                }
                self.stop_using();
                if let Some(snd) = ba["disabled_sound"].as_str() {
                    let pitch = 0.8 + self.life.rng.next_float() * 0.4;
                    self.play_sound(snd, 7, self.pos, 0.8, pitch);
                }
            }
        }
        blocked
    }

    /// The block by an item from the attacker `a` against the player. Adult hoglin and non-baby zoglin
    /// throw the target even on a full block; the ravager (outside its roar) stuns itself
    /// one time out of two (40 ticks, cry, event 39, mutual pushback) otherwise knocks the target
    /// back hard; the others only push on a partial block.
    fn blocked_by_item(&mut self, a: &mut crate::mobs::mob::Mob, full: bool) {
        use crate::mobs::species::Kind;
        match a.kind {
            Kind::Hoglin | Kind::Zoglin => {
                if !a.is_baby() {
                    self.hoglin_throw(a, crate::mobs::mob::Ent::Player);
                }
            }
            Kind::Ravager => {
                if a.ill.roar_tick == 0 {
                    if a.rng.next_double() < 0.5 {
                        a.ill.stun_tick = 40;
                        self.mob_sound(a, "minecraft:entity.ravager.stunned", 1.0, 1.0);
                        self.mob_event(a, 39);
                        self.player_push_from(a);
                    } else {
                        let (xd, zd) = (self.pos[0] - a.pos[0], self.pos[2] - a.pos[2]);
                        let dd = (xd * xd + zd * zd).max(0.001);
                        self.player_add_velocity([xd / dd * 4.0, 0.2, zd / dd * 4.0]);
                    }
                }
            }
            _ => {
                if !full {
                    self.player_knockback(0.5, self.pos[0] - a.pos[0], self.pos[2] - a.pos[2]);
                }
            }
        }
    }

    /// `blocks_attacks` component of the item in use, delay included:
    /// read at the start of the hit, before a possible disabling.
    pub(crate) fn using_blocks_attacks(&self) -> Option<Value> {
        let (hand, _) = self.life.vit.using?;
        let s = self.slots[self.hand_slot(hand)].as_ref()?;
        self.comp_json(s, "minecraft:blocks_attacks")
    }

    /// Sound of the parried hit.
    pub(crate) fn blocked_sound(&mut self, ba: &Value) {
        if let Some(snd) = ba["block_sound"].as_str() {
            let pitch = 0.8 + self.life.rng.next_float() * 0.4;
            self.play_sound(snd, 7, self.pos, 1.0, pitch);
        }
    }

    /// Source of a hit dealt by the mob itself: its position; living direct entity
    /// except spit and indirect magic (non-living direct entity); disabling by the held
    /// weapon (5 s for the guardian).
    pub(crate) fn melee_src(&self, m: &crate::mobs::mob::Mob, ty: &str) -> DmgSrc {
        let direct_living = !matches!(ty, "minecraft:spit" | "minecraft:indirect_magic");
        let disable_secs = if m.kind == crate::mobs::species::Kind::Warden {
            5.0
        } else {
            m.equipment[0]
                .as_ref()
                .and_then(|w| self.comp_json(w, "minecraft:weapon"))
                .and_then(|w| w["disable_blocking_for_seconds"].as_f64())
                .unwrap_or(0.0) as f32
        };
        DmgSrc { pos: Some(m.pos), direct_living, piercing_arrow: false, disable_secs, attacker_pos: Some(m.pos) }
    }

    /// Does the player parry a new ram hit (the application of the block by an item replayed by the
    /// goat after the damage)? True if a part is blocked.
    pub(crate) fn reapply_blocking(&mut self, ty: &str, damage: f32, from: [f64; 3]) -> bool {
        let src = DmgSrc { pos: Some(from), direct_living: true, attacker_pos: Some(from), ..DmgSrc::default() };
        self.apply_item_blocking(ty, damage, &src) > 0.0
    }

    // ---------- Item use (`use_item`) ----------

    /// Item uses added by W8c, in game order: worn equipment,
    /// blocking item, buckets. True if the use is handled.
    pub(crate) fn w8c_use_item(&mut self, hand: i32) -> bool {
        if self.game_mode == 3 || self.life.vit.dead {
            return false;
        }
        let slot = self.hand_slot(hand);
        let Some(s) = self.slots[slot].clone() else { return false };
        if self.on_cooldown(&s) {
            return true;
        }
        // Bundle: prolonged use that empties it stack by stack.
        if crate::menus::bundle::contents(&self.life.items, &s).is_some() {
            self.start_using(hand, BUNDLE_USE_TICKS);
            return true;
        }
        if self.comp_json(&s, "minecraft:consumable").is_some() {
            return false;
        }
        if let Some(eq) = self.comp_json(&s, "minecraft:equippable")
            && eq["swappable"].as_bool().unwrap_or(true)
        {
            return self.swap_equipment(slot, &s, &eq);
        }
        if self.comp_json(&s, "minecraft:blocks_attacks").is_some() {
            self.start_using(hand, BLOCKING_USE_TICKS);
            return true;
        }
        if s.id == "minecraft:knowledge_book" {
            self.knowledge_book_use(slot, &s);
            return true;
        }
        // Goat horn: without an instrument, the use fails without doing anything.
        if s.id == "minecraft:goat_horn" {
            if let Some(inst) = self.comp_json(&s, "minecraft:instrument") {
                self.instrument_use(hand, slot, &s, &inst);
            }
            return true;
        }
        match s.id.as_str() {
            "minecraft:bucket" => self.bucket_use(slot, 0).is_some(),
            "minecraft:water_bucket" => self.bucket_use(slot, 1).is_some(),
            "minecraft:lava_bucket" => self.bucket_use(slot, 2).is_some(),
            id if crate::mobs::sealife::fish::bucket_kind(id).is_some() => {
                // Mob bucket: water, then the mob.
                if let Some((p, face)) = self.pov_hit(false)
                    && self.fish_bucket_use(slot, p, face)
                {
                    self.broadcast_menus();
                }
                true
            }
            _ => false,
        }
    }

    /// Compass on a lodestone: lock sound, target set (dimension and block,
    /// tracked). A lone compass in survival is changed in place; otherwise (stack, or creative)
    /// a single-unit copy carries the target, the stack loses one (except creative) and the copy goes into
    /// the inventory, or on the ground if it is full.
    pub(crate) fn compass_use_on(&mut self, slot: usize, p: [i32; 3]) -> bool {
        if self.state_name(self.st(p)) != "minecraft:lodestone" {
            return false;
        }
        let Some(s) = self.slots[slot].clone() else { return false };
        let at = p.map(|v| f64::from(v) + 0.5);
        self.play_sound("minecraft:item.lodestone_compass.lock", 7, at, 1.0, 1.0);
        let target = rc_proto::nbt::Nbt::Compound(vec![(
            "target".into(),
            rc_proto::nbt::Nbt::Compound(vec![
                ("dimension".into(), rc_proto::nbt::Nbt::String(self.dim.name.clone())),
                ("pos".into(), rc_proto::nbt::Nbt::IntArray(p.to_vec())),
            ]),
        )]);
        let creative = self.infinite_materials();
        if !creative && s.count == 1 {
            let mut one = s;
            crate::items::set_component(&mut one, "minecraft:lodestone_tracker", target);
            self.slots[slot] = Some(one);
        } else {
            let mut one = s.clone();
            one.count = 1;
            crate::items::set_component(&mut one, "minecraft:lodestone_tracker", target);
            if !creative {
                let mut rest = s;
                rest.count -= 1;
                self.slots[slot] = (rest.count > 0).then_some(rest);
            }
            let held = self.held as usize;
            if let Some(left) = crate::menus::inventory_add(&mut self.slots, &self.life.items, held, one, creative) {
                self.drop_from_player_with(left, false);
            }
        }
        self.broadcast_menus();
        true
    }

    /// Inventory tick of lodestone compasses: a tracked target in this dimension
    /// whose lodestone has disappeared (no more point of interest, or out of the world limits) is
    /// erased (the compass spins wildly). Without a tracked compass, the sweep does nothing else
    /// than read the slots.
    pub(crate) fn lodestone_compass_tick(&mut self) {
        if self.life.vit.dead || !self.player_ready() {
            return;
        }
        // Verdict of the last target seen: the POIs do not change during this sweep, compasses
        // linked to the same lodestone only look for it once
        // (`RUSTCRAFT_OPT_NO_COMPASS_MEMO=1`: one search per compass, to remeasure).
        let memo_on = !rc_data::env_flag!("RUSTCRAFT_OPT_NO_COMPASS_MEMO");
        let mut memo: Option<([i32; 3], bool)> = None;
        // Inventory, armor and offhand (not the crafting grid 0-4).
        for slot in 5..self.slots.len() {
            // Only the compass tracks a lodestone; the component set on another item stays.
            let Some(target) = self.slots[slot]
                .as_ref()
                .filter(|s| s.id == "minecraft:compass")
                .and_then(|s| crate::items::component(s, "minecraft:lodestone_tracker"))
                .filter(|t| t.get("tracked").and_then(rc_proto::nbt::Nbt::as_i64).is_none_or(|v| v != 0))
                .and_then(|t| t.get("target"))
                .and_then(|t| {
                    let dim = t.get("dimension")?.as_str()?;
                    match t.get("pos")? {
                        rc_proto::nbt::Nbt::IntArray(a) if a.len() == 3 && dim == self.dim.name => {
                            Some([a[0], a[1], a[2]])
                        }
                        _ => None,
                    }
                })
            else {
                continue;
            };
            let keep = match memo {
                Some((t, k)) if memo_on && t == target => k,
                _ => {
                    let k = self.lodestone_target_kept(target);
                    memo = Some((target, k));
                    k
                }
            };
            if keep {
                continue;
            }
            if let Some(s) = &mut self.slots[slot] {
                crate::items::set_component(s, "minecraft:lodestone_tracker", rc_proto::nbt::Nbt::Compound(Vec::new()));
            }
            self.broadcast_menus();
        }
    }

    /// Does the tracked target of a compass remain (lodestone present within the limits)?
    fn lodestone_target_kept(&mut self, target: [i32; 3]) -> bool {
        let (min_y, sections) = (self.dim.height.min_y, self.dim.height.sections as i32);
        let in_bounds = (min_y..min_y + sections * 16).contains(&target[1])
            && (-30_000_000..30_000_000).contains(&target[0])
            && (-30_000_000..30_000_000).contains(&target[2]);
        if in_bounds && self.poi_type(target) == Some(crate::poi::PoiType::Lodestone) {
            return true;
        }
        // Known difference: the game rereads the POIs of a never-loaded chunk from disk;
        // we only know them once the chunk is indexed, and keep the target until then.
        in_bounds && !self.mobs.poi.is_indexed((target[0] >> 4, target[2] >> 4))
    }

    /// Debug stick (creative and commands allowed) on block `p` of slot `slot`:
    /// `cycle` (use on the block) moves the property kept for this block (the first by
    /// default) to its next value, previous when sneaking, without neighbour updates;
    /// otherwise (start of breaking) the kept property becomes the next one (previous when sneaking),
    /// kept in `debug_stick_state`. Message in the action bar. False if refused.
    pub(crate) fn debug_stick_use(&mut self, slot: usize, p: [i32; 3], cycle: bool) -> bool {
        use rc_text::{Arg, Component};
        if !self.gamemaster_blocks() {
            return false;
        }
        let Some(s) = self.slots[slot].clone() else { return false };
        let state = self.get(p);
        let block = self.state_name(state).to_owned();
        let props: Vec<(String, Vec<String>, usize)> = self
            .v()
            .props_sorted(state)
            .into_iter()
            .map(|(n, vals, i)| (n.to_owned(), vals.to_vec(), i))
            .collect();
        if props.is_empty() {
            let msg = Component::translate("item.minecraft.debug_stick.empty", vec![Arg::Text(block)]);
            let _ = self.send(Event::ActionBar(msg));
            return false;
        }
        // Component removed from the stack: nothing. Otherwise map block → property (empty by default).
        if self.comp_json(&s, "minecraft:debug_stick_state").is_none() {
            return false;
        }
        let mut map: Vec<(String, rc_proto::nbt::Nbt)> =
            match crate::items::component(&s, "minecraft:debug_stick_state") {
                Some(rc_proto::nbt::Nbt::Compound(f)) => f.clone(),
                _ => Vec::new(),
            };
        let back = self.sneaking;
        let remembered = map
            .iter()
            .find(|(b, _)| *b == block)
            .and_then(|(_, n)| n.as_str())
            .and_then(|n| props.iter().position(|p| p.0 == n));
        let k = if cycle {
            let k = remembered.unwrap_or(0);
            let (name, vals, cur) = &props[k];
            let next = if back { previous_index(vals.len(), Some(*cur)) } else { (cur + 1) % vals.len() };
            let ns = self.v().set(state, name, &vals[next]);
            self.set_block(p, ns, crate::blocks::flag::CLIENTS | crate::blocks::flag::KNOWN_SHAPE);
            let msg = Component::translate(
                "item.minecraft.debug_stick.update",
                vec![Arg::Text(name.clone()), Arg::Text(vals[next].clone())],
            );
            let _ = self.send(Event::ActionBar(msg));
            return true;
        } else if back {
            previous_index(props.len(), remembered)
        } else {
            remembered.map_or(0, |r| (r + 1) % props.len())
        };
        let (name, vals, cur) = &props[k];
        // Component updated: the full map (block → property), this block replaced.
        map.retain(|(b, _)| *b != block);
        map.push((block.clone(), rc_proto::nbt::Nbt::String(name.clone())));
        if let Some(st) = &mut self.slots[slot] {
            crate::items::set_component(st, "minecraft:debug_stick_state", rc_proto::nbt::Nbt::Compound(map));
        }
        let msg = Component::translate(
            "item.minecraft.debug_stick.select",
            vec![Arg::Text(name.clone()), Arg::Text(vals[*cur].clone())],
        );
        let _ = self.send(Event::ActionBar(msg));
        self.broadcast_menus();
        true
    }

    /// Knowledge book: consumed first (even without a recipe), then its recipes
    /// (`recipes` component) learned, and the "used" statistic. An unknown recipe
    /// makes everything fail after the consumption (the book is lost), like in the game.
    fn knowledge_book_use(&mut self, slot: usize, s: &Stack) {
        let ids: Vec<String> = crate::items::component(s, "minecraft:recipes")
            .map(|n| crate::nbtio::list(Some(n)).iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        if !self.infinite_materials() {
            let mut rest = s.clone();
            rest.count -= 1;
            self.slots[slot] = (rest.count > 0).then_some(rest);
        }
        self.broadcast_menus();
        if ids.is_empty() || ids.iter().any(|id| self.life.recipes.index(id).is_none()) {
            return;
        }
        self.award_recipes(&ids);
        self.award_stat("minecraft:used", &s.id, 1);
    }

    /// Goat horn: prolonged use of the instrument's duration, instrument sound
    /// attached to the player (records category, volume range / 16), `instrument_play` vibration,
    /// possible wear, cooldown of the same duration, "used" statistic. The instrument is
    /// a registry id or an inline definition.
    fn instrument_use(&mut self, hand: i32, slot: usize, s: &Stack, inst: &Value) {
        let def = match inst {
            Value::String(id) => match self.life.pack.get("instrument", id) {
                Some(v) => (*v).clone(),
                None => return,
            },
            o => o.clone(),
        };
        let duration = def["use_duration"].as_f64().unwrap_or(0.0) as f32;
        let ticks = (duration * 20.0).floor() as i32;
        self.start_using(hand, ticks);
        let sound = match &def["sound_event"] {
            Value::String(t) => Some(t.clone()),
            o => o["sound_id"].as_str().map(str::to_owned),
        };
        let volume = def["range"].as_f64().unwrap_or(0.0) as f32 / 16.0;
        // The game's client plays this sound itself for the one blowing (they are excluded from the
        // broadcast); our client does not predict it yet: also sent to the player.
        if let Some(sid) = sound.and_then(|n| self.sound_id(&n)) {
            let _ = self.send(Event::Sound(rc_proto_v777::play::SoundPacket {
                sound: rc_proto_v777::play::SoundRef::Id(sid),
                category: 2,
                pos: None,
                entity: Some(self.player_id),
                volume,
                pitch: 1.0,
                seed: crate::vitals::sound_seed(),
            }));
        }
        let pos = self.pos;
        self.game_event_at("minecraft:instrument_play", pos, Some(crate::mobs::mob::Ent::Player));
        let wear = def["durability_damage"].as_i64().unwrap_or(0) as i32;
        if wear > 0 {
            self.hurt_item(slot, wear);
        }
        if duration > 0.0 {
            let group = self
                .comp_json(s, "minecraft:use_cooldown")
                .and_then(|c| c["cooldown_group"].as_str().map(str::to_owned))
                .unwrap_or_else(|| s.id.clone());
            self.add_cooldown(&group, ticks);
        }
        self.award_stat("minecraft:used", &s.id, 1);
    }

    /// Use tick before the countdown: the bundle drops a stack on the first tick,
    /// then one tick out of two after the first 10.
    pub(crate) fn w8c_use_tick(&mut self, hand: i32, left: i32) {
        let slot = self.hand_slot(hand);
        let Some(mut bag_stack) = self.slots[slot].clone() else { return };
        if bag_stack.id == "minecraft:brush" {
            self.brush_use_tick(hand, left); // G5-F10
            return;
        }
        let Some(list) = crate::menus::bundle::contents(&self.life.items, &bag_stack) else { return };
        let first = left == BUNDLE_USE_TICKS;
        if !(first || (left < BUNDLE_USE_TICKS - 10 && left % 2 == 0)) || list.is_empty() {
            return;
        }
        let id = bag_stack.id.clone();
        let mut bag = crate::menus::bundle::Bag::of(&self.life.items, list);
        let Some(out) = bag.remove_one(&self.life.items, -1) else { return };
        bag.store(&self.life.items, &mut bag_stack);
        self.slots[slot] = Some(bag_stack);
        self.drop_from_player_with(out, false); // thrown from the hand (`drop(…, true)`)
        let p = self.pos.map(f64::floor);
        let at = [p[0] + 0.5, p[1] + 0.5, p[2] + 0.5];
        let pitch = 0.8 + self.life.rng.next_float() * 0.4;
        self.play_sound("minecraft:item.bundle.drop_contents", 7, at, 0.8, pitch);
        self.award_stat("minecraft:used", &id, 1);
        self.broadcast_menus();
    }

    /// Equipment worn by right click.
    fn swap_equipment(&mut self, hand_slot: usize, s: &Stack, eq: &Value) -> bool {
        let target = match eq["slot"].as_str().unwrap_or("") {
            "head" => 5,
            "chest" => 6,
            "legs" => 7,
            "feet" => 8,
            "offhand" => 45,
            "mainhand" => 36 + self.held as usize,
            _ => return false,
        };
        if let Some(allowed) = eq.get("allowed_entities")
            && !self.entity_set_has(allowed, "minecraft:player")
        {
            return false;
        }
        let worn = self.slots[target].clone();
        let creative = self.game_mode == 1;
        let locked = worn.as_ref().is_some_and(|w| self.life.items.enchantment(w, "minecraft:binding_curse") > 0);
        if (locked && !creative) || worn.as_ref().is_some_and(|w| crate::items::same(w, s)) {
            return true;
        }
        self.award_stat("minecraft:used", &s.id, 1);
        if s.count <= 1 {
            // Empty hand without a worn piece in survival (the held stack leaves in full).
            let to_hand = worn.clone().or_else(|| creative.then(|| s.clone()));
            self.set_equipment(target, Some(s.clone()), worn.clone());
            self.slots[hand_slot] = to_hand;
        } else {
            let mut one = s.clone();
            one.count = 1;
            if !creative {
                let mut rest = s.clone();
                rest.count -= 1;
                self.slots[hand_slot] = Some(rest);
            }
            self.set_equipment(target, Some(one), worn.clone());
            if let Some(w) = worn {
                let held = self.held as usize;
                if let Some(rest) = crate::menus::inventory_add(&mut self.slots, &self.life.items, held, w, creative) {
                    self.drop_from_player_with(rest, false);
                }
            }
        }
        self.broadcast_menus();
        true
    }

    /// Equipment slot set (placement into an equipment slot → the reaction to equipping an item): equipment sound and
    /// `equip`/`unequip` event if the stack changes.
    pub(crate) fn set_equipment(&mut self, slot: usize, new: Option<Stack>, old: Option<Stack>) {
        self.slots[slot] = new.clone();
        self.on_equip_item(slot, old.as_ref(), new.as_ref());
    }

    /// The reaction to equipping a player's item: the item's sound if it goes into its slot, game event for
    /// armor slots.
    pub(crate) fn on_equip_item(&mut self, slot: usize, old: Option<&Stack>, new: Option<&Stack>) {
        if self.game_mode == 3 {
            return;
        }
        let same = match (old, new) {
            (None, None) => true,
            (Some(a), Some(b)) => crate::items::same(a, b),
            _ => false,
        };
        if same {
            return;
        }
        let name = match slot {
            5 => "head",
            6 => "chest",
            7 => "legs",
            8 => "feet",
            45 => "offhand",
            _ => "mainhand",
        };
        let eq = new.and_then(|n| self.comp_json(n, "minecraft:equippable"));
        if let Some(eq) = &eq
            && eq["slot"].as_str() == Some(name)
        {
            let snd = eq["equip_sound"].as_str().unwrap_or("minecraft:item.armor.equip_generic").to_owned();
            self.play_sound(&snd, 7, self.pos, 1.0, 1.0);
        }
        if (5..9).contains(&slot) {
            let ev = if eq.is_some() { "minecraft:equip" } else { "minecraft:unequip" };
            let pos = self.pos;
            self.game_event_at(ev, pos, Some(crate::mobs::mob::Ent::Player));
        }
    }

    /// Does a set of entity types (`#tag`, id or list) contain `ty`?
    pub(crate) fn entity_set_has(&self, set: &Value, ty: &str) -> bool {
        match set {
            Value::String(x) => match x.strip_prefix('#') {
                Some(tag) => self.entity_type_in(ty, tag),
                None => x == ty,
            },
            Value::Array(a) => a.iter().any(|v| self.entity_set_has(v, ty)),
            _ => false,
        }
    }

    /// Aim from the eyes: block outlines, plus the fluid sources
    /// if `sources`. Slot and face hit.
    pub(crate) fn pov_hit(&self, sources: bool) -> Option<([i32; 3], usize)> {
        let from = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let view = crate::mobs::cubes::view_vector(self.pitch, self.yaw);
        let range = self.player_attribute("minecraft:block_interaction_range");
        let to = [from[0] + view[0] * range, from[1] + view[1] * range, from[2] + view[2] * range];
        if from == to {
            return None;
        }
        let lerp = |k: f64, a: f64, b: f64| a + k * (b - a);
        let te: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, to[i], from[i]));
        let fe: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, from[i], to[i]));
        let mut cell = fe.map(|v| v.floor() as i32);
        if let Some(n) = self.pov_cell(cell, from, to, sources) {
            return Some((cell, face_of(n)));
        }
        let d: [f64; 3] = std::array::from_fn(|i| te[i] - fe[i]);
        let sign = d.map(|v| {
            if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            }
        });
        let delta: [f64; 3] = std::array::from_fn(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / d[i] });
        let frac = |v: f64| v - v.floor();
        let mut t: [f64; 3] =
            std::array::from_fn(|i| delta[i] * if sign[i] > 0 { 1.0 - frac(fe[i]) } else { frac(fe[i]) });
        while t[0] <= 1.0 || t[1] <= 1.0 || t[2] <= 1.0 {
            let a = if t[0] < t[1] {
                if t[0] < t[2] { 0 } else { 2 }
            } else if t[1] < t[2] {
                1
            } else {
                2
            };
            cell[a] += sign[a];
            t[a] += delta[a];
            if let Some(n) = self.pov_cell(cell, from, to, sources) {
                return Some((cell, face_of(n)));
            }
        }
        None
    }

    /// A cell of the aim: block outline and shape of the source fluid, the closest wins
    /// (the block on a tie).
    fn pov_cell(&self, cell: [i32; 3], from: [f64; 3], to: [f64; 3], sources: bool) -> Option<[i32; 3]> {
        let s = self.get(cell);
        let outline = self.data.state(s).map_or(&[][..], |i| self.data.shape(i.outline));
        let block = clip_boxes(outline, cell, from, to);
        let fluid = if sources {
            let f = self.fluid_of(s);
            if f.kind != 0 && f.source {
                let above = self.fluid_at([cell[0], cell[1] + 1, cell[2]]);
                let h = if above.kind == f.kind { 1.0 } else { f64::from(f32::from(f.amount) / 9.0) };
                let b = rc_data::Aabb { min: [0.0; 3], max: [1.0, h, 1.0] };
                clip_boxes(&[b], cell, from, to)
            } else {
                None
            }
        } else {
            None
        };
        match (block, fluid) {
            (Some(b), Some(f)) => Some(if b.0 <= f.0 { b.1 } else { f.1 }),
            (Some(b), None) => Some(b.1),
            (None, Some(f)) => Some(f.1),
            (None, None) => None,
        }
    }

    /// `kind` 0 empty bucket, 1 water, 2 lava. `None`: nothing aimed at (the use passes).
    fn bucket_use(&mut self, slot: usize, kind: u8) -> Option<bool> {
        let (pos, face) = self.pov_hit(kind == 0)?;
        let d = FACE_DIRS[face];
        let side = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
        if !self.within_block_range(pos, 1.0) || self.game_mode == 2 {
            return Some(false);
        }
        let Some(stack) = self.slots[slot].clone() else { return Some(false) };
        if kind != 0 {
            if !self.may_interact(pos) {
                return Some(false);
            }
            let clicked = self.get(pos);
            let container = self.v().sim(clicked) & rc_data::sim_bits::LIQUID_CONTAINER != 0;
            let place = if container && kind == 1 { pos } else { side };
            if self.bucket_empty(kind, place, Some(side)) {
                self.trig_location("minecraft:placed_block", place, Some(&stack));
                self.award_stat("minecraft:used", &stack.id, 1);
                if self.game_mode != 1 {
                    self.filled_result(slot, Stack { id: "minecraft:bucket".into(), count: 1, components: None }, true);
                }
                self.send_block(pos);
                self.send_block(side);
                return Some(true);
            }
            self.send_block(pos);
            self.send_block(side);
            return Some(false);
        }
        // Empty bucket: pickup.
        let s = self.get(pos);
        let name = self.data.block_of(s).map(|b| b.name.clone()).unwrap_or_default();
        let f = self.fluid_of(s);
        let taken = if self.v().has(s, "waterlogged") {
            if self.v().get_bool(s, "waterlogged") {
                let ns = self.v().set_bool(s, "waterlogged", false);
                self.set_block(pos, ns, crate::blocks::flag::ALL);
                if !self.can_survive(ns, pos) {
                    self.destroy_block(pos, true, 512);
                }
                Some(("minecraft:water_bucket", "minecraft:item.bucket.fill"))
            } else {
                None
            }
        } else if (name == "minecraft:water" || name == "minecraft:lava") && f.source {
            let air = self.default_state("minecraft:air");
            self.set_block(pos, air, crate::blocks::flag::ALL | 8);
            Some(if f.kind == 2 {
                ("minecraft:lava_bucket", "minecraft:item.bucket.fill_lava")
            } else {
                ("minecraft:water_bucket", "minecraft:item.bucket.fill")
            })
        } else if name == "minecraft:powder_snow" {
            let air = self.default_state("minecraft:air");
            self.set_block(pos, air, crate::blocks::flag::ALL | 8);
            if !self.v().is_air(s) {
                self.level_event(2001, pos, s as i32);
            }
            Some(("minecraft:powder_snow_bucket", "minecraft:item.bucket.fill_powder_snow"))
        } else {
            None
        };
        let Some((full, sound)) = taken else {
            self.send_block(pos);
            return Some(false);
        };
        self.award_stat("minecraft:used", "minecraft:bucket", 1);
        // The game's client predicts this sound for the one acting: sent here to the player.
        self.play_sound(sound, 7, self.pos, 1.0, 1.0);
        self.game_event_block("minecraft:fluid_pickup", pos, Some(crate::mobs::mob::Ent::Player), None);
        let filled = Stack { id: full.into(), count: 1, components: None };
        self.filled_result(slot, filled.clone(), true);
        self.trig_filled_bucket(&filled);
        self.send_block(pos);
        Some(true)
    }

    /// Emptying of the contents of a water (1) or lava (2) bucket carried by the player; `retry`: fallback
    /// slot on the face hit when the aimed slot does not receive the fluid.
    fn bucket_empty(&mut self, kind: u8, p: [i32; 3], retry: Option<[i32; 3]>) -> bool {
        let s = self.get(p);
        let v = self.v();
        let name = self.data.block_of(s).map(|b| b.name.clone()).unwrap_or_default();
        let portal = name == "minecraft:end_portal" || name == "minecraft:end_gateway";
        let may_replace = !portal && (v.replaceable(s) || !v.legacy_solid(s));
        let container = v.sim(s) & rc_data::sim_bits::LIQUID_CONTAINER != 0;
        let loggable = v.has(s, "waterlogged");
        let already = v.get_bool(s, "waterlogged");
        let can_hold = container && loggable && kind == 1 && !already && self.accepts_water(s, self.game_mode == 1);
        let place_liquid = may_replace || can_hold;
        let inside = v.is_air(s) || (place_liquid && (!self.sneaking || retry.is_none()));
        if !inside {
            return retry.is_some_and(|q| self.bucket_empty(kind, q, None));
        }
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        if kind == 1 && self.dim.name == "minecraft:the_nether" {
            let r = &mut self.mobs.rng;
            let pitch = 2.6 + (r.next_float() - r.next_float()) * 0.8;
            self.play_sound("minecraft:block.fire.extinguish", 4, at, 0.5, pitch);
            let corner = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
            self.particles("minecraft:large_smoke", corner, [1.0, 1.0, 1.0], 0.0, 8);
            return true;
        }
        if container && kind == 1 {
            if can_hold {
                self.place_water_in(p, s); // G6-F01: campfire, dried ghast
            }
        } else {
            let liquid = name == "minecraft:water" || name == "minecraft:lava";
            if may_replace && !liquid {
                self.destroy_block(p, true, 512);
            }
            let f = crate::blocks::level::Fluid { kind, source: true, amount: 8, falling: false };
            let ns = self.legacy_block(f);
            let was_source = self.fluid_of(s).source;
            if !self.set_block(p, ns, crate::blocks::flag::ALL | 8) && !was_source {
                return false;
            }
        }
        let snd = if kind == 2 { "minecraft:item.bucket.empty_lava" } else { "minecraft:item.bucket.empty" };
        // The game's client predicts this sound for the one acting: sent here to the player.
        self.play_sound(snd, 4, at, 1.0, 1.0);
        self.game_event_block("minecraft:fluid_place", p, Some(crate::mobs::mob::Ent::Player), None);
        true
    }

    // ---------- Shears on a mob's equipment ----------

    /// Shears on a mob (before its own interaction): the first worn piece
    /// whose equipment can be sheared (saddle, wolf armor, harness…) falls off, except for a curse
    /// of binding outside creative. Wolf: only by its owner; the others: without a passenger.
    /// Wear of the shears, `shear` event, item dropped at the passengers' mean point,
    /// `player_sheared_equipment` trigger, equipment shearing sound.
    pub(crate) fn shear_equipment(&mut self, m: &mut crate::mobs::mob::Mob, slot: usize) -> bool {
        use crate::mobs::species::Kind;
        if self.slots[slot].as_ref().is_none_or(|s| s.id != "minecraft:shears") || self.sneaking {
            return false;
        }
        let allowed = match m.kind {
            Kind::Wolf => m.owner == Some(self.uuid),
            _ => m.mount.passengers.is_empty(),
        };
        if !allowed {
            return false;
        }
        for i in 0..m.equipment.len() {
            let Some(worn) = m.equipment[i].clone() else { continue };
            let Some(eq) = self.comp_json(&worn, "minecraft:equippable") else { continue };
            let locked = self.life.items.enchantment(&worn, "minecraft:binding_curse") > 0 && self.game_mode != 1;
            if !eq["can_be_sheared"].as_bool().unwrap_or(false) || locked {
                continue;
            }
            self.hurt_item(slot, 1);
            let off = crate::mobs::mounts::riding::passenger_average(m);
            self.mount_set_slot(m, i, None);
            self.sheared_by_player(m);
            let r = &mut self.mobs.rng;
            let vel = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
            self.spawn_item([m.pos[0] + off[0], m.pos[1] + off[1], m.pos[2] + off[2]], vel, worn.clone(), 10);
            self.trig_sheared_equipment(&worn, crate::progress::pred::Who::Mob(m));
            let sound = match &eq["shearing_sound"] {
                Value::String(t) => t.clone(),
                o => o["sound_id"].as_str().unwrap_or("minecraft:item.shears.snip").to_owned(),
            };
            let cat = Self::mob_sound_source(m);
            self.world_sound(&sound, cat, m.pos, 1.0, 1.0);
            return true;
        }
        false
    }
}
