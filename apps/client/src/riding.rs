//! Mounts and vehicles (`mounts.md`) and orientation imposed by the server
//! (`protocol-supplements.md` § 4 to 7): `player_rotation`, `player_look_at`, `set_camera`,
//! `move_vehicle`.
//!
//! Every tick: local player mounting / dismounting, physics of the root vehicle they pilot
//! (`rc_physics::mounts`, input of the previous tick), placement of all the passengers on their
//! seat, clamping of the view in a boat, riding sounds; then sending of the rotation and the vehicle.

use hashbrown::HashMap;

use rc_client::link::Move;
use rc_physics::mounts::{Controls, Kind as MountKind, Mount, Pilot};
use rc_proto_v777::entity::Meta;

use crate::App;
use crate::entities::{Entity, Kind};

/// Brings an angle back into [−180, 180).
fn wrap(a: f32) -> f32 {
    let r = (a + 180.0).rem_euclid(360.0) - 180.0;
    if r >= 180.0 { r - 360.0 } else { r }
}

/// Minecart that carries a villager or a wandering trader: attachment point lowered to its
/// origin (the kind tested is the passenger's, not the vehicle's).
fn lowered_seat(vehicle: &str, rider: Option<&str>) -> bool {
    vehicle.ends_with("minecart") && matches!(rider, Some("villager" | "wandering_trader"))
}

fn is_boat(kind: &str) -> bool {
    kind.ends_with("_boat") || kind.ends_with("_raft")
}

fn is_equine(kind: &str) -> bool {
    matches!(kind, "horse" | "donkey" | "mule" | "skeleton_horse" | "zombie_horse" | "camel" | "camel_husk")
}

/// "Passenger" attachment point of the types that declare one (height; setback for a few).
fn declared_point(kind: &str) -> Option<[f64; 3]> {
    let y = |v: f64| Some([0.0, v, 0.0]);
    match kind {
        "cat" => y(0.5125),
        "chicken" => Some([0.0, 0.7, -0.1]),
        "cow" | "mooshroom" => y(1.36875),
        "donkey" => y(1.1125),
        "drowned" | "zombie" | "piglin" | "piglin_brute" => y(2.0125),
        "elder_guardian" => y(2.350625),
        "enderman" => y(2.80625),
        "endermite" | "silverfish" => y(0.2375),
        "ender_dragon" => y(3.0),
        "evoker" | "illusioner" | "pillager" | "vindicator" | "zombified_piglin" => y(2.0),
        "fox" => Some([0.0, 0.6375, -0.25]),
        "frog" => Some([0.0, 0.375, -0.25]),
        "ghast" => y(4.0625),
        "goat" => y(1.1125),
        "guardian" => y(0.975),
        "hoglin" | "zoglin" => y(1.49375),
        "horse" => y(1.44375),
        "husk" => y(2.075),
        "llama" | "trader_llama" => Some([0.0, 1.37, -0.3]),
        "mule" => y(1.2125),
        "nautilus" | "zombie_nautilus" => y(1.1375),
        "ocelot" => y(0.6375),
        "parrot" => y(0.4625),
        "phantom" => y(0.3375),
        "pig" => y(0.86875),
        "ravager" => Some([0.0, 2.2625, -0.0625]),
        "sheep" => y(1.2375),
        "skeleton_horse" | "zombie_horse" => y(1.31875),
        "sniffer" => y(2.09375),
        "spider" | "cave_spider" => y(0.765),
        "turtle" => Some([0.0, 0.55625, -0.25]),
        "vex" => y(0.7375),
        "warden" => y(3.15),
        "witch" => y(2.2625),
        "wolf" => Some([0.0, 0.81875, -0.0625]),
        "zombie_villager" => y(2.125),
        k if k.ends_with("minecart") => y(0.1875),
        _ => None,
    }
}

/// "Vehicle" attachment of a passenger `r` (height removed from the seat's attachment point), following
/// its scale attribute `scale`. Riding offset r of the types that declare one: attachment =
/// (0, −r, 0); baby zombie, drowned, husk, piglin and zombified piglin have their own
/// value (0.1875), the zombie villager too (0.125).
pub(crate) fn rider_attach(r: &Entity, scale: f64) -> f64 {
    if matches!(r.kind.as_str(), "player" | "mannequin") {
        return 0.6 * scale;
    }
    let base = if r.baby() {
        match r.kind.as_str() {
            "zombie" | "husk" | "drowned" | "piglin" | "zombified_piglin" => 0.1875,
            "zombie_villager" => 0.125,
            k => -riding_offset(k) * 0.5,
        }
    } else {
        -riding_offset(&r.kind)
    };
    base * scale
}

fn riding_offset(kind: &str) -> f64 {
    match kind {
        "allay" | "vex" => 0.04,
        "bogged" | "drowned" | "husk" | "parched" | "piglin" | "piglin_brute" | "skeleton" | "stray" | "zombie"
        | "zombie_villager" | "zombified_piglin" => -0.7,
        "evoker" | "illusioner" | "pillager" | "vindicator" => -0.6,
        "ghast" | "happy_ghast" => 0.5,
        "giant" => -3.75,
        "phantom" => -0.125,
        "wither_skeleton" => -0.875,
        _ => 0.0,
    }
}

