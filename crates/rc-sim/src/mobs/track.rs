//! Network tracking of a mob, like the official server: pairing
//! (add, metadata, equipment), then each tick the changes: relative position
//! (1/4096 of a block) or full sync, rotation, head, velocity, data.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::{AddEntity, Delta, Move, PositionPath, PositionSync};

use super::mob::Mob;
use super::mth::pack_degrees;
use crate::lan::Audience;
use crate::server::Server;

#[derive(Debug, Clone, Default)]
pub struct Tracker {
    /// Clients given the entity. Bit 0: the host's (for an object not tracked per player: every
    /// client). Bit k + 1: the LAN guest of seat k (mobs only). One byte holds the host and the 7
    /// guests of an open world, so the tracker of every item, orb and mob keeps its size.
    observers: u8,
    base: [f64; 3],
    last_yaw: i8,
    last_pitch: i8,
    last_head: i8,
    last_vel: [f64; 3],
    tick: i32,
    teleport_delay: i32,
    was_on_ground: bool,
    /// Velocity to send back to the client and to the owner (hits received).
    pub sync_velocity: bool,
    /// F04: passenger at the last send.
    pub was_riding: bool,
    /// Pushed object (immediate send requested: explosion…): position and velocity sent at the next
    /// tracking, even outside the interval.
    pub needs_sync: bool,
    /// Fingerprints of the syncable attributes at the last send.
    pub attr_hash: super::mob::AttrPrints,
}

impl Tracker {
    /// The first state seen (what the client was given on pairing): later sends compare with it.
    pub(crate) fn begin(&mut self, pos: [f64; 3], vel: [f64; 3], on_ground: bool) {
        if !self.seen() {
            self.set_seen(true);
            self.base = pos;
            self.last_vel = vel;
            self.was_on_ground = on_ground;
        }
    }

    /// The clients were just given the entity at `pos` (a fresh announce): once tracking began,
    /// the next relative move is coded from there.
    pub(crate) fn announced_at(&mut self, pos: [f64; 3]) {
        if self.seen() {
            self.base = pos;
        }
    }

    /// The host's client holds the entity (an object not tracked per player: every client).
    pub(crate) fn seen(&self) -> bool {
        self.observers & 1 != 0
    }

    pub(crate) fn set_seen(&mut self, seen: bool) {
        self.observers = self.observers & !1 | u8::from(seen);
    }

    /// LAN guests holding the mob, by seat (bit k: the guest of seat k).
    pub(crate) fn guests(&self) -> u8 {
        self.observers >> 1
    }

    /// Whether the guest of seat `seat` holds the mob.
    pub(crate) fn has_guest(&self, seat: usize) -> bool {
        seat < crate::lan::GUEST_SEATS && self.guests() & (1 << seat) != 0
    }

    /// Notes whether the guest of seat `seat` (below `lan::GUEST_SEATS`) holds the mob.
    pub(crate) fn set_guest(&mut self, seat: usize, holds: bool) {
        let bit = 1u8 << (seat + 1);
        self.observers = if holds { self.observers | bit } else { self.observers & !bit };
    }

    /// Some client holds the entity: the host's or a guest's.
    pub(crate) fn paired(&self) -> bool {
        self.observers != 0
    }

    /// Every client forgot the entity (a world left: they clear their entities); whether one held
    /// it.
    pub(crate) fn forget_observers(&mut self) -> bool {
        std::mem::take(&mut self.observers) != 0
    }

    /// One player's client forgot the mob (that player changed world): `None` the host's, else
    /// the guest of that seat; the other players' pairings are kept.
    pub(crate) fn forget_player(&mut self, seat: Option<usize>) {
        match seat {
            None => self.set_seen(false),
            Some(k) if k < crate::lan::GUEST_SEATS => self.set_guest(k, false),
            Some(_) => {}
        }
    }

    /// Guest seat `seat` was freed: the guests above it move down one seat.
    pub(crate) fn guest_seat_freed(&mut self, seat: usize) {
        let guests = crate::lan::without_seat(u32::from(self.guests()), seat) as u8;
        self.observers = self.observers & 1 | guests << 1;
    }

    /// Position and velocity to announce the entity with, like the game's add packet built from
    /// its tracking: once tracking began, the last ones sent (the base the next relative move is
    /// coded from, which every client already holds); before, the entity's own.
    pub(crate) fn announce_state(&self, pos: [f64; 3], vel: [f64; 3]) -> ([f64; 3], [f64; 3]) {
        if self.paired() { (self.base, self.last_vel) } else { (pos, vel) }
    }
}

