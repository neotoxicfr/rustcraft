//! Miscellaneous game-side extras (`docs/specs/miscellaneous.md`): leashes, pickup,
//! screen effects (server and observed entity), projectile power, transient blocks,
//! experimental minecarts, disk alert, server data, subtitles.

use hashbrown::HashMap;
use std::time::{Duration, Instant};

use rc_client::misc::{Misc as Packet, MinecartStep};
use rc_proto_v777::entity::Meta;

use crate::App;
use crate::entities::{Entities, Entity, Kind, rot_lerp};
use crate::post_fx::Effect;

/// Flying copy of a picked-up entity (§ 2.3).
struct Pickup {
    copy: Entity,
    from: [f64; 3],
    collector: i32,
    target: [f64; 3],
    target_o: [f64; 3],
    age: u32,
}

/// Transient block placed by a falling block (§ 5.5).
pub struct Transient {
    pub pos: [i32; 3],
    pub state: u32,
    born: Instant,
}

/// Experimental minecart: queue of steps and batch in progress (§ 5.6).
#[derive(Default)]
struct Cart {
    queue: Vec<MinecartStep>,
    current: Vec<MinecartStep>,
    start: Option<MinecartStep>,
    remaining: i32,
    total: f32,
}

#[derive(Default)]
pub struct Misc {
    /// Leashes: held entity → expected holder (resolved at draw time).
    pub leashes: HashMap<i32, i32>,
    pickups: Vec<Pickup>,
    pub transient: Vec<Transient>,
    /// Screen effects imposed by the server (order received).
    pub server_effects: Vec<String>,
    /// Effect of the observed entity, active (F4 toggles it), and camera state seen at the last frame.
    entity_effect: Option<Effect>,
    effect_on: bool,
    camera_seen: Option<(Option<i32>, bool)>,
    /// Received acceleration power of propelled projectiles.
    pub power: HashMap<i32, f64>,
    carts: HashMap<i32, Cart>,
    /// "Low disk space" notification: last (re)launch.
    disk_toast: Option<Instant>,
    pub subtitles: crate::subtitles::Subtitles,
    /// Server list entry of the current connection: (name, address).
    pub server: Option<(String, String)>,
    /// "Insecure server" notification already shown on this connection: kept across
    /// reconfiguration, reset to false on every new connection, transfer included (the game
    /// restarts from the connection cookie at false).
    pub unsecure_seen: bool,
}

/// Entity by id, local player included.
pub fn entity(ents: &Entities, id: i32) -> Option<&Entity> {
    if id == ents.local && ents.me.is_some() { ents.me.as_ref() } else { ents.map.get(&id) }
}

fn attachable(e: &Entity) -> bool {
    e.class == Kind::Mob || e.kind.ends_with("_boat") || e.kind.ends_with("_raft")
}

/// Propelled projectiles (§ 5.4).
fn propelled(kind: &str) -> bool {
    matches!(
        kind,
        "fireball" | "small_fireball" | "dragon_fireball" | "wither_skull" | "wind_charge" | "breeze_wind_charge"
    )
}

/// Valid PNG of at most 1024 × 1024 (signature and IHDR header).
pub fn valid_icon(png: &[u8]) -> bool {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if png.len() < 24 || png[..8] != SIG || &png[12..16] != b"IHDR" {
        return false;
    }
    let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    (1..=1024).contains(&w) && (1..=1024).contains(&h)
}

fn lerp3(t: f64, a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + t * (b[i] - a[i]))
}

/// Flight duration of a pickup: 3 ticks; `RUSTCRAFT_PICKUPTICKS=<n>` lengthens it (check
/// on screen of the path, too brief for a capture).
fn pickup_life() -> u32 {
    static LIFE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *LIFE.get_or_init(|| {
        rc_data::env_str!("RUSTCRAFT_PICKUPTICKS").and_then(|v| v.parse().ok()).filter(|&n| n > 0).unwrap_or(3)
    })
}

/// Middle of the body: (x, (y + eye y) / 2, z).
fn mid_body(e: &Entity, table: &[[f32; 4]]) -> [f64; 3] {
    let eye = f64::from(e.dims(table)[1]);
    [e.pos[0], e.pos[1] + eye / 2.0, e.pos[2]]
}

impl Misc {
    /// New world: everything that depends on entities is forgotten.
    pub fn reset_world(&mut self) {
        self.leashes.clear();
        self.pickups.clear();
        self.transient.clear();
        self.power.clear();
        self.carts.clear();
        self.camera_seen = None;
        self.entity_effect = None;
    }

    /// "Spectator shader effects" key (F4): toggles the active state without forgetting the effect.
    pub fn toggle_effect(&mut self) {
        if self.entity_effect.is_some() {
            self.effect_on = !self.effect_on;
        }
    }