/// Physical kind of a vehicle that the client knows how to pilot.
fn mount_kind(kind: &str) -> Option<MountKind> {
    Some(match kind {
        k if is_boat(k) => MountKind::Boat,
        "horse" | "donkey" | "mule" | "zombie_horse" => MountKind::Equine { float: true, water_slowdown: 0.8 },
        "skeleton_horse" => MountKind::Equine { float: false, water_slowdown: 0.96 },
        "camel" | "camel_husk" => MountKind::Camel,
        "pig" => MountKind::Pig,
        "strider" => MountKind::Strider,
        "happy_ghast" => MountKind::HappyGhast,
        "nautilus" | "zombie_nautilus" => MountKind::Nautilus,
        _ => return None,
    })
}

/// Default attributes of the type (the server only sends those that differ).
fn default_attrs(kind: &str) -> rc_physics::mounts::Attrs {
    let mut a = rc_physics::mounts::Attrs::default();
    let f = |v: f32| f64::from(v);
    let (speed, jump, step) = match kind {
        "horse" | "zombie_horse" => (f(0.225), 0.7, 1.0),
        "donkey" | "mule" => (f(0.175), 0.5, 1.0),
        "skeleton_horse" => (f(0.2), 0.7, 1.0),
        "camel" | "camel_husk" => (f(0.09), f(0.42), 1.5),
        "pig" => (0.25, a.jump_strength, a.step_height),
        "strider" => (f(0.175), a.jump_strength, a.step_height),
        "happy_ghast" => {
            a.flying_speed = 0.05;
            (0.05, a.jump_strength, a.step_height)
        }
        "nautilus" => (1.0, a.jump_strength, a.step_height),
        "zombie_nautilus" => (f(1.1), a.jump_strength, a.step_height),
        _ => (a.movement_speed, a.jump_strength, a.step_height),
    };
    (a.movement_speed, a.jump_strength, a.step_height) = (speed, jump, step);
    a
}

/// Camel posture from the date of its last change (meta 20): (sitting, in
/// transition, posture time).
fn camel_pose(e: &Entity, game_time: i64) -> (bool, bool, i64) {
    let t = match e.meta.get(&20) {
        Some(Meta::Long(t)) => *t,
        _ => 0,
    };
    let sitting = t < 0;
    let time = game_time - t.abs();
    (sitting, time < if sitting { 40 } else { 52 }, time)
}

/// Camel seat height (§ 3.4): `h` current height, `s` scale, `front` driver.
fn camel_seat_height(h: f64, s: f32, baby: bool, front: bool, pose: (bool, bool, i64)) -> f64 {
    let (sitting, transition, time) = pose;
    let mut base = h - if baby { 0.093_75 } else { 0.375 };
    let d = s * 1.43;
    let drop = d - s * 0.2;
    let bottom = d - drop;
    if transition {
        let duration = if sitting { 40 } else { 52 };
        let (half, flex) = if sitting {
            (28, if front { 0.5 } else { 0.1 })
        } else {
            (if front { 24 } else { 32 }, if front { 0.6 } else { 0.35 })
        };
        let t = (time as f32).clamp(0.0, duration as f32);
        let first = t < half as f32;
        let part = if first { t / half as f32 } else { (t - half as f32) / (duration - half) as f32 };
        let flex_pt = d - flex * drop;
        let lerp = |a: f32, b: f32| a + part * (b - a);
        base += f64::from(if sitting {
            if first { lerp(d, flex_pt) } else { lerp(flex_pt, bottom) }
        } else if first {
            lerp(bottom - d, bottom - flex_pt)
        } else {
            lerp(bottom - flex_pt, 0.0)
        });
    }
    if sitting && !transition {
        base += f64::from(bottom);
    }
    base
}

/// Looping riding sound played by the client (§ 5.4).
struct RideSound {
    handle: rc_audio::SoundHandle,
    vehicle: i32,
    /// Minecart: underwater (sound of the submerged interior); ghast, nautilus: single sound.
    kind: RideSoundKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RideSoundKind {
    Minecart { underwater: bool },
    Ghast,
    Nautilus,
}

/// Keyboard-free piloting script (`RUSTCRAFT_RIDETEST`): `wait:n`, `keys:<wasdjcr>:n`,
/// `look:yaw:pitch`, `cmd:command`, `log` (position of the mount).
struct Script {
    steps: Vec<Vec<String>>,
    at: usize,
    wait: u32,
    keys: rc_physics::Input,
    /// Keys released at the end of the current wait (`keys:…:n`, n > 0).
    release: bool,
}

/// State of the mounts in `App`.
#[derive(Default)]
pub(crate) struct Riding {
    /// Attributes received from the other entities (name → value).
    attrs: HashMap<i32, HashMap<String, f64>>,
    /// Locally simulated vehicle (root) and its physics.
    sim: Option<(i32, Mount)>,
    /// Inputs set by the player at the previous tick.
    controls: Controls,
    /// Direct vehicle of the local player at the previous tick.
    vehicle: Option<i32>,
    /// Jump strength sent this tick, given to the mount after its physics.
    pub jump: Option<i32>,
    /// Piloted boat and movement key held: attack and use without effect.
    pub hands_busy: bool,
    /// Acceleration duration seen (pig, strider), to detect a new value.
    boost_seen: HashMap<i32, i32>,
    sounds: Vec<RideSound>,
    /// Sustained sounds of the other entities (minecarts, bees, bubble column).
    pub(crate) loops: crate::entity_loops::Loops,
    script: Option<Script>,
    /// View yaw added by the boat's rotation at the last tick (camera smoothing).
    pub turn: f32,
}

impl Riding {
    pub fn new() -> Self {
        let script = rc_data::env_str!("RUSTCRAFT_RIDETEST").map(|s| Script {
            steps: s.split(';').filter(|x| !x.is_empty()).map(|x| x.split(':').map(str::to_owned).collect()).collect(),
            at: 0,
            wait: 0,
            keys: Default::default(),
            release: false,
        });
        Riding { script, ..Default::default() }
    }

