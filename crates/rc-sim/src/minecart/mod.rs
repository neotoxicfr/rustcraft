//! Minecarts of the integrated server (mission G5-F06): plain minecart, chest, furnace, TNT,
//! hopper, spawner and command block minecarts. "Old" behaviour of game 26.3 (that
//! of a world without the `minecart_improvements` experiment): movement on rails, leaving the rail,
//! pushes, mounting, damage and breaking, contents and loot, variants, save in the game's format.
//!
//! Minecarts read with a chunk are taken out of the frozen entities (`Server.entities`) and
//! simulated here; commands still see them as "other" entities (`E::Other`).
//!
//! The boats and rafts share this vehicle structure (`CKind::Boat`, their own code in
//! `crate::boat`): the game builds both on one vehicle base and one container base.

mod physics;
mod save;
mod track;
pub(crate) mod use_;
mod variants;

#[cfg(test)]
mod tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use crate::mobs::jrand::JRand;
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

pub(crate) use track::CTrack;

/// Kind of vehicle: the seven minecarts, and the boats and rafts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CKind {
    Rideable,
    Chest,
    Furnace,
    Tnt,
    Hopper,
    Spawner,
    Command,
    Boat(crate::boat::BoatType),
}

impl CKind {
    pub const MINECARTS: [CKind; 7] =
        [CKind::Rideable, CKind::Chest, CKind::Furnace, CKind::Tnt, CKind::Hopper, CKind::Spawner, CKind::Command];

    pub fn name(self) -> &'static str {
        match self {
            CKind::Rideable => "minecraft:minecart",
            CKind::Chest => "minecraft:chest_minecart",
            CKind::Furnace => "minecraft:furnace_minecart",
            CKind::Tnt => "minecraft:tnt_minecart",
            CKind::Hopper => "minecraft:hopper_minecart",
            CKind::Spawner => "minecraft:spawner_minecart",
            CKind::Command => "minecraft:command_block_minecart",
            CKind::Boat(b) => b.name(),
        }
    }

    pub fn from_name(n: &str) -> Option<CKind> {
        CKind::MINECARTS
            .into_iter()
            .find(|k| k.name() == n)
            .or_else(|| crate::boat::BoatType::from_name(n).map(CKind::Boat))
    }

    /// A boat or a raft (with or without a chest).
    pub fn is_boat(self) -> bool {
        matches!(self, CKind::Boat(_))
    }

    /// Item dropped on breaking: the spawner and the command block minecart only
    /// give back a plain minecart; a boat gives back its own item.
    pub fn drop_item(self) -> &'static str {
        match self {
            CKind::Chest => "minecraft:chest_minecart",
            CKind::Furnace => "minecraft:furnace_minecart",
            CKind::Tnt => "minecraft:tnt_minecart",
            CKind::Hopper => "minecraft:hopper_minecart",
            CKind::Boat(b) => b.name(),
            _ => "minecraft:minecart",
        }
    }

    /// Item of the entity.
    pub fn pick_item(self) -> &'static str {
        match self {
            CKind::Command => "minecraft:command_block_minecart",
            k => k.drop_item(),
        }
    }

    /// Container size (chest minecart and chest boat 27, hopper 5), 0 without a container.
    pub fn container_size(self) -> usize {
        match self {
            CKind::Chest => 27,
            CKind::Hopper => 5,
            CKind::Boat(b) if b.chest => 27,
            _ => 0,
        }
    }

    /// A chest of 27 slots (chest minecart or chest boat): the chest menu, the container
    /// opening and closing events, the piglins.
    pub fn has_chest(self) -> bool {
        self.container_size() == 27
    }

    /// Default offset of the displayed block (minecarts only).
    pub fn default_offset(self) -> i32 {
        match self {
            CKind::Chest => 8,
            CKind::Hopper => 1,
            _ => 6,
        }
    }

    /// Width and height of the box.
    pub fn size(self) -> (f64, f64) {
        if self.is_boat() { (crate::boat::WIDTH, crate::boat::HEIGHT) } else { (WIDTH, HEIGHT) }
    }

    /// Height of the eyes above the feet: a boat's are at its top, a minecart's at 0.85 of it.
    pub fn eye_height(self) -> f64 {
        if self.is_boat() { crate::boat::HEIGHT } else { HEIGHT * 0.85 }
    }
}

