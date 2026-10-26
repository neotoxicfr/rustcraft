//! Integrated server events → game packets of protocol 777 (LAN, S9). The in-memory link of
//! singleplayer publishes already decoded `Event`s; a remote client receives their network form,
//! the exact inverse of the decoders (`rc-proto-v777`, `rc-client`).

use rc_client::EntityEvent;
use rc_client::Event;
use rc_client::ids::Ids;
use rc_proto::nbt::{self, Nbt};
use rc_proto::varint::{write_varint, write_varlong};
use rc_proto::{BlockPos, Encode, write_str};
use rc_proto_v777::entity::{Meta, Particle};
use rc_proto_v777::item::ItemStack;

use crate::server::Server;

/// Encoder state specific to a connection.
#[derive(Debug, Default)]
pub struct EncState {
    /// Last window opened (`container_close`).
    pub window: i32,
}

/// Packet: identifier then body.
fn pk(id: i32, f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut w = Vec::with_capacity(32);
    write_varint(&mut w, id);
    f(&mut w);
    w
}

fn bpos(w: &mut Vec<u8>, p: [i32; 3]) {
    BlockPos { x: p[0], y: p[1], z: p[2] }.encode(w);
}

fn vec3d(w: &mut Vec<u8>, v: [f64; 3]) {
    for x in v {
        x.encode(w);
    }
}

/// Angle in degrees → byte.
fn angle(w: &mut Vec<u8>, deg: f32) {
    w.push((deg * 256.0 / 360.0).floor() as i32 as u8);
}

/// "Low precision" velocity.
pub fn lp_vec3(w: &mut Vec<u8>, v: [f64; 3]) {
    let clean = |x: f64| if x.is_nan() { 0.0 } else { x.clamp(-1.7179869183e10, 1.7179869183e10) };
    let v = v.map(clean);
    let m = v[0].abs().max(v[1].abs()).max(v[2].abs());
    if m < 3.051_944_088_384_301e-5 {
        w.push(0);
        return;
    }
    let scale = m.ceil() as u64;
    let big = scale & 3 != scale;
    let pack = |x: f64| -> u64 { ((x / scale as f64 * 0.5 + 0.5) * 32766.0).round() as u64 & 0x7fff };
    let mark = if big { (scale & 3) | 4 } else { scale & 3 };
    let b = mark | pack(v[0]) << 3 | pack(v[1]) << 18 | pack(v[2]) << 33;
    w.push(b as u8);
    w.push((b >> 8) as u8);
    ((b >> 16) as u32).encode(w);
    if big {
        write_varint(w, (scale >> 2) as i32);
    }
}

fn opt_nbt(w: &mut Vec<u8>, n: Option<&Nbt>) {
    n.is_some().encode(w);
    if let Some(n) = n {
        nbt::write(w, Some(n));
    }
}

/// Text component (NBT network form).
fn text(w: &mut Vec<u8>, c: &rc_text::Component) {
    let n = crate::commands::text::to_nbt(c);
    nbt::write(w, Some(&n));
}

/// Optional stack.
pub fn stack(w: &mut Vec<u8>, s: Option<&ItemStack>) {
    match s {
        None => write_varint(w, 0),
        Some(s) if s.count <= 0 => write_varint(w, 0),
        Some(s) => {
            write_varint(w, s.count);
            write_varint(w, s.item);
            patch(w, s);
        }
    }
}

fn patch(w: &mut Vec<u8>, s: &ItemStack) {
    write_varint(w, s.added.len() as i32);
    write_varint(w, s.removed.len() as i32);
    for (ty, bytes) in &s.added {
        write_varint(w, i32::from(*ty));
        w.extend_from_slice(bytes);
    }
    for ty in &s.removed {
        write_varint(w, i32::from(*ty));
    }
}

/// Price of an offer: item, quantity, required components (type + value).
fn cost(w: &mut Vec<u8>, s: &ItemStack) {
    write_varint(w, s.item);
    write_varint(w, s.count);
    write_varint(w, s.added.len() as i32);
    for (ty, bytes) in &s.added {
        write_varint(w, i32::from(*ty));
        w.extend_from_slice(bytes);
    }
}

/// `merchant_offers`: window, offers (price, result, optional second price, sold out,
/// uses, maximum, experience, special difference, multiplier, demand), level,
/// experience, progress bar, restocking.
pub(crate) fn merchant_offers(w: &mut Vec<u8>, m: &rc_proto_v777::special::MerchantOffers) {
    write_varint(w, m.window);
    write_varint(w, m.offers.len() as i32);
    for o in &m.offers {
        cost(w, &o.cost_a);
        stack(w, o.result.as_ref());
        o.cost_b.is_some().encode(w);
        if let Some(b) = &o.cost_b {
            cost(w, b);
        }
        o.out_of_stock.encode(w);
        for v in [o.uses, o.max_uses, o.xp, o.special_diff] {
            v.encode(w);
        }
        o.price_multiplier.encode(w);
        o.demand.encode(w);
    }
    write_varint(w, m.level);
    write_varint(w, m.xp);
    m.show_progress.encode(w);
    m.can_restock.encode(w);
}

/// `map_item_data`: map, scale, lock, optional decorations, optional rectangle.
pub(crate) fn map_item_data(w: &mut Vec<u8>, m: &rc_proto_v777::map::MapItemData) {
    write_varint(w, m.id);
    m.scale.encode(w);
    m.locked.encode(w);
    m.decorations.is_some().encode(w);
    if let Some(list) = &m.decorations {
        write_varint(w, list.len() as i32);
        for d in list {
            write_varint(w, d.kind);
            d.x.encode(w);
            d.y.encode(w);
            (d.rot as i8).encode(w);
            d.name.is_some().encode(w);
            if let Some(n) = &d.name {
                nbt::write(w, Some(n));
            }
        }
    }
    match &m.patch {
        None => 0u8.encode(w),
        Some(p) => {
            for b in [p.width, p.height, p.x0, p.y0] {
                b.encode(w);
            }
            write_varint(w, p.colors.len() as i32);
            w.extend_from_slice(&p.colors);
        }
    }
}

/// Stack template (item, quantity, patch).
fn template(w: &mut Vec<u8>, s: &ItemStack) {
    write_varint(w, s.item);
    write_varint(w, s.count.max(1));
    patch(w, s);
}