    /// Attributes of another entity (`update_attributes`).
    pub fn attributes(&mut self, entity: i32, list: &[rc_proto_v777::play::Attribute], names: &[String]) {
        let map = self.attrs.entry(entity).or_default();
        for a in list {
            if let Some(n) = names.get(a.id as usize) {
                let v = crate::hud_more::attribute_value(n, a);
                if rc_data::env_flag!("RUSTCRAFT_RIDELOG") {
                    eprintln!("attribute {entity} {n} = {v}");
                }
                map.insert(n.clone(), v);
            }
        }
    }

    /// Piloting script active (clicks accepted without a captured mouse).
    pub fn scripted(&self) -> bool {
        self.script.is_some()
    }

    /// Jump key held by the test script.
    pub fn script_jump(&self) -> bool {
        self.script.as_ref().is_some_and(|s| s.keys.jump)
    }
}

impl App {
    /// Vehicle of which the local player is a passenger.
    pub(crate) fn vehicle(&self) -> Option<i32> {
        self.entities.map.iter().find(|(_, e)| e.passengers.contains(&self.player_id)).map(|(id, _)| *id)
    }

    /// Root vehicle (the one that mounts nothing).
    fn root_vehicle(&self, mut id: i32) -> i32 {
        for _ in 0..8 {
            match self.entities.map.iter().find(|(_, e)| e.passengers.contains(&id)) {
                Some((v, _)) => id = *v,
                None => break,
            }
        }
        id
    }

    fn stack_item_name(&self, stack: Option<&rc_proto_v777::item::ItemStack>) -> &str {
        stack
            .and_then(|s| usize::try_from(s.item).ok())
            .and_then(|i| self.item_names.get(i))
            .map_or("", |n| n.strip_prefix("minecraft:").unwrap_or(n))
    }

    /// Does the client pilot this root vehicle (§ 2, local authority)?
    fn local_authority(&self, root: i32) -> bool {
        let Some(v) = self.entities.map.get(&root) else { return false };
        if v.passengers.first() != Some(&self.player_id) {
            return false;
        }
        let saddled = !self.stack_item_name(v.equipment[7].as_ref()).is_empty();
        let holds = |name: &str| {
            [self.inventory.hotbar(self.hud.selected), self.inventory.offhand()]
                .into_iter()
                .any(|s| self.stack_item_name(s) == name)
        };
        let k = v.kind.as_str();
        if is_boat(k) {
            return true;
        }
        match k {
            "horse" | "donkey" | "mule" | "skeleton_horse" | "zombie_horse" | "camel" | "camel_husk" | "nautilus"
            | "zombie_nautilus" => saddled,
            "pig" => saddled && holds("carrot_on_a_stick"),
            "strider" => saddled && holds("warped_fungus_on_a_stick"),
            // Harness ("body" slot) and not standing still (meta 19).
            "happy_ghast" => v.equipment[6].is_some() && !v.meta_bool(19),
            _ => false,
        }
    }

    /// Do the normal physics and position sending apply this tick? No if the camera follows
    /// another entity or if the player is a passenger.
    pub(crate) fn moves_freely(&self) -> bool {
        self.exact.camera.is_none() && self.vehicle().is_none()
    }

    fn dims_table(&self) -> &[[f32; 4]] {
        self.renderer.as_ref().and_then(|r| r.entities.as_ref()).map_or(&[][..], |e| e.dims.as_slice())
    }

    /// Width (dimension table) and current height of an entity.
    fn width_height(&self, e: &Entity) -> (f64, f64) {
        let s = e.base_size(self.dims_table());
        (f64::from(s.w), f64::from(s.h))
    }

    fn entity_scale(&self, id: i32) -> f64 {
        self.riding.attrs.get(&id).and_then(|m| m.get("minecraft:scale")).copied().unwrap_or(1.0)
    }

    // ---------- Tick ----------

    /// Tick of the passenger player and of the vehicles: physics of the piloted vehicle, seats, view.
    pub(crate) fn ride_tick(&mut self, input: &mut rc_physics::Input) {
        self.ride_script(input);
        let vehicle = self.vehicle();
        if vehicle != self.riding.vehicle {
            self.vehicle_changed(self.riding.vehicle, vehicle);
            self.riding.vehicle = vehicle;
        }
        let root = vehicle.map(|v| self.root_vehicle(v));
        let authority = root.is_some_and(|r| self.local_authority(r));
        let mut rotation = 0.0;
        match root {
            Some(r) if authority => rotation = self.simulate(r),
            _ => self.riding.sim = None,
        }
        self.riding.jump = None;
        self.place_passengers();
        self.tick_ride_sounds();
        self.tick_loop_sounds();
        self.riding.turn = 0.0;
        self.riding.hands_busy = false;
        if let Some(h) = &mut self.camera.hand {
            h.busy = false;
        }
        // Local player drawn seated on their mount.
        if let Some(me) = &mut self.entities.me {
            me.vehicle = vehicle;
        }
        let Some(v) = vehicle else { return };
        // Passenger: target posture (sneaking during the dismount), no swimming.
        self.player.crouching = input.shift;
        self.player.swimming = false;
        self.player.set_passenger_pose(input.shift);
        self.player.vel = [0.0; 3];
        self.player.fall_distance = 0.0;
        // Sprint: only on a piloted camel.
        let camel =
            authority && root.and_then(|r| self.entities.map.get(&r)).is_some_and(|e| e.kind.starts_with("camel"));
        let sprint = if camel { (self.player.sprinting || input.sprint) && input.forward } else { false };
        if sprint != self.player.sprinting {
            self.player.sprinting = sprint;
            use rc_client::link::action;
            let _ = self.session.link.send_player_command(if sprint {
                action::START_SPRINTING
            } else {
                action::STOP_SPRINTING
            });
        }
        // Boat: boat rotation added to the view, then clamping; boat inputs.
        let boat = self.entities.map.get(&v).filter(|e| is_boat(&e.kind)).map(|e| e.passengers.first().copied());
        if let Some(first) = boat {
            self.yaw += rotation;
            self.riding.turn = rotation;
            self.clamp_boat_view();
            if authority && first == Some(self.player_id) {
                self.riding.hands_busy = input.left || input.right || input.forward || input.backward;
            }
        }
        self.player.yaw = self.yaw;
        self.player.pitch = self.pitch;
        self.riding.controls = Controls::from_input(*input);
        if let Some(h) = &mut self.camera.hand {
            h.busy = self.riding.hands_busy;
        }
    }