/// Whether a player standing at `pos`, with view distance `view` and the chunks `sent`, tracks the
/// mob: its tracking range (`range`, chunks) capped by the view, horizontal distance, its chunk
/// received.
fn mob_in_view(m: &Mob, range: i32, pos: [f64; 3], view: i32, sent: &hashbrown::HashSet<(i32, i32)>) -> bool {
    let range = (f64::from(range) * 16.0).min(f64::from(view) * 16.0);
    let dx = pos[0] - m.pos()[0];
    let dz = pos[2] - m.pos()[2];
    let c = ((m.pos()[0].floor() as i32) >> 4, (m.pos()[2].floor() as i32) >> 4);
    dx * dx + dz * dz <= range * range && sent.contains(&c) && !m.removed
}

/// What the tracking reads of a player: where it stands, its view distance, the chunks its client
/// holds.
struct ObserverView<'a> {
    pos: [f64; 3],
    view: i32,
    sent: &'a hashbrown::HashSet<(i32, i32)>,
}

impl Server {
    /// The host as the tracking sees it, if it stands in this world (a world where only a LAN
    /// guest stands ticks with that guest installed): the server's own state, or during a
    /// guest's swap the state kept in that guest's seat.
    fn host_observer_view(&self) -> Option<ObserverView<'_>> {
        let swapped_host = self.lan.as_deref().and_then(|l| l.active.and_then(|i| l.guests[i].ctx.as_deref()));
        let (view, client_world) = match swapped_host {
            Some(c) => (ObserverView { pos: c.pos, view: c.view, sent: &c.sent }, &c.client_world),
            None => (ObserverView { pos: self.pos, view: self.view, sent: &self.sent }, &self.client_world),
        };
        self.stands_here(client_world).then_some(view)
    }

    /// The guest of seat `k` as the tracking sees it, if it stands in this world: its seat's state,
    /// or the server's own while that guest is installed.
    fn guest_observer_view<'a>(&'a self, l: &'a crate::lan::Lan, k: usize) -> Option<ObserverView<'a>> {
        if l.active == Some(k) {
            let view = ObserverView { pos: self.pos, view: self.view, sent: &self.sent };
            return self.stands_here(&self.client_world).then_some(view);
        }
        let c = l.guests[k].ctx.as_deref()?;
        self.stands_here(&c.client_world).then_some(ObserverView { pos: c.pos, view: c.view, sent: &c.sent })
    }
}

/// Tracking range of the player entity type (chunks).
const PLAYER_TRACKING: i32 = 32;

/// Mobs each thread refreshes at least when the refresh ahead of the tracking is shared with
/// the helper threads. Waking a helper and waiting for it costs about what refreshing a few
/// hundred mobs does: with helpers the tracking pass is slower at 200 to 740 mobs and a third
/// shorter at 4,250 (`docs/perf.md`). Under twice this count the server thread refreshes alone.
const REFRESH_AHEAD_MOBS_PER_THREAD: usize = 512;

/// `RUSTCRAFT_SYNCED_DATA_MOBS_PER_THREAD=<count>`: another count than
/// `REFRESH_AHEAD_MOBS_PER_THREAD`, for the quiet-machine bench that settles it (read once).
fn refresh_ahead_mobs_per_thread() -> usize {
    static COUNT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *COUNT.get_or_init(|| mobs_per_thread_from(rc_data::env_str!("RUSTCRAFT_SYNCED_DATA_MOBS_PER_THREAD")))
}

/// The count a `setting` asks for; the default without one, or when it is not a count above 0.
pub(super) fn mobs_per_thread_from(setting: Option<&str>) -> usize {
    let asked = setting.and_then(|count| count.trim().parse().ok()).filter(|&count| count > 0);
    asked.unwrap_or(REFRESH_AHEAD_MOBS_PER_THREAD)
}

/// Tests: how the current thread refreshes the synced data, instead of the rule of
/// `refresh_ahead_runs` (equivalence proofs and benches by thread count).
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum ForcedRefresh {
    /// By the tracking of each mob (former path).
    InTracking,
    /// Ahead of the tracking, the list cut into `runs` runs, one per thread (one run: the
    /// server thread alone).
    Ahead { runs: usize },
}

