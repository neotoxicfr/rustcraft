//! Integrated server beacon (G5-F11): block entity tick, menu and `set_beacon` packet.
//!
//! Tick, as in the game: the column above the beacon is examined in slices of 10 blocks
//! up to the surface (stained glass or beacon: the segment continues; a block opaque to light
//! other than bedrock: the column is cut); every 80 game ticks, if the last complete
//! column carried a beam, the pyramid is recounted (4 tiers at most), then the
//! effects are given to the players in range and the ambient sound played; when a column is
//! finished, going from 0 to at least 1 level plays the activation sound and triggers
//! `construct_beacon` for nearby players, the opposite transition plays the deactivation sound.
//!
//! The server keeps only what changes behaviour from the column: whether a segment is
//! open, whether the last complete column had one. Colours and heights of the segments
//! only serve rendering, which the client computes itself. The levels live in memory as in
//! the game: they restart from zero on load (the `Levels` field is written, never read back), hence
//! the activation sound replayed when the chunk of a complete beacon loads (faithful to the game).

use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;

use crate::blocks::P;
use crate::blocks::level::BeTick;
use crate::menus::{Kind, Menu};
use crate::server::Server;

/// Network type of the beacon menu (`menu` registry).
pub(crate) const MENU_TYPE: i32 = 9;

/// Offered effects, with the number of tiers they require.
const EFFECTS: [(&str, i32); 6] = [
    ("minecraft:speed", 1),
    ("minecraft:haste", 1),
    ("minecraft:resistance", 2),
    ("minecraft:jump_boost", 2),
    ("minecraft:strength", 3),
    ("minecraft:regeneration", 4),
];

/// Tiers required by an effect: 0 for no effect, `i32::MAX` for an effect outside the list.
fn required(e: Option<&str>) -> i32 {
    match e {
        None => 0,
        Some(e) => EFFECTS.iter().find(|x| x.0 == e).map_or(i32::MAX, |x| x.1),
    }
}

/// Is the choice (primary, secondary) allowed with `levels` tiers?
pub(crate) fn valid_choice(primary: Option<&str>, secondary: Option<&str>, levels: i32) -> bool {
    if secondary.is_some() && levels < 4 {
        return false;
    }
    let (pl, sl) = (required(primary), required(secondary));
    if pl > levels || sl > levels || pl >= 4 {
        return false;
    }
    sl == 0 || sl >= 4 || primary == secondary
}

/// Ticker state: last height examined, segment open in the current column,
/// beam of the last complete column, tiers of the pyramid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BeaconTick {
    pub last_y: i32,
    pub checking: bool,
    pub beam: bool,
    pub levels: i32,
}

/// Saved effect (`primary_effect`, `secondary_effect`), ignored outside the list.
fn stored(be: &Nbt, key: &str) -> Option<&'static str> {
    let id = be.get(key).and_then(Nbt::as_str)?;
    EFFECTS.iter().find(|x| x.0 == id).map(|x| x.0)
}

fn center(p: P) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

impl Server {
    /// Ticker set on creation or loading of the block entity (fresh state, levels at zero).
    pub(crate) fn beacon_ticker_register(&mut self, p: P) {
        if matches!(self.bs.be_ticks.get(&p), Some((_, BeTick::Beacon(_)))) {
            return;
        }
        let id = self.add_ticker(p);
        let b = BeaconTick { last_y: self.dim.height.min_y - 1, checking: false, beam: false, levels: 0 };
        self.bs.be_ticks.insert(p, (id, BeTick::Beacon(b)));
    }

    fn beacon_state(&self, p: P) -> Option<BeaconTick> {
        match self.bs.be_ticks.get(&p) {
            Some((_, BeTick::Beacon(b))) => Some(*b),
            _ => None,
        }
    }

    fn beacon_sound(&self, p: P, name: &str) {
        self.play_sound(name, 4, center(p), 1.0, 1.0);
    }

    /// Does block `s` stop the beam (opaque to light, except bedrock)?
    fn beam_stopper(&self, s: u32) -> bool {
        self.data.state(s).is_some_and(|i| i.light_dampening >= 15)
            && self.v().block(s) != self.bs.t.block_id("bedrock")
    }