/// Width and height of the box (all minecarts).
pub const WIDTH: f64 = 0.98f32 as f64;
pub const HEIGHT: f64 = 0.7f32 as f64;
/// Attachment point of the passengers.
pub const SEAT: f64 = 0.1875f32 as f64;

/// Living minecart.
#[derive(Debug, Clone)]
pub struct Cart {
    pub id: i32,
    pub uuid: u128,
    pub kind: CKind,
    pub pos: [f64; 3],
    /// Position and yaw at the start of the tick.
    pub old_pos: [f64; 3],
    pub yaw_o: f32,
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub on_rails: bool,
    pub flipped: bool,
    /// "First tick" flag (saved under `HasTicked`, as is).
    pub first_tick: bool,
    pub fall_distance: f64,
    /// Stuck this tick (cobweb, powder snow): factors of the next movement.
    pub stuck: [f64; 3],
    pub fire: i32,
    pub in_water: bool,
    pub in_lava: bool,
    /// Main supporting block (on the ground), distance travelled and next step (events).
    pub support: Option<[i32; 3]>,
    pub move_dist: f32,
    pub next_step: f32,
    /// Ticks of life (counter incremented by the world before each tick).
    pub tick_count: i32,
    // --- Vehicle. ---
    pub hurt_time: i32,
    pub hurt_dir: i32,
    pub damage: f32,
    // --- Minecart. ---
    /// Custom displayed block (`DisplayState`), offset (`DisplayOffset`).
    pub display: Option<u32>,
    pub display_offset: i32,
    pub no_gravity: bool,
    pub silent: bool,
    pub invulnerable: bool,
    // --- Container (chest, hopper). ---
    pub items: Vec<Option<Stack>>,
    pub loot: Option<String>,
    pub loot_seed: i64,
    // --- Furnace. ---
    pub fuel: i32,
    pub push: [f64; 3],
    // --- TNT. ---
    pub fuse: i32,
    pub power_base: f32,
    pub speed_factor: f32,
    /// Lit by a source (author of the explosion): `Some(None)` for an anonymous source.
    pub ignition: Option<Option<Ent>>,
    // --- Hopper. ---
    pub enabled: bool,
    /// Item taken during this tick.
    pub consumed: bool,
    // --- Command block. ---
    pub last_activated: i32,
    /// Activation to run at the end of the tick: position and rotation of the source.
    pub pending_command: Option<([f64; 3], [f32; 2])>,
    // --- Boat. ---
    pub boat: crate::boat::BoatState,
    // --- Passengers. ---
    pub passengers: Vec<Ent>,
    /// Passengers not simulated (NBT kept as is).
    pub foreign: Vec<Nbt>,
    /// Fields kept as is (name, tags, spawner, command…), simulated keys removed.
    pub nbt: Nbt,
    pub rng: JRand,
    pub removed: bool,
    /// Immediate send on the next tracking.
    pub needs_sync: bool,
    meta: Vec<(u8, Meta, bool)>,
    pub track: CTrack,
    /// Piston pushes of the current tick (its own cap).
    pub piston_deltas: rc_physics::piston::PistonDeltas,
    /// Rank in the entity walk (`walk::next_rank`), stamped when it enters the list.
    pub(crate) rank: u64,
}

/// Minecarts of the open world (exchanged with the dimension, like projectiles).
#[derive(Debug, Default)]
pub struct Carts {
    /// Empty slot during the tick of its minecart (taken out without a copy).
    pub list: Vec<Option<Box<Cart>>>,
    /// Entity chunks whose minecarts have been read.
    pub adopted: hashbrown::HashSet<(i32, i32)>,
    /// Position and kind of the vehicle being ticked (taken out of the list), for detector rails
    /// (minecarts only) and pressure plates.
    pub ticking: Option<([f64; 3], CKind)>,
    /// Direct source of the current damage when it is a projectile: does it burn? (TNT
    /// only lights from a burning projectile, whatever the damage type).
    pub direct: Option<bool>,
    /// Same indication for the next explosion (fireball, wither skull).
    pub blast_direct: Option<bool>,
    /// Slot (empty) of the minecart being ticked.
    pub ticking_index: Option<usize>,
    /// A boat may be in the list: set when one enters it, recomputed before each tick. The solid
    /// entity searches (every entity's move) skip the list when it holds no boat.
    pub any_boat: bool,
}