impl Server {
    /// Options type of a particle from its name (registry order).
    fn particle_opts(&self, kind: i32) -> &'static str {
        let name = self
            .data
            .registry("minecraft:particle_type")
            .and_then(|r| r.entries.get(kind as usize).cloned())
            .unwrap_or_default();
        match name.trim_start_matches("minecraft:") {
            "block" | "block_marker" | "falling_dust" | "dust_pillar" | "block_crumble" => "state",
            "dust" => "dust",
            "dust_color_transition" => "dust_transition",
            "entity_effect" | "tinted_leaves" | "flash" => "color",
            "effect" | "instant_effect" => "color_power",
            "dragon_breath" | "sculk_charge" => "float",
            "shriek" => "varint",
            "item" => "item",
            "vibration" => "vibration",
            "trail" => "trail",
            "geyser" | "geyser_plume" => "geyser",
            "geyser_base" | "geyser_poof" => "geyser_base",
            _ => "none",
        }
    }

    fn particle(&self, w: &mut Vec<u8>, p: &Particle) {
        write_varint(w, p.kind);
        match self.particle_opts(p.kind) {
            "state" | "varint" => write_varint(w, p.state),
            "dust" => {
                p.color.encode(w);
                p.scale.encode(w);
            }
            "dust_transition" => {
                p.color.encode(w);
                p.color2.encode(w);
                p.scale.encode(w);
            }
            "color" => p.color.encode(w),
            "color_power" => {
                p.color.encode(w);
                p.scale.encode(w);
            }
            "float" => p.scale.encode(w),
            "geyser" => 0i32.encode(w),
            "geyser_base" => {
                0i32.encode(w);
                0i32.encode(w);
            }
            "item" => {
                write_varint(w, p.item.max(1));
                write_varint(w, 1);
                write_varint(w, 0);
                write_varint(w, 0);
            }
            "vibration" => {
                use rc_proto_v777::entity::VibDest;
                let (dest, ticks) = p.vib.unwrap_or((VibDest::Block([0, 0, 0]), 0));
                match dest {
                    VibDest::Block(b) => {
                        write_varint(w, 0);
                        bpos(w, b);
                    }
                    VibDest::Entity { id, y_offset } => {
                        write_varint(w, 1);
                        write_varint(w, id);
                        y_offset.encode(w);
                    }
                }
                write_varint(w, ticks);
            }
            "trail" => {
                vec3d(w, [0.0; 3]);
                p.color.encode(w);
                write_varint(w, 1);
            }
            _ => {}
        }
    }

    fn meta_value(&self, w: &mut Vec<u8>, m: &Meta) {
        match m {
            Meta::Byte(v) => {
                write_varint(w, 0);
                v.encode(w);
            }
            Meta::Int(v) => {
                write_varint(w, 1);
                write_varint(w, *v);
            }
            Meta::Long(v) => {
                write_varint(w, 2);
                write_varlong(w, *v);
            }
            Meta::Float(v) => {
                write_varint(w, 3);
                v.encode(w);
            }
            Meta::String(s) => {
                write_varint(w, 4);
                write_str(w, s);
            }
            Meta::Text(t) => {
                write_varint(w, 6);
                opt_nbt(w, t.as_ref());
            }
            Meta::Item(s) => {
                write_varint(w, 7);
                stack(w, s.as_ref());
            }
            Meta::Bool(b) => {
                write_varint(w, 8);
                b.encode(w);
            }
            Meta::Rotations(r) => {
                write_varint(w, 9);
                for x in r {
                    x.encode(w);
                }
            }
            Meta::BlockPos(p) => {
                write_varint(w, 11);
                p.is_some().encode(w);
                if let Some(p) = p {
                    bpos(w, *p);
                }
            }
            Meta::Direction(d) => {
                write_varint(w, 12);
                write_varint(w, *d);
            }
            Meta::Entity(u) => {
                write_varint(w, 13);
                u.is_some().encode(w);
                if let Some(u) = u {
                    u.encode(w);
                }
            }
            Meta::BlockState(s) => {
                write_varint(w, 15);
                write_varint(w, s.unwrap_or(0));
            }
            Meta::Particle(p) => {
                write_varint(w, 16);
                self.particle(w, p);
            }
            Meta::Particles(list) => {
                write_varint(w, 17);
                write_varint(w, list.len() as i32);
                for p in list {
                    self.particle(w, p);
                }
            }
            Meta::Villager(v) => {
                write_varint(w, 18);
                for x in v {
                    write_varint(w, *x);
                }
            }
            Meta::OptionalInt(v) => {
                write_varint(w, 19);
                write_varint(w, v.map_or(0, |x| x + 1));
            }
            Meta::Pose(p) => {
                write_varint(w, 20);
                write_varint(w, *p);
            }
            Meta::Variant(v, ty) => {
                write_varint(w, i32::from(*ty));
                if *ty == 34 {
                    write_varint(w, v + 1);
                } else {
                    write_varint(w, *v);
                }
            }
            Meta::GlobalPos(g) => {
                write_varint(w, 33);
                g.is_some().encode(w);
                if let Some((d, p)) = g {
                    write_str(w, d);
                    bpos(w, *p);
                }
            }
            Meta::Vector3(v) => {
                write_varint(w, 39);
                for x in v {
                    x.encode(w);
                }
            }
            Meta::Quaternion(q) => {
                write_varint(w, 40);
                for x in q {
                    x.encode(w);
                }
            }
            Meta::Profile(b) => {
                write_varint(w, 41);
                w.extend_from_slice(b);
            }
        }
    }

    fn entity_packets(&self, ids: &Ids, e: &EntityEvent, out: &mut Vec<Vec<u8>>) {
        use rc_proto_v777::entity::Delta;
        match e {
            EntityEvent::Add(a) => out.push(pk(ids.add_entity, |w| {
                write_varint(w, a.id);
                a.uuid.encode(w);
                write_varint(w, a.kind);
                vec3d(w, a.pos);
                lp_vec3(w, a.velocity);
                angle(w, a.pitch);
                angle(w, a.yaw);
                angle(w, a.head_yaw);
                write_varint(w, a.data);
            })),
            EntityEvent::Remove(list) => out.push(pk(ids.remove_entities, |w| {
                write_varint(w, list.len() as i32);
                for x in list {
                    write_varint(w, *x);
                }
            })),
            EntityEvent::Move(m) => match (&m.delta, m.rot) {
                (Some(d), rot) => {
                    let id = if rot.is_some() { ids.move_entity_pos_rot } else { ids.move_entity_pos };
                    out.push(pk(id, |w| {
                        write_varint(w, m.id);
                        let (n, steps): (i32, Vec<(i32, [i16; 3])>) = match d {
                            Delta::Linear(v) => (0, vec![(0, *v)]),
                            Delta::Steps(s) => (s.len() as i32, s.clone()),
                        };
                        write_varint(w, (n << 1) | i32::from(m.on_ground));
                        if n == 0 {
                            for x in steps[0].1 {
                                x.encode(w);
                            }
                        } else {
                            for (t, v) in &steps {
                                write_varint(w, *t);
                                for x in v {
                                    x.encode(w);
                                }
                            }
                        }
                        if let Some((y, p)) = rot {
                            angle(w, y);
                            angle(w, p);
                        }
                    }));
                }
                (None, Some((y, p))) => out.push(pk(ids.move_entity_rot, |w| {
                    write_varint(w, m.id);
                    m.on_ground.encode(w);
                    angle(w, y);
                    angle(w, p);
                })),
                (None, None) => {}
            },
            EntityEvent::Teleport(t) => out.push(pk(ids.teleport_entity, |w| {
                write_varint(w, t.id);
                vec3d(w, t.pos);
                vec3d(w, t.velocity);
                t.yaw.encode(w);
                t.pitch.encode(w);
                t.relative.encode(w);
                t.on_ground.encode(w);
            })),
            EntityEvent::Sync(s) => out.push(pk(ids.entity_position_sync, |w| {
                write_varint(w, s.id);
                if s.path.linear {
                    write_varint(w, 0);
                    vec3d(w, s.path.end());
                } else {
                    write_varint(w, 1);
                    write_varint(w, s.path.steps.len() as i32);
                    for (p, t) in &s.path.steps {
                        vec3d(w, *p);
                        write_varint(w, *t);
                    }
                }
                s.yaw.encode(w);
                s.pitch.encode(w);
                s.on_ground.encode(w);
            })),
            EntityEvent::Head { id, yaw } => out.push(pk(ids.rotate_head, |w| {
                write_varint(w, *id);
                angle(w, *yaw);
            })),
            EntityEvent::Motion { id, velocity } => out.push(pk(ids.set_entity_motion, |w| {
                write_varint(w, *id);
                lp_vec3(w, *velocity);
            })),
            EntityEvent::Data { id, values } => out.push(pk(ids.set_entity_data, |w| {
                write_varint(w, *id);
                for (i, v) in values {
                    w.push(*i);
                    self.meta_value(w, v);
                }
                w.push(0xff);
            })),
            EntityEvent::Equipment { id, slots } => {
                if slots.is_empty() {
                    return;
                }
                out.push(pk(ids.set_equipment, |w| {
                    write_varint(w, *id);
                    for (k, (slot, s)) in slots.iter().enumerate() {
                        let more = k + 1 < slots.len();
                        w.push(slot | if more { 0x80 } else { 0 });
                        stack(w, s.as_ref());
                    }
                }));
            }
            EntityEvent::Event { id, event } => out.push(pk(ids.entity_event, |w| {
                id.encode(w);
                event.encode(w);
            })),
            EntityEvent::Swing { id, hand, kind, duration } => out.push(pk(ids.swing_animation, |w| {
                write_varint(w, *id);
                write_varint(w, *hand);
                write_varint(w, *kind);
                write_varint(w, *duration);
            })),
            EntityEvent::Hurt { id, yaw } => out.push(pk(ids.hurt_animation, |w| {
                write_varint(w, *id);
                yaw.encode(w);
            })),
            // Damage: type kept, causes and source position unknown (absent).
            EntityEvent::Damage { id, source } => out.push(pk(ids.damage_event, |w| {
                write_varint(w, *id);
                write_varint(w, *source);
                write_varint(w, 0);
                write_varint(w, 0);
                false.encode(w);
            })),
            EntityEvent::Passengers { vehicle, passengers } => out.push(pk(ids.set_passengers, |w| {
                write_varint(w, *vehicle);
                write_varint(w, passengers.len() as i32);
                for p in passengers {
                    write_varint(w, *p);
                }
            })),
            EntityEvent::Effect { id, effect, amplifier, duration, flags } => {
                out.push(pk(ids.update_mob_effect, |w| {
                    write_varint(w, *id);
                    write_varint(w, *effect);
                    write_varint(w, *amplifier);
                    write_varint(w, *duration);
                    flags.encode(w);
                }))
            }
            EntityEvent::RemoveEffect { id, effect } => out.push(pk(ids.remove_mob_effect, |w| {
                write_varint(w, *id);
                write_varint(w, *effect);
            })),
            EntityEvent::Animate { id, action } => out.push(pk(ids.animate, |w| {
                write_varint(w, *id);
                w.push(*action);
            })),
        }
    }

    /// Batch of chunks of a network connection: start, chunks, end with their count.
    pub(crate) fn chunk_batch(&self, chunks: &[rc_proto_v777::chunk::ChunkPacket]) -> Vec<Vec<u8>> {
        let ids = *self.peer.ids();
        let mut out = Vec::with_capacity(chunks.len() + 2);
        out.push(pk(ids.chunk_batch_start, |_| {}));
        for c in chunks {
            out.push(pk(ids.chunk, |w| self.chunk_body(w, c)));
        }
        out.push(pk(ids.chunk_batch_finished, |w| write_varint(w, chunks.len() as i32)));
        out
    }

    /// Chunk packet with light.
    fn chunk_body(&self, w: &mut Vec<u8>, c: &rc_proto_v777::chunk::ChunkPacket) {
        c.x.encode(w);
        c.z.encode(w);
        write_varint(w, c.heightmaps.len() as i32);
        for (k, data) in &c.heightmaps {
            write_varint(w, *k);
            write_varint(w, data.len() as i32);
            for x in data.iter() {
                x.encode(w);
            }
        }
        let states = self.data.states.len() as u32;
        let biomes = self.states.biome_names.len() as u32;
        let bits = |n: u32| (u32::BITS - n.max(2).saturating_sub(1).leading_zeros()) as u8;
        let mut sec = Vec::with_capacity(4096);
        let fluids = &self.phys.fluids;
        for s in &c.sections {
            s.non_empty.encode(&mut sec);
            // States with a non-empty fluid (water, lava, submerged blocks): the game client does not look
            // for a current or swimming in a section that announces zero.
            (s.blocks.count_where(4096, |v| fluids.fluid(v) != rc_physics::Fluid::None) as i16).encode(&mut sec);
            container(&mut sec, &s.blocks, 4096, true, bits(states));
            container(&mut sec, &s.biomes, 64, false, bits(biomes));
        }
        write_varint(w, sec.len() as i32);
        w.extend_from_slice(&sec);
        write_varint(w, c.block_entities.len() as i32);
        for b in &c.block_entities {
            w.push((b.x << 4) | (b.z & 15));
            b.y.encode(w);
            write_varint(w, b.kind);
            nbt::write(w, b.data.as_ref());
        }
        light(w, &c.light);
    }

    /// Packets of an event (empty: nothing to send to a remote client).
    pub(crate) fn encode_event(&self, st: &mut EncState, e: &Event, out: &mut Vec<Vec<u8>>) {
        let ids = *self.peer.ids();
        match e {
            Event::Joined { login, .. } => out.push(pk(ids.login, |w| {
                let l = login;
                l.player_id.encode(w);
                l.hardcore.encode(w);
                write_varint(w, l.dimensions.len() as i32);
                for d in &l.dimensions {
                    write_str(w, d);
                }
                write_varint(w, l.max_players);
                write_varint(w, l.view_distance);
                write_varint(w, l.simulation_distance);
                l.reduced_debug_info.encode(w);
                l.show_death_screen.encode(w);
                l.limited_crafting.encode(w);
                spawn_info(w, &l.spawn);
                l.online_mode.encode(w);
                l.enforces_secure_chat.encode(w);
            })),
            Event::Respawn { keep, .. } => {
                let info = self.lan_spawn_info();
                out.push(pk(ids.respawn, |w| {
                    spawn_info(w, &info);
                    w.push(*keep);
                }));
            }
            Event::Time { game_time, clocks } => {
                let names = self.regs.entries("minecraft:world_clock");
                out.push(pk(ids.set_time, |w| {
                    game_time.encode(w);
                    let list: Vec<_> = clocks
                        .iter()
                        .filter_map(|(n, t, f, s)| names.iter().position(|x| x == n).map(|i| (i as i32, *t, *f, *s)))
                        .collect();
                    write_varint(w, list.len() as i32);
                    for (i, t, f, s) in list {
                        write_varint(w, i);
                        write_varlong(w, t);
                        f.encode(w);
                        s.encode(w);
                    }
                }));
            }
            Event::LightUpdate { x, z, light: l } => out.push(pk(ids.light_update, |w| {
                write_varint(w, *x);
                write_varint(w, *z);
                light(w, l);
            })),
            Event::Chat { component, .. } => out.push(pk(ids.system_chat, |w| {
                text(w, component);
                false.encode(w);
            })),
            Event::ActionBar(c) => out.push(pk(ids.system_chat, |w| {
                text(w, c);
                true.encode(w);
            })),
            Event::Commands(_) => {
                if let Some(body) = crate::lan::commands_body(self, self.cmd.op) {
                    out.push(pk(ids.commands, |w| w.extend_from_slice(&body)));
                }
            }
            Event::CommandSuggestions(s) => out.push(pk(ids.command_suggestions, |w| {
                write_varint(w, s.id);
                write_varint(w, s.start);
                write_varint(w, s.length);
                write_varint(w, s.list.len() as i32);
                for (t, tip) in &s.list {
                    write_str(w, t);
                    opt_nbt(w, tip.as_ref());
                }
            })),
            Event::ChatCompletions { action, entries } => out.push(pk(ids.custom_chat_completions, |w| {
                write_varint(w, *action);
                write_varint(w, entries.len() as i32);
                for e in entries {
                    write_str(w, e);
                }
            })),
            Event::GameEvent { kind, param } => out.push(pk(ids.game_event, |w| {
                w.push(*kind);
                param.encode(w);
            })),
            Event::PlayerInfo(u) => out.push(pk(ids.player_info_update, |w| info_update(w, u))),
            Event::PlayersRemoved(list) => out.push(pk(ids.player_info_remove, |w| {
                write_varint(w, list.len() as i32);
                for u in list {
                    u.encode(w);
                }
            })),
            Event::Sound(s) => {
                use rc_proto_v777::play::SoundRef;
                let sref = |w: &mut Vec<u8>| match &s.sound {
                    SoundRef::Id(i) => write_varint(w, i + 1),
                    SoundRef::Named(n) => {
                        write_varint(w, 0);
                        write_str(w, n);
                        false.encode(w);
                    }
                };
                match (s.pos, s.entity) {
                    (_, Some(ent)) => out.push(pk(ids.sound_entity, |w| {
                        sref(w);
                        write_varint(w, s.category);
                        write_varint(w, ent);
                        s.volume.encode(w);
                        s.pitch.encode(w);
                        s.seed.encode(w);
                    })),
                    (Some(p), None) => out.push(pk(ids.sound, |w| {
                        sref(w);
                        write_varint(w, s.category);
                        for v in p {
                            ((v * 8.0) as i32).encode(w);
                        }
                        s.volume.encode(w);
                        s.pitch.encode(w);
                        s.seed.encode(w);
                    })),
                    (None, None) => {}
                }
            }
            Event::Particles(p) => out.push(pk(ids.level_particles, |w| {
                self.particle(w, &p.particle);
                p.force.encode(w);
                p.always_visible.encode(w);
                vec3d(w, p.pos);
                for x in p.delta {
                    x.encode(w);
                }
                for x in p.speed {
                    x.encode(w);
                }
                write_varint(w, p.count);
                write_varint(w, p.mode);
            })),
            Event::LevelEvent { kind, pos, data, global } => out.push(pk(ids.level_event, |w| {
                kind.encode(w);
                bpos(w, *pos);
                data.encode(w);
                global.encode(w);
            })),
            Event::StopSound { category, name } => out.push(pk(ids.stop_sound, |w| {
                w.push(u8::from(category.is_some()) | (u8::from(name.is_some()) << 1));
                if let Some(c) = category {
                    write_varint(w, *c);
                }
                if let Some(n) = name {
                    write_str(w, n);
                }
            })),
            Event::Score(s) => score(&ids, s, out),
            Event::Boss(b) => out.push(pk(ids.boss_event, |w| {
                use rc_proto_v777::play::BossOp;
                b.id.encode(w);
                match &b.op {
                    BossOp::Add { name, progress, color, style, flags } => {
                        write_varint(w, 0);
                        nbt::write(w, name.as_ref());
                        progress.encode(w);
                        write_varint(w, *color);
                        write_varint(w, *style);
                        w.push(*flags);
                    }
                    BossOp::Remove => write_varint(w, 1),
                    BossOp::Progress(p) => {
                        write_varint(w, 2);
                        p.encode(w);
                    }
                    BossOp::Name(n) => {
                        write_varint(w, 3);
                        nbt::write(w, n.as_ref());
                    }
                    BossOp::Style { color, style } => {
                        write_varint(w, 4);
                        write_varint(w, *color);
                        write_varint(w, *style);
                    }
                    BossOp::Flags(f) => {
                        write_varint(w, 5);
                        w.push(*f);
                    }
                }
            })),
            Event::Title(t) => out.push(pk(ids.set_title_text, |w| nbt::write(w, t.as_ref()))),
            Event::Subtitle(t) => out.push(pk(ids.set_subtitle_text, |w| nbt::write(w, t.as_ref()))),
            Event::TitleTimes(t) => out.push(pk(ids.set_titles_animation, |w| {
                for x in t {
                    x.encode(w);
                }
            })),
            Event::ClearTitles(r) => out.push(pk(ids.clear_titles, |w| r.encode(w))),
            Event::Border(b) => border(&ids, b, out),
            Event::TabList { header, footer } => out.push(pk(ids.tab_list, |w| {
                let empty = Nbt::String(String::new());
                nbt::write(w, Some(header.as_ref().unwrap_or(&empty)));
                nbt::write(w, Some(footer.as_ref().unwrap_or(&empty)));
            })),
            Event::Cooldown { group, ticks } => out.push(pk(ids.cooldown, |w| {
                write_str(w, group);
                write_varint(w, *ticks);
            })),
            Event::Health { health, food, saturation } => out.push(pk(ids.set_health, |w| {
                health.encode(w);
                write_varint(w, *food);
                saturation.encode(w);
            })),
            Event::Experience { progress, level, total } => out.push(pk(ids.set_experience, |w| {
                progress.encode(w);
                write_varint(w, *level);
                write_varint(w, *total);
            })),
            Event::HeldSlot(s) => out.push(pk(ids.set_held_slot, |w| write_varint(w, *s))),
            Event::Inventory(inv) => {
                use rc_proto_v777::item::Inventory;
                match inv {
                    Inventory::Content { container, state, slots, carried } => {
                        out.push(pk(ids.container_set_content, |w| {
                            write_varint(w, *container);
                            write_varint(w, *state);
                            write_varint(w, slots.len() as i32);
                            for s in slots {
                                stack(w, s.as_ref());
                            }
                            stack(w, carried.as_ref());
                        }))
                    }
                    Inventory::Slot { container, state, slot, stack: s } => out.push(pk(ids.container_set_slot, |w| {
                        write_varint(w, *container);
                        write_varint(w, *state);
                        slot.encode(w);
                        stack(w, s.as_ref());
                    })),
                    Inventory::PlayerSlot { slot, stack: s } => out.push(pk(ids.set_player_inventory, |w| {
                        write_varint(w, *slot);
                        stack(w, s.as_ref());
                    })),
                    Inventory::Cursor(s) => out.push(pk(ids.set_cursor_item, |w| stack(w, s.as_ref()))),
                }
            }
            Event::OpenScreen { window, kind, title } => {
                st.window = *window;
                out.push(pk(ids.open_screen, |w| {
                    write_varint(w, *window);
                    write_varint(w, *kind);
                    match title {
                        Some(t) => text(w, t),
                        None => nbt::write(w, Some(&Nbt::String(String::new()))),
                    }
                }));
            }
            Event::ContainerClosed => out.push(pk(ids.container_close, |w| write_varint(w, st.window))),
            Event::ContainerData { window, prop, value } => out.push(pk(ids.container_set_data, |w| {
                write_varint(w, *window);
                prop.encode(w);
                value.encode(w);
            })),
            Event::UpdateRecipes(r) => {
                out.push(pk(ids.update_recipes, |w| crate::lan::recipes::update_recipes(self, w, r)))
            }
            Event::Recipes(r) => crate::lan::recipes::recipe_event(self, &ids, r, out),
            Event::Entity(e) => self.entity_packets(&ids, e, out),
            Event::SpawnPosition { dimension, pos } => out.push(pk(ids.spawn_position, |w| {
                write_str(w, dimension);
                bpos(w, *pos);
                self.level.spawn_yaw.encode(w);
                self.level.spawn_pitch.encode(w);
            })),
            Event::Died { entity, message } => out.push(pk(ids.player_combat_kill, |w| {
                write_varint(w, *entity);
                match message {
                    Some(m) => text(w, m),
                    None => nbt::write(w, Some(&Nbt::String(String::new()))),
                }
            })),
            Event::Chunk(c) => {
                out.push(pk(ids.chunk_batch_start, |_| {}));
                out.push(pk(ids.chunk, |w| self.chunk_body(w, c)));
                out.push(pk(ids.chunk_batch_finished, |w| write_varint(w, 1)));
            }
            Event::ForgetChunk { x, z } => out.push(pk(ids.forget_chunk, |w| {
                ((i64::from(*z) << 32) | (i64::from(*x) & 0xffff_ffff)).encode(w);
            })),
            Event::BlockUpdate { pos, state } => out.push(pk(ids.block_update, |w| {
                pos.encode(w);
                write_varint(w, *state as i32);
            })),
            Event::SectionUpdate { section, changes } => out.push(pk(ids.section_update, |w| {
                let v = ((i64::from(section[0]) & 0x3f_ffff) << 42)
                    | (i64::from(section[2]) & 0x3f_ffff) << 20
                    | (i64::from(section[1]) & 0xf_ffff);
                v.encode(w);
                write_varint(w, changes.len() as i32);
                for (local, s) in changes {
                    let (x, z, y) = (i64::from(local & 15), i64::from((local >> 4) & 15), i64::from((local >> 8) & 15));
                    write_varlong(w, (i64::from(*s) << 12) | (x << 8) | (z << 4) | y);
                }
            })),
            Event::BlockChangedAck(s) => out.push(pk(ids.block_changed_ack, |w| write_varint(w, *s))),
            Event::BlockDestruction { id, pos, stage } => out.push(pk(ids.block_destruction, |w| {
                write_varint(w, *id);
                bpos(w, *pos);
                w.push(*stage);
            })),
            Event::BlockEvent { pos, b0, b1, block } => out.push(pk(ids.block_event, |w| {
                bpos(w, *pos);
                w.push(*b0);
                w.push(*b1);
                write_varint(w, *block);
            })),
            Event::BlockEntityData { pos, kind, nbt: n } => out.push(pk(ids.block_entity_data, |w| {
                bpos(w, *pos);
                write_varint(w, *kind);
                nbt::write(w, n.as_ref());
            })),
            Event::ChunkCenter { x, z } => out.push(pk(ids.chunk_center, |w| {
                write_varint(w, *x);
                write_varint(w, *z);
            })),
            Event::ViewDistance(v) => out.push(pk(ids.chunk_radius, |w| write_varint(w, *v))),
            Event::Teleport(t) => out.push(pk(ids.player_position, |w| {
                write_varint(w, t.id);
                vec3d(w, t.pos);
                vec3d(w, t.delta);
                t.yaw.encode(w);
                t.pitch.encode(w);
                t.relative.encode(w);
            })),
            Event::Kicked(c) => out.push(pk(ids.disconnect, |w| text(w, c))),
            Event::Abilities(a) => out.push(pk(ids.abilities, |w| {
                let f = u8::from(a.invulnerable)
                    | u8::from(a.flying) << 1
                    | u8::from(a.may_fly) << 2
                    | u8::from(a.instabuild) << 3;
                w.push(f);
                a.flying_speed.encode(w);
                a.walking_speed.encode(w);
            })),
            Event::Attributes { entity, attributes } => out.push(pk(ids.update_attributes, |w| {
                write_varint(w, *entity);
                write_varint(w, attributes.len() as i32);
                for a in attributes {
                    write_varint(w, a.id);
                    a.base.encode(w);
                    write_varint(w, a.modifiers.len() as i32);
                    for m in &a.modifiers {
                        write_str(w, &m.id);
                        m.amount.encode(w);
                        write_varint(w, m.operation);
                    }
                }
            })),
            Event::Tags(t) => out.push(pk(ids.update_tags, |w| tags(w, t))),
            Event::TickingState { rate, frozen } => out.push(pk(ids.ticking_state, |w| {
                rate.encode(w);
                frozen.encode(w);
            })),
            Event::TickingStep(n) => out.push(pk(ids.ticking_step, |w| write_varint(w, *n))),
            Event::Explode(x) => out.push(pk(ids.explode, |w| {
                vec3d(w, x.center);
                x.radius.encode(w);
                x.blocks.encode(w);
                x.knockback.is_some().encode(w);
                if let Some(k) = x.knockback {
                    vec3d(w, k);
                }
                self.particle(w, &x.particle);
                match &x.sound {
                    rc_proto_v777::play::SoundRef::Id(i) => write_varint(w, i + 1),
                    rc_proto_v777::play::SoundRef::Named(n) => {
                        write_varint(w, 0);
                        write_str(w, n);
                        false.encode(w);
                    }
                }
                write_varint(w, x.block_particles.len() as i32);
                for (p, scale, speed, weight) in &x.block_particles {
                    self.particle(w, p);
                    scale.encode(w);
                    speed.encode(w);
                    write_varint(w, *weight);
                }
                x.play_sound.encode(w);
            })),
            Event::PlayerRotation { yaw, yaw_relative, pitch, pitch_relative } => {
                out.push(pk(ids.player_rotation, |w| {
                    yaw.encode(w);
                    yaw_relative.encode(w);
                    pitch.encode(w);
                    pitch_relative.encode(w);
                }))
            }
            Event::LookAt { eyes, target, entity } => out.push(pk(ids.player_look_at, |w| {
                write_varint(w, i32::from(*eyes));
                vec3d(w, *target);
                entity.is_some().encode(w);
                if let Some((id, e)) = entity {
                    write_varint(w, *id);
                    write_varint(w, i32::from(*e));
                }
            })),
            Event::SetCamera(id) => out.push(pk(ids.set_camera, |w| write_varint(w, *id))),
            Event::MoveVehicle { pos, yaw, pitch } => out.push(pk(ids.move_vehicle, |w| {
                vec3d(w, *pos);
                yaw.encode(w);
                pitch.encode(w);
            })),
            Event::ShowDialog(d) => out.push(pk(ids.show_dialog, |w| match d {
                rc_client::extra::DialogRef::Registry(i) => write_varint(w, i + 1),
                rc_client::extra::DialogRef::Inline(n) => {
                    write_varint(w, 0);
                    nbt::write(w, Some(n));
                }
            })),
            Event::ClearDialog => out.push(pk(ids.clear_dialog, |_| {})),
            Event::ChunkBiomes(list) => {
                let biomes = self.states.biome_names.len() as u32;
                let bits = (u32::BITS - biomes.max(2).saturating_sub(1).leading_zeros()) as u8;
                out.push(pk(ids.chunks_biomes, |w| {
                    write_varint(w, list.len() as i32);
                    for (x, z, cs) in list {
                        ((i64::from(*z) << 32) | (i64::from(*x) & 0xffff_ffff)).encode(w);
                        let mut b = Vec::new();
                        for c in cs {
                            container(&mut b, c, 64, false, bits);
                        }
                        write_varint(w, b.len() as i32);
                        w.extend_from_slice(&b);
                    }
                }));
            }
            Event::Difficulty { id, locked } => out.push(pk(ids.change_difficulty, |w| {
                write_varint(w, *id);
                locked.encode(w);
            })),
            Event::Misc(rc_client::misc::Misc::Take { item, collector, amount }) => {
                out.push(pk(ids.take_item_entity, |w| {
                    write_varint(w, *item);
                    write_varint(w, *collector);
                    write_varint(w, *amount);
                }))
            }
            Event::Progress(p) => crate::lan::progress_packets(self, &ids, p, out),
            // W8a: sign editor, written book, data (F3+I), game rules.
            Event::OpenSignEditor { pos, front } => out.push(pk(ids.open_sign_editor, |w| {
                bpos(w, *pos);
                write_varint(w, i32::from(*front)); // face: 1 front, 0 back
            })),
            Event::OpenBook(hand) => out.push(pk(ids.open_book, |w| write_varint(w, *hand))),
            // Maps, trading and mount inventory of the guests.
            Event::MapData(m) => out.push(pk(ids.map_item_data, |w| map_item_data(w, m))),
            Event::MerchantOffers(m) => out.push(pk(ids.merchant_offers, |w| merchant_offers(w, m))),
            Event::MountScreen { window, columns, entity } => {
                st.window = *window;
                out.push(pk(ids.mount_screen_open, |w| {
                    write_varint(w, *window);
                    write_varint(w, *columns);
                    entity.encode(w);
                }));
            }
            Event::TagQuery { id, nbt: n } => out.push(pk(ids.tag_query, |w| {
                write_varint(w, *id);
                nbt::write(w, n.as_ref());
            })),
            Event::GameRuleValues(v) => out.push(pk(ids.game_rule_values, |w| {
                write_varint(w, v.len() as i32);
                for (k, x) in v {
                    rc_proto::write_str(w, k);
                    rc_proto::write_str(w, x);
                }
            })),
            // Configuration phase, in-memory link or nothing useful for a remote client.
            _ => {}
        }
    }

    /// Last place of death of the player (spawn info of the connection and of respawns).
    pub(crate) fn last_death_pos(&self) -> Option<(String, BlockPos)> {
        self.life.vit.last_death.as_ref().map(|(d, p)| (d.clone(), BlockPos { x: p[0], y: p[1], z: p[2] }))
    }

    /// Spawn information of the current player (`respawn`).
    pub(crate) fn lan_spawn_info(&self) -> rc_proto_v777::config::SpawnInfo {
        let flat = self.is_flat();
        let t = self.dimension_type_of(&self.dim.name);
        let dt = self.regs.entries("minecraft:dimension_type").iter().position(|n| *n == t).unwrap_or(0) as i32;
        rc_proto_v777::config::SpawnInfo {
            dimension_type: dt,
            dimension: self.dim.name.clone(),
            hashed_seed: crate::server::sha256_seed(self.seed()),
            game_mode: self.game_mode,
            previous_game_mode: self.pdata.previous_game_mode,
            debug: false,
            flat,
            last_death: self.last_death_pos(),
            portal_cooldown: self.dims.player.cooldown,
            sea_level: if flat { -63 } else { 63 },
        }
    }
}

