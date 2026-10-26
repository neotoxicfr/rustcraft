//! Paintings and frames: placement by the item, survival (support, collisions, attached
//! neighbours), breaking, item placed and rotated in a frame.

use rc_physics::Aabb;
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::{
    DKind, Use, base_fields, block_pos_of, dbl, dir_2d, dir_3d, flag, frame_box, kind_of, named, painting_box,
    put_after, value_2d,
};
use crate::blocks::{Dir, Kind, P, rel};
use crate::mobs::mob::Ent;
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

/// Item of the frame carrying a map (map identifier component).
pub(super) fn has_map(e: &Nbt) -> bool {
    e.get("Item").and_then(|i| i.get("components")).and_then(|c| c.get("minecraft:map_id")).is_some()
}

fn center(b: &Aabb) -> [f64; 3] {
    std::array::from_fn(|i| (b.min[i] + b.max[i]) / 2.0)
}

impl Server {
    /// Attachment block and direction (painting: 2D `facing`, south by default; frame: 3D `Facing`,
    /// down by default).
    pub(super) fn hang_place(&self, k: DKind, e: &Nbt) -> (P, Dir) {
        let d = if k == DKind::Painting {
            int(e, "facing").map_or(Dir::South, dir_2d)
        } else {
            int(e, "Facing").map_or(Dir::Down, dir_3d)
        };
        (block_pos_of(e), d)
    }

    /// Painting variants of the registry (identifier order).
    fn painting_entries(&self) -> &[String] {
        match self.data.registry("minecraft:painting_variant") {
            Some(r) => &r.entries,
            None => self.regs.entries("minecraft:painting_variant"),
        }
    }

    pub(super) fn painting_variant_index(&self, e: &Nbt) -> Option<usize> {
        let v = e.get("variant").and_then(Nbt::as_str)?;
        self.painting_entries().iter().position(|n| n == v)
    }

    /// Width and height of a variant (blocks).
    fn variant_size(&self, name: &str) -> Option<(i32, i32)> {
        let short = name.trim_start_matches("minecraft:");
        self.data
            .creative
            .paintings
            .iter()
            .find(|(n, _, _)| n.trim_start_matches("minecraft:") == short)
            .map(|&(_, w, h)| (i32::from(w), i32::from(h)))
    }

    /// Size of the painting; without a readable variant, the first of the registry (default value).
    pub(super) fn painting_size(&self, e: &Nbt) -> (i32, i32) {
        let name = e
            .get("variant")
            .and_then(Nbt::as_str)
            .map(str::to_owned)
            .or_else(|| self.painting_entries().first().cloned())
            .unwrap_or_default();
        self.variant_size(&name).unwrap_or((1, 1))
    }

    /// Box colliding with a block or outside the border.
    pub(super) fn box_hits_level(&self, b: &Aabb) -> bool {
        let k = self.border_bounds();
        if b.min[0] < k[0] || b.max[0] > k[1] || b.min[2] < k[2] || b.max[2] > k[3] {
            return true;
        }
        let mut boxes = Vec::new();
        rc_physics::collide::collect(&self.world(), b, &mut boxes);
        boxes.iter().any(|s| s.intersects(b))
    }

