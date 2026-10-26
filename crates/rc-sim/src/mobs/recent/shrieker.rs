//! Sculk shrieker (block entity, `server-mobs.md` § 4.F14): listens (radius 8) for the
//! clicking of sculk sensors triggered by a player, or shrieks when a player walks on
//! it; warns nearby players (shared warning level, 200-tick cooldown,
//! drops every 12,000), then after 90 ticks responds: warden summoned at level 4
//! (20 tries at ±5/6, on a support), otherwise a distant sound; darkness at 40 blocks.

use rc_proto::nbt::Nbt;

use super::super::mob::Ent;
use super::super::species::Kind;
use super::vibration::{Src, VibData, VibInfo, Who};
use crate::nbtio;
use crate::server::Server;

/// Maximum warning level.
pub const MAX_WARNING: i32 = 4;

/// The player's warning tracking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tracker {
    pub ticks_since_warning: i32,
    pub warning_level: i32,
    pub cooldown: i32,
}

impl Tracker {
    /// `tick` (every player tick).
    pub fn tick(&mut self) {
        if self.ticks_since_warning >= 12000 {
            self.warning_level = (self.warning_level - 1).clamp(0, MAX_WARNING);
            self.ticks_since_warning = 0;
        } else {
            self.ticks_since_warning += 1;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
        }
    }

    fn increase(&mut self) {
        if self.cooldown <= 0 {
            self.ticks_since_warning = 0;
            self.cooldown = 200;
            self.warning_level = (self.warning_level + 1).clamp(0, MAX_WARNING);
        }
    }

    pub fn load(n: Option<&Nbt>) -> Tracker {
        let Some(n) = n else { return Tracker::default() };
        let i = |k: &str| nbtio::int(n, k).unwrap_or(0).max(0) as i32;
        Tracker {
            ticks_since_warning: i("ticks_since_last_warning"),
            warning_level: i("warning_level"),
            cooldown: i("cooldown_ticks"),
        }
    }

    pub fn save(&self) -> Nbt {
        Nbt::Compound(vec![
            ("ticks_since_last_warning".into(), Nbt::Int(self.ticks_since_warning)),
            ("warning_level".into(), Nbt::Int(self.warning_level)),
            ("cooldown_ticks".into(), Nbt::Int(self.cooldown)),
        ])
    }
}

pub fn is_shrieker(name: &str) -> bool {
    name == "minecraft:sculk_shrieker"
}

/// Player behind a source: player, steering rider, shooter of a
/// projectile, owner of an item.
fn player_of(s: &Server, src: Option<Src>) -> bool {
    match src {
        Some(Src::Living(Ent::Player)) => true,
        Some(Src::Living(Ent::Mob(id))) => s.mob_ref(id).is_some_and(Server::player_controls),
        Some(Src::Projectile(o)) => o == Some(Ent::Player),
        Some(Src::Item { owner, .. }) => owner == Some(Ent::Player),
        _ => false,
    }
}

/// Not already shrieking, player behind the source.
pub fn can_receive(s: &Server, p: [i32; 3], src: Option<Src>) -> bool {
    s.prop(p, "shrieking").as_deref() != Some("true") && player_of(s, src)
}

/// Shriek attempted for the player (shooter first).
pub fn receive(s: &mut Server, p: [i32; 3], info: &VibInfo, owner: Option<Ent>) {
    let src = s.vib_source(info);
    let player = if owner.is_some() { owner == Some(Ent::Player) } else { player_of(s, src) };
    if player {
        s.shrieker_try_shriek(p);
    }
}

impl Server {
    /// Ticker of a vibration block (shrieker, sculk sensor) and registered listener.
    pub(crate) fn vib_block_register(&mut self, p: [i32; 3]) {
        if self
            .mobs
            .rec
            .vib_blocks
            .get(&p)
            .is_some_and(|&id| self.bs.tickers.iter().chain(self.bs.pending_tickers.iter()).any(|t| *t == (id, p)))
        {
            return;
        }
        let id = self.add_ticker(p);
        self.mobs.rec.vib_blocks.insert(p, id);
        // Sensor: retained frequency always written (0 by default), before the listener.
        if !is_shrieker(self.bname(p))
            && let Some(be) = self.be_get_mut(p)
            && be.get("last_vibration_frequency").is_none()
        {
            nbtio::set(be, "last_vibration_frequency", Nbt::Int(0));
        }
        let data = VibData::load(self.block_entity(p).and_then(|b| b.get("listener")));
        self.mobs.rec.vib.blocks.insert(p, data);
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.listener_move(Who::Block(p), c);
    }

    pub(crate) fn vib_block_is_ticker(&self, id: u64, p: [i32; 3]) -> bool {
        self.mobs.rec.vib_blocks.get(&p) == Some(&id)
    }

    pub(crate) fn vib_block_ticker_alive(&mut self, id: u64, p: [i32; 3]) -> bool {
        let alive = self.vib_block_is_ticker(id, p)
            && super::vibration::block_ear(self.bname(p)).is_some()
            && self.block_entity(p).is_some();
        if !alive && self.vib_block_is_ticker(id, p) {
            self.vib_block_forget(p);
        }
        alive
    }

    /// Vibration block removed: ticker, data and listener forgotten.
    pub(crate) fn vib_block_forget(&mut self, p: [i32; 3]) {
        self.mobs.rec.vib_blocks.remove(&p);
        self.mobs.rec.vib.blocks.remove(&p);
        self.listener_remove(Who::Block(p));
    }