fn spawn_info(w: &mut Vec<u8>, s: &rc_proto_v777::config::SpawnInfo) {
    write_varint(w, s.dimension_type);
    write_str(w, &s.dimension);
    s.hashed_seed.encode(w);
    write_varint(w, s.game_mode);
    write_varint(w, s.previous_game_mode.map_or(0, |m| m + 1));
    s.debug.encode(w);
    s.flat.encode(w);
    s.last_death.is_some().encode(w);
    if let Some((d, p)) = &s.last_death {
        write_str(w, d);
        p.encode(w);
    }
    write_varint(w, s.portal_cooldown);
    write_varint(w, s.sea_level);
}

/// `RUSTCRAFT_OPT_LANREMAP=0`: old container encoder (unfolding, palette by linear
/// search per value). Default: canonical palette drawn from the raw indices, same bytes.
fn lan_remap() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_LANREMAP") != Some("0"))
}

/// `RUSTCRAFT_OPT_LANRAWPAL=1`: memory palette written as is when it is safe (see
/// [`container_raw`]); different bytes, same decoded values. Off by default (measured gain
/// too small to justify changing the bytes, see docs/perf.md).
fn lan_rawpal() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_LANRAWPAL") == Some("1"))
}

/// Paletted container: single value, palette (4–8 bits for blocks, 1–3 for
/// biomes) or direct identifiers.
fn container(w: &mut Vec<u8>, c: &rc_world::Container, n: usize, blocks: bool, global_bits: u8) {
    if lan_rawpal() && container_raw(w, c, n, blocks, global_bits) {
        return;
    }
    if lan_remap() {
        container_remap(w, c, n, blocks, global_bits);
    } else {
        container_reference(w, c, n, blocks, global_bits);
    }
}

