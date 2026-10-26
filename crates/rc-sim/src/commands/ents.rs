//! Uniform access to singleplayer entities for commands: player, simulated mobs (S4),
//! dropped items and orbs (S2), other entities read from the world (frozen, NBT only).

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_text::{Component, HoverEvent};

use crate::nbtio;
use crate::server::Server;

/// Designated entity (network identifier, except the player).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum E {
    Player,
    Mob(i32),
    Item(i32),
    Orb(i32),
    Other(i32),
}

/// Active effect: identifier (`minecraft:speed`), amplifier, duration (−1 infinite), flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub id: String,
    pub amplifier: i32,
    pub duration: i32,
    pub ambient: bool,
    pub particles: bool,
    pub icon: bool,
}

impl Effect {
    pub fn to_nbt(&self) -> Nbt {
        Nbt::Compound(vec![
            ("id".into(), Nbt::String(self.id.clone())),
            ("amplifier".into(), Nbt::Byte(self.amplifier as i8)),
            ("duration".into(), Nbt::Int(self.duration)),
            ("ambient".into(), Nbt::Byte(i8::from(self.ambient))),
            ("show_particles".into(), Nbt::Byte(i8::from(self.particles))),
            ("show_icon".into(), Nbt::Byte(i8::from(self.icon))),
        ])
    }

    pub fn from_nbt(v: &Nbt) -> Option<Effect> {
        let b = |k: &str, d: bool| nbtio::int(v, k).map_or(d, |x| x != 0);
        Some(Effect {
            id: v.get("id")?.as_str()?.to_owned(),
            amplifier: nbtio::int(v, "amplifier").unwrap_or(0) as i32,
            duration: nbtio::int(v, "duration").unwrap_or(0) as i32,
            ambient: b("ambient", false),
            particles: b("show_particles", true),
            icon: b("show_icon", true),
        })
    }
}

impl Server {
    /// All living entities of the open world, in order of addition (identifiers).
    pub fn all_ents(&self) -> Vec<E> {
        let mut out = Vec::new();
        if !self.life.vit.dead {
            out.push(E::Player);
        }
        let mut rest: Vec<E> = Vec::new();
        rest.extend(self.mobs.list.iter().flatten().filter(|m| m.alive()).map(|m| E::Mob(m.id)));
        rest.extend(self.life.drops.iter().map(|d| E::Item(d.id)));
        rest.extend(self.life.orbs.iter().map(|o| E::Orb(o.id)));
        for l in self.entities.values() {
            rest.extend(l.iter().map(|(id, _)| E::Other(*id)));
        }
        // F15: crystals, clouds, dragon balls, shulker bullets.
        rest.extend(self.mobs.end.snapshots.iter().map(|(id, _)| E::Other(*id)));
        // G5-F06: minecarts.
        rest.extend(self.carts.list.iter().flatten().filter(|c| !c.removed).map(|c| E::Other(c.id)));
        rest.sort_by_key(|e| self.ent_id(*e));
        out.extend(rest);
        out
    }

    pub fn ent_id(&self, e: E) -> i32 {
        match e {
            E::Player => self.player_id,
            E::Mob(i) | E::Item(i) | E::Orb(i) | E::Other(i) => i,
        }
    }