    /// NBT of the block entity: listener (vibration data).
    pub(crate) fn vib_block_store(&mut self, p: [i32; 3]) {
        let data = self.mobs.rec.vib.blocks.get(&p).map(VibData::save);
        if let Some(be) = self.be_get_mut(p)
            && let Some(d) = data
        {
            nbtio::set(be, "listener", d);
        }
    }

    fn shrieker_warning(&self, p: [i32; 3]) -> i32 {
        self.block_entity(p).and_then(|b| nbtio::int(b, "warning_level")).unwrap_or(0) as i32
    }

    fn shrieker_set_warning(&mut self, p: [i32; 3], v: i32) {
        if let Some(be) = self.be_get_mut(p) {
            nbtio::set(be, "warning_level", Nbt::Int(v));
        }
    }

    /// An entity on the ground walks on a shrieker (effect block, every tick): shriek attempted if a
    /// player is behind it.
    pub(crate) fn shrieker_step_on(&mut self, p: [i32; 3], src: Src) {
        if is_shrieker(self.bname(p)) && self.block_entity(p).is_some() && player_of(self, Some(src)) {
            self.shrieker_try_shriek(p);
        }
    }

    /// Level reset to 0; with no possible response we always shriek, otherwise only
    /// if the warning went up.
    pub(crate) fn shrieker_try_shriek(&mut self, p: [i32; 3]) {
        if self.prop(p, "shrieking").as_deref() == Some("true") {
            return;
        }
        self.shrieker_set_warning(p, 0);
        if !self.shrieker_can_respond(p) || self.shrieker_try_warn(p) {
            self.shrieker_shriek(p);
        }
    }

    /// No warden within ±24, player outside cooldown;
    /// level raised then copied.
    fn shrieker_try_warn(&mut self, p: [i32; 3]) -> bool {
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let near_warden = self.mobs.of_kind(Kind::Warden).any(|m| {
            let b = m.bbox();
            (0..3).all(|i| b.max[i] > c[i] - 24.0 && b.min[i] < c[i] + 24.0)
        });
        if near_warden {
            return false;
        }
        // A single player (singleplayer): the trigger.
        let mut t = self.mobs.rec.warden_tracker;
        if t.cooldown > 0 {
            return false;
        }
        t.increase();
        self.mobs.rec.warden_tracker = t;
        self.shrieker_set_warning(p, t.warning_level);
        true
    }

    /// `shriek`: "shrieking" state, tick in 90, event 3007, `shriek` game event.
    fn shrieker_shriek(&mut self, p: [i32; 3]) {
        let s = self.st(p);
        if let Some(n) = self.with_prop(s, "shrieking", "true") {
            self.set_block(p, n, 2);
        }
        let b = self.v().block(self.st(p));
        self.schedule_tick(p, b, 90);
        self.level_event(3007, p, 0);
        self.game_event_block("minecraft:shriek", p, Some(Ent::Player), None);
    }

    /// Can summon, outside peaceful, rule `spawn_wardens`.
    fn shrieker_can_respond(&self, p: [i32; 3]) -> bool {
        self.prop(p, "can_summon").as_deref() == Some("true")
            && self.level.difficulty_id() != 0
            && self.rule("minecraft:spawn_wardens", true)
    }

    /// End of the shriek (scheduled tick): state reset, response. True if the block is a shrieker.
    pub(crate) fn shrieker_block_tick(&mut self, state: u32, p: [i32; 3]) -> bool {
        if !is_shrieker(self.state_name(state)) {
            return false;
        }
        if self.data.state_properties(state).any(|(k, v)| k == "shrieking" && v == "true") {
            if let Some(n) = self.with_prop(state, "shrieking", "false") {
                self.set_update(p, n);
            }
            self.shrieker_try_respond(p);
        }
        true
    }

    /// Shrieker removed during its shriek: it responds anyway.
    pub(crate) fn shrieker_pre_remove(&mut self, p: [i32; 3], old: u32) {
        if is_shrieker(self.state_name(old))
            && self.data.state_properties(old).any(|(k, v)| k == "shrieking" && v == "true")
        {
            self.shrieker_try_respond(p);
        }
        self.vib_block_forget(p);
    }

    /// Warden summoned (level 4) or distant sound; darkness at 40 blocks.
    fn shrieker_try_respond(&mut self, p: [i32; 3]) {
        let level = self.shrieker_warning(p);
        if !self.shrieker_can_respond(p) || level <= 0 {
            return;
        }
        if !self.shrieker_summon(p, level) {
            let sound = match level {
                1 => Some("minecraft:entity.warden.nearby_close"),
                2 => Some("minecraft:entity.warden.nearby_closer"),
                3 => Some("minecraft:entity.warden.nearby_closest"),
                4 => Some("minecraft:entity.warden.listening_angry"),
                _ => None,
            };
            if let Some(snd) = sound {
                let r = &mut self.mobs.rng;
                let x = p[0] + r.int_between(-10, 10);
                let y = p[1] + r.int_between(-10, 10);
                let z = p[2] + r.int_between(-10, 10);
                self.world_sound(snd, 5, [f64::from(x), f64::from(y), f64::from(z)], 5.0, 1.0);
            }
        }
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        super::warden::apply_darkness_around(self, c, None, 40.0);
    }

    /// Level 4, 20 tries at ±5/6, on a full support (collisions not
    /// checked), reason "triggered".
    fn shrieker_summon(&mut self, p: [i32; 3], level: i32) -> bool {
        level >= MAX_WARNING
            && self
                .spawn_util(Kind::Warden, super::super::spawn::Reason::Triggered, p, 20, 5, 6, false, false)
                .is_some()
    }
}