/// Network palette widths: blocks 4–8 bits, biomes 1–3.
fn net_palette_bits(blocks: bool) -> (u8, u8) {
    if blocks { (4, 8) } else { (1, 3) }
}

/// Same bytes as [`container_reference`]: canonical palette (first appearance) computed
/// on the raw indices of the memory container, without unfolding or searching the palette by value.
fn container_remap(w: &mut Vec<u8>, c: &rc_world::Container, n: usize, blocks: bool, global_bits: u8) {
    let (min, max) = net_palette_bits(blocks);
    let mut palette = [0u32; 256];
    let mut idx = [0u8; rc_world::SECTION_BLOCKS];
    let Some(k) = c.first_seen_indices(n, 1 << max, global_bits, &mut palette, &mut idx) else {
        // Too many values: direct identifiers at the global width.
        w.push(global_bits);
        let mut values = [0u32; rc_world::SECTION_BLOCKS];
        c.unpack_into(&mut values[..n]);
        let vpl = 64 / global_bits as usize;
        for chunk in values[..n].chunks(vpl) {
            let mut acc = 0u64;
            for (j, &v) in chunk.iter().enumerate() {
                acc |= u64::from(v) << (j * global_bits as usize);
            }
            acc.encode(w);
        }
        return;
    };
    if k == 1 {
        w.push(0);
        write_varint(w, palette[0] as i32);
        return;
    }
    let bits = ((usize::BITS - (k - 1).leading_zeros()) as u8).max(min);
    w.push(bits);
    write_varint(w, k as i32);
    for &p in &palette[..k] {
        write_varint(w, p as i32);
    }
    let vpl = 64 / bits as usize;
    for chunk in idx[..n].chunks(vpl) {
        let mut acc = 0u64;
        for (j, &r) in chunk.iter().enumerate() {
            acc |= u64::from(r) << (j * bits as usize);
        }
        acc.encode(w);
    }
}