    /// Third-person camera setback due to the living mount (§ 5.2): scale ×
    /// `camera_distance` (happy ghast 8, otherwise 4); 0 without a living mount.
    pub(crate) fn vehicle_camera_distance(&self) -> f64 {
        let Some((id, e)) = self.riding.vehicle.and_then(|v| self.entities.map.get(&v).map(|e| (v, e))) else {
            return 0.0;
        };
        if e.class == Kind::Other {
            return 0.0;
        }
        let a = self.riding.attrs.get(&id);
        let dist = a.and_then(|m| m.get("minecraft:camera_distance")).copied();
        let dist = dist.unwrap_or(if e.kind == "happy_ghast" { 8.0 } else { 4.0 }).clamp(0.0, 32.0);
        let age = if e.baby() { 0.5 } else { 1.0 };
        self.entity_scale(id) * age * dist
    }

    /// Clamping of the view of a boat passenger to ±105° of the boat (§ 1.4), also applied on
    /// every mouse movement.
    pub(crate) fn clamp_boat_view(&mut self) {
        let Some(v) = self.riding.vehicle else { return };
        let Some(e) = self.entities.map.get(&v) else { return };
        if !is_boat(&e.kind) {
            return;
        }
        let d = wrap(self.yaw - e.yaw);
        self.yaw += d.clamp(-105.0, 105.0) - d;
        self.player.yaw = self.yaw;
    }

    /// Mounting or dismounting of the local player (§ 1.1, 1.5).
    fn vehicle_changed(&mut self, old: Option<i32>, new: Option<i32>) {
        if rc_client::protolog::enabled() {
            eprintln!("mount: {old:?} → {new:?}");
        }
        self.stop_ride_sounds();
        if self.player.sprinting {
            self.player.sprinting = false;
            let _ = self.session.link.send_player_command(rc_client::link::action::STOP_SPRINTING);
        }
        let Some(v) = new else { return };
        let Some(e) = self.entities.map.get(&v) else { return };
        let kind = e.kind.clone();
        if is_boat(&kind) {
            self.yaw = e.yaw;
        } else if is_equine(&kind) || kind.ends_with("llama") {
            // Mount view, old values (no smoothing).
            self.yaw = e.yaw_o.rem_euclid(360.0);
            self.pitch = e.pitch_o.clamp(-90.0, 90.0);
        }
        (self.player.yaw, self.player.pitch) = (self.yaw, self.pitch);
        if old.is_none() {
            let key = match self.options.s("key_key.sneak") {
                "" => "key.keyboard.left.shift".to_owned(),
                k => k.to_owned(),
            };
            let msg = if kind == "cushion" { "mount.sit" } else { "mount.onboard" };
            let arg = rc_text::Arg::Component(rc_text::Component::translate(key, Vec::new()));
            self.chat.action_bar(rc_text::Component::translate(msg, vec![arg]));
        }
        self.start_ride_sounds(v, &kind);
    }