#[cfg(test)]
thread_local! {
    static FORCED_REFRESH: std::cell::Cell<Option<ForcedRefresh>> = const { std::cell::Cell::new(None) };
}

/// Tests: forces the refresh of the synced data for the current thread (`None`: the game's rule).
#[cfg(test)]
pub(crate) fn force_refresh(forced: Option<ForcedRefresh>) {
    FORCED_REFRESH.with(|f| f.set(forced));
}

/// Runs the list of `mobs` mobs is cut into for the refresh ahead of the tracking, one per
/// thread (one run: the server thread alone); none when the tracking refreshes each mob itself.
///
/// `RUSTCRAFT_OPT_SYNCED_DATA_AHEAD=0`: refreshed by the tracking (old path).
/// `RUSTCRAFT_OPT_PARALLEL_SYNCED_DATA=0`: never shared with the helper threads.
/// `RUSTCRAFT_OPT_LARGE_CROWD_REFRESHED_AHEAD_ALONE=1`: a large crowd left to a single thread
/// is still refreshed ahead (off by default: by the tracking, see `RefreshAheadRule::runs`).
fn refresh_ahead_runs(mobs: usize) -> Option<usize> {
    #[cfg(test)]
    match FORCED_REFRESH.with(std::cell::Cell::get) {
        Some(ForcedRefresh::InTracking) => return None,
        Some(ForcedRefresh::Ahead { runs }) => return Some(runs),
        None => {}
    }
    if !rc_data::opt_switch!("RUSTCRAFT_OPT_SYNCED_DATA_AHEAD") {
        return None;
    }
    let shared = rc_data::opt_switch!("RUSTCRAFT_OPT_PARALLEL_SYNCED_DATA");
    let rule = RefreshAheadRule {
        threads: if shared { crate::helper_pool::logical_cores() } else { 1 },
        mobs_per_thread: refresh_ahead_mobs_per_thread(),
        large_crowd_alone: rc_data::opt_switch!("RUSTCRAFT_OPT_LARGE_CROWD_REFRESHED_AHEAD_ALONE", default_off),
    };
    rule.runs(mobs)
}

/// What the refresh ahead of the tracking may use (switches and machine).
#[derive(Debug, Clone, Copy)]
pub(super) struct RefreshAheadRule {
    /// Threads allowed to share the refresh (1: the server thread alone).
    pub threads: usize,
    /// Mobs each thread refreshes at least.
    pub mobs_per_thread: usize,
    /// A large crowd left to a single thread is still refreshed ahead.
    pub large_crowd_alone: bool,
}

impl RefreshAheadRule {
    /// Runs for a list of `mobs` mobs: one per `mobs_per_thread` mobs, at most one per thread;
    /// none when the tracking is to refresh each mob.
    ///
    /// A crowd large enough for the helpers but left to a single thread (one core, or the
    /// sharing switched off) goes back to the tracking: walked twice by one thread it no longer
    /// fits the caches, and the loop of its own was equal to 37 % slower from 1,300 mobs up
    /// (`docs/perf.md`).
    pub(super) fn runs(&self, mobs: usize) -> Option<usize> {
        let mobs_per_thread = self.mobs_per_thread.max(1);
        let runs = (mobs / mobs_per_thread).clamp(1, self.threads.max(1));
        let large_crowd = mobs >= 2 * mobs_per_thread;
        if runs == 1 && large_crowd && !self.large_crowd_alone {
            return None;
        }
        Some(runs)
    }
}

fn unpack(b: i8) -> f32 {
    f32::from(b) * 360.0 / 256.0
}

fn enc(v: f64) -> i64 {
    rc_proto_v777::entity::delta_unit(v)
}

/// The tracking sends a velocity `new` after `sent` if they differ beyond 1e-7 squared, or at
/// all when it fell to zero.
pub fn velocity_changed(sent: [f64; 3], new: [f64; 3]) -> bool {
    let d = [new[0] - sent[0], new[1] - sent[1], new[2] - sent[2]];
    let diff = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    diff > 1.0e-7 || (diff > 0.0 && new == [0.0; 3])
}

/// Velocity as the client decodes it.
pub fn lp_quantize(v: [f64; 3]) -> [f64; 3] {
    let v = v.map(|x| if x.is_nan() { 0.0 } else { x.clamp(-1.717_986_918_3e10, 1.717_986_918_3e10) });
    let m = v[0].abs().max(v[1].abs()).max(v[2].abs());
    if m < 3.051_944_088_384_301e-5 {
        return [0.0; 3];
    }
    let scale = m.ceil();
    v.map(|x| {
        let q = ((x / scale * 0.5 + 0.5) * 32766.0).round().min(32766.0);
        (q * 2.0 / 32766.0 - 1.0) * scale
    })
}