/// Writes the palette and the memory words as is if that is safe for every client: width
/// matching the network strategy (palette: the smallest network width for its size;
/// direct: global width), exact word count, every index in the palette. Returns `false`
/// (nothing written) otherwise.
fn container_raw(w: &mut Vec<u8>, c: &rc_world::Container, n: usize, blocks: bool, global_bits: u8) -> bool {
    let rc_world::Container::Packed { bits, palette, data } = c else { return false };
    let bits = *bits;
    if data.len() != rc_world::longs_for(n, bits) {
        return false;
    }
    let (min, max) = net_palette_bits(blocks);
    match palette {
        Some(p) => {
            let need = ((usize::BITS - p.len().max(1).saturating_sub(1).leading_zeros()) as u8).max(min);
            if p.len() < 2 || bits != need || bits > max || !indices_within(data, bits, n, p.len()) {
                return false;
            }
            w.push(bits);
            write_varint(w, p.len() as i32);
            for &v in p.iter() {
                write_varint(w, v as i32);
            }
        }
        None => {
            if bits != global_bits || bits <= max {
                return false;
            }
            w.push(bits);
        }
    }
    for d in data.iter() {
        d.encode(w);
    }
    true
}

/// True if the first `n` indices of `data` (width `bits`) are all `< len`.
fn indices_within(data: &[u64], bits: u8, n: usize, len: usize) -> bool {
    if len >= 1 << bits {
        return true;
    }
    let b = bits as usize;
    let vpl = 64 / b;
    let mask = (1u64 << b) - 1;
    let len = len as u64;
    let mut left = n;
    for &word in data {
        let mut x = word;
        for _ in 0..vpl.min(left) {
            if x & mask >= len {
                return false;
            }
            x >>= b;
        }
        left = left.saturating_sub(vpl);
    }
    true
}