    fn other(&self, id: i32) -> Option<&Nbt> {
        self.entities
            .values()
            .flat_map(|l| l.iter())
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n)
            .or_else(|| self.end_other(id)) // F15
            .or_else(|| self.cart_ref(id).map(|c| &c.nbt)) // G5-F06: fields kept (name, tags)
    }

    fn other_mut(&mut self, id: i32) -> Option<&mut Nbt> {
        if self.cart_ref(id).is_some() {
            return self.cart_mut(id).map(|c| &mut c.nbt); // G5-F06
        }
        self.entities.values_mut().flat_map(|l| l.iter_mut()).find(|(i, _)| *i == id).map(|(_, n)| n)
    }

    /// G5-F06: designated minecart.
    pub(crate) fn cart_of(&self, e: E) -> Option<&crate::minecart::Cart> {
        match e {
            E::Other(i) => self.cart_ref(i),
            _ => None,
        }
    }

    pub fn ent_exists(&self, e: E) -> bool {
        match e {
            E::Player => true,
            E::Mob(i) => self.mobs.mob_ref_any(i).is_some_and(|m| m.alive()),
            E::Item(i) => self.life.drops.iter().any(|d| d.id == i),
            E::Orb(i) => self.life.orbs.iter().any(|d| d.id == i),
            E::Other(i) => self.other(i).is_some(),
        }
    }

    pub fn ent_type(&self, e: E) -> String {
        if let Some(c) = self.cart_of(e) {
            return c.kind.name().to_owned();
        }
        match e {
            E::Player => "minecraft:player".into(),
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or_else(String::new, |m| m.kind.info().name.to_owned()),
            E::Item(_) => "minecraft:item".into(),
            E::Orb(_) => "minecraft:experience_orb".into(),
            E::Other(i) => self.other(i).and_then(|n| n.get("id")?.as_str().map(str::to_owned)).unwrap_or_default(),
        }
    }

    pub fn e_pos(&self, e: E) -> [f64; 3] {
        if let Some(c) = self.cart_of(e) {
            return c.pos;
        }
        match e {
            E::Player => self.pos,
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or([0.0; 3], |m| m.pos),
            E::Item(i) => self.life.drops.iter().find(|d| d.id == i).map_or([0.0; 3], |d| d.pos),
            E::Orb(i) => self.life.orbs.iter().find(|d| d.id == i).map_or([0.0; 3], |d| d.pos),
            E::Other(i) => self.other(i).and_then(|n| nbtio::f64s::<3>(n.get("Pos"))).unwrap_or_default(),
        }
    }

    /// Yaw, pitch.
    pub fn ent_rot(&self, e: E) -> [f32; 2] {
        if let Some(c) = self.cart_of(e) {
            return [c.yaw, c.pitch];
        }
        match e {
            E::Player => [self.yaw, self.pitch],
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or([0.0; 2], |m| [m.yaw, m.pitch]),
            E::Item(_) | E::Orb(_) => [0.0; 2],
            E::Other(i) => self.other(i).and_then(|n| nbtio::f32s::<2>(n.get("Rotation"))).unwrap_or_default(),
        }
    }

    pub fn ent_uuid(&self, e: E) -> u128 {
        if let Some(c) = self.cart_of(e) {
            return c.uuid;
        }
        match e {
            E::Player => self.uuid,
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or(0, |m| m.uuid),
            E::Item(i) | E::Orb(i) => 0x5332_0000_0000_0000_0000_0000_0000_0000 | i as u32 as u128,
            E::Other(i) => self.other(i).and_then(|n| nbtio::uuid(n.get("UUID"))).unwrap_or(0),
        }
    }

    /// Width, height, eye height (version data, mobs: babies included).
    pub fn ent_dims(&self, e: E) -> [f32; 3] {
        match e {
            E::Player => {
                let (w, h, eye) = self.player_dims();
                [w as f32, h as f32, eye as f32]
            }
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or([0.6, 1.8, 1.62], |m| {
                let (w, h, eye) = crate::mobs::species::dims(m.kind, m.is_baby());
                [w, h, eye]
            }),
            _ => {
                let ty = self.ent_type(e);
                let idx =
                    self.data.registry("minecraft:entity_type").and_then(|r| r.entries.iter().position(|x| *x == ty));
                idx.and_then(|i| self.data.entity_dims.get(i)).map_or([0.25, 0.25, 0.2], |d| [d[0], d[1], d[2]])
            }
        }
    }

    pub fn ent_eye_height(&self, e: E) -> f64 {
        f64::from(self.ent_dims(e)[2])
    }

    pub fn e_bbox(&self, e: E) -> rc_physics::Aabb {
        // Painting and frames: box according to direction and size; small stand or marker.
        if let E::Other(i) = e
            && let Some(b) = self.other(i).and_then(|n| self.deco_box(n))
        {
            return b;
        }
        let p = self.e_pos(e);
        let [w, h, _] = self.ent_dims(e);
        let hw = f64::from(w) / 2.0;
        rc_physics::Aabb { min: [p[0] - hw, p[1], p[2] - hw], max: [p[0] + hw, p[1] + f64::from(h), p[2] + hw] }
    }

    /// Full NBT of the entity (the writing without identifier + `id`, except the player).
    pub fn ent_nbt(&self, e: E) -> Nbt {
        if let Some(c) = self.cart_of(e) {
            return self.cart_to_nbt(c);
        }
        match e {
            E::Player => {
                let mut n = self.player_nbt();
                nbtio::remove(&mut n, "DataVersion");
                n
            }
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or(Nbt::Compound(Vec::new()), |m| self.mob_nbt(m)),
            E::Item(i) => {
                let Some(d) = self.life.drops.iter().find(|d| d.id == i) else { return Nbt::Compound(Vec::new()) };
                let dbl = |v: [f64; 3]| Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect());
                Nbt::Compound(vec![
                    ("id".into(), Nbt::String("minecraft:item".into())),
                    ("Pos".into(), dbl(d.pos)),
                    ("Motion".into(), dbl(d.vel)),
                    ("Rotation".into(), Nbt::List(vec![Nbt::Float(d.yaw), Nbt::Float(0.0)])),
                    ("OnGround".into(), Nbt::Byte(i8::from(d.on_ground))),
                    ("Health".into(), Nbt::Short(d.health as i16)),
                    ("Age".into(), Nbt::Short(d.age as i16)),
                    ("PickupDelay".into(), Nbt::Short(d.pickup_delay as i16)),
                    ("UUID".into(), nbtio::uuid_nbt(self.ent_uuid(e))),
                    ("Item".into(), d.stack.to_nbt(None)),
                ])
            }
            E::Orb(i) => {
                let Some(d) = self.life.orbs.iter().find(|d| d.id == i) else { return Nbt::Compound(Vec::new()) };
                let dbl = |v: [f64; 3]| Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect());
                Nbt::Compound(vec![
                    ("id".into(), Nbt::String("minecraft:experience_orb".into())),
                    ("Pos".into(), dbl(d.pos)),
                    ("Motion".into(), dbl(d.vel)),
                    ("Health".into(), Nbt::Short(d.health as i16)),
                    ("Age".into(), Nbt::Short(d.age as i16)),
                    ("Value".into(), Nbt::Short(d.value as i16)),
                    ("Count".into(), Nbt::Int(d.count)),
                    ("UUID".into(), nbtio::uuid_nbt(self.ent_uuid(e))),
                ])
            }
            E::Other(i) => self.other(i).cloned().unwrap_or(Nbt::Compound(Vec::new())),
        }
    }

    /// Replaces the NBT of an entity (`data merge`); the player is refused by the caller.
    pub fn ent_set_nbt(&mut self, e: E, n: &Nbt) {
        // G5-F06: minecart read back (`load`), identifier and UUID kept.
        if let E::Other(i) = e
            && let Some((k, mut c)) = self.cart_take(i)
        {
            let mut full = n.clone();
            nbtio::set(&mut full, "UUID", nbtio::uuid_nbt(c.uuid));
            nbtio::remove(&mut full, "Passengers");
            c.load(self, &full);
            self.cart_put(k, c);
            return;
        }
        match e {
            E::Player => {
                // Only the player's tags and effects can be modified.
                if let Some(t) = n.get("Tags") {
                    nbtio::set(&mut self.pdata.root, "Tags", t.clone());
                }
            }
            E::Mob(i) => {
                if let Some(idx) = self.mobs.index_of(i)
                    && let Some(mut m) = self.mobs.list[idx].take()
                {
                    let before = m.pos;
                    // Reading back keeps the entity's UUID (a provided `UUID` is ignored).
                    let uuid = m.uuid;
                    self.load_mob(&mut m, n);
                    m.uuid = uuid;
                    if m.pos != before {
                        m.needs_sync = true;
                    }
                    m.sync_data();
                    self.mobs.list[idx] = Some(m);
                }
            }
            E::Item(i) => {
                if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == i) {
                    if let Some(s) = n.get("Item").and_then(crate::player::Stack::from_nbt) {
                        d.stack = s;
                    }
                    if let Some(p) = nbtio::f64s::<3>(n.get("Pos")) {
                        d.pos = p;
                    }
                    if let Some(v) = nbtio::int(n, "PickupDelay") {
                        d.pickup_delay = v as i32;
                    }
                    if let Some(v) = nbtio::int(n, "Age") {
                        d.age = v as i32;
                    }
                    if let Some(v) = nbtio::int(n, "Health") {
                        d.health = v as i32;
                    }
                }
            }
            E::Orb(i) => {
                if let Some(o) = self.life.orbs.iter_mut().find(|o| o.id == i)
                    && let Some(v) = nbtio::int(n, "Health")
                {
                    o.health = v as i32;
                }
            }
            E::Other(i) => {
                let mut new = n.clone();
                if let Some(o) = self.other_mut(i) {
                    // Identifier and UUID kept (`load` does not change them).
                    for k in ["id", "UUID"] {
                        if let Some(v) = o.get(k).cloned() {
                            nbtio::set(&mut new, k, v);
                        }
                    }
                    *o = new.clone();
                }
                self.send_entity_meta(i, &new);
            }
        }
    }

    /// Tags (`Tags`).
    pub fn ent_tags(&self, e: E) -> Vec<String> {
        let n = match e {
            E::Player => self.pdata.root.get("Tags").cloned(),
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| m.nbt.as_ref()?.get("Tags").cloned()),
            E::Other(i) => self.other(i).and_then(|n| n.get("Tags").cloned()),
            _ => None,
        };
        nbtio::list(n.as_ref()).iter().filter_map(|t| t.as_str().map(str::to_owned)).collect()
    }

    pub fn ent_set_tags(&mut self, e: E, tags: &[String]) {
        let v = Nbt::List(tags.iter().map(|t| Nbt::String(t.clone())).collect());
        match e {
            E::Player => nbtio::set(&mut self.pdata.root, "Tags", v),
            E::Mob(i) => {
                if let Some(m) = self.mobs.get_mut(i) {
                    let n = m.nbt.get_or_insert_with(|| Nbt::Compound(Vec::new()));
                    nbtio::set(n, "Tags", v);
                }
            }
            E::Other(i) => {
                if let Some(o) = self.other_mut(i) {
                    nbtio::set(o, "Tags", v);
                }
            }
            _ => {}
        }
    }

    /// Score holder name: the player's name, otherwise the UUID as text.
    pub fn ent_score_name(&self, e: E) -> String {
        match e {
            E::Player => self.name.clone(),
            _ => nbtio::uuid_text(self.ent_uuid(e)),
        }
    }

    /// Raw name (without style): custom name or translated name of the type.
    pub fn ent_custom_name(&self, e: E) -> Option<Component> {
        let n = match e {
            E::Player => return None,
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| m.custom_name.clone()),
            E::Other(i) => self.other(i).and_then(|n| n.get("CustomName").cloned()),
            E::Item(_) | E::Orb(_) => None,
        };
        n.as_ref().and_then(rc_text::from_nbt)
    }

    /// Simple name: player, custom name, carried item, or translated type.
    pub fn ent_name(&self, e: E) -> Component {
        if e == E::Player {
            return Component::text(self.name.clone());
        }
        if let Some(c) = self.ent_custom_name(e) {
            return c;
        }
        if let E::Item(i) = e
            && let Some(d) = self.life.drops.iter().find(|d| d.id == i)
        {
            return self.stack_name(&d.stack);
        }
        let ty = self.ent_type(e);
        Component::translate(format!("entity.{}", ty.replace(':', ".")), Vec::new())
    }

    /// Displayed name: team, "entity" hover, insertion (UUID or name),
    /// "/tell" click for a player.
    pub fn ent_display(&self, e: E) -> Component {
        let base = self.ent_name(e);
        let score_name = self.ent_score_name(e);
        let mut c = self.cmd.scores.decorate(&score_name, base.clone());
        let hover_name = if e == E::Player { Some(Component::text(self.name.clone())) } else { Some(base) };
        c.style.hover = Some(std::sync::Arc::new(HoverEvent::Entity {
            id: self.ent_type(e),
            uuid: self.ent_uuid(e),
            name: hover_name,
        }));
        c.style.insertion = Some(std::sync::Arc::from(if e == E::Player {
            self.name.clone()
        } else {
            nbtio::uuid_text(self.ent_uuid(e))
        }));
        if e == E::Player {
            super::text::suggest_click(&mut c, format!("/tell {} ", self.name));
        }
        c
    }

    pub(crate) fn player_display_name(&self) -> Component {
        self.ent_display(E::Player)
    }

    /// Teleportation of an entity (position, rotation).
    pub fn ent_teleport(&mut self, e: E, pos: [f64; 3], rot: Option<[f32; 2]>) {
        if let E::Other(i) = e
            && let Some(c) = self.cart_mut(i)
        {
            c.pos = pos;
            c.old_pos = pos;
            if let Some([y, p]) = rot {
                c.yaw = y;
                c.pitch = p;
            }
            c.needs_sync = true;
            return;
        }
        match e {
            E::Player => {
                if let Some([y, p]) = rot {
                    self.yaw = y;
                    self.pitch = p;
                }
                self.teleport_to(pos);
                self.update_tracking();
            }
            E::Mob(i) => {
                if let Some(m) = self.mobs.get_mut(i) {
                    m.pos = pos;
                    m.old = pos;
                    if let Some([y, p]) = rot {
                        m.yaw = y;
                        m.pitch = p;
                        m.head_yaw = y;
                        m.body_yaw = y;
                    }
                    m.needs_sync = true;
                    m.nav.stop();
                }
            }
            E::Item(i) => {
                if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == i) {
                    d.pos = pos;
                }
            }
            E::Orb(i) => {
                if let Some(d) = self.life.orbs.iter_mut().find(|d| d.id == i) {
                    d.pos = pos;
                }
            }
            E::Other(i) => {
                if let Some(o) = self.other_mut(i) {
                    nbtio::set(o, "Pos", Nbt::List(pos.iter().map(|&x| Nbt::Double(x)).collect()));
                    if let Some([y, p]) = rot {
                        nbtio::set(o, "Rotation", Nbt::List(vec![Nbt::Float(y), Nbt::Float(p)]));
                    }
                }
                let [yaw, pitch] = rot.unwrap_or([0.0, 0.0]);
                let _ = self.send(Event::Entity(EntityEvent::Teleport(rc_proto_v777::entity::TeleportEntity {
                    id: i,
                    pos,
                    velocity: [0.0; 3],
                    yaw,
                    pitch,
                    relative: 0,
                    on_ground: false,
                })));
            }
        }
    }

    /// Teleportation by a command (`tp`), like the game: `~`/`^` axes and absent or
    /// relative rotation sent to the player as relative flags (its momentum is kept on those axes, its view
    /// is not realigned), then vertical speed zero and on the ground (except when gliding),
    /// navigation stopped. `rot`: rotation resolved against the source, `None` = unchanged.
    pub fn command_teleport(&mut self, e: E, pos: [f64; 3], rel: [bool; 3], rot: Option<[f32; 2]>, rot_rel: [bool; 2]) {
        use crate::mobs::mth::wrap;
        use rc_proto_v777::play::relative as r;
        if e == E::Player {
            self.player_remove_vehicle(); // No longer a passenger
        }
        let cur = self.e_pos(e);
        let [cy, cp] = self.ent_rot(e);
        let [ty, tp] = rot.unwrap_or([cy, cp]);
        // Values sent: difference for a relative axis, absolute otherwise (rotations wrapped).
        let sent: [f64; 3] = std::array::from_fn(|k| if rel[k] { pos[k] - cur[k] } else { pos[k] });
        let ry = wrap(if rot_rel[0] { ty - cy } else { ty });
        let rp = wrap(if rot_rel[1] { tp - cp } else { tp });
        let abs: [f64; 3] = std::array::from_fn(|k| if rel[k] { cur[k] + sent[k] } else { sent[k] });
        let fy = if rot_rel[0] { cy + ry } else { ry };
        let fp = (if rot_rel[1] { cp + rp } else { rp }).clamp(-90.0, 90.0);
        if e == E::Player {
            let mut flags = 0;
            for (k, (p, d)) in [(r::X, r::DELTA_X), (r::Y, r::DELTA_Y), (r::Z, r::DELTA_Z)].into_iter().enumerate() {
                if rel[k] {
                    flags |= p | d;
                }
            }
            if rot_rel[0] {
                flags |= r::YAW;
            }
            if rot_rel[1] {
                flags |= r::PITCH;
            }
            self.yaw = fy;
            self.pitch = fp;
            self.teleport_id = if self.teleport_id == i32::MAX { 0 } else { self.teleport_id + 1 };
            self.pos = abs;
            self.teleport = Some((self.teleport_id, abs, self.tick));
            let _ = self.send(Event::Teleport(rc_proto_v777::play::PlayerPosition {
                id: self.teleport_id,
                pos: sent,
                delta: [0.0; 3],
                yaw: ry,
                pitch: rp,
                relative: flags,
            }));
            self.update_tracking();
        } else {
            self.ent_teleport(e, abs, Some([fy, fp]));
        }
    }

    /// `rotate <target> <rotation>`: rotation resolved against the source, applied relative or
    /// absolute per component (pitch clamped to ±90); the player only receives a rotation
    /// packet (its speed and position do not move).
    pub fn command_rotate(&mut self, e: E, rot: [f32; 2], rel: [bool; 2]) {
        let [cy, cp] = self.ent_rot(e);
        let sy = if rel[0] { rot[0] - cy } else { rot[0] };
        let sp = if rel[1] { rot[1] - cp } else { rot[1] };
        let fy = if rel[0] { cy + sy } else { sy };
        let fp = (if rel[1] { cp + sp } else { sp }).clamp(-90.0, 90.0);
        if e == E::Player {
            self.yaw = fy;
            self.pitch = fp;
            let _ =
                self.send(Event::PlayerRotation { yaw: sy, yaw_relative: rel[0], pitch: sp, pitch_relative: rel[1] });
        } else {
            let pos = self.e_pos(e);
            self.ent_teleport(e, pos, Some([fy, fp]));
        }
    }

    /// After a command teleportation: the game restarts from a zero speed and keeps that
    /// of the relative axes (`rel`) only on those, then cancels the vertical and puts the entity on
    /// the ground, except for a living entity gliding.
    pub fn after_command_teleport(&mut self, e: E, rel: [bool; 3]) {
        if let E::Other(i) = e
            && let Some(c) = self.cart_mut(i)
        {
            c.vel[1] = 0.0;
            zero_absolute(&mut c.vel, rel);
            c.on_ground = true;
            return;
        }
        match e {
            E::Player => {
                if !self.proj_player.fall_flying {
                    self.on_ground = true;
                }
            }
            E::Mob(i) => {
                if let Some(m) = self.mobs.get_mut(i) {
                    m.vel[1] = 0.0;
                    zero_absolute(&mut m.vel, rel);
                    m.on_ground = true;
                }
            }
            E::Item(i) => {
                if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == i) {
                    d.vel[1] = 0.0;
                    zero_absolute(&mut d.vel, rel);
                    d.on_ground = true;
                }
            }
            E::Orb(i) => {
                if let Some(d) = self.life.orbs.iter_mut().find(|d| d.id == i) {
                    d.vel[1] = 0.0;
                    zero_absolute(&mut d.vel, rel);
                }
            }
            E::Other(i) => {
                if let Some(o) = self.other_mut(i) {
                    if let Some(Nbt::List(m)) = o.get("Motion").cloned()
                        && m.len() == 3
                    {
                        let keep = |k: usize| if rel[k] { m[k].clone() } else { Nbt::Double(0.0) };
                        nbtio::set(o, "Motion", Nbt::List(vec![keep(0), Nbt::Double(0.0), keep(2)]));
                    }
                    nbtio::set(o, "OnGround", Nbt::Byte(1));
                }
            }
        }
    }

    /// Look of an entity towards a point from its feet or its eyes (anchor of the
    /// source); the player receives the look packet (point, or entity and its anchor).
    pub fn command_look_at(&mut self, e: E, from_eyes: bool, t: [f64; 3], entity: Option<(E, bool)>) {
        let mut from = self.e_pos(e);
        if from_eyes {
            from[1] += self.ent_eye_height(e);
        }
        let (dx, dy, dz) = (t[0] - from[0], t[1] - from[1], t[2] - from[2]);
        let sd = (dx * dx + dz * dz).sqrt();
        // Degrees like the game: fast arctangent (table), `× 180 / (float)π`, rounded in
        // float.
        use crate::mobs::mth::atan2;
        let deg = |a: f64| (a * 180.0 / f64::from(std::f32::consts::PI)) as f32;
        let pitch = crate::mobs::mth::wrap(-deg(atan2(dy, sd)));
        let yaw = crate::mobs::mth::wrap(deg(atan2(dz, dx)) - 90.0);
        if e == E::Player {
            let entity = entity.map(|(x, eyes)| (self.ent_id(x), eyes));
            let _ = self.send(Event::LookAt { eyes: from_eyes, target: t, entity });
            self.yaw = yaw;
            self.pitch = pitch;
        } else {
            let pos = self.e_pos(e);
            self.ent_teleport(e, pos, Some([yaw, pitch]));
        }
    }

    /// `kill`: infinite "generic kill" damage for living entities, removal for the rest.
    pub fn ent_kill(&mut self, e: E) {
        match e {
            // F15: the dragon is removed on the spot (victory of the fight).
            E::Mob(i) if self.end_kill_mob(i) => {}
            E::Player => {
                self.hurt("minecraft:generic_kill", f32::MAX);
            }
            E::Mob(i) => {
                if let Some(idx) = self.mobs.index_of(i)
                    && let Some(mut m) = self.mobs.list[idx].take()
                {
                    self.mob_hurt(&mut m, "minecraft:generic_kill", f32::MAX, None, None);
                    self.mobs.list[idx] = Some(m);
                }
            }
            E::Item(i) => {
                self.entity_scores_removed(self.ent_uuid(e));
                self.life.drops.retain(|d| d.id != i);
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![i])));
            }
            E::Orb(i) => {
                self.entity_scores_removed(self.ent_uuid(e));
                self.life.orbs.retain(|d| d.id != i);
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![i])));
            }
            // F15: crystal (fight notified), cloud, ball, bullet.
            E::Other(i) if self.end_kill_other(i) => {}
            // G5-F06: minecart killed (the contents drop), removed on the next minecart tick.
            E::Other(i) if self.cart_ref(i).is_some() => {
                if let Some((k, mut c)) = self.cart_take(i) {
                    self.cart_kill(&mut c);
                    self.cart_put(k, c);
                }
            }
            E::Other(i) => {
                self.entity_scores_removed(self.ent_uuid(e));
                for l in self.entities.values_mut() {
                    l.retain(|(x, _)| *x != i);
                }
                self.deco_forget(i);
                self.life.drop_chunks.extend(self.entities.keys().copied().collect::<Vec<_>>());
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![i])));
            }
        }
    }

    /// Alive in the sense of the `@e` selectors.
    pub fn e_alive(&self, e: E) -> bool {
        match e {
            E::Player => !self.life.vit.dead,
            _ => self.ent_exists(e),
        }
    }

    /// Living entity: player, mobs, others according to their NBT.
    pub fn ent_is_living(&self, e: E) -> bool {
        match e {
            E::Player | E::Mob(_) => true,
            E::Item(_) | E::Orb(_) => false,
            E::Other(i) => self.other(i).is_some_and(|n| n.get("Health").is_some()),
        }
    }

    /// Entity from its UUID.
    pub fn ent_by_uuid(&self, u: u128) -> Option<E> {
        self.all_ents().into_iter().find(|e| self.ent_uuid(*e) == u)
    }

    /// Name of a stack (custom name or item name).
    pub fn stack_name(&self, s: &crate::player::Stack) -> Component {
        if let Some(n) = crate::items::component(s, "minecraft:custom_name").and_then(rc_text::from_nbt) {
            return n;
        }
        if let Some(n) = crate::items::component(s, "minecraft:item_name").and_then(rc_text::from_nbt) {
            return n;
        }
        let key = self
            .data
            .items
            .iter()
            .zip(self.data.registry("minecraft:item").map(|r| r.entries.as_slice()).unwrap_or(&[]))
            .find(|(_, n)| **n == s.id)
            .map(|(i, _)| i.name_key.clone())
            .unwrap_or_else(|| format!("item.{}", s.id.replace(':', ".")));
        Component::translate(key, Vec::new())
    }
}

impl crate::mobs::Mobs {
    /// Mob from its network identifier.
    pub fn mob_ref_any(&self, id: i32) -> Option<&crate::mobs::mob::Mob> {
        self.list.iter().flatten().find(|m| m.id == id).map(|b| &**b)
    }
}

/// Resets the horizontal speed of the absolute axes of a command teleportation.
fn zero_absolute(vel: &mut [f64; 3], rel: [bool; 3]) {
    for k in [0, 2] {
        if !rel[k] {
            vel[k] = 0.0;
        }
    }
}