    /// Screen effects (§ 6.2): server list, then active entity effect. `camera`: observed
    /// entity (other than the player), `kind`: its kind, `detached`: 3rd-person view.
    pub fn effects(&mut self, camera: Option<i32>, kind: Option<&str>, detached: bool) -> Vec<Effect> {
        if self.camera_seen != Some((camera, detached)) {
            self.camera_seen = Some((camera, detached));
            self.entity_effect = if detached { None } else { kind.and_then(Effect::for_camera) };
            self.effect_on = true;
        }
        let mut out: Vec<Effect> = self.server_effects.iter().filter_map(|id| Effect::from_id(id)).collect();
        if let Some(e) = self.entity_effect.filter(|_| self.effect_on) {
            out.push(e);
        }
        out
    }

    /// Flying copies of the frame, at their interpolated position.
    pub fn pickup_copies(&self, pt: f32) -> Vec<Entity> {
        self.pickups
            .iter()
            .map(|p| {
                let t = ((f64::from(p.age) + f64::from(pt)) / f64::from(pickup_life())).powi(2);
                let target = lerp3(f64::from(pt), p.target_o, p.target);
                let pos = lerp3(t, p.from, target);
                let mut e = p.copy.clone();
                (e.pos, e.pos_o) = (pos, pos);
                e
            })
            .collect()
    }

    /// A column mesh accepted: its transient blocks are now in it.
    pub fn column_meshed(&mut self, x: i32, z: i32) {
        self.transient.retain(|t| (t.pos[0] >> 4, t.pos[2] >> 4) != (x, z));
    }

    /// "Low disk space" notification (§ 5.7), without a notification manager: top
    /// right, no sliding.
    pub fn draw_toast(&mut self, gui: &mut crate::gui::Gui, queue: &wgpu::Queue, duration: f64) {
        let Some(at) = self.disk_toast else { return };
        if at.elapsed() > Duration::from_secs_f64(10.0 * duration) {
            self.disk_toast = None;
            return;
        }
        gui.next_layer();
        let flat = |key: &str, gui: &crate::gui::Gui| {
            let c = rc_text::Component::translate(key, vec![]);
            rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang))
        };
        let title = flat("chunk.toast.lowDiskSpace", gui);
        let message = flat("chunk.toast.lowDiskSpace.description", gui);
        let titles = rc_text::wrap(gui, &title, 200.0);
        let lines = rc_text::wrap(gui, &message, 200.0);
        let widest = titles.iter().chain(&lines).map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
        let w = (widest.max(160) + 30) as f32;
        let h = (20 + 12 * (titles.len().max(1) as i32 - 1) + 12 * lines.len().max(1) as i32) as f32;
        let x = gui.width.floor() - w;
        gui.sprite("toast/system", x, 0.0, w, h, 0xffff_ffff);
        let mut y = if lines.is_empty() { 12.0 } else { 7.0 };
        for t in &titles {
            gui.text(queue, t, x + 18.0, y, 0xffff_ff00, false, 0);
            y += 12.0;
        }
        for l in &lines {
            gui.text(queue, l, x + 18.0, y, 0xffff_ffff, false, 0);
            y += 12.0;
        }
    }
}

impl App {
    /// Packets of `rc_client::misc`.
    pub(crate) fn apply_misc(&mut self, d: Packet) {
        match d {
            Packet::Link { held, holder } => {
                // Only mobs, boats and rafts attach; detaching stays local.
                if self.entities.map.get(&held).is_some_and(attachable) {
                    if holder == 0 {
                        self.misc.leashes.remove(&held);
                    } else {
                        self.misc.leashes.insert(held, holder);
                    }
                }
            }
            Packet::Take { item, collector, amount } => self.on_take(item, collector, amount),
            Packet::PostEffects(list) => self.misc.server_effects = list,
            Packet::ProjectilePower { id, power } => {
                if self.entities.map.get(&id).is_some_and(|e| propelled(&e.kind)) {
                    self.misc.power.insert(id, power);
                }
            }
            Packet::TransientBlock { pos, state } => {
                let p = [pos.x, pos.y, pos.z];
                self.misc.transient.retain(|t| t.pos != p);
                let state = u32::try_from(state).unwrap_or(0);
                self.misc.transient.push(Transient { pos: p, state, born: Instant::now() });
            }
            Packet::MinecartSteps { id, steps } => {
                if self.exact.features.iter().any(|f| f == "minecraft:minecart_improvements")
                    && self.entities.map.get(&id).is_some_and(|e| e.kind.ends_with("minecart"))
                {
                    self.misc.carts.entry(id).or_default().queue.extend(steps);
                }
            }
            Packet::LowDiskSpace => self.misc.disk_toast = Some(Instant::now()),
            Packet::ServerData { motd: _, icon } => {
                // MOTD kept by the in-memory entry only (nothing in game); valid icon written
                // to `servers.dat` in a background task.
                if let (Some((name, ip)), Some(icon)) = (self.misc.server.clone(), icon.filter(|i| valid_icon(i))) {
                    let _ = std::thread::Builder::new().name("servers.dat".into()).spawn(move || {
                        let dir = crate::servers::game_dir();
                        if let Err(e) = crate::servers::ServerList::update_icon(&dir, &name, &ip, Some(icon)) {
                            eprintln!("servers.dat: {e}");
                        }
                    });
                }
            }
        }
    }

