//! Sculk sensors (normal and calibrated): vibration listener of the block entity (radius 8,
//! 16 for the calibrated one, which also filters the frequency received from behind), phases inactive →
//! active (30 ticks, 10 for the calibrated one) → cooldown (10 ticks), power by
//! distance, frequency read by the comparator, resonance of neighbouring amethyst blocks,
//! clicking that wakes shriekers, forced step of an entity walking on it.

use rc_proto::nbt::Nbt;

use super::{Dir, P, below, flag, rel};
use crate::mobs::recent::shrieker::is_shrieker;
use crate::mobs::recent::vibration::{BlockEar, Src, VibInfo, block_ear, frequency, resonance_event};
use crate::server::Server;

const PHASE: &str = "sculk_sensor_phase";

/// Note pitch (two octaves) of the resonance by frequency 0 to 15.
const RESONANCE_NOTES: [i32; 16] = [0, 0, 2, 4, 6, 7, 9, 10, 12, 14, 15, 18, 19, 21, 22, 24];

fn sensor(name: &str) -> Option<BlockEar> {
    block_ear(name).filter(|e| *e != BlockEar::Shrieker)
}

/// Power emitted for a vibration received at `distance` blocks: at least 1, 15 on contact.
pub(crate) fn power_for_distance(distance: f32, radius: i32) -> i32 {
    let scale = 15.0 / f64::from(radius);
    (15 - (scale * f64::from(distance)).floor() as i32).max(1)
}

/// Pitch of the resonance sound of a frequency.
pub(crate) fn resonance_pitch(freq: i32) -> f32 {
    let n = RESONANCE_NOTES[freq.clamp(0, 15) as usize];
    2f64.powf(f64::from(n - 12) / 12.0) as f32
}

fn center(p: P) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

/// Accepts the vibration coming from block `origin`: neither a break nor a placement on itself, a
/// non-zero frequency, inactive phase; the calibrated one also requires the signal received from behind to be zero
/// or equal to the frequency.
pub(crate) fn can_receive(s: &Server, p: P, origin: P, event: &str) -> bool {
    let st = s.get(p);
    let Some(ear) = sensor(s.state_name(st)) else { return false };
    if ear == BlockEar::Calibrated {
        let back = s.v().facing(st).opposite();
        let wanted = s.signal(rel(p, back), back);
        if wanted != 0 && frequency(event) != wanted {
            return false;
        }
    }
    if origin == p && matches!(event, "minecraft:block_destroy" | "minecraft:block_place") {
        return false;
    }
    frequency(event) != 0 && s.v().get(st, PHASE) == Some("inactive")
}

/// Vibration arrived (`dest`: sensor centre): frequency retained, power according to the
/// distance between blocks, activation.
pub(crate) fn receive(s: &mut Server, p: P, info: &VibInfo, dest: [f64; 3]) {
    let st = s.get(p);
    let Some(ear) = sensor(s.state_name(st)) else { return };
    if s.v().get(st, PHASE) != Some("inactive") {
        return;
    }
    let freq = frequency(&info.event);
    if let Some(be) = s.be_get_mut(p) {
        crate::nbtio::set(be, "last_vibration_frequency", Nbt::Int(freq));
    }
    let o = info.pos.map(|v| v.floor() as i32);
    let d = dest.map(|v| v.floor() as i32);
    let dist = ((0..3).map(|i| f64::from(o[i] - d[i]).powi(2)).sum::<f64>()).sqrt() as f32;
    let power = power_for_distance(dist, ear.radius());
    let src = s.vib_source(info);
    s.sensor_activate(p, st, ear, src, power, freq);
}

impl Server {
    fn sensor_neighbours(&mut self, p: P, st: u32) {
        let b = self.v().block(st);
        self.update_neighbors_at(p, b);
        self.update_neighbors_at(below(p), b);
    }

    fn sensor_pitch(&mut self) -> f32 {
        self.bs.rng.next_float() * 0.2 + 0.8
    }

    /// Active phase: power, end tick, neighbours, resonance, clicking (event attributed
    /// to the source), sound except underwater.
    fn sensor_activate(&mut self, p: P, st: u32, ear: BlockEar, src: Option<Src>, power: i32, freq: i32) {
        let Some(ns) = self.with_prop(st, PHASE, "active") else { return };
        let ns = self.v().set_int(ns, "power", power);
        self.set_update(p, ns);
        let b = self.v().block(st);
        self.schedule_tick(p, b, if ear == BlockEar::Calibrated { 10 } else { 30 });
        self.sensor_neighbours(p, st);
        self.sensor_resonate(p, src, freq);
        self.game_event_src("minecraft:sculk_sensor_tendrils_clicking", center(p), src, None);
        if !self.v().get_bool(st, "waterlogged") {
            let pitch = self.sensor_pitch();
            self.block_sound(p, "minecraft:block.sculk_sensor.clicking", 1.0, pitch);
        }
    }