/// `RUSTCRAFT_OPT_SKIP_VEHICLES_WITHOUT_BOATS` (default on): the solid entity searches skip the
/// vehicle list when it holds no boat (`=0`: they walk it every time).
pub(crate) fn skip_vehicles_without_boats() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SKIP_VEHICLES_WITHOUT_BOATS")
}

/// Player state tied to minecarts (does not change with the dimension).
#[derive(Debug, Default)]
pub struct PlayerCart {
    /// Minecart ridden by the player.
    pub vehicle: Option<i32>,
    /// Closing in progress: the player's minecart is not written into the chunks.
    pub unloading: bool,
}

impl Cart {
    /// New minecart (own random, UUID v4 drawn from that random like projectiles).
    pub fn new(kind: CKind, id: i32, pos: [f64; 3]) -> Cart {
        let mut rng = JRand::from_time(i64::from(id) ^ 0x4D43);
        let uuid = (rng.next_long() as u128) << 64 | rng.next_long() as u64 as u128;
        let uuid = (uuid & !(0xF000u128 << 64) | (0x4000u128 << 64)) & !(0xC000_0000_0000_0000u128)
            | 0x8000_0000_0000_0000u128;
        let mut c = Cart {
            id,
            uuid,
            kind,
            pos,
            old_pos: pos,
            yaw_o: 0.0,
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            horizontal_collision: false,
            vertical_collision: false,
            on_rails: false,
            flipped: false,
            first_tick: true,
            fall_distance: 0.0,
            stuck: [0.0; 3],
            fire: 0,
            in_water: false,
            in_lava: false,
            support: None,
            move_dist: 0.0,
            next_step: 1.0,
            tick_count: 0,
            hurt_time: 0,
            hurt_dir: 1,
            damage: 0.0,
            display: None,
            display_offset: kind.default_offset(),
            no_gravity: false,
            silent: false,
            invulnerable: false,
            items: vec![None; kind.container_size()],
            loot: None,
            loot_seed: 0,
            fuel: 0,
            push: [0.0; 3],
            fuse: -1,
            power_base: 4.0,
            speed_factor: 1.0,
            ignition: None,
            enabled: true,
            consumed: false,
            last_activated: 0,
            pending_command: None,
            boat: crate::boat::BoatState::default(),
            passengers: Vec::new(),
            foreign: Vec::new(),
            nbt: Nbt::Compound(Vec::new()),
            rng,
            removed: false,
            needs_sync: false,
            meta: Vec::new(),
            track: CTrack::default(),
            piston_deltas: Default::default(),
            rank: 0,
        };
        c.define_meta();
        c
    }

    /// Collision box (feet at bottom centre).
    pub fn bbox(&self) -> rc_physics::Aabb {
        bbox_of(self.kind, self.pos)
    }

    pub fn width(&self) -> f64 {
        self.kind.size().0
    }

    pub fn height(&self) -> f64 {
        self.kind.size().1
    }

    pub fn block_pos(&self) -> [i32; 3] {
        self.pos.map(|v| v.floor() as i32)
    }

    /// Occupied vehicle.
    pub fn is_vehicle(&self) -> bool {
        !self.passengers.is_empty() || !self.foreign.is_empty()
    }

    fn define(&mut self, i: u8, v: Meta) {
        self.meta.push((i, v, false));
    }

    /// Default values of the data: vehicle 8-10, minecart 11-12,
    /// furnace 13, command block 13-14; boat: paddles 11-12, bubble time 13.
    fn define_meta(&mut self) {
        self.define(0, Meta::Byte(0));
        self.define(8, Meta::Int(0));
        self.define(9, Meta::Int(1));
        self.define(10, Meta::Float(0.0));
        if self.kind.is_boat() {
            self.define(11, Meta::Bool(false));
            self.define(12, Meta::Bool(false));
            self.define(13, Meta::Int(0));
            return;
        }
        self.define(11, Meta::BlockState(None));
        self.define(12, Meta::Int(self.kind.default_offset()));
        match self.kind {
            CKind::Furnace => self.define(13, Meta::Bool(false)),
            CKind::Command => {
                self.define(13, Meta::String(String::new()));
                self.define(14, Meta::Component(Nbt::String(String::new())));
            }
            _ => {}
        }
    }