/// Old encoder (reference of the proofs and `RUSTCRAFT_OPT_LANREMAP=0`).
fn container_reference(w: &mut Vec<u8>, c: &rc_world::Container, n: usize, blocks: bool, global_bits: u8) {
    let mut values = vec![0u32; n];
    c.unpack_into(&mut values);
    let mut palette: Vec<u32> = Vec::new();
    for &v in &values {
        if !palette.contains(&v) {
            palette.push(v);
            if palette.len() > 256 {
                break;
            }
        }
    }
    if palette.len() == 1 {
        w.push(0);
        write_varint(w, palette[0] as i32);
        return;
    }
    let need = (usize::BITS - (palette.len() - 1).leading_zeros()) as u8;
    let (bits, indirect) = if blocks {
        if need <= 8 { (need.max(4), true) } else { (global_bits, false) }
    } else if need <= 3 {
        (need.max(1), true)
    } else {
        (global_bits, false)
    };
    w.push(bits);
    if indirect {
        write_varint(w, palette.len() as i32);
        for p in &palette {
            write_varint(w, *p as i32);
        }
    }
    let vpl = 64 / bits as usize;
    let mut data = vec![0u64; n.div_ceil(vpl)];
    for (i, v) in values.iter().enumerate() {
        let raw = if indirect { palette.iter().position(|p| p == v).unwrap_or(0) as u64 } else { u64::from(*v) };
        data[i / vpl] |= raw << ((i % vpl) * bits as usize);
    }
    for d in data {
        d.encode(w);
    }
}

/// Bit set as a byte array (protocol 777).
fn bitset(w: &mut Vec<u8>, words: &[u64]) {
    let mut bytes: Vec<u8> = words.iter().flat_map(|x| x.to_le_bytes()).collect();
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    write_varint(w, bytes.len() as i32);
    w.extend_from_slice(&bytes);
}

fn light(w: &mut Vec<u8>, l: &rc_proto_v777::chunk::LightData) {
    let mask = |layers: &[rc_world::LightLayer]| {
        let mut m = vec![0u64; layers.len().div_ceil(64).max(1)];
        for (i, x) in layers.iter().enumerate() {
            if x.is_some() {
                m[i / 64] |= 1 << (i % 64);
            }
        }
        m
    };
    let sky_mask = if l.sky_mask.is_empty() { mask(&l.sky) } else { l.sky_mask.clone() };
    let block_mask = if l.block_mask.is_empty() { mask(&l.block) } else { l.block_mask.clone() };
    bitset(w, &sky_mask);
    bitset(w, &block_mask);
    bitset(w, &l.sky_empty);
    bitset(w, &l.block_empty);
    for (layers, m) in [(&l.sky, &sky_mask), (&l.block, &block_mask)] {
        let present: Vec<&[u8; rc_world::LIGHT_BYTES]> = layers
            .iter()
            .enumerate()
            .filter(|(i, _)| m.get(i / 64).is_some_and(|x| x >> (i % 64) & 1 == 1))
            .filter_map(|(_, x)| x.as_deref())
            .collect();
        write_varint(w, present.len() as i32);
        for p in present {
            write_varint(w, rc_world::LIGHT_BYTES as i32);
            w.extend_from_slice(p);
        }
    }
}

/// Properties of a profile (signed skin): count, then name, value and optional signature.
pub(crate) fn properties(w: &mut Vec<u8>, props: &[rc_proto_v777::players::Property]) {
    write_varint(w, props.len() as i32);
    for p in props {
        write_str(w, &p.name);
        write_str(w, &p.value);
        p.signature.is_some().encode(w);
        if let Some(s) = &p.signature {
            write_str(w, s);
        }
    }
}

fn info_update(w: &mut Vec<u8>, u: &rc_proto_v777::players::InfoUpdate) {
    use rc_proto_v777::players::action;
    w.push(u.actions);
    write_varint(w, u.entries.len() as i32);
    for e in &u.entries {
        e.uuid.encode(w);
        if u.actions & action::ADD_PLAYER != 0 {
            let (name, props) = e.profile.clone().unwrap_or_default();
            write_str(w, &name);
            properties(w, &props);
        }
        if u.actions & action::INITIALIZE_CHAT != 0 {
            false.encode(w);
        }
        if u.actions & action::UPDATE_GAME_MODE != 0 {
            write_varint(w, e.game_mode.unwrap_or(0));
        }
        if u.actions & action::UPDATE_LISTED != 0 {
            e.listed.unwrap_or(true).encode(w);
        }
        if u.actions & action::UPDATE_LATENCY != 0 {
            write_varint(w, e.latency.unwrap_or(0));
        }
        if u.actions & action::UPDATE_DISPLAY_NAME != 0 {
            let d = e.display_name.clone().flatten();
            opt_nbt(w, d.as_ref());
        }
        if u.actions & action::UPDATE_LIST_ORDER != 0 {
            write_varint(w, e.list_order.unwrap_or(0));
        }
        if u.actions & action::UPDATE_HAT != 0 {
            e.show_hat.unwrap_or(true).encode(w);
        }
    }
}