    /// Neighbouring resonant blocks (down, up, north, south, west, east): resonance event for
    /// the frequency (with their state) and amethyst sound.
    fn sensor_resonate(&mut self, p: P, src: Option<Src>, freq: i32) {
        if !(1..=15).contains(&freq) {
            return;
        }
        for d in Dir::ALL {
            let q = rel(p, d);
            if self.btag_id(q, crate::tag_bits::TagId::VibrationResonators) {
                let qs = self.get(q);
                self.game_event_src(&resonance_event(freq), center(q), src, Some(qs));
                self.block_sound(q, "minecraft:block.amethyst_block.resonate", 1.0, resonance_pitch(freq));
            }
        }
    }

    /// Scheduled tick of the sensor; true if the block is one.
    pub(crate) fn sensor_block_tick(&mut self, st: u32, p: P) -> bool {
        if sensor(self.state_name(st)).is_none() {
            return false;
        }
        match self.v().get(st, PHASE) {
            Some("active") => {
                let Some(ns) = self.with_prop(st, PHASE, "cooldown") else { return true };
                let ns = self.v().set_int(ns, "power", 0);
                self.set_update(p, ns);
                let b = self.v().block(st);
                self.schedule_tick(p, b, 10);
                self.sensor_neighbours(p, st);
            }
            Some("cooldown") => {
                if let Some(ns) = self.with_prop(st, PHASE, "inactive") {
                    self.set_update(p, ns);
                }
                if !self.v().get_bool(st, "waterlogged") {
                    let pitch = self.sensor_pitch();
                    self.block_sound(p, "minecraft:block.sculk_sensor.clicking_stop", 1.0, pitch);
                }
            }
            _ => {}
        }
        true
    }

    /// Entity on the ground (feet `pos`, supporting block `support`), each tick: effect block at 0.2
    /// under the feet looked up only if one of its possible cells (feet column or that of the
    /// supporting block, the supporting block itself) is a registered vibration block.
    pub(crate) fn sensor_entity_on(&mut self, pos: [f64; 3], support: Option<P>, src: Src, warden: bool) {
        let y = (pos[1] - f64::from(0.2f32)).floor() as i32;
        let vb = &self.mobs.rec.vib_blocks;
        let feet = [pos[0].floor() as i32, y, pos[2].floor() as i32];
        if !vb.contains_key(&feet) && !support.is_some_and(|s| vb.contains_key(&s) || vb.contains_key(&[s[0], y, s[2]]))
        {
            return;
        }
        let at = self.on_pos(pos, support, 0.2);
        self.sensor_step_on(at, src, pos, warden);
    }

    /// Entity on the ground on block `p` (effect block, each tick): an inactive sensor receives
    /// a forced step, except from the warden.
    pub(crate) fn sensor_step_on(&mut self, p: P, src: Src, origin: [f64; 3], warden: bool) {
        // Called for every entity on the ground: the small table of registered vibration blocks
        // first (avoids reading the chunk), then the block identifier.
        if !self.mobs.rec.vib_blocks.contains_key(&p) {
            return;
        }
        let st = self.get(p);
        // Shrieker: shriek attempt at each tick for a player behind the entity (itself,
        // steering rider), without looking at sneaking.
        if is_shrieker(self.state_name(st)) {
            self.shrieker_step_on(p, src);
            return;
        }
        let (b, ids) = (self.v().block(st), &self.bs.t.id);
        if warden || (b != ids.sculk_sensor && b != ids.calibrated_sculk_sensor) || self.block_entity(p).is_none() {
            return;
        }
        if self.v().get(st, PHASE) == Some("inactive") && can_receive(self, p, p, "minecraft:step") {
            self.vib_force_schedule(p, "minecraft:step", origin, Some(src));
        }
    }

    /// Placed with a power but no scheduled tick (command, structure): reset to 0 (flags 18).
    pub(crate) fn sensor_on_place(&mut self, st: u32, p: P, same_block: bool) {
        if same_block || sensor(self.state_name(st)).is_none() {
            return;
        }
        let b = self.v().block(st);
        if self.v().get_int(st, "power") > 0 && !self.has_scheduled_tick(p, b) {
            let off = self.v().set_int(st, "power", 0);
            self.set_block(p, off, flag::CLIENTS | flag::KNOWN_SHAPE);
        }
    }

    /// Removed: neighbours notified only if it was active. True if the block is a sensor.
    pub(crate) fn sensor_after_removal(&mut self, old: u32, p: P) -> bool {
        if sensor(self.state_name(old)).is_none() {
            return false;
        }
        if self.v().get(old, PHASE) == Some("active") {
            self.sensor_neighbours(p, old);
        }
        true
    }

    /// Comparator: last frequency received during the active phase, otherwise 0.
    pub(crate) fn sensor_comparator(&self, p: P, st: u32) -> i32 {
        if self.v().get(st, PHASE) == Some("active") { self.be_int(p, "last_vibration_frequency") } else { 0 }
    }

    /// Calibrated: no signal toward the back (side of its input).
    pub(crate) fn sensor_signal_blocked(&self, st: u32, d: Dir) -> bool {
        self.state_name(st) == "minecraft:calibrated_sculk_sensor" && self.v().facing(st) == d
    }
}