    /// `take_item_entity` (§ 2).
    fn on_take(&mut self, item: i32, collector: i32, amount: i32) {
        let Some(e) = self.entities.map.get(&item) else { return };
        if !e.meta_bool(4)
            && let Some(a) = self.audio.get()
        {
            let (r1, r2) = (self.world_random.f(), self.world_random.f());
            let (name, volume, pitch) = if e.kind == "experience_orb" {
                ("minecraft:entity.experience_orb.pickup", 0.1, (r1 - r2) * 0.35 + 0.9)
            } else {
                ("minecraft:entity.item.pickup", 0.2, (r1 - r2) * 1.4 + 2.0)
            };
            a.play(name, rc_audio::Category::Player, Some(e.pos), volume, pitch, None);
        }
        let table = self.renderer.as_ref().and_then(|r| r.entities.as_ref()).map_or(&[][..], |r| r.dims.as_slice());
        if let Some(c) = entity(&self.entities, collector) {
            // Frozen copy before the stack is reduced, no highlight outline.
            let mut copy = e.clone();
            if let Some(Meta::Byte(f)) = copy.meta.get(&0).cloned() {
                copy.meta.insert(0, Meta::Byte(f & !0x40));
            }
            let target = mid_body(c, table);
            self.misc.pickups.push(Pickup { copy, from: e.pos, collector, target, target_o: target, age: 0 });
        }
        let kind = e.kind.clone();
        match kind.as_str() {
            "item" => {
                let Some(e) = self.entities.map.get_mut(&item) else { return };
                let empty = match e.meta.get(&8).cloned() {
                    Some(Meta::Item(Some(mut stack))) => {
                        stack.count -= amount;
                        let empty = stack.count <= 0;
                        e.meta.insert(8, Meta::Item(Some(stack)));
                        empty
                    }
                    _ => true,
                };
                if empty {
                    self.entities.map.remove(&item);
                }
            }
            "experience_orb" => {}
            _ => {
                self.entities.map.remove(&item);
            }
        }
    }

    /// Client tick, after that of the entities.
    pub(crate) fn misc_tick(&mut self, runs: bool) {
        let table = self.renderer.as_ref().and_then(|r| r.entities.as_ref()).map_or(&[][..], |r| r.dims.as_slice());
        let ents = &mut self.entities;
        let d = &mut self.misc;
        // Leashes: held entities that have disappeared are forgotten.
        d.leashes.retain(|held, _| ents.map.contains_key(held));
        d.transient.retain(|t| t.born.elapsed() < Duration::from_secs(1));
        // Pickups: target re-read (memory of the previous tick), removed at age 3.
        for p in &mut d.pickups {
            p.age += 1;
            p.target_o = p.target;
            if let Some(c) = entity(ents, p.collector) {
                p.target = mid_body(c, table);
            }
        }
        d.pickups.retain(|p| p.age < pickup_life());
        if !runs {
            return;
        }
        // Propelled projectiles: received power kept for their tick (`projectile_sim.rs`).
        d.power.retain(|id, _| ents.map.contains_key(id));
        // Experimental minecarts: batch of 3 ticks shared according to the weights.
        d.carts.retain(|id, _| ents.map.contains_key(id));
        for (id, cart) in &mut d.carts {
            let Some(e) = ents.map.get_mut(id) else { continue };
            cart.remaining -= 1;
            if cart.remaining <= 0 {
                cart.start =
                    Some(MinecartStep { pos: e.pos, vel: e.velocity, yaw: e.yaw, pitch: e.pitch, weight: 0.0 });
                cart.current.clear();
                if !cart.queue.is_empty() {
                    cart.current = std::mem::take(&mut cart.queue);
                    cart.total = cart.current.iter().map(|s| s.weight).filter(|w| *w > 0.0).sum();
                    cart.remaining = if cart.total == 0.0 { 0 } else { 3 };
                }
            }
            if let Some(s) = cart.at(1.0) {
                (e.pos, e.velocity, e.yaw, e.pitch) = (s.pos, s.vel, s.yaw, s.pitch);
            }
        }
    }
}