fn number_format(w: &mut Vec<u8>, f: &rc_proto_v777::scores::NumberFormat) {
    use rc_proto_v777::scores::NumberFormat;
    match f {
        NumberFormat::Blank => write_varint(w, 0),
        NumberFormat::Styled(n) => {
            write_varint(w, 1);
            nbt::write(w, n.as_ref());
        }
        NumberFormat::Fixed(n) => {
            write_varint(w, 2);
            nbt::write(w, n.as_ref());
        }
    }
}

fn opt_format(w: &mut Vec<u8>, f: Option<&rc_proto_v777::scores::NumberFormat>) {
    f.is_some().encode(w);
    if let Some(f) = f {
        number_format(w, f);
    }
}

fn score(ids: &Ids, s: &rc_proto_v777::scores::ScoreEvent, out: &mut Vec<Vec<u8>>) {
    use rc_proto_v777::scores::ScoreEvent;
    match s {
        ScoreEvent::Objective { name, mode, display, hearts, format } => out.push(pk(ids.set_objective, |w| {
            write_str(w, name);
            mode.encode(w);
            if *mode == 0 || *mode == 2 {
                let empty = Nbt::String(String::new());
                nbt::write(w, Some(display.as_ref().unwrap_or(&empty)));
                write_varint(w, i32::from(*hearts));
                opt_format(w, format.as_ref());
            }
        })),
        ScoreEvent::Display { slot, objective } => out.push(pk(ids.set_display_objective, |w| {
            write_varint(w, *slot);
            write_str(w, objective);
        })),
        ScoreEvent::Score { holder, objective, value, display, format } => out.push(pk(ids.set_score, |w| {
            write_str(w, holder);
            write_str(w, objective);
            write_varint(w, *value);
            opt_nbt(w, display.as_ref());
            opt_format(w, format.as_ref());
        })),
        ScoreEvent::Reset { holder, objective } => out.push(pk(ids.reset_score, |w| {
            write_str(w, holder);
            objective.is_some().encode(w);
            if let Some(o) = objective {
                write_str(w, o);
            }
        })),
        ScoreEvent::Team { name, mode, params, members } => out.push(pk(ids.set_player_team, |w| {
            write_str(w, name);
            mode.encode(w);
            if let Some(p) = params {
                let empty = Nbt::String(String::new());
                nbt::write(w, Some(p.display.as_ref().unwrap_or(&empty)));
                nbt::write(w, Some(p.prefix.as_ref().unwrap_or(&empty)));
                nbt::write(w, Some(p.suffix.as_ref().unwrap_or(&empty)));
                write_varint(w, p.nametag);
                write_varint(w, p.collision);
                p.color.is_some().encode(w);
                if let Some(c) = p.color {
                    write_varint(w, c);
                }
                p.options.encode(w);
            }
            if matches!(mode, 0 | 3 | 4) {
                write_varint(w, members.len() as i32);
                for m in members {
                    write_str(w, m);
                }
            }
        })),
    }
}

fn border(ids: &Ids, b: &rc_proto_v777::border::BorderPacket, out: &mut Vec<Vec<u8>>) {
    use rc_proto_v777::border::BorderPacket as B;
    out.push(match *b {
        B::Init { x, z, old_size, new_size, ticks, max_size, warn_blocks, warn_ticks } => {
            pk(ids.initialize_border, |w| {
                x.encode(w);
                z.encode(w);
                old_size.encode(w);
                new_size.encode(w);
                write_varlong(w, ticks);
                write_varint(w, max_size);
                write_varint(w, warn_blocks);
                write_varint(w, warn_ticks);
            })
        }
        B::Center { x, z } => pk(ids.set_border_center, |w| {
            x.encode(w);
            z.encode(w);
        }),
        B::Lerp { old_size, new_size, ticks } => pk(ids.set_border_lerp_size, |w| {
            old_size.encode(w);
            new_size.encode(w);
            write_varlong(w, ticks);
        }),
        B::Size(s) => pk(ids.set_border_size, |w| s.encode(w)),
        B::WarningDelay(d) => pk(ids.set_border_warning_delay, |w| write_varint(w, d)),
        B::WarningDistance(d) => pk(ids.set_border_warning_distance, |w| write_varint(w, d)),
    });
}

/// Tags per registry (configuration and play).
pub fn tags(w: &mut Vec<u8>, t: &rc_proto_v777::play::Tags) {
    write_varint(w, t.len() as i32);
    for (reg, list) in t {
        write_str(w, reg);
        write_varint(w, list.len() as i32);
        for (name, ids) in list {
            write_str(w, name);
            write_varint(w, ids.len() as i32);
            for i in ids {
                write_varint(w, *i);
            }
        }
    }
}

/// Item template exported for the other modules (`recipes`, advancements).
pub fn write_template(w: &mut Vec<u8>, s: &ItemStack) {
    template(w, s);
}

#[cfg(test)]
pub(crate) mod container_tests {
    use rc_world::{Container, SECTION_BIOMES, SECTION_BLOCKS, longs_for};

    use super::{container_raw, container_reference, container_remap};

    /// Number of states and biomes of the test context (global widths 15 and 6 bits).
    const STATES: u32 = 30_000;
    const BIOMES: u32 = 64;
    const BLOCK_BITS: u8 = 15;
    const BIOME_BITS: u8 = 6;