impl Server {
    /// Pairs or unpairs the mob with each player by that player's own range and chunks, like the
    /// game's entity tracking: the host first, then the guests standing in this world, by seat.
    /// The pairing and removal packets go to that player alone. Each player is read where its
    /// state lies, whichever player is installed (a mob spawned by a guest's action is tracked
    /// in that guest's packet).
    fn update_mob_observers(&self, m: &mut Mob) {
        let range = self.effective_tracking(m);
        let host_sees = self.host_observer_view().is_some_and(|v| mob_in_view(m, range, v.pos, v.view, v.sent));
        if host_sees != m.track.seen() {
            self.pair_or_unpair(m, Audience::HOST, host_sees);
            m.track.set_seen(host_sees);
        }
        let Some(l) = self.lan.as_deref() else { return };
        for (k, g) in l.guests.iter().enumerate().take(crate::lan::GUEST_SEATS) {
            let shown = g.joined && !g.gone;
            let sees = shown
                && self.guest_observer_view(l, k).is_some_and(|v| mob_in_view(m, range, v.pos, v.view, v.sent));
            if sees != m.track.has_guest(k) {
                // A guest gone or not yet in game no longer has a client to tell.
                if shown {
                    self.pair_or_unpair(m, Audience::guest(k), sees);
                }
                m.track.set_guest(k, sees);
            }
        }
    }