    /// All blocks of the support area are full or diodes.
    fn supported(&self, b: &Aabb) -> bool {
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.floor() as i32);
        let v = self.v();
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    let s = self.get([x, y, z]);
                    if !(v.legacy_solid(s) || matches!(v.kind(s), Kind::Repeater | Kind::Comparator)) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// No other attached item in the box: neither of the same direction, nor (except
    /// `same_type_ok`) of the same kind.
    fn hang_coexist(&self, id: i32, k: DKind, d: Dir, pop: &Aabb, same_type_ok: bool) -> bool {
        // Attachment box of at most 4 blocks per side: chunks with a 2-block margin.
        let c0 = ((pop.min[0] - 2.0).floor() as i32) >> 4;
        let c1 = ((pop.max[0] + 2.0).floor() as i32) >> 4;
        let z0 = ((pop.min[2] - 2.0).floor() as i32) >> 4;
        let z1 = ((pop.max[2] + 2.0).floor() as i32) >> 4;
        for cx in c0..=c1 {
            for cz in z0..=z1 {
                let Some(list) = self.entities.get(&(cx, cz)) else { continue };
                for (oid, o) in list {
                    let Some(ok) = kind_of(o).filter(|k| k.hanging()) else { continue };
                    if *oid == id {
                        continue;
                    }
                    let od = self.hang_place(ok, o).1;
                    if !((!same_type_ok && ok == k) || od == d) {
                        continue;
                    }
                    if self.deco_box(o).is_some_and(|b| b.intersects(pop)) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// `survives` of a painting or a frame.
    pub(super) fn hang_survives(&self, id: i32, k: DKind, e: &Nbt) -> bool {
        let (pos, d) = self.hang_place(k, e);
        if k == DKind::Painting {
            let (w, h) = self.painting_size(e);
            let b = painting_box(pos, d, w, h);
            if self.box_hits_level(&b) {
                return false;
            }
            // Support area: box moved back half a block into the wall, trimmed by 1e-7.
            let s = d.step().map(|v| f64::from(v as f32 * -0.5f32));
            let sup = b.offset(s[0], s[1], s[2]).deflate(1.0e-7);
            return self.supported(&sup) && self.hang_coexist(id, k, d, &b, false);
        }
        if flag(e, "Fixed") {
            return true;
        }
        let pop = frame_box(pos, d, false);
        if self.box_hits_level(&pop) {
            return false;
        }
        let s = self.get(rel(pos, d.opposite()));
        let v = self.v();
        let ok = v.legacy_solid(s) || (d.horizontal() && matches!(v.kind(s), Kind::Repeater | Kind::Comparator));
        ok && self.hang_coexist(id, k, d, &pop, true)
    }

    /// Compound of a new painting or frame attached at `pos`, face `d`.
    fn hang_nbt(&self, k: DKind, pos: P, d: Dir, variant: Option<&str>, uuid: u128) -> Nbt {
        let b = match k {
            DKind::Painting => {
                let (w, h) = variant.and_then(|v| self.variant_size(v)).unwrap_or((1, 1));
                painting_box(pos, d, w, h)
            }
            _ => frame_box(pos, d, false),
        };
        // Rotation: painting and wall frame turned towards the face; frame on the ground or ceiling
        // lying down (pitch ∓90).
        let (yaw, pitch) = if d.horizontal() {
            (value_2d(d) as f32 * 90.0, 0.0f32)
        } else {
            (0.0, if d == Dir::Up { -90.0 } else { 90.0 })
        };
        let mut f = base_fields(k, center(&b), yaw, pitch, uuid);
        if k == DKind::Painting {
            // Game order: direction, attachment block, variant.
            f.push(("facing".into(), Nbt::Byte(value_2d(d) as i8)));
            f.push(("block_pos".into(), Nbt::IntArray(pos.to_vec())));
            if let Some(v) = variant {
                f.push(("variant".into(), Nbt::String(v.to_owned())));
            }
        } else {
            f.push(("block_pos".into(), Nbt::IntArray(pos.to_vec())));
            f.push(("ItemRotation".into(), Nbt::Byte(0)));
            f.push(("ItemDropChance".into(), Nbt::Float(1.0)));
            f.push(("Facing".into(), Nbt::Byte(d as i8)));
            f.push(("Invisible".into(), Nbt::Byte(0)));
            f.push(("Fixed".into(), Nbt::Byte(0)));
        }
        Nbt::Compound(f)
    }

    /// The placement of an attached entity (painting, frame, glow frame); `None` if the item is
    /// not one.
    pub(crate) fn hanging_use_on(&mut self, slot: usize, st: &Stack, clicked: P, face: Dir) -> Option<Use> {
        let k = match st.id.as_str() {
            "minecraft:painting" => DKind::Painting,
            "minecraft:item_frame" => DKind::Frame,
            "minecraft:glow_item_frame" => DKind::GlowFrame,
            _ => return None,
        };
        let pos = rel(clicked, face);
        // Painting on a wall only; frame within the world height.
        let may = if k == DKind::Painting { face.horizontal() } else { self.in_build_height(pos[1]) };
        if !may {
            return Some(Use::Fail);
        }
        let mut e = if k == DKind::Painting {
            // Placeable variants that hold, then the largest ones, draw.
            let placeable: Vec<String> = self
                .painting_entries()
                .iter()
                .filter(|n| self.p_tag("minecraft:painting_variant", "minecraft:placeable", n))
                .cloned()
                .collect();
            if placeable.is_empty() {
                return Some(Use::Consume);
            }
            let fits: Vec<(String, i32)> = placeable
                .into_iter()
                .filter_map(|v| {
                    let n = self.hang_nbt(k, pos, face, Some(&v), 0);
                    let (w, h) = self.variant_size(&v)?;
                    self.hang_survives(-1, k, &n).then_some((v, w * h))
                })
                .collect();
            let Some(max) = fits.iter().map(|f| f.1).max() else { return Some(Use::Consume) };
            let best: Vec<&String> = fits.iter().filter(|f| f.1 == max).map(|f| &f.0).collect();
            let pick = best[self.life.rng.next_int_bound(best.len() as i32) as usize].clone();
            let u = self.deco_uuid();
            self.hang_nbt(k, pos, face, Some(&pick), u)
        } else {
            let u = self.deco_uuid();
            self.hang_nbt(k, pos, face, None, u)
        };
        // Stack settings: name, variant, entity data.
        self.apply_stack_config(&mut e, st);
        if k == DKind::Painting && let Some(v) = crate::items::component(st, "minecraft:painting/variant") {
            if let Some(v) = v.as_str() {
                nbtio::set(&mut e, "variant", Nbt::String(v.to_owned()));
            }
            let (w, h) = self.painting_size(&e);
            let b = painting_box(pos, face, w, h);
            nbtio::set(&mut e, "Pos", dbl(center(&b)));
        }
        if !self.hang_survives(-1, k, &e) {
            return Some(Use::Consume);
        }
        let sound = match k {
            DKind::Painting => "minecraft:entity.painting.place",
            DKind::Frame => "minecraft:entity.item_frame.place",
            _ => "minecraft:entity.glow_item_frame.place",
        };
        self.deco_sound(&e, sound, 6, 1.0, 1.0);
        let at = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.game_event_at("minecraft:entity_place", at, Some(Ent::Player));
        self.deco_spawn(e);
        self.consume_held(slot);
        Some(Use::Success)
    }

    /// Custom name and entity data of the stack.
    pub(super) fn apply_stack_config(&self, e: &mut Nbt, st: &Stack) {
        if let Some(n) = crate::items::component(st, "minecraft:custom_name") {
            put_after(e, "UUID", "CustomName", n.clone());
        }
        if let Some(d) = crate::items::component(st, "minecraft:entity_data")
            && d.get("id").and_then(Nbt::as_str) == e.get("id").and_then(Nbt::as_str)
        {
            let uuid = e.get("UUID").cloned();
            let mut add = d.clone();
            nbtio::remove(&mut add, "id");
            crate::commands::cmds_data::merge(e, &add);
            match uuid {
                Some(u) => nbtio::set(e, "UUID", u),
                None => nbtio::remove(e, "UUID"),
            }
        }
    }

    /// Hit by a player on an attached entity (from a player attack).
    pub(super) fn attached_hurt_by_player(&mut self, id: i32, k: DKind, e: &Nbt) {
        let creative = self.game_mode == 1;
        let hurt = if flag(e, "Invulnerable") && !creative {
            false
        } else if k == DKind::Cushion {
            // Cushion: refused to someone who cannot build.
            self.game_mode != 2 && self.attached_kill(id, e)
        } else if k.frame() {
            self.frame_hurt(id, k, e, creative)
        } else {
            self.attached_kill(id, e)
        };
        if !hurt {
            self.world_sound("minecraft:entity.player.attack.nodamage", 7, self.pos, 1.0, 1.0);
        }
    }

    /// Death of an attached entity and its loot (`kill` then dropping an item).
    pub(super) fn attached_kill(&mut self, id: i32, e: &Nbt) -> bool {
        self.attached_kill_by(id, e, Some(Ent::Player))
    }

    /// Death attributed to `by` (no attacker: the entity itself).
    pub(super) fn attached_kill_by(&mut self, id: i32, e: &Nbt, by: Option<Ent>) -> bool {
        let at = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.deco_remove(id);
        self.game_event_at("minecraft:entity_die", at, by);
        self.deco_drop_item(e, by);
        true
    }

    /// The damage taken by a frame on the server side from a player attack.
    fn frame_hurt(&mut self, id: i32, k: DKind, e: &Nbt, creative: bool) -> bool {
        if flag(e, "Fixed") {
            // Fixed: only creative breaks it; neither item nor frame drops.
            return creative && self.attached_kill(id, e);
        }
        let Some(item) = e.get("Item").and_then(Stack::from_nbt) else {
            return self.attached_kill(id, e);
        };
        // Item present: it drops alone, the frame stays.
        self.deco_edit(id, |n| nbtio::remove(n, "Item"));
        self.deco_data(id, vec![(9, Meta::Item(None))]);
        self.frame_drop_contents(e, Some(item), false, creative);
        let at = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.game_event_at("minecraft:block_change", at, Some(Ent::Player));
        let s = if k == DKind::GlowFrame {
            "minecraft:entity.glow_item_frame.remove_item"
        } else {
            "minecraft:entity.item_frame.remove_item"
        };
        self.deco_sound(e, s, 6, 1.0, 1.0);
        true
    }

    /// Break sound, frame and item dropped.
    pub(super) fn frame_broken(&mut self, k: DKind, e: &Nbt, by: Option<Ent>) {
        let creative = by == Some(Ent::Player) && self.game_mode == 1;
        let s = if k == DKind::GlowFrame { "minecraft:entity.glow_item_frame.break" } else { "minecraft:entity.item_frame.break" };
        self.deco_sound(e, s, 6, 1.0, 1.0);
        let item = e.get("Item").and_then(Stack::from_nbt);
        self.frame_drop_contents(e, item, true, creative);
        let at = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.game_event_at("minecraft:block_change", at, by);
    }

    /// Loot of a non-fixed frame.
    fn frame_drop_contents(&mut self, e: &Nbt, item: Option<Stack>, with_frame: bool, creative: bool) {
        if flag(e, "Fixed") {
            return;
        }
        // Item emptied: comparators notified, even without loot.
        let k = kind_of(e).unwrap_or(DKind::Frame);
        self.frame_signal_changed(k, e);
        if !self.rule("minecraft:entity_drops", true) || creative {
            return;
        }
        let d = self.hang_place(k, e).1;
        if with_frame {
            let id = if k == DKind::GlowFrame { "minecraft:glow_item_frame" } else { "minecraft:item_frame" };
            self.deco_drop_at(e, Some(d), named(id, e));
        }
        if let Some(it) = item {
            let chance = crate::nbtio::float(e, "ItemDropChance").unwrap_or(1.0);
            if self.life.rng.next_float() < chance {
                self.deco_drop_at(e, Some(d), it);
            }
        }
    }

    /// Item placed in an empty frame, otherwise rotation by an eighth.
    pub(super) fn frame_interact(&mut self, id: i32, k: DKind, slot: usize) -> bool {
        let Some(e) = self.deco_nbt(id).cloned() else { return false };
        if flag(&e, "Fixed") {
            return false;
        }
        let glow = k == DKind::GlowFrame;
        let at = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        if e.get("Item").and_then(Stack::from_nbt).is_none() {
            let Some(held) = self.slots[slot].clone() else { return false };
            let one = Stack { count: 1, ..held };
            let nbt = one.to_nbt(None);
            self.deco_edit(id, |n| nbtio::set(n, "Item", nbt));
            self.deco_data(id, vec![(9, Meta::Item(self.net(Some(&one))))]);
            let s = if glow { "minecraft:entity.glow_item_frame.add_item" } else { "minecraft:entity.item_frame.add_item" };
            self.deco_sound(&e, s, 6, 1.0, 1.0);
            self.frame_signal_changed(k, &e);
            self.game_event_at("minecraft:block_change", at, Some(Ent::Player));
            self.consume_held(slot);
            self.send_slot(slot);
            return true;
        }
        let s = if glow { "minecraft:entity.glow_item_frame.rotate_item" } else { "minecraft:entity.item_frame.rotate_item" };
        self.deco_sound(&e, s, 6, 1.0, 1.0);
        let r = ((int(&e, "ItemRotation").unwrap_or(0) + 1) % 8) as i8;
        self.deco_edit(id, |n| nbtio::set(n, "ItemRotation", Nbt::Byte(r)));
        self.deco_data(id, vec![(10, Meta::Int(i32::from(r)))]);
        self.frame_signal_changed(k, &e);
        self.game_event_at("minecraft:block_change", at, Some(Ent::Player));
        true
    }

    /// Item or rotation of a frame changed: neighbouring comparators notified
    /// (the neighbour update for the output signal from the frame's block).
    pub(super) fn frame_signal_changed(&mut self, k: DKind, e: &Nbt) {
        let p = self.hang_place(k, e).0;
        let air = self.v().block(0);
        self.update_neighbour_for_output_signal(p, air);
    }

    /// Signal of a frame read by a comparator through a conductive block: the only frame
    /// facing `d` whose box touches block `p`; item present: rotation % 8 + 1,
    /// otherwise 0. `None`: no frame, or several.
    pub(crate) fn frame_signal_at(&self, p: P, d: Dir) -> Option<i32> {
        let cell = Aabb { min: p.map(f64::from), max: p.map(|v| f64::from(v) + 1.0) };
        let (cx, cz) = (p[0] >> 4, p[2] >> 4);
        let mut found = None;
        for l in &self.deco.live {
            if l.removed || !l.kind.frame() || (l.chunk.0 - cx).abs() > 1 || (l.chunk.1 - cz).abs() > 1 {
                continue;
            }
            let Some(e) = self.deco_nbt(l.id) else { continue };
            if self.hang_place(l.kind, e).1 != d || !self.deco_box(e).is_some_and(|b| b.intersects(&cell)) {
                continue;
            }
            if found.is_some() {
                return None;
            }
            let item = e.get("Item").and_then(Stack::from_nbt).is_some();
            found = Some(if item { (int(e, "ItemRotation").unwrap_or(0) as i32) % 8 + 1 } else { 0 });
        }
        found
    }
}