    /// Default value of the type for a data item (pairing: only the others are sent).
    fn default_meta(&self, i: u8) -> Meta {
        match i {
            0 => Meta::Byte(0),
            8 => Meta::Int(0),
            9 => Meta::Int(1),
            10 => Meta::Float(0.0),
            11 | 12 if self.kind.is_boat() => Meta::Bool(false),
            13 if self.kind.is_boat() => Meta::Int(0),
            11 => Meta::BlockState(None),
            12 => Meta::Int(self.kind.default_offset()),
            13 if self.kind == CKind::Furnace => Meta::Bool(false),
            13 => Meta::String(String::new()),
            14 => Meta::Component(Nbt::String(String::new())),
            7 => Meta::Int(0),
            _ => Meta::Text(Some(Nbt::String(String::new()))),
        }
    }

    /// Changes a data item (marked if it differs).
    pub fn set_meta(&mut self, i: u8, v: Meta) {
        if let Some(s) = self.meta.iter_mut().find(|s| s.0 == i) {
            if s.1 != v {
                s.1 = v;
                s.2 = true;
            }
        } else {
            self.meta.push((i, v, true));
        }
    }

    pub fn meta(&self, i: u8) -> Option<&Meta> {
        self.meta.iter().find(|s| s.0 == i).map(|s| &s.1)
    }

    fn take_dirty(&mut self) -> Vec<(u8, Meta)> {
        let mut out = Vec::new();
        for s in &mut self.meta {
            if s.2 {
                s.2 = false;
                out.push((s.0, s.1.clone()));
            }
        }
        out
    }

    /// Forgets the changes nobody receives (minecart out of tracking): the marks are cleared without
    /// copying the values (`RUSTCRAFT_OPT_UNSENT_CHANGES_UNCOPIED=0`: copied into a list that is
    /// dropped, old path).
    fn discard_changes(&mut self) {
        if crate::mobs::mob::unsent_changes_uncopied() {
            self.meta.iter_mut().for_each(|s| s.2 = false);
        } else {
            self.take_dirty();
        }
    }

    /// Vehicle data copied (shake, direction, damage).
    pub fn sync_hurt(&mut self) {
        self.set_meta(8, Meta::Int(self.hurt_time));
        self.set_meta(9, Meta::Int(self.hurt_dir));
        self.set_meta(10, Meta::Float(self.damage));
    }

    /// Displayed block and offset copied (minecarts: a boat's data 11-12 are its paddles).
    pub fn sync_display(&mut self) {
        if self.kind.is_boat() {
            return;
        }
        self.set_meta(11, Meta::BlockState(self.display.map(|s| s as i32)));
        self.set_meta(12, Meta::Int(self.display_offset));
    }
}

/// Box of a vehicle of kind `kind` placed at `p`.
pub fn bbox_of(kind: CKind, p: [f64; 3]) -> rc_physics::Aabb {
    let (width, height) = kind.size();
    let r = width / 2.0;
    rc_physics::Aabb { min: [p[0] - r, p[1], p[2] - r], max: [p[0] + r, p[1] + height, p[2] + r] }
}

impl Server {
    /// Minecart by network identifier.
    pub(crate) fn cart_ref(&self, id: i32) -> Option<&Cart> {
        self.carts.list.iter().flatten().find(|c| c.id == id && !c.removed).map(|b| &**b)
    }

    pub(crate) fn cart_mut(&mut self, id: i32) -> Option<&mut Cart> {
        self.carts.list.iter_mut().flatten().find(|c| c.id == id && !c.removed).map(|b| &mut **b)
    }

    pub(crate) fn cart_index(&self, id: i32) -> Option<usize> {
        self.carts.list.iter().position(|c| c.as_ref().is_some_and(|c| c.id == id && !c.removed))
    }