    /// Pairs (`pair`) or removes the mob for the players of `to`. The first player given the
    /// mob starts its shared tracking state; a later one is given that state as it stands.
    fn pair_or_unpair(&self, m: &mut Mob, to: Audience, pair: bool) {
        self.to_audience(to, || {
            if !pair {
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![m.id])));
            } else if m.track.paired() {
                self.pair_one_more(m);
            } else {
                self.pair(m);
            }
        });
    }

    /// Tracking range (chunks): the type's, or that of a direct or indirect passenger if
    /// larger (a mounted player brings its mount to 32 chunks).
    pub(crate) fn effective_tracking(&self, m: &Mob) -> i32 {
        self.tracking_with_passengers(m.kind.info().tracking, &m.mount.passengers)
    }

    /// `base` range (chunks) raised by direct and indirect passengers (mobs, minecarts).
    pub(crate) fn tracking_with_passengers(&self, base: i32, passengers: &[super::mob::Ent]) -> i32 {
        let mut range = base;
        // Common case: no passenger, nothing to walk (nor allocation).
        if passengers.is_empty() {
            return range;
        }
        let mut stack: Vec<super::mob::Ent> = passengers.to_vec();
        let mut guard = 0;
        while let Some(p) = stack.pop() {
            guard += 1;
            if guard > 64 {
                break;
            }
            match p {
                super::mob::Ent::Player => range = range.max(PLAYER_TRACKING),
                super::mob::Ent::Mob(id) => {
                    if let Some(o) = self.mob_ref(id) {
                        range = range.max(o.kind.info().tracking);
                        stack.extend(o.mount.passengers.iter().copied());
                    }
                }
            }
        }
        range
    }

    /// First pairing: add, non-default data, equipment; the shared tracking state starts from
    /// what was sent. The caller notes who was paired.
    fn pair(&self, m: &mut Mob) {
        m.sync_data();
        let add = AddEntity {
            id: m.id,
            uuid: m.uuid,
            kind: self.entity_kind_id(m.kind.info().name),
            pos: m.pos(),
            velocity: lp_quantize(m.vel),
            pitch: unpack(pack_degrees(m.pitch)),
            yaw: unpack(pack_degrees(m.yaw)),
            head_yaw: unpack(pack_degrees(m.head_yaw)),
            // F14: emerging warden (1).
            data: super::recent::add_entity_data(m),
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let values = m.non_default();
        m.discard_changes();
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id: m.id, values }));
        }
        let slots: rc_proto_v777::entity::Equipment =
            m.equipment.iter().enumerate().filter_map(|(i, s)| Some((i as u8, self.net(Some(s.as_ref()?))))).collect();
        if !slots.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Equipment { id: m.id, slots }));
        }
        // Active effects: never sent at pairing (only a rider receives them).
        self.pair_mount(m); // F04: passengers, mount attributes
        self.pair_leash(m); // G4-F07a: held lead
        let base = m.pos();
        let t = &mut m.track;
        t.base = base;
        t.last_yaw = pack_degrees(m.yaw);
        t.last_pitch = pack_degrees(m.pitch);
        t.last_head = pack_degrees(m.head_yaw);
        t.last_vel = m.vel;
        t.was_on_ground = m.on_ground;
    }

    /// Pairing of one more player while others hold the mob, like the game's shared tracker: the
    /// add carries the state last sent to them (the position the next moves are coded from, the
    /// rotations, the velocity), then the current values and equipment. Nothing the other players
    /// are still to be sent (changed data or attributes) is marked as sent.
    fn pair_one_more(&self, m: &mut Mob) {
        m.sync_data();
        let t = &m.track;
        let add = AddEntity {
            id: m.id,
            uuid: m.uuid,
            kind: self.entity_kind_id(m.kind.info().name),
            pos: t.base,
            velocity: lp_quantize(t.last_vel),
            pitch: unpack(t.last_pitch),
            yaw: unpack(t.last_yaw),
            head_yaw: unpack(t.last_head),
            data: super::recent::add_entity_data(m),
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let values = m.non_default();
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id: m.id, values }));
        }
        let slots: rc_proto_v777::entity::Equipment =
            m.equipment.iter().enumerate().filter_map(|(i, s)| Some((i as u8, self.net(Some(s.as_ref()?))))).collect();
        if !slots.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Equipment { id: m.id, slots }));
        }
        // The pairing sends the passengers and attributes as they stand without marking them sent:
        // a change of this tick still reaches the players already paired at the end-of-tick send.
        let attributes_sent = m.track.attr_hash;
        let passengers_sent = std::mem::take(&mut m.mount.sent_passengers);
        self.pair_mount(m); // F04: passengers, mount attributes
        m.track.attr_hash = attributes_sent;
        m.mount.sent_passengers = passengers_sent;
        self.pair_leash(m); // G4-F07a: held lead
    }

    /// The mob leaves every client that holds it (it changes world).
    pub(crate) fn unpair(&self, m: &mut Mob) {
        if m.track.paired() {
            self.to_audience(Audience::observers_of(&m.track), || {
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![m.id])));
            });
            m.track.forget_observers();
        }
    }

    pub(crate) fn entity_kind_id(&self, name: &str) -> i32 {
        self.entity_kind_index(name).map_or(0, |i| i as i32)
    }

    /// Network tracking of every mob of the list, in list order (end of the mob tick).
    pub(crate) fn track_mobs(&mut self) {
        let refreshed_ahead = self.refresh_synced_data_ahead();
        for slot in 0..self.mobs.list.len() {
            let Some(mut m) = self.mobs.list.take(slot) else { continue };
            self.send_changes_refreshing_unless(&mut m, refreshed_ahead);
            self.mobs.list.put(slot, m);
        }
    }

    /// Refreshes the synced data of every mob in a loop of its own, ahead of the tracking, which
    /// then sends in list order without refreshing again: two short loops run faster than the
    /// long one (tracking pass 1 to 14 % shorter up to 740 mobs), and a large crowd is cut into
    /// runs shared with the helper threads. False when the tracking is to refresh each mob (the
    /// refresh ahead switched off, or a large crowd with a single thread to walk it twice).
    ///
    /// Same packets: the refresh reads and writes its own mob only, and nothing the tracking of
    /// a mob does before its own refresh (range test, pairing, boss bar) changes what the
    /// refresh reads.
    fn refresh_synced_data_ahead(&mut self) -> bool {
        let Some(runs) = refresh_ahead_runs(self.mobs.list.len()) else { return false };
        if runs <= 1 {
            self.mobs.list.iter_mut().for_each(Mob::sync_data);
        } else {
            self.mobs.list.update_in_parallel(runs, Mob::sync_data);
        }
        true
    }

    /// End of tick (the chunk map tick → sending the changes): refreshes the synced data of the
    /// mob, then sends what changed.
    pub(crate) fn send_changes(&self, m: &mut Mob) {
        self.send_changes_refreshing_unless(m, false);
    }

    /// Sends the changes of a mob, refreshing its synced data on the way unless the loop ahead
    /// of the tracking already did this tick (`refreshed_ahead`).
    fn send_changes_refreshing_unless(&self, m: &mut Mob, refreshed_ahead: bool) {
        self.update_mob_observers(m);
        super::wither::boss::bar_track(self, m); // F16: boss bar
        if !refreshed_ahead {
            m.sync_data();
        }
        #[cfg(test)]
        if refreshed_ahead {
            m.assert_synced_data_is_fresh();
        }
        if !m.track.paired() {
            m.discard_changes();
            m.track.tick += 1;
            return;
        }
        // One set of packets, to the players paired with the mob only (the game's tracked entity).
        self.to_audience(Audience::observers_of(&m.track), || self.send_mob_changes(m));
    }

    /// What changed of a paired mob since the last send: passengers, then on its interval (or when
    /// forced) position or rotation, velocity, data, attributes, head; a velocity asked for.
    fn send_mob_changes(&self, m: &mut Mob) {
        self.track_passengers(m); // F04
        // F10: the allay is tracked every 2 ticks.
        let interval = if m.kind == super::species::Kind::Allay { 2 } else { 3 };
        let id = m.id;
        let dirty_data = m.data.iter().any(|s| s.dirty);
        let t_tick = m.track.tick;
        if m.needs_sync || t_tick % interval == 0 || dirty_data {
            let yaw = pack_degrees(m.yaw);
            let pitch = pack_degrees(m.pitch);
            let rot = (i32::from(yaw) - i32::from(m.track.last_yaw)).abs() >= 1
                || (i32::from(pitch) - i32::from(m.track.last_pitch)).abs() >= 1;
            m.track.teleport_delay += 1;
            let pos = m.pos();
            let d = [pos[0] - m.track.base[0], pos[1] - m.track.base[1], pos[2] - m.track.base[2]];
            let changed = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= f64::from(7.629_394_5e-6f32);
            // F04: a passenger sends only its rotation; on dismount, full sync.
            let riding = m.mount.vehicle.is_some();
            let was_riding = std::mem::replace(&mut m.track.was_riding, riding);
            let send_pos = !riding && (changed || t_tick % 60 == 0);
            // F14: precise position required (immobilised happy ghast): full sync.
            let precise = super::recent::requires_precise_position(m);
            let full = !riding
                && (precise || m.track.teleport_delay > 400 || m.track.was_on_ground != m.on_ground || was_riding);
            let mut sent_pos = false;
            let mut sent_rot = false;
            if full {
                m.track.was_on_ground = m.on_ground;
                m.track.teleport_delay = 0;
                self.sync_full(m);
                sent_pos = true;
                sent_rot = true;
            } else if send_pos {
                let e = [
                    enc(pos[0]) - enc(m.track.base[0]),
                    enc(pos[1]) - enc(m.track.base[1]),
                    enc(pos[2]) - enc(m.track.base[2]),
                ];
                if e.iter().any(|v| !(-32768..=32767).contains(v)) {
                    self.sync_full(m);
                    sent_rot = true;
                } else {
                    let mv = Move {
                        id,
                        on_ground: m.on_ground,
                        delta: Some(Delta::Linear([e[0] as i16, e[1] as i16, e[2] as i16])),
                        rot: rot.then(|| (unpack(yaw), unpack(pitch))),
                    };
                    let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                    sent_rot = rot;
                }
                sent_pos = true;
            } else if rot {
                let mv = Move { id, on_ground: m.on_ground, delta: None, rot: Some((unpack(yaw), unpack(pitch))) };
                let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                sent_rot = true;
            }
            // Velocity (mobs: tracked position difference).
            let v = m.vel;
            let dv = [v[0] - m.track.last_vel[0], v[1] - m.track.last_vel[1], v[2] - m.track.last_vel[2]];
            let diff = dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2];
            // Bat: type without velocity tracking, except forced synchronisation.
            // F16: nor the wither.
            let deltas =
                (m.kind != super::species::Kind::Bat && !super::wither::no_track_deltas(m.kind)) || m.needs_sync;
            if deltas && (diff > 1.0e-7 || (diff > 0.0 && v == [0.0; 3])) {
                m.track.last_vel = v;
                let _ = self.send(Event::Entity(EntityEvent::Motion { id, velocity: lp_quantize(v) }));
            }
            let values = m.take_dirty();
            if !values.is_empty() {
                let _ = self.send(Event::Entity(EntityEvent::Data { id, values }));
            }
            // Then only the changed syncable attributes (base or modifiers).
            if super::mob::syncable_fingerprint(&m.attrs) != m.track.attr_hash {
                self.send_mob_attributes(m, true);
            }
            if sent_pos {
                m.track.base = pos;
            }
            if sent_rot {
                m.track.last_yaw = yaw;
                m.track.last_pitch = pitch;
            }
            let head = pack_degrees(m.head_yaw);
            if (i32::from(head) - i32::from(m.track.last_head)).abs() >= 1 {
                let _ = self.send(Event::Entity(EntityEvent::Head { id, yaw: unpack(head) }));
                m.track.last_head = head;
            }
            m.needs_sync = false;
        }
        m.track.tick += 1;
        if m.track.sync_velocity {
            m.track.sync_velocity = false;
            let _ = self.send(Event::Entity(EntityEvent::Motion { id, velocity: lp_quantize(m.vel) }));
        }
    }

    /// Tracking of an item without head or rotation (dropped item, orb): same rule as
    /// mobs, interval specific to the type (20 for items and orbs).
    pub(crate) fn track_object(
        &self,
        t: &mut Tracker,
        id: i32,
        pos: [f64; 3],
        vel: [f64; 3],
        on_ground: bool,
        interval: i32,
    ) {
        self.track_turned(t, id, pos, vel, on_ground, interval, [0.0; 2]);
    }

    /// Same tracking for an object rotated once and for all (armour stand, mannequin):
    /// the full sync carries its rotation (`rot`: yaw, pitch).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn track_turned(
        &self,
        t: &mut Tracker,
        id: i32,
        pos: [f64; 3],
        vel: [f64; 3],
        on_ground: bool,
        interval: i32,
        rot: [f32; 2],
    ) {
        t.begin(pos, vel, on_ground);
        let tick = t.tick;
        if tick % interval == 0 || t.needs_sync {
            t.needs_sync = false;
            t.teleport_delay += 1;
            let d = [pos[0] - t.base[0], pos[1] - t.base[1], pos[2] - t.base[2]];
            let changed = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= f64::from(7.629_394_5e-6f32);
            let send_pos = changed || tick % 60 == 0;
            let full = t.teleport_delay > 400 || t.was_on_ground != on_ground;
            let sync = |s: &Server| {
                let p = PositionSync {
                    id,
                    path: PositionPath { linear: true, steps: vec![(pos, 0)] },
                    yaw: rot[0],
                    pitch: rot[1],
                    on_ground,
                };
                let _ = s.send(Event::Entity(EntityEvent::Sync(p)));
            };
            if full {
                t.was_on_ground = on_ground;
                t.teleport_delay = 0;
                sync(self);
                t.base = pos;
            } else if send_pos {
                let e = [enc(pos[0]) - enc(t.base[0]), enc(pos[1]) - enc(t.base[1]), enc(pos[2]) - enc(t.base[2])];
                if e.iter().any(|v| !(-32768..=32767).contains(v)) {
                    sync(self);
                } else {
                    let mv = Move {
                        id,
                        on_ground,
                        delta: Some(Delta::Linear([e[0] as i16, e[1] as i16, e[2] as i16])),
                        rot: None,
                    };
                    let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                }
                t.base = pos;
            }
            if velocity_changed(t.last_vel, vel) {
                t.last_vel = vel;
                let _ = self.send(Event::Entity(EntityEvent::Motion { id, velocity: lp_quantize(vel) }));
            }
        }
        t.tick += 1;
        // Hit received (velocity send requested): velocity sent back as is after the tracking.
        if t.sync_velocity {
            t.sync_velocity = false;
            let _ = self.send(Event::Entity(EntityEvent::Motion { id, velocity: lp_quantize(vel) }));
        }
    }

    /// F14: position sync sent right away, coding base re-aligned.
    pub(crate) fn sync_position_now(&self, m: &mut Mob) {
        if m.track.paired() {
            m.track.base = m.pos();
            self.to_audience(Audience::observers_of(&m.track), || self.sync_full(m));
        }
    }

    fn sync_full(&self, m: &Mob) {
        let s = PositionSync {
            id: m.id,
            path: PositionPath { linear: true, steps: vec![(m.pos(), 0)] },
            yaw: m.yaw,
            pitch: m.pitch,
            on_ground: m.on_ground,
        };
        let _ = self.send(Event::Entity(EntityEvent::Sync(s)));
    }
}