    /// Physics of the piloted root vehicle; returns its rotation per tick (boat).
    fn simulate(&mut self, root: i32) -> f32 {
        let Some(e) = self.entities.map.get(&root) else { return 0.0 };
        let Some(kind) = mount_kind(&e.kind) else { return 0.0 };
        let (w, h) = self.width_height(e);
        let mut m = match self.riding.sim.take() {
            Some((id, m)) if id == root && m.kind == kind => m,
            _ => {
                let mut m = Mount::new(kind, e.pos, e.yaw, e.pitch, w, h);
                if kind != MountKind::Boat {
                    m.vel = e.velocity;
                }
                m
            }
        };
        // Position and rotation of the entity (repositioning by `move_vehicle` included).
        (m.pos, m.yaw, m.pitch, m.width, m.height) = (e.pos, e.yaw, e.pitch, w, h);
        m.attrs = default_attrs(&e.kind);
        if let Some(a) = self.riding.attrs.get(&root) {
            for (n, v) in a {
                m.attrs.set(n, *v);
            }
        }
        m.fast_lava = self.player.fast_lava;
        let game_time = self.env.env.clocks.game_time;
        match kind {
            MountKind::Equine { .. } => m.standing = e.meta_byte(18) & 0x20 != 0,
            MountKind::Camel => {
                let (sitting, transition, _) = camel_pose(e, game_time);
                m.refuse_move = sitting || transition;
                m.sitting_still = sitting && !transition;
                m.standing = false;
            }
            MountKind::Pig | MountKind::Strider => {
                let total = e.meta_int(18).unwrap_or(0);
                if self.riding.boost_seen.insert(root, total).is_some_and(|old| old != total) && total > 0 {
                    m.start_boost(total);
                }
                m.suffocating = kind == MountKind::Strider && e.meta_bool(19);
            }
            _ => {}
        }
        let pilot = Pilot {
            yaw: self.yaw,
            pitch: self.pitch,
            controls: self.riding.controls,
            sprinting: self.player.sprinting,
        };
        // Solid entities, except the vehicle itself.
        let colliders = crate::entity_motion::colliders(&self.entities, Some(root), None);
        let world = crate::GridWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            t: &self.phys,
            border: &self.weather.border,
            colliders: &colliders,
            movers: Some(&self.block_entities),
            noclip: None,
        };
        let out = m.tick(&pilot, &world, &self.phys);
        if let Some((l, r)) = out.paddles {
            let _ = self.session.link.send_paddle_boat(l, r);
        }
        // The jump strength of this tick only acts on the next tick.
        if let Some(n) = self.riding.jump {
            m.on_jump(n);
        }
        if let Some(e) = self.entities.map.get_mut(&root) {
            e.pos = m.pos;
            e.yaw = m.yaw;
            e.pitch = m.pitch;
            e.on_ground = m.on_ground;
            e.velocity = m.vel;
            if kind != MountKind::Boat {
                // Piloted setting: old yaw = body = head = yaw.
                e.yaw_o = m.yaw;
                (e.body, e.head) = (m.yaw, m.yaw);
            } else {
                (e.body, e.head) = (m.yaw, m.yaw);
                e.meta.insert(11, Meta::Bool(m.paddles.0));
                e.meta.insert(12, Meta::Bool(m.paddles.1));
            }
            e.redo_walk();
            if out.stand_up {
                e.meta.insert(20, Meta::Long(game_time));
            }
        }
        if out.stand_up
            && let Some(a) = self.audio.get()
        {
            a.play("minecraft:entity.camel.stand", rc_audio::Category::Neutral, Some(m.pos), 1.0, 1.0, None);
        }
        if rc_data::env_flag!("RUSTCRAFT_RIDELOG") {
            eprintln!(
                "mount {root} {:?}: pos {:.5} {:.5} {:.5} vel {:.5} {:.5} {:.5} yaw {:.3} ground {} state {:?}",
                kind, m.pos[0], m.pos[1], m.pos[2], m.vel[0], m.vel[1], m.vel[2], m.yaw, m.on_ground, m.status
            );
        }
        let rotation = if kind == MountKind::Boat { m.rotation } else { 0.0 };
        self.riding.sim = Some((root, m));
        rotation
    }

    // ---------- Seats ----------

    /// "Passenger" attachment point of vehicle `v` for rank `rank` (before rotation).
    fn passenger_point(&self, v: &Entity, vid: i32, rank: usize, rider: Option<&str>, rider_animal: bool) -> [f64; 3] {
        let k = v.kind.as_str();
        if lowered_seat(k, rider) {
            return [0.0; 3];
        }
        let count = v.passengers.len();
        let (_, h) = self.width_height(v);
        if is_boat(k) {
            let y = if k.ends_with("_raft") { f64::from(h as f32 * 0.888_888_9) } else { f64::from(h as f32 / 3.0) };
            let mut z: f32 = if k.contains("chest") { 0.15 } else { 0.0 };
            if count > 1 {
                z = if rank == 0 { 0.2 } else { -0.6 };
                if rider_animal {
                    z += 0.2;
                }
            }
            return [0.0, y, f64::from(z)];
        }
        let living = v.class != Kind::Other;
        let age = if v.baby() { if k.starts_with("camel") { 0.6 } else { 0.5 } } else { 1.0 };
        let s = if living { self.entity_scale(vid) * age } else { 1.0 };
        match k {
            "camel" | "camel_husk" => {
                let game_time = self.env.env.clocks.game_time;
                let pose = camel_pose(v, game_time);
                let height = if pose.0 { 0.945 } else { 2.375 } * s;
                let y = camel_seat_height(height, s as f32, v.baby(), rank == 0, pose);
                let mut z = 0.5f32;
                if count > 1 {
                    if rank != 0 {
                        z = -0.7;
                    }
                    if rider_animal {
                        z += 0.2;
                    }
                }
                return [0.0, f64::from(y as f32), f64::from(z * s as f32)];
            }
            "strider" => {
                let osc = 0.12 * (v.walk_pos * 1.5).cos() * 2.0 * v.walk_speed.min(0.25);
                return [0.0, 1.7 * s + f64::from(osc) * s, 0.0];
            }
            "happy_ghast" => {
                const SEATS: [[f64; 3]; 4] = [[0.0, 4.0, 1.7], [-1.7, 4.0, 0.0], [0.0, 4.0, -1.7], [1.7, 4.0, 0.0]];
                return SEATS[rank.min(3)].map(|c| c * s);
            }
            "cushion" => return [0.0, 0.25, 0.0],
            "zombie_horse" if v.baby() => return [0.0, 1.35 * 0.7, 0.0],
            _ => {}
        }
        let mut p = declared_point(k).unwrap_or_else(|| [0.0, h / s.max(1e-6), 0.0]).map(|c| c * s);
        if matches!(k, "horse" | "donkey" | "mule" | "skeleton_horse" | "zombie_horse") {
            // Rearing: old value of the animation (equine rendering, extra[3]).
            let a = f64::from(v.extra[3]);
            p[1] += 0.15 * a * s;
            p[2] += -0.7 * a * s;
        }
        p
    }