    /// Block entity tick.
    pub(crate) fn beacon_tick(&mut self, p: P, mut b: BeaconTick) {
        let [x, y, z] = p;
        let mut cy = if b.last_y < y {
            b.checking = false;
            b.last_y = y - 1;
            y
        } else {
            b.last_y + 1
        };
        let surface = self.surface_y(x, z, 1);
        let mut i = 0;
        while i < 10 && cy <= surface {
            let s = self.get([x, cy, z]);
            if self.bs.t.beam.get(usize::from(self.v().block(s))).copied().unwrap_or(false) {
                b.checking = true;
            } else if !b.checking || self.beam_stopper(s) {
                b.checking = false;
                b.last_y = surface;
                break;
            }
            cy += 1;
            b.last_y += 1;
            i += 1;
        }
        let prev = b.levels;
        let pulse = self.time % 80 == 0;
        if pulse && b.beam {
            b.levels = self.beacon_base(x, y, z);
        }
        let apply = pulse && b.levels > 0 && b.beam;
        let finished = b.last_y >= surface;
        if finished {
            b.last_y = self.dim.height.min_y - 1;
            b.beam = b.checking;
        }
        // State stored before the effects (an advancement can run a function that removes the beacon).
        if let Some((_, t)) = self.bs.be_ticks.get_mut(&p) {
            *t = BeTick::Beacon(b);
        }
        if b.levels != prev {
            self.beacon_store_levels(p, b.levels);
        }
        if apply {
            self.beacon_effects(p, b.levels);
            self.beacon_sound(p, "minecraft:block.beacon.ambient");
        }
        if finished {
            let (was, is) = (prev > 0, b.levels > 0);
            if !was && is {
                self.beacon_sound(p, "minecraft:block.beacon.activate");
                let (xf, yf, zf) = (f64::from(x), f64::from(y), f64::from(z));
                let levels = b.levels;
                self.beacon_players([xf - 10.0, yf - 9.0, zf - 10.0], [xf + 10.0, yf + 5.0, zf + 10.0], &|sv| {
                    sv.trig_construct_beacon(levels);
                });
            } else if was && !is {
                self.beacon_sound(p, "minecraft:block.beacon.deactivate");
            }
        }
    }