    fn rng(seed: u64) -> impl FnMut(u64) -> u64 {
        let mut s = seed | 1;
        move |n| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % n
        }
    }

    /// Container made by hand (raw indices given).
    fn manual(bits: u8, palette: Option<Vec<u32>>, raws: &[u32]) -> Container {
        let vpl = 64 / bits as usize;
        let mut data = vec![0u64; longs_for(raws.len(), bits)];
        for (i, &r) in raws.iter().enumerate() {
            data[i / vpl] |= u64::from(r) << ((i % vpl) * bits as usize);
        }
        Container::packed(raws.len(), bits, palette.map(Into::into), data.into()).unwrap()
    }

    /// Block sections: terrain layers, scattered values (2 to 1,000 distinct, including the
    /// 256/257 crossing), non-canonical palettes left by writes (with and without
    /// widening), index outside the palette, width too large, direct.
    pub(crate) fn block_sections() -> Vec<Container> {
        let mut r = rng(0x5eed_1234);
        let mut out = Vec::new();
        let terrain: Vec<u32> = (0..4096u32)
            .map(|i| match i >> 8 {
                0..=8 if i % 97 == 0 => 20 + i % 5,
                0..=8 => 1,
                9..=10 => 10,
                11 => 9,
                _ => 0,
            })
            .collect();
        out.push(Container::pack_reference(&terrain, 4, 8, BLOCK_BITS));
        for distinct in [2u32, 3, 16, 17, 100, 255, 256, 257, 1000] {
            let values: Vec<u32> = (0..4096u32).map(|i| ((i * 7919) % distinct) * 29 % STATES).collect();
            let base = Container::pack_reference(&values, 4, 8, BLOCK_BITS);
            out.push(base.clone());
            for widen in [false, true] {
                let mut c = base.clone();
                for step in 0..600 {
                    let v = r(u64::from(distinct) + 40) as u32 * 31 % STATES;
                    c.set_with(r(4096) as usize, v, 4, 8, BLOCK_BITS, true, widen);
                    if step % 150 == 149 {
                        out.push(c.clone());
                    }
                }
            }
        }
        // Single value, and palette of a single used value.
        out.push(Container::Single(7));
        out.push(manual(4, Some(vec![9, 3, 5]), &[1; 4096]));
        // Index outside the palette (read as state 0).
        let mut raws: Vec<u32> = (0..4096u32).map(|i| i % 3).collect();
        raws[77] = 9;
        out.push(manual(4, Some(vec![5, 6, 8]), &raws));
        // Width 6 for 3 entries (the network would take 4), and width 5 for 20 entries.
        out.push(manual(6, Some(vec![5, 6, 8]), &(0..4096u32).map(|i| i % 3).collect::<Vec<_>>()));
        out.push(manual(5, Some((100..120).collect()), &(0..4096u32).map(|i| i % 20).collect::<Vec<_>>()));
        // Direct at the global width, few or many values; narrower direct.
        out.push(manual(BLOCK_BITS, None, &(0..4096u32).map(|i| i % 4 * 1000).collect::<Vec<_>>()));
        out.push(manual(BLOCK_BITS, None, &(0..4096u32).map(|i| i * 7 % STATES).collect::<Vec<_>>()));
        out.push(manual(14, None, &(0..4096u32).map(|i| i * 3).collect::<Vec<_>>()));
        out
    }

    pub(crate) fn biome_sections() -> Vec<Container> {
        let mut r = rng(77);
        let mut out = vec![Container::Single(3)];
        for distinct in [2u32, 3, 5, 8, 9, 30] {
            let values: Vec<u32> = (0..64u32).map(|_| r(u64::from(distinct)) as u32 * 2 % BIOMES).collect();
            let base = Container::pack_reference(&values, 1, 3, BIOME_BITS);
            out.push(base.clone());
            let mut c = base;
            for _ in 0..40 {
                c.set_with(r(64) as usize, r(12) as u32, 1, 3, BIOME_BITS, true, true);
                out.push(c.clone());
            }
        }
        let mut raws: Vec<u32> = (0..64u32).map(|i| i % 3).collect();
        raws[5] = 3;
        out.push(manual(2, Some(vec![4, 8, 12]), &raws));
        out.push(manual(3, Some(vec![4, 8]), &(0..64u32).map(|i| i % 2).collect::<Vec<_>>()));
        out
    }

    fn encode(f: fn(&mut Vec<u8>, &Container, usize, bool, u8), c: &Container, blocks: bool) -> Vec<u8> {
        let mut w = Vec::new();
        if blocks {
            f(&mut w, c, SECTION_BLOCKS, true, BLOCK_BITS);
        } else {
            f(&mut w, c, SECTION_BIOMES, false, BIOME_BITS);
        }
        w
    }

    #[test]
    fn remap_writes_the_reference_bytes() {
        let mut checked = 0;
        for (blocks, list) in [(true, block_sections()), (false, biome_sections())] {
            for c in &list {
                assert_eq!(encode(container_remap, c, blocks), encode(container_reference, c, blocks), "{c:?}");
                checked += 1;
            }
        }
        assert!(checked > 100, "{checked} containers");
    }

    /// Chunk packet of a section (blocks `b`, biomes `m`) read back by the client decoder.
    fn decode(b: &[u8], m: &[u8]) -> (Container, Container) {
        let mut sec = Vec::new();
        rc_proto::Encode::encode(&1i16, &mut sec);
        rc_proto::Encode::encode(&0i16, &mut sec);
        sec.extend_from_slice(b);
        sec.extend_from_slice(m);
        let mut w = Vec::new();
        rc_proto::Encode::encode(&0i32, &mut w);
        rc_proto::Encode::encode(&0i32, &mut w);
        rc_proto::varint::write_varint(&mut w, 0);
        rc_proto::varint::write_varint(&mut w, sec.len() as i32);
        w.extend_from_slice(&sec);
        rc_proto::varint::write_varint(&mut w, 0);
        super::light(&mut w, &rc_proto_v777::chunk::LightData::default());
        let ctx = rc_proto_v777::chunk::ChunkContext::new(1, STATES, BIOMES);
        let mut p = rc_proto_v777::chunk::read_chunk(&mut rc_proto::Reader::new(&w), &ctx).expect("decoding");
        let s = p.sections.pop().unwrap();
        (s.blocks, s.biomes)
    }

    /// Direct write of the memory palette: where it applies, the client reads back the same
    /// values; the guards refuse the index outside the palette and the non-conforming width.
    #[test]
    fn raw_palette_decodes_same_values_and_guards_hold() {
        let single = encode(container_reference, &Container::Single(0), false);
        let mut used = 0;
        for c in block_sections() {
            let mut w = Vec::new();
            if container_raw(&mut w, &c, SECTION_BLOCKS, true, BLOCK_BITS) {
                used += 1;
                let (got, _) = decode(&w, &single);
                assert!((0..SECTION_BLOCKS).all(|i| got.get(i) == c.get(i)), "{c:?}");
            } else {
                assert!(w.is_empty());
            }
            // The read-back fallback path also gives the same values.
            let (got, _) = decode(&encode(container_remap, &c, true), &single);
            assert!((0..SECTION_BLOCKS).all(|i| got.get(i) == c.get(i)));
        }
        let stone = encode(container_reference, &Container::Single(1), true);
        for c in biome_sections() {
            let mut w = Vec::new();
            if container_raw(&mut w, &c, SECTION_BIOMES, false, BIOME_BITS) {
                used += 1;
                let (_, got) = decode(&stone, &w);
                assert!((0..SECTION_BIOMES).all(|i| got.get(i) == c.get(i)), "{c:?}");
            }
        }
        assert!(used > 40, "direct write exercised too little: {used}");
        // Sabotages: each would be read back without error by our client, but is not conforming.
        let mut raws: Vec<u32> = (0..4096u32).map(|i| i % 3).collect();
        raws[4095] = 3;
        let refused = [
            // Index outside the palette in the last word.
            manual(4, Some(vec![5, 6, 8]), &raws),
            // Non-conforming widths: 6 bits for 3 entries, 5 bits for 16 entries.
            manual(6, Some(vec![5, 6, 8]), &(0..4096u32).map(|i| i % 3).collect::<Vec<_>>()),
            manual(5, Some((100..116).collect()), &(0..4096u32).map(|i| i % 16).collect::<Vec<_>>()),
            // Direct narrower than the global width.
            manual(14, None, &(0..4096u32).map(|i| i * 3).collect::<Vec<_>>()),
        ];
        for c in &refused {
            assert!(!container_raw(&mut Vec::new(), c, SECTION_BLOCKS, true, BLOCK_BITS), "{c:?}");
        }
        let conform = manual(5, Some((100..117).collect()), &(0..4096u32).map(|i| i % 17).collect::<Vec<_>>());
        assert!(container_raw(&mut Vec::new(), &conform, SECTION_BLOCKS, true, BLOCK_BITS));
        let mut braws: Vec<u32> = (0..64u32).map(|i| i % 3).collect();
        braws[63] = 3;
        assert!(!container_raw(&mut Vec::new(), &manual(2, Some(vec![4, 8, 12]), &braws), SECTION_BIOMES, false, BIOME_BITS));
    }

    /// Micro-bench (`cargo test --release -p rc-sim lan::encode::container_tests::bench -- --ignored
    /// --nocapture`): µs per block section, old encoder / raw indices / direct palette,
    /// 1 thread then 2, 4… threads up to the number of logical cores.
    #[test]
    #[ignore = "micro-bench"]
    fn bench() {
        use std::hint::black_box;
        use std::time::Instant;
        let sets: Vec<(&str, Vec<Container>)> = vec![
            ("all", block_sections()),
            ("palette<=16", block_sections().into_iter().filter(|c| matches!(c, Container::Packed { bits: 4, palette: Some(_), .. })).collect()),
        ];
        type Enc = fn(&mut Vec<u8>, &Container, usize, bool, u8);
        fn raw_or_remap(w: &mut Vec<u8>, c: &Container, n: usize, b: bool, g: u8) {
            if !container_raw(w, c, n, b, g) {
                container_remap(w, c, n, b, g);
            }
        }
        let encs: [(&str, Enc); 3] = [("old", container_reference), ("remap", container_remap), ("rawpal", raw_or_remap)];
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        for (name, list) in &sets {
            for (en, f) in encs {
                let mut line = format!("{name:12} {en:7}");
                let mut threads = 1;
                while threads <= cores {
                    let reps = 40usize;
                    let mut best = f64::MAX;
                    for _ in 0..5 {
                        let t = Instant::now();
                        std::thread::scope(|s| {
                            for _ in 0..threads {
                                s.spawn(|| {
                                    let mut w = Vec::with_capacity(16 << 10);
                                    for _ in 0..reps {
                                        for c in list {
                                            w.clear();
                                            f(&mut w, c, SECTION_BLOCKS, true, BLOCK_BITS);
                                            black_box(&w);
                                        }
                                    }
                                });
                            }
                        });
                        let per = t.elapsed().as_secs_f64() * 1e6 / (reps * list.len() * threads) as f64;
                        best = best.min(per);
                    }
                    line += &format!("  {threads}t {best:.2}");
                    threads *= 2;
                }
                eprintln!("{line} µs/section ({} sections)", list.len());
            }
        }
    }
}