impl Cart {
    /// State interpolated by steps at fraction `pt`.
    fn at(&self, pt: f32) -> Option<MinecartStep> {
        let first = *self.current.first()?;
        let alpha = (3.0 - self.remaining as f32 + pt) / 3.0;
        let goal = self.total * alpha;
        let mut acc = 0.0;
        let mut prev = self.start.unwrap_or(first);
        for s in &self.current {
            if s.weight <= 0.0 {
                continue;
            }
            if acc + s.weight >= goal {
                let f = ((goal - acc) / s.weight).clamp(0.0, 1.0);
                return Some(lerp_step(f, &prev, s));
            }
            acc += s.weight;
            prev = *s;
        }
        let last = *self.current.last()?;
        Some(lerp_step(1.0, &prev, &last))
    }
}

fn lerp_step(f: f32, a: &MinecartStep, b: &MinecartStep) -> MinecartStep {
    let t = f64::from(f);
    MinecartStep {
        pos: lerp3(t, a.pos, b.pos),
        vel: lerp3(t, a.vel, b.vel),
        yaw: rot_lerp(f, a.yaw, b.yaw),
        pitch: rot_lerp(f, a.pitch, b.pitch),
        weight: 0.0,
    }
}

/// Light (block, sky) at the block of an entity's eyes: block 15 if it is burning.
fn eye_light(e: &Entity, pt: f32, table: &[[f32; 4]], light: &dyn Fn(i32, i32, i32) -> (u8, u8)) -> (u8, u8) {
    let p = e.render_pos(pt);
    let y = p[1] + f64::from(e.dims(table)[1]);
    let (b, s) = light(p[0].floor() as i32, y.floor() as i32, p[2].floor() as i32);
    (if e.flags() & 1 != 0 { 15 } else { b }, s)
}

/// Vertices of the leash ropes of the frame (visible held entities: their box, that of the holder or
/// their union in the field of view — the union contains both).
#[allow(clippy::too_many_arguments)]
pub fn leash_vertices(
    d: &Misc,
    ents: &Entities,
    ctx: &crate::leash::Ctx<'_>,
    origin: [i32; 2],
    cam: [f64; 3],
    light: &dyn Fn(i32, i32, i32) -> (u8, u8),
    visible: &dyn Fn([f32; 3], [f32; 3]) -> bool,
) -> Vec<crate::leash::Vertex> {
    let mut out = Vec::new();
    for (&held, &holder) in &d.leashes {
        let (Some(e), Some(h)) = (ents.map.get(&held), entity(ents, holder)) else { continue };
        let (p, q) = (e.render_pos(ctx.pt), h.render_pos(ctx.pt));
        if (0..3).map(|i| (p[i] - cam[i]).powi(2)).sum::<f64>() > 128.0 * 128.0 {
            continue;
        }
        let (ew, eh, _) = crate::leash::size(e, ctx.table);
        let (hw, hh, _) = crate::leash::size(h, ctx.table);
        let rel = |v: f64, i: usize| match i {
            0 => (v - f64::from(origin[0])) as f32,
            2 => (v - f64::from(origin[1])) as f32,
            _ => v as f32,
        };
        let lo: [f32; 3] = std::array::from_fn(|i| {
            let (a, b) = if i == 1 { (p[1], q[1]) } else { (p[i] - ew / 2.0, q[i] - hw / 2.0) };
            rel(a.min(b), i)
        });
        let hi: [f32; 3] = std::array::from_fn(|i| {
            let (a, b) = if i == 1 { (p[1] + eh, q[1] + hh) } else { (p[i] + ew / 2.0, q[i] + hw / 2.0) };
            rel(a.max(b), i)
        });
        if !visible(lo, hi) {
            continue;
        }
        let lights = [eye_light(e, ctx.pt, ctx.table, light), eye_light(h, ctx.pt, ctx.table, light)];
        for rope in crate::leash::ropes(e, h, holder, ctx) {
            crate::leash::geometry(&rope, lights, origin, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_check() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&64u32.to_be_bytes());
        png.extend_from_slice(&64u32.to_be_bytes());
        assert!(valid_icon(&png));
        png[16..20].copy_from_slice(&2048u32.to_be_bytes());
        assert!(!valid_icon(&png));
        assert!(!valid_icon(b"GIF89a"));
    }

    #[test]
    fn cart_steps_share_three_ticks_by_weight() {
        let s = |x: f64, w: f32| MinecartStep { pos: [x, 0.0, 0.0], vel: [0.0; 3], yaw: 0.0, pitch: 0.0, weight: w };
        let cart = Cart {
            queue: vec![],
            current: vec![s(1.0, 1.0), s(3.0, 2.0)],
            start: Some(s(0.0, 0.0)),
            remaining: 3,
            total: 3.0,
        };
        // α = 1/3: end of the first step; α = 2/3: middle of the second.
        assert!((cart.at(1.0).unwrap().pos[0] - 1.0).abs() < 1e-6);
        assert!((cart.at(2.0).unwrap().pos[0] - 2.0).abs() < 1e-6);
    }
}