    /// Takes a minecart out of the list (to be put back with [`Server::cart_put`]).
    pub(crate) fn cart_take(&mut self, id: i32) -> Option<(usize, Box<Cart>)> {
        let i = self.cart_index(id)?;
        Some((i, self.carts.list[i].take()?))
    }

    pub(crate) fn cart_put(&mut self, i: usize, c: Box<Cart>) {
        if let Some(slot) = self.carts.list.get_mut(i) {
            *slot = Some(c);
        }
    }

    /// Added to the world: paired at once if visible.
    pub(crate) fn cart_add(&mut self, mut c: Cart) {
        c.sync_hurt();
        c.sync_display();
        if self.cart_visible(&c) {
            self.cart_pair(&mut c);
        }
        self.cart_push(Box::new(c));
    }

    /// At the end of the list, with its rank in the entity walk.
    pub(crate) fn cart_push(&mut self, mut c: Box<Cart>) {
        c.rank = crate::walk::next_rank();
        self.carts.any_boat |= c.kind.is_boat();
        self.carts.list.push(Some(c));
    }

    /// The minecarts as their own walk (tests): network tracking of the previous state, then
    /// tick of each in an active chunk, passengers included, then removals.
    #[cfg(test)]
    pub(crate) fn minecarts_tick(&mut self) {
        self.walk_alone(crate::walk::bit(crate::walk::Fam::Cart), Self::carts_track, |s| s.carts_post(true));
    }

    /// Network tracking BEFORE the walk, like the official server (entity tracking comes before
    /// their tick): the client receives the state of the previous tick.
    pub(crate) fn carts_track(&mut self) {
        let mut any_boat = false;
        for i in 0..self.carts.list.len() {
            let Some(mut c) = self.carts.list[i].take() else { continue };
            self.cart_send_changes(&mut c);
            any_boat |= c.kind.is_boat() && !c.removed;
            self.carts.list[i] = Some(c);
        }
        self.carts.any_boat = any_boat;
    }

    /// Minecart of slot `i`, at its rank in the entity walk (once per world tick, command block
    /// ones included). Frozen (`tick freeze`), only one carrying the player ticks, even through
    /// another passenger.
    pub(crate) fn cart_visit(&mut self, i: usize) {
        let Some(mut cart) = self.carts.list.get_mut(i).and_then(Option::take) else { return };
        let frozen = !self.cmd.ticking.runs()
            && self.pcart.vehicle != Some(cart.id)
            && !self.passengers_carry_player(&cart.passengers, cart.id);
        if cart.removed || frozen {
            self.carts.list[i] = Some(cart);
            return;
        }
        let ticking = self.entity_ticks_at(cart.pos);
        if self.meet(&mut cart.rank, ticking, crate::walk::Fam::Cart) != crate::walk::Meet::Tick {
            self.carts.list[i] = Some(cart);
            return;
        }
        let id = cart.id;
        cart.old_pos = cart.pos;
        cart.yaw_o = cart.yaw;
        cart.tick_count += 1;
        self.carts.ticking_index = Some(i);
        self.cart_tick(&mut cart);
        self.carts.ticking_index = None;
        if let Some(slot) = self.carts.list.get_mut(i) {
            *slot = Some(cart);
        }
        self.cart_run_pending_command(id);
        self.cart_tick_passengers(id);
    }

    /// After the entity walk: the player's minecart (`with_player`), removals.
    pub(crate) fn carts_post(&mut self, with_player: bool) {
        if with_player {
            self.cart_player_tick();
        }
        let mut gone = Vec::new();
        let mut removed = Vec::new();
        for slot in self.carts.list.iter_mut() {
            if slot.as_ref().is_some_and(|c| c.removed) {
                let c = slot.take().expect("minecart");
                if c.track.seen {
                    gone.push(c.id);
                }
                removed.push(c);
            }
        }
        self.carts.list.retain(Option::is_some);
        for c in removed {
            self.cart_release_passengers(&c);
        }
        if !gone.is_empty() {
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Remove(gone)));
        }
    }

    /// Dimension change: minecarts forgotten by the client.
    pub(crate) fn carts_forget_tracking(&mut self) {
        for c in self.carts.list.iter_mut().flatten() {
            c.track.seen = false;
        }
    }
}