    /// Seat of passenger `pid` (rank `rank`) of vehicle `vid`.
    fn seat(&self, vid: i32, rank: usize, pid: i32) -> Option<[f64; 3]> {
        let v = self.entities.map.get(&vid)?;
        let rider = self.entities.map.get(&pid);
        let animal = rider.is_some_and(|r| r.class == Kind::Mob && riding_offset(&r.kind) == 0.0 && r.kind != "player");
        let p = self.passenger_point(v, vid, rank, rider.map(|r| r.kind.as_str()), animal);
        // Rotation of −yaw around Y (tabulated sines).
        let r = -v.yaw * (std::f32::consts::PI / 180.0);
        let (s, c) = (f64::from(rc_physics::sin_t(r)), f64::from(rc_physics::cos_t(r)));
        let rot = [p[0] * c + p[2] * s, p[1], p[2] * c - p[0] * s];
        // "Vehicle" attachment of the passenger.
        let attach = if pid == self.player_id {
            0.6 * self.player.attribute("minecraft:scale").unwrap_or(1.0)
        } else {
            rider.map_or(0.0, |r| rider_attach(r, self.entity_scale(pid)))
        };
        Some([v.pos[0] + rot[0], v.pos[1] + rot[1] - attach, v.pos[2] + rot[2]])
    }

    /// All the passengers on their seat, roots first (§ 1.2, 1.3).
    fn place_passengers(&mut self) {
        let mut queue: Vec<i32> = self
            .entities
            .map
            .iter()
            .filter(|(_, e)| !e.passengers.is_empty() && e.vehicle.is_none_or(|v| !self.entities.map.contains_key(&v)))
            .map(|(id, _)| *id)
            .collect();
        let mut done = 0;
        while done < queue.len() && done < 4096 {
            let vid = queue[done];
            done += 1;
            let Some(v) = self.entities.map.get(&vid) else { continue };
            let (list, body, equine) = (v.passengers.clone(), v.body, is_equine(&v.kind));
            for (rank, pid) in list.into_iter().enumerate() {
                let Some(seat) = self.seat(vid, rank, pid) else { continue };
                if pid == self.player_id {
                    self.player.pos = seat;
                    self.player.vel = [0.0; 3];
                } else if let Some(p) = self.entities.map.get_mut(&pid) {
                    p.pos = seat;
                    if equine && p.class != Kind::Other {
                        p.body = body;
                    }
                    if !p.passengers.is_empty() {
                        queue.push(pid);
                    }
                }
            }
        }
    }

    // ---------- Riding sounds ----------

    fn start_ride_sounds(&mut self, v: i32, kind: &str) {
        let Some(a) = self.audio.get() else { return };
        let Some(e) = self.entities.map.get(&v) else { return };
        if e.meta_bool(4) {
            return;
        }
        // Without attenuation, placed on the vehicle, started at zero volume (the game starts them in
        // silence; without this flag, our engine refused a sound at zero volume: never played).
        let pos = e.pos.map(|v| f64::from(v as f32));
        let mut play = |event: &str, kind: RideSoundKind| {
            let req = rc_audio::SoundRequest::new(event, rc_audio::Category::Neutral)
                .at_unattenuated(pos)
                .volume(0.0)
                .looping(0)
                .start_silent();
            if let Some(handle) = a.play_request(req) {
                self.riding.sounds.push(RideSound { handle, vehicle: v, kind });
            }
        };
        if kind.ends_with("minecart") {
            play("minecraft:entity.minecart.inside.underwater", RideSoundKind::Minecart { underwater: true });
            play("minecraft:entity.minecart.inside", RideSoundKind::Minecart { underwater: false });
        } else if kind == "happy_ghast" {
            play("minecraft:entity.happy_ghast.riding", RideSoundKind::Ghast);
        } else if kind == "nautilus" || kind == "zombie_nautilus" {
            play("minecraft:entity.nautilus.riding", RideSoundKind::Nautilus);
        }
    }

    fn stop_ride_sounds(&mut self) {
        let sounds = std::mem::take(&mut self.riding.sounds);
        if let Some(a) = self.audio.get() {
            for s in sounds {
                a.stop_handle(s.handle);
            }
        }
    }