    /// `Levels` of the block entity kept up to date for saving, without marking the chunk to
    /// be rewritten (the game does not mark it for a change of levels).
    fn beacon_store_levels(&mut self, p: P, levels: i32) {
        let Some(i) = self.be_index(p) else { return };
        if let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))
            && let Some(be) = c.block_entities.get_mut(i)
        {
            crate::nbtio::set(be, "Levels", Nbt::Int(levels));
        }
    }

    /// Complete tiers of the pyramid under the beacon (full squares of `beacon_base_blocks`).
    pub(crate) fn beacon_base(&self, x: i32, y: i32, z: i32) -> i32 {
        let Some(base) = self.block_tags.get("minecraft:beacon_base_blocks") else { return 0 };
        let min_y = self.dim.height.min_y;
        let mut levels = 0;
        for step in 1..=4 {
            let ly = y - step;
            if ly < min_y {
                break;
            }
            let full = (x - step..=x + step).all(|lx| {
                (z - step..=z + step).all(|lz| {
                    let b = self.data.state(self.get([lx, ly, lz])).map_or(u32::MAX, |s| u32::from(s.block));
                    base.binary_search(&b).is_ok()
                })
            });
            if !full {
                break;
            }
            levels = step;
        }
        levels
    }

    /// Effects given to the players of the area (radius 10 + 10 per tier, the whole height above).
    fn beacon_effects(&mut self, p: P, levels: i32) {
        let Some(be) = self.be_get(p) else { return };
        let (Some(primary), secondary) = (stored(be, "primary_effect"), stored(be, "secondary_effect")) else { return };
        let r = f64::from(levels * 10 + 10);
        let amp = i32::from(levels >= 4 && Some(primary) == secondary);
        let dur = (9 + levels * 2) * 20;
        let second = secondary.filter(|s| levels >= 4 && *s != primary);
        let h = f64::from(self.dim.height.sections as i32 * 16);
        let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        let min = [x - r, y - r, z - r];
        let max = [x + 1.0 + r, y + 1.0 + r + h, z + 1.0 + r];
        self.beacon_players(min, max, &|sv| {
            sv.add_effect(beacon_effect(primary, dur, amp));
            if let Some(s) = second {
                sv.add_effect(beacon_effect(s, dur, 0));
            }
        });
    }

    /// `f` for each player (spectators excluded) whose box touches the area, the host first.
    fn beacon_players(&mut self, min: [f64; 3], max: [f64; 3], f: &dyn Fn(&mut Server)) {
        let touches = |sv: &Server| {
            let b = sv.player_aabb();
            sv.game_mode != 3 && (0..3).all(|a| b.min[a] < max[a] && b.max[a] > min[a])
        };
        if touches(self) {
            f(self);
        }
        let n = self.lan.as_ref().map_or(0, |l| l.guests.len());
        for i in 0..n {
            self.as_guest(i, |sv| {
                if touches(sv) {
                    f(sv);
                }
            });
        }
    }

    /// Block entity removed (block broken or replaced): deactivation sound, ticker forgotten.
    pub(crate) fn beacon_removed(&mut self, p: P) {
        self.beacon_sound(p, "minecraft:block.beacon.deactivate");
        self.bs.be_ticks.remove(&p);
    }

    /// Menu data: tiers, primary and secondary effects (registry rank + 1, 0 for none).
    pub(crate) fn beacon_data(&self, p: P) -> [i32; 3] {
        let levels = self.beacon_state(p).map_or(0, |b| b.levels);
        let enc = |k: &str| {
            self.be_get(p).and_then(|be| stored(be, k)).and_then(|e| self.effect_registry_index(e)).map_or(0, |i| i + 1)
        };
        [levels, enc("primary_effect"), enc("secondary_effect")]
    }

    fn effect_registry_index(&self, id: &str) -> Option<i32> {
        self.data.registry("minecraft:mob_effect")?.entries.iter().position(|e| e == id).map(|i| i as i32)
    }

    /// Beacon menu opened for `p` (the block entity must exist).
    pub(crate) fn beacon_menu(&self, p: P, window: i32) -> Option<Menu> {
        self.be_get(p)?;
        let mut m = Menu::beacon(window);
        m.data = self.beacon_data(p).to_vec();
        m.block = Some(p);
        Some(m)
    }

    /// `set_beacon`: chosen effects paid for by the item in the slot; a refused choice (without
    /// payment, or not allowed by the pyramid) disconnects the player, as in the game.
    pub(crate) fn beacon_packet(&mut self, r: &mut rc_proto::Reader<'_>) {
        use rc_proto::Decode;
        let mut read = || -> Option<Option<i32>> {
            if bool::decode(r).ok()? { read_varint(r).ok().map(Some) } else { Some(None) }
        };
        let (Some(primary), Some(secondary)) = (read(), read()) else { return };
        let name = |id: Option<i32>| -> Option<Option<String>> {
            match id {
                None => Some(None),
                Some(i) => self
                    .data
                    .registry("minecraft:mob_effect")
                    .and_then(|r| r.entries.get(usize::try_from(i).ok()?).cloned())
                    .map(Some),
            }
        };
        let (Some(primary), Some(secondary)) = (name(primary), name(secondary)) else { return };
        self.beacon_set(primary.as_deref(), secondary.as_deref());
    }

    pub(crate) fn beacon_set(&mut self, primary: Option<&str>, secondary: Option<&str>) {
        let Some(m) = self.life.menu.as_ref().filter(|m| m.kind == Kind::Beacon) else { return };
        let (block, paid) = (m.block, m.own.first().is_some_and(Option::is_some));
        if !self.menu_valid() {
            return;
        }
        let Some(p) = block else { return };
        let levels = self.beacon_data(p)[0];
        if !paid || !valid_choice(primary, secondary, levels) {
            self.kick_current("multiplayer.disconnect.generic");
            return;
        }
        // Primary effect: choice sound if the beacon has a beam; only the effects of the list
        // are kept.
        if self.beacon_state(p).is_some_and(|b| b.beam) {
            self.beacon_sound(p, "minecraft:block.beacon.power_select");
        }
        let keep = |e: Option<&str>| e.and_then(|e| EFFECTS.iter().find(|x| x.0 == e)).map(|x| x.0);
        if let Some(be) = self.be_get_mut(p) {
            for (k, e) in [("primary_effect", keep(primary)), ("secondary_effect", keep(secondary))] {
                match e {
                    Some(e) => crate::nbtio::set(be, k, Nbt::String(e.to_owned())),
                    None => crate::nbtio::remove(be, k),
                }
            }
        }
        // Payment consumed (one unit).
        let data = self.beacon_data(p).to_vec();
        if let Some(m) = self.life.menu.as_mut() {
            if let Some(s) = &mut m.own[0] {
                s.count -= 1;
            }
            if m.own[0].as_ref().is_some_and(|s| s.count <= 0) {
                m.own[0] = None;
            }
            m.data = data;
        }
        self.broadcast_menus();
    }

    /// The current player (host or LAN guest) is sent back with the message `key`.
    pub(crate) fn kick_current(&mut self, key: &str) {
        match self.lan.as_ref().and_then(|l| l.active) {
            Some(i) => self.lan_kick(i, key),
            None => {
                let _ = self.send(rc_client::Event::Kicked(rc_text::Component::translate(key, Vec::new())));
            }
        }
    }
}

fn beacon_effect(id: &str, dur: i32, amp: i32) -> crate::effects::Inst {
    let mut e = crate::effects::Inst::new(id, dur, amp);
    e.ambient = true;
    e
}