    /// Volume of the riding sounds according to the vehicle's speed; stop if it vanished.
    fn tick_ride_sounds(&mut self) {
        if self.riding.sounds.is_empty() {
            return;
        }
        // Vehicle vanished or became silent: sounds stopped for good, like the game.
        let gone = self.riding.sounds.iter().any(|s| self.entities.map.get(&s.vehicle).is_none_or(|e| e.meta_bool(4)));
        if gone || self.vehicle() != self.riding.sounds.first().map(|s| s.vehicle) {
            self.stop_ride_sounds();
            return;
        }
        let eyes = self.player.eyes_in_water;
        let Some(a) = self.audio.get() else { return };
        for s in &self.riding.sounds {
            let Some(e) = self.entities.map.get(&s.vehicle) else { continue };
            let d: [f64; 3] = std::array::from_fn(|i| e.pos[i] - e.pos_o[i]);
            let (speed, active) = match s.kind {
                // Minecart: speed synchronised by the server (it is never piloted here).
                RideSoundKind::Minecart { underwater } => (e.velocity[0].hypot(e.velocity[2]), underwater == eyes),
                RideSoundKind::Ghast => ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(), !eyes),
                RideSoundKind::Nautilus => ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(), eyes),
            };
            let k = if matches!(s.kind, RideSoundKind::Minecart { .. }) { 0.75 } else { 5.0 };
            let v = if active && speed >= 0.01 { k * speed.clamp(0.0, 1.0) as f32 } else { 0.0 };
            a.set_sound_params(s.handle, v, 1.0);
            a.set_position(s.handle, e.pos);
        }
    }

    // ---------- Test script ----------

    /// `RUSTCRAFT_RIDETEST`: keys held, look, commands, position log.
    fn ride_script(&mut self, input: &mut rc_physics::Input) {
        let Some(s) = &mut self.riding.script else { return };
        if s.wait > 0 {
            s.wait -= 1;
            if s.wait == 0 && std::mem::take(&mut s.release) {
                s.keys = Default::default();
            }
        } else if let Some(step) = s.steps.get(s.at).cloned() {
            s.at += 1;
            let num = |i: usize| step.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            eprintln!("riding: {}", step.join(":"));
            match step[0].as_str() {
                "wait" => s.wait = num(1) as u32,
                "keys" => {
                    let k = step.get(1).map_or("", String::as_str);
                    s.keys = rc_physics::Input {
                        forward: k.contains('w'),
                        backward: k.contains('s'),
                        left: k.contains('a'),
                        right: k.contains('d'),
                        jump: k.contains('j'),
                        shift: k.contains('c'),
                        sprint: k.contains('r'),
                    };
                    s.wait = num(2) as u32;
                    s.release = s.wait > 0;
                }
                "look" => {
                    self.yaw = num(1) as f32;
                    self.pitch = num(2) as f32;
                }
                "cmd" => {
                    let c = step[1..].join(":");
                    let _ = self.session.link.send_command(&c);
                }
                "f5" => self.camera.view = self.camera.view.next(),
                "slot" => self.hud.selected = (num(1) as usize).min(8),
                // Right click (use), for example the carrot on a stick.
                "use" => {
                    self.act.press(1);
                    self.act.release(1);
                }
                // Capture now (with `RUSTCRAFT_CAPTURE`), then closing.
                "shot" => {
                    let now = self.started.elapsed().as_secs_f64();
                    if let Some(c) = &mut self.capture {
                        c.1 = now;
                    }
                }
                "log" => {
                    if let Some(v) = self.riding.vehicle.and_then(|v| self.entities.map.get(&v)) {
                        eprintln!("riding: vehicle {} {:.6} {:.6} {:.6}", v.kind, v.pos[0], v.pos[1], v.pos[2]);
                    }
                    let p = self.player.pos;
                    eprintln!("riding: player {:.6} {:.6} {:.6} yaw {:.3}", p[0], p[1], p[2], self.yaw);
                }
                _ => {}
            }
        }
        let Some(s) = &self.riding.script else { return };
        let k = s.keys;
        input.forward |= k.forward;
        input.backward |= k.backward;
        input.left |= k.left;
        input.right |= k.right;
        input.jump |= k.jump;
        input.shift |= k.shift;
        input.sprint |= k.sprint;
    }

    // ---------- Packets ----------

    /// Inventory key on a vehicle with its own inventory: request to the server (§ 1.6).
    pub(crate) fn vehicle_inventory(&mut self) -> bool {
        let Some(e) = self.vehicle().and_then(|v| self.entities.map.get(&v)) else { return false };
        let k = e.kind.as_str();
        let own = (is_boat(k) && k.contains("chest"))
            || is_equine(k)
            || matches!(k, "llama" | "trader_llama" | "nautilus" | "zombie_nautilus");
        if own {
            let _ = self.session.link.send_player_command(rc_client::link::action::OPEN_INVENTORY);
        }
        own
    }

    /// End-of-tick sends when the normal physics does not apply: nothing if the camera
    /// follows another entity; passenger: rotation, then piloted root vehicle.
    pub(crate) fn send_moves_exact(&mut self) {
        if self.exact.camera.is_some() {
            return;
        }
        let Some(v) = self.vehicle() else { return };
        let root = self.root_vehicle(v);
        let link = self.session.link.clone();
        let p = &self.player;
        let _ = link.send_move(Move::Rot { yaw: p.yaw, pitch: p.pitch }, false, false);
        if self.local_authority(root)
            && let Some(e) = self.entities.map.get(&root)
        {
            let _ = link.send_move_vehicle(e.target_pos(), e.yaw, e.pitch, e.on_ground);
        }
    }

    /// Movement packet targeting the vehicle the client pilots: only the decoding base
    /// follows (`entities.md` § 4.3); `move_vehicle` and `teleport_entity` move it.
    pub(crate) fn piloted_move(&mut self, e: &rc_client::EntityEvent) -> bool {
        use rc_client::EntityEvent as E;
        let id = match e {
            E::Move(m) => m.id,
            E::Sync(s) => s.id,
            E::Motion { id, .. } => *id,
            _ => return false,
        };
        let piloted = self.vehicle().is_some_and(|v| self.root_vehicle(v) == id && self.local_authority(id));
        if piloted {
            self.entities.update_base(e);
        }
        piloted
    }

    /// Speed of the root vehicle `id` if it is piloted locally (boat bubble column,
    /// events 71/72); false otherwise.
    pub(crate) fn piloted_velocity(&mut self, id: i32, f: impl FnOnce(&mut [f64; 3])) -> bool {
        let piloted = self.vehicle().is_some_and(|v| self.root_vehicle(v) == id && self.local_authority(id));
        if !piloted {
            return false;
        }
        match &mut self.riding.sim {
            Some((sid, m)) if *sid == id => f(&mut m.vel),
            _ => match self.entities.map.get_mut(&id) {
                Some(e) => f(&mut e.velocity),
                None => return false,
            },
        }
        true
    }

    /// `move_vehicle` received: repositioning of the piloted vehicle, and reply in every case where it
    /// is piloted locally.
    pub(crate) fn on_move_vehicle(&mut self, pos: [f64; 3], yaw: f32, pitch: f32) {
        let Some(v) = self.vehicle() else { return };
        let root = self.root_vehicle(v);
        if !self.local_authority(root) {
            return;
        }
        let Some(e) = self.entities.map.get_mut(&root) else { return };
        let t = e.target_pos();
        let d2: f64 = (0..3).map(|i| (t[i] - pos[i]).powi(2)).sum();
        if d2 > 1e-5 * 1e-5 {
            if rc_client::protolog::enabled() {
                eprintln!("move_vehicle: repositioning {t:?} to {pos:?}");
            }
            e.snap_to(pos, yaw.rem_euclid(360.0), pitch.clamp(-90.0, 90.0));
        }
        let _ = self.session.link.send_move_vehicle(e.pos, e.yaw, e.pitch, e.on_ground);
    }

    /// `player_rotation` (§ 4): rotation applied then immediate reply (flags false).
    pub(crate) fn on_player_rotation(&mut self, yaw: f32, yaw_rel: bool, pitch: f32, pitch_rel: bool) {
        let y = if yaw_rel { self.yaw } else { 0.0 } + yaw;
        let x = (if pitch_rel { self.pitch } else { 0.0 } + pitch).clamp(-90.0, 90.0);
        if !y.is_finite() || !x.is_finite() {
            return;
        }
        (self.yaw, self.pitch) = (y, x);
        (self.player.yaw, self.player.pitch) = (y, x);
        let _ = self.session.link.send_move(Move::Rot { yaw: y, pitch: x }, false, false);
    }

    /// `player_look_at` (§ 5): no reply, the rotation leaves at the next send.
    pub(crate) fn on_look_at(&mut self, eyes: bool, target: [f64; 3], entity: Option<(i32, bool)>) {
        let table = self.dims_table();
        let target = match entity.and_then(|(id, eyes)| self.entities.map.get(&id).map(|e| (e, eyes))) {
            Some((e, true)) => [e.pos[0], e.pos[1] + f64::from(e.dims(table)[1]), e.pos[2]],
            Some((e, false)) => e.pos,
            None => target,
        };
        let p = self.player.pos;
        let from = [p[0], p[1] + if eyes { self.player.eye_height() } else { 0.0 }, p[2]];
        let (dx, dy, dz) = (target[0] - from[0], target[1] - from[1], target[2] - from[2]);
        let h = dx.hypot(dz);
        let pitch = wrap((-dy.atan2(h).to_degrees()) as f32);
        let yaw = wrap((dz.atan2(dx).to_degrees() - 90.0) as f32);
        (self.yaw, self.pitch) = (yaw, pitch);
        (self.player.yaw, self.player.pitch) = (yaw, pitch);
    }

    /// `set_camera` (§ 6): unknown entity ignored; the local player restores the normal view.
    pub(crate) fn on_set_camera(&mut self, id: i32) {
        if rc_client::protolog::enabled() {
            let e = self.entities.map.get(&id);
            eprintln!(
                "camera: entity {id} ({}), yaw {:?}, player {}",
                e.map_or("unknown", |e| e.kind.as_str()),
                e.map(|e| e.yaw),
                self.player_id
            );
        }
        if id == self.player_id {
            self.exact.camera = None;
        } else if self.entities.map.contains_key(&id) {
            self.exact.camera = Some(id);
        }
    }

    /// Entity followed by the camera (other than the player), if it still exists.
    fn camera_entity(&self) -> Option<&Entity> {
        self.exact.camera.and_then(|id| self.entities.map.get(&id))
    }

    /// Eye height of the followed entity.
    pub(crate) fn camera_eye_height(&self) -> Option<f64> {
        let table = self.dims_table();
        self.camera_entity().map(|e| f64::from(e.dims(table)[1]))
    }

    /// Interpolated feet and rotation of the followed entity.
    pub(crate) fn camera_pose(&self, pt: f32) -> Option<([f64; 3], f32, f32)> {
        let e = self.camera_entity()?;
        let yaw = e.yaw_o + pt * wrap(e.yaw - e.yaw_o);
        let pitch = e.pitch_o + pt * (e.pitch - e.pitch_o);
        Some((e.render_pos(pt), yaw, pitch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles() {
        assert_eq!(wrap(190.0), -170.0);
        assert_eq!(wrap(-180.0), -180.0);
        assert_eq!(wrap(180.0), -180.0);
        assert_eq!(wrap(45.0), 45.0);
    }

    #[test]
    fn villager_sits_low_in_a_minecart_only() {
        assert!(lowered_seat("minecart", Some("villager")));
        assert!(lowered_seat("chest_minecart", Some("wandering_trader")));
        // Villager in a boat, pig in a minecart, mount of a villager: normal seat.
        assert!(!lowered_seat("oak_boat", Some("villager")));
        assert!(!lowered_seat("minecart", Some("pig")));
        assert!(!lowered_seat("villager", Some("player")));
        assert!(!lowered_seat("minecart", None));
    }

    #[test]
    fn camel_seat() {
        // Standing outside transition: height − 0.375.
        assert!((camel_seat_height(2.375, 1.0, false, true, (false, false, 100)) - 2.0).abs() < 1e-9);
        // Sitting outside transition: + (1.43 − (1.43 − 0.2)) = + 0.2.
        assert!((camel_seat_height(0.945, 1.0, false, true, (true, false, 100)) - (0.57 + 0.2)).abs() < 1e-6);
        // End of the "stands up" transition: the seat reaches the standing height from below.
        let end = camel_seat_height(2.375, 1.0, false, true, (false, true, 51));
        assert!(end > 2.0 - 0.02 && end < 2.0);
    }
}
