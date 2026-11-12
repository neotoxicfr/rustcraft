//! Locator bar waypoints sent by the integrated server (`docs/specs/hud.md` § 4.8).
//!
//! Every living entity whose transmit range is above zero is a transmitter (every player by
//! default, any other one through a command, its saved data or its equipment); every player whose
//! receive range is above zero receives. Each (receiver, transmitter) pair holds one connection
//! that decides how precisely the receiver learns where the transmitter is: its exact block while
//! its chunk is in the receiver's view, its chunk outside the view, only a direction beyond
//! [`REALLY_FAR`] blocks. The rules, their quirks included, are the game's (notes of A2-W1).
//!
//! The moment is ours: the game reacts to each position change and each movement packet as it
//! happens; we compare one snapshot of the players and transmitters with the previous one, at the
//! end of the tick and before `/waypoint` answers, which sends the same packets. [`Waypoints`]
//! holds no reference to the server: it reads a [`Snapshot`] and returns the packets, so the rules
//! are tested and measured alone.

use hashbrown::HashMap;
use rc_proto::nbt::Nbt;
use rc_proto_v777::waypoint::{Waypoint, WaypointId, WaypointOp, WaypointPos};

use crate::commands::ents::E;
use crate::commands::scores::Team;
use crate::server::Server;

/// Style of an icon that has none of its own.
pub(crate) const DEFAULT_STYLE: &str = "minecraft:default";
pub(crate) const TRANSMIT_RANGE: &str = "minecraft:waypoint_transmit_range";
const RECEIVE_RANGE: &str = "minecraft:waypoint_receive_range";
/// Beyond this distance (blocks) a waypoint only carries a direction.
const REALLY_FAR: f32 = 332.0;
/// Smallest turn (half a degree, in radians) that a direction-only waypoint sends again.
const AZIMUTH_STEP: f32 = 0.008_726_646;
/// Colour a black team gives its members' waypoints (pure black would not show on the bar),
/// opaque like the game's value.
const BLACK_TEAM_COLOR: u32 = 0xFF30_3030;
/// Opaque alpha the game puts above a colour it builds from channels (hex or float list).
const OPAQUE: u32 = 0xFF00_0000;

/// Icon of a waypoint: style and optional colour, as saved (`locator_bar_icon`) and sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Icon {
    pub style: &'static str,
    /// The game's whole integer, alpha included (saved as is); the packet and the texts only
    /// read its low 24 bits.
    pub color: Option<u32>,
}

/// Colour of `/waypoint modify ... color hex`, read by the command parser's own rule (signed
/// digits or pairs, opaque like the game's value). `None` for a text the parser refuses.
pub(crate) fn hex_color(text: &str) -> Option<u32> {
    rc_commands::args::hex_color(text).ok()
}

/// The low 24 bits of a colour: what the packet and the texts carry.
pub(crate) fn rgb_of(color: u32) -> u32 {
    color & 0xFF_FFFF
}

impl Default for Icon {
    fn default() -> Icon {
        Icon { style: DEFAULT_STYLE, color: None }
    }
}

impl Icon {
    /// Saved only when it differs from the default.
    fn has_data(self) -> bool {
        self.style != DEFAULT_STYLE || self.color.is_some()
    }

    /// Icon a new connection copies: a missing colour taken from the entity's team.
    pub(crate) fn with_team_color(self, team_color: Option<u32>) -> Icon {
        Icon { color: self.color.or(team_color), ..self }
    }

    /// Icon saved under `locator_bar_icon` in an entity compound (the default when absent).
    pub(crate) fn of_entity(entity: Option<&Nbt>) -> Icon {
        let Some(saved) = entity.and_then(|e| e.get("locator_bar_icon")) else { return Icon::default() };
        // Styles are a handful of identifiers: interned, the icon stays a plain copyable value.
        let style = saved.get("style").and_then(Nbt::as_str).map_or(DEFAULT_STYLE, crate::mobs::mob::intern);
        let color = match saved.get("color") {
            // Three floats: each channel rounded down at single precision, out of range values
            // wrapped to 8 bits (no clamp), opaque: the game's reading.
            Some(Nbt::List(rgb)) if rgb.len() == 3 => {
                let channel = |v: &Nbt| {
                    let value = crate::mobs::mob::f64_of(v).unwrap_or(0.0) as f32;
                    rc_data::java_math::floor_f32_to_i32(value * 255.0) as u32 & 0xFF
                };
                Some(OPAQUE | channel(&rgb[0]) << 16 | channel(&rgb[1]) << 8 | channel(&rgb[2]))
            }
            // An integer is kept whole, alpha included.
            Some(c) => c.as_i64().map(|v| v as u32),
            None => None,
        };
        Icon { style, color }
    }

    /// `locator_bar_icon` of an entity compound rewritten (left out when it is the default).
    pub(crate) fn save_into(self, entity: &mut Nbt) {
        if !self.has_data() {
            crate::nbtio::remove(entity, "locator_bar_icon");
            return;
        }
        let mut fields = vec![("style".to_owned(), Nbt::String(self.style.to_owned()))];
        if let Some(c) = self.color {
            fields.push(("color".to_owned(), Nbt::Int(c as i32)));
        }
        crate::nbtio::set(entity, "locator_bar_icon", Nbt::Compound(fields));
    }
}

/// Colour a team gives the waypoints of its members (none without a colour).
pub(crate) fn team_color(team: Option<&Team>) -> Option<u32> {
    match team?.color.as_deref()? {
        "black" => Some(BLACK_TEAM_COLOR),
        name => rc_text::parse_color(name),
    }
}

/// A living entity as the waypoint rules see it this tick.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Body {
    pub id: i32,
    pub uuid: u128,
    pub pos: [f64; 3],
    /// Past its first tick: a brand-new entity offers no connection yet.
    pub ticked: bool,
    pub spectator: bool,
    pub transmit_range: f64,
    /// Icon its new connections copy: its own, its team's colour filling a missing colour.
    pub icon: Icon,
}

/// A player as the waypoint rules see it this tick.
#[derive(Debug, Clone)]
pub(crate) struct Player {
    pub body: Body,
    /// Where its packets go: `None` the host, `Some(k)` LAN guest `k`.
    pub seat: Option<usize>,
    pub receive_range: f64,
    /// Chunk tracking view: centre chunk and view distance.
    pub view_center: (i32, i32),
    pub view: i32,
    /// Entities it rides, nearest first: a transmitter it rides does not show to it.
    pub vehicles: Vec<i32>,
    /// Sent a movement packet since the last pass (the game updates its connections on each one).
    pub sent_movement: bool,
    /// Rank of its entity: renewed when the player entity is replaced (respawn).
    pub generation: u64,
}

impl Player {
    /// Is `chunk` within this player's view (the chunk tracking test)?
    fn sees_chunk(&self, chunk: [i32; 2]) -> bool {
        crate::server::in_view_distance(self.view_center, self.view, (chunk[0], chunk[1]))
    }
}

/// What the waypoint rules read in one pass.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    /// `locator_bar` game rule.
    pub locator_bar: bool,
    pub players: Vec<Player>,
    /// Living entities other than players that transmit, or that the manager already knows.
    pub others: Vec<Body>,
}

/// A packet for one player.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outgoing {
    pub seat: Option<usize>,
    pub packet: Waypoint,
}

/// What a receiver was told of a transmitter's place.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Told {
    /// Exact block: its chunk is in the receiver's view.
    Block([i32; 3]),
    /// Chunk only: outside the receiver's view.
    Chunk([i32; 2]),
    /// Direction only (radians): farther than [`REALLY_FAR`].
    Azimuth(f32),
}

/// Connection of one receiver to one transmitter.
#[derive(Debug, Clone, Copy)]
struct Connection {
    transmitter: u128,
    /// Copied when the connection is made: a later team change does not reach it.
    icon: Icon,
    told: Told,
}

/// What was seen of an entity at the last pass.
#[derive(Debug, Clone, Copy)]
struct Known {
    is_player: bool,
    transmit_range: f64,
    receive_range: f64,
    generation: u64,
    last_pos: Option<[f64; 3]>,
    /// Tracked as a transmitter, with its order of tracking (`/waypoint list`).
    tracked: Option<u64>,
}

impl Known {
    fn new(is_player: bool, generation: u64) -> Known {
        Known { is_player, transmit_range: 0.0, receive_range: 0.0, generation, last_pos: None, tracked: None }
    }
}

/// Place of an entity in the snapshot of the pass.
#[derive(Debug, Clone, Copy)]
enum Place {
    Player(usize),
    Other(usize),
}

/// The world's waypoint manager: transmitters tracked, connections, requests between passes.
#[derive(Debug)]
pub(crate) struct Waypoints {
    known: HashMap<i32, Known>,
    /// Keyed by (receiver id, transmitter id).
    connections: HashMap<(i32, i32), Connection>,
    /// Game rule seen at the last pass (`None`: no pass yet).
    locator_bar: Option<bool>,
    tracking_order: u64,
    /// Players that sent a movement packet since the last pass.
    movement_packets: Vec<i32>,
    /// Transmitters whose icon `/waypoint modify` changed: untracked, then tracked again.
    retrack: Vec<i32>,
    /// Players whose team changed: their connections made again (new colour).
    remake: Vec<i32>,
    /// Snapshot index, rebuilt at each pass (its memory kept from tick to tick).
    index: HashMap<i32, Place>,
    /// Bodies of the snapshot (players first) whose pairs the pass already refreshed from the
    /// transmitter's side (memory kept from tick to tick).
    refreshed: Vec<bool>,
    /// `RUSTCRAFT_OPT_WAYPOINT_PAIR_ONCE` (default on): a pair whose two sides both moved is
    /// refreshed once, unless a pair of that transmitter did not settle (then the second
    /// refresh, from the receiver's side, is done like the game's second update).
    pub(crate) refresh_pair_once: bool,
    /// `RUSTCRAFT_OPT_WAYPOINT_SKIP_IDLE_MOB_SCAN` (default on): no walk of the mob list while
    /// no mob holds a transmit range (the usual world).
    pub(crate) skip_idle_mob_scan: bool,
    /// Snapshot of the last pass, its memory reused by the next one (`Server::waypoints_sync`).
    pub(crate) spare_snapshot: Snapshot,
    /// Views of the guests taken at the end of their own tick (tick, seat, view), for the pass
    /// of that same tick.
    pub(crate) guest_views: Vec<(u64, usize, Player)>,
    /// `RUSTCRAFT_OPT_WAYPOINT_GUEST_VIEW_FROM_TICK` (default on): a guest's view is taken at the
    /// end of its own tick, its state already installed, instead of being installed again for
    /// the pass (two state swaps per guest and tick).
    pub(crate) guest_view_from_tick: bool,
}

impl Default for Waypoints {
    fn default() -> Waypoints {
        Waypoints {
            known: HashMap::new(),
            connections: HashMap::new(),
            locator_bar: None,
            tracking_order: 0,
            movement_packets: Vec::new(),
            retrack: Vec::new(),
            remake: Vec::new(),
            index: HashMap::new(),
            refreshed: Vec::new(),
            refresh_pair_once: rc_data::opt_switch!("RUSTCRAFT_OPT_WAYPOINT_PAIR_ONCE"),
            skip_idle_mob_scan: rc_data::opt_switch!("RUSTCRAFT_OPT_WAYPOINT_SKIP_IDLE_MOB_SCAN"),
            spare_snapshot: Snapshot::default(),
            guest_views: Vec::new(),
            guest_view_from_tick: rc_data::opt_switch!("RUSTCRAFT_OPT_WAYPOINT_GUEST_VIEW_FROM_TICK"),
        }
    }
}

impl Waypoints {
    /// A movement packet of player `id` (the game updates the player's connections on each one).
    pub(crate) fn note_movement_packet(&mut self, id: i32) {
        if !self.movement_packets.contains(&id) {
            self.movement_packets.push(id);
        }
    }

    pub(crate) fn sent_movement_packet(&self, id: i32) -> bool {
        self.movement_packets.contains(&id)
    }

    /// Non-player entities tracked (or about to be, `/waypoint modify` tracks any living entity)
    /// with no transmit range: the pass must still see them, the mob scan only gathers those with
    /// a range. A mob whose range fell to zero is simply absent, which untracks it like the game.
    pub(crate) fn tracked_without_range(&self) -> impl Iterator<Item = i32> + '_ {
        let rangeless = |k: &Known| !k.is_player && k.transmit_range <= 0.0;
        let kept = move |id: &i32, k: &Known| rangeless(k) && (k.tracked.is_some() || self.retrack.contains(id));
        self.known.iter().filter(move |(id, k)| kept(id, k)).map(|(id, _)| *id)
    }

    /// `/waypoint modify` changed the icon of `id`: the next pass untracks it and tracks it again
    /// (even with no range: it is then listed without any connection, like the game).
    pub(crate) fn retrack(&mut self, id: i32, is_player: bool) {
        self.known.entry(id).or_insert_with(|| Known::new(is_player, 0));
        self.retrack.push(id);
    }

    /// The team of player `id` changed: its connections are made again at the next pass.
    pub(crate) fn remake(&mut self, id: i32) {
        self.remake.push(id);
    }

    /// Tracked transmitters, in the order they were tracked.
    pub(crate) fn tracked(&self) -> Vec<i32> {
        let mut v: Vec<(u64, i32)> = self.known.iter().filter_map(|(id, k)| Some((k.tracked?, *id))).collect();
        v.sort_unstable();
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// One pass: what changed since the previous snapshot turned into the game's packets.
    pub(crate) fn sync(&mut self, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        self.index_snapshot(snap);
        if self.locator_bar == Some(true) && !snap.locator_bar {
            self.break_all(snap, out);
        }
        let turned_on = self.locator_bar == Some(false) && snap.locator_bar;
        self.locator_bar = Some(snap.locator_bar);
        self.forget_departed(snap, out);
        self.follow_ranges(snap, out);
        for id in std::mem::take(&mut self.retrack) {
            if self.index.contains_key(&id) {
                self.untrack(id, snap, out);
                self.track(id, snap, out);
            }
        }
        for id in std::mem::take(&mut self.remake) {
            if self.index.contains_key(&id) {
                self.connect_to_every_receiver(id, snap, out);
            }
        }
        if turned_on {
            for p in &snap.players {
                self.update_player(p, snap, &[], out);
            }
        }
        self.follow_moves(snap, out);
        self.movement_packets.clear();
    }

    fn index_snapshot(&mut self, snap: &Snapshot) {
        self.index.clear();
        for (i, p) in snap.players.iter().enumerate() {
            self.index.insert(p.body.id, Place::Player(i));
        }
        for (i, o) in snap.others.iter().enumerate() {
            self.index.entry(o.id).or_insert(Place::Other(i));
        }
    }

    fn body<'s>(&self, snap: &'s Snapshot, id: i32) -> Option<&'s Body> {
        Some(match *self.index.get(&id)? {
            Place::Player(i) => &snap.players[i].body,
            Place::Other(i) => &snap.others[i],
        })
    }

    fn player<'s>(&self, snap: &'s Snapshot, id: i32) -> Option<&'s Player> {
        match *self.index.get(&id)? {
            Place::Player(i) => Some(&snap.players[i]),
            Place::Other(_) => None,
        }
    }

    /// Every body of the snapshot, players first.
    fn bodies(snap: &Snapshot) -> impl Iterator<Item = &Body> {
        snap.players.iter().map(|p| &p.body).chain(snap.others.iter())
    }

    /// Entities gone from the world, and players whose entity was replaced: a player leaves the
    /// receivers (which also untracks it), another entity is untracked.
    fn forget_departed(&mut self, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        let departed: Vec<(i32, bool)> = self
            .known
            .iter()
            .filter(|(id, k)| match self.index.get(*id) {
                None => true,
                Some(Place::Player(i)) => snap.players[*i].generation != k.generation,
                Some(Place::Other(_)) => false,
            })
            .map(|(id, k)| (*id, k.is_player))
            .collect();
        for (id, is_player) in departed {
            if is_player {
                self.remove_player(id, snap, out);
            } else {
                self.untrack(id, snap, out);
            }
            self.known.remove(&id);
        }
    }

    /// Receive and transmit ranges that changed (arrivals included, from zero): the game's
    /// reaction to an updated attribute.
    fn follow_ranges(&mut self, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        for (i, p) in snap.players.iter().enumerate() {
            let id = p.body.id;
            let k = *self.known.entry(id).or_insert_with(|| Known::new(true, p.generation));
            let joined_receivers = p.receive_range != k.receive_range && p.receive_range > 0.0;
            if p.receive_range != k.receive_range {
                self.known_mut(id).receive_range = p.receive_range;
                if joined_receivers {
                    self.add_player(i, snap, out);
                } else {
                    self.remove_player(id, snap, out);
                }
            }
            // Joining the receivers already tracked it (once is what a joining player gets).
            if !joined_receivers || p.body.transmit_range <= 0.0 {
                self.follow_transmit_range(&p.body, snap, out);
            } else {
                self.known_mut(id).transmit_range = p.body.transmit_range;
            }
        }
        for b in &snap.others {
            self.known.entry(b.id).or_insert_with(|| Known::new(false, 0));
            self.follow_transmit_range(b, snap, out);
        }
    }

    /// Transmit range changed: tracked above zero, untracked at zero.
    fn follow_transmit_range(&mut self, b: &Body, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        if b.transmit_range == self.known[&b.id].transmit_range {
            return;
        }
        self.known_mut(b.id).transmit_range = b.transmit_range;
        if b.transmit_range > 0.0 {
            self.track(b.id, snap, out);
        } else {
            self.untrack(b.id, snap, out);
        }
    }

    /// Positions changed since the last pass: a transmitter that moved updates its connections,
    /// a receiver that moved (or sent a movement packet) updates its own.
    fn follow_moves(&mut self, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        let mut refreshed = std::mem::take(&mut self.refreshed);
        refreshed.clear();
        for b in Self::bodies(snap) {
            let k = self.known[&b.id];
            let moved = k.last_pos != Some(b.pos) && b.ticked && b.transmit_range > 0.0 && k.tracked.is_some();
            refreshed.push(moved && self.update_transmitter(b, snap, out));
        }
        // A transmitter refreshed above whose pairs all settled met every player with the same
        // snapshot: refreshing them again from the receiver's side finds nothing more to tell.
        let skip: &[bool] = if self.refresh_pair_once { &refreshed } else { &[] };
        for p in &snap.players {
            let moved = self.known[&p.body.id].last_pos != Some(p.body.pos) && p.body.ticked;
            if (moved || p.sent_movement) && p.receive_range > 0.0 {
                self.update_player(p, snap, skip, out);
            }
        }
        for b in Self::bodies(snap) {
            self.known_mut(b.id).last_pos = Some(b.pos);
        }
        self.refreshed = refreshed;
    }

    fn known_mut(&mut self, id: i32) -> &mut Known {
        self.known.entry(id).or_insert_with(|| Known::new(false, 0))
    }

    fn is_tracked(&self, id: i32) -> bool {
        self.known.get(&id).is_some_and(|k| k.tracked.is_some())
    }

    /// Is player `id` among the receivers (the game's player set: added when its receive range
    /// rose above zero, removed when it fell or the player left)?
    fn receives(&self, id: i32) -> bool {
        self.known.get(&id).is_some_and(|k| k.receive_range > 0.0)
    }

    /// A player joins the receivers: connected to every tracked transmitter, then tracked itself
    /// if it transmits.
    fn add_player(&mut self, i: usize, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        let me = &snap.players[i];
        for b in Self::bodies(snap) {
            if self.is_tracked(b.id) {
                self.connect(me, b, snap.locator_bar, out);
            }
        }
        if me.body.transmit_range > 0.0 {
            self.track(me.body.id, snap, out);
        }
    }

    /// A player leaves the receivers: told the end of each of its connections, and no longer
    /// tracked as a transmitter either (even while its transmit range stays, like the game).
    fn remove_player(&mut self, id: i32, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        let seat = self.player(snap, id).map(|p| p.seat);
        self.connections.retain(|&(receiver, _), c| {
            if receiver != id {
                return true;
            }
            if let Some(seat) = seat {
                out.push(Outgoing { seat, packet: untrack_packet(c.transmitter) });
            }
            false
        });
        self.untrack(id, snap, out);
    }

    fn track(&mut self, id: i32, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        if !self.is_tracked(id) {
            self.tracking_order += 1;
            let order = self.tracking_order;
            self.known_mut(id).tracked = Some(order);
        }
        self.connect_to_every_receiver(id, snap, out);
    }

    /// Every connection to `id` ended (each receiver told), `id` no longer tracked.
    fn untrack(&mut self, id: i32, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        let index = &self.index;
        self.connections.retain(|&(receiver, transmitter), c| {
            if transmitter != id {
                return true;
            }
            if let Some(Place::Player(i)) = index.get(&receiver) {
                out.push(Outgoing { seat: snap.players[*i].seat, packet: untrack_packet(c.transmitter) });
            }
            false
        });
        if let Some(k) = self.known.get_mut(&id) {
            k.tracked = None;
        }
    }

    /// Game rule turned off: every connection ended.
    fn break_all(&mut self, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        for (&(receiver, _), c) in &self.connections {
            if let Some(Place::Player(i)) = self.index.get(&receiver) {
                out.push(Outgoing { seat: snap.players[*i].seat, packet: untrack_packet(c.transmitter) });
            }
        }
        self.connections.clear();
    }

    /// A connection made from scratch for every receiver (tracking, team change).
    fn connect_to_every_receiver(&mut self, id: i32, snap: &Snapshot, out: &mut Vec<Outgoing>) {
        let Some(t) = self.body(snap, id) else { return };
        for p in &snap.players {
            if self.receives(p.body.id) {
                self.connect(p, t, snap.locator_bar, out);
            }
        }
    }

    /// A transmitter moved: its connections updated, the receivers without one offered one.
    /// Returns whether every pair settled (see [`Self::refresh`]).
    fn update_transmitter(&mut self, t: &Body, snap: &Snapshot, out: &mut Vec<Outgoing>) -> bool {
        let mut all_settled = true;
        for p in &snap.players {
            let offer = self.receives(p.body.id);
            all_settled &= self.refresh(p, t, offer, snap.locator_bar, out);
        }
        all_settled
    }

    /// A receiver moved: its connections updated, the tracked transmitters without one offered
    /// one; bodies marked in `already` (snapshot order) were just refreshed from their side.
    fn update_player(&mut self, r: &Player, snap: &Snapshot, already: &[bool], out: &mut Vec<Outgoing>) {
        for (k, t) in Self::bodies(snap).enumerate() {
            if already.get(k) == Some(&true) {
                continue;
            }
            let offer = self.is_tracked(t.id);
            self.refresh(r, t, offer, snap.locator_bar, out);
        }
    }

    /// New connection of `r` to `t` (replacing any), or the old one ended when `t` offers none.
    fn connect(&mut self, r: &Player, t: &Body, locator_bar: bool, out: &mut Vec<Outgoing>) {
        if r.body.id == t.id || !locator_bar {
            return;
        }
        let key = (r.body.id, t.id);
        match make_connection(t, r) {
            Some(c) => {
                out.push(Outgoing { seat: r.seat, packet: packet(WaypointOp::Track, &c) });
                self.connections.insert(key, c);
            }
            None => {
                if let Some(old) = self.connections.remove(&key) {
                    out.push(Outgoing { seat: r.seat, packet: untrack_packet(old.transmitter) });
                }
            }
        }
    }

    /// Pair (`r`, `t`) after a move: its connection brought up to date, or made again when `t`'s
    /// place no longer fits its kind; without one, one offered when `offer` allows.
    ///
    /// Returns whether the pair settled: a second refresh with the same snapshot would send
    /// nothing. It does not when the connection left is already broken, as the game would find
    /// on its next update: a direction told while the transmitter's chunk is in view, or a
    /// chunk just updated into the view.
    fn refresh(&mut self, r: &Player, t: &Body, offer: bool, locator_bar: bool, out: &mut Vec<Outgoing>) -> bool {
        use hashbrown::hash_map::Entry;
        if r.body.id == t.id || !locator_bar {
            return true;
        }
        let mut pair = match self.connections.entry((r.body.id, t.id)) {
            Entry::Occupied(pair) => pair,
            Entry::Vacant(free) => {
                let Some(c) = offer.then(|| make_connection(t, r)).flatten() else { return true };
                out.push(Outgoing { seat: r.seat, packet: packet(WaypointOp::Track, &c) });
                return !is_broken(free.insert(c), t, r);
            }
        };
        let c = pair.get_mut();
        if !is_broken(c, t, r) {
            let Some(now) = changed_place(c, t, r) else { return true };
            c.told = now;
            out.push(Outgoing { seat: r.seat, packet: packet(WaypointOp::Update, c) });
            return !is_broken(c, t, r);
        }
        match make_connection(t, r) {
            Some(new) => {
                out.push(Outgoing { seat: r.seat, packet: packet(WaypointOp::Track, &new) });
                *c = new;
                !is_broken(c, t, r)
            }
            None => {
                out.push(Outgoing { seat: r.seat, packet: untrack_packet(c.transmitter) });
                pair.remove();
                true
            }
        }
    }
}

/// The game's entity distance: differences rounded to single precision, then their norm.
fn distance(a: [f64; 3], b: [f64; 3]) -> f32 {
    let d = [(a[0] - b[0]) as f32, (a[1] - b[1]) as f32, (a[2] - b[2]) as f32];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Direction sent for a far transmitter: the receiver-minus-transmitter vector turned a quarter
/// clockwise ((x, z) becomes (−z, x)), through the game's approximate `atan2`.
fn azimuth(transmitter: [f64; 3], receiver: [f64; 3]) -> f32 {
    let (dx, dz) = (receiver[0] - transmitter[0], receiver[2] - transmitter[2]);
    crate::mobs::mth::atan2(dx, -dz) as f32
}

fn chunk_of(pos: [f64; 3]) -> [i32; 2] {
    let b = crate::mobs::mth::block_of(pos);
    [b[0] >> 4, b[2] >> 4]
}

fn manhattan(a: [i32; 3], b: [i32; 3]) -> i64 {
    (0..3).map(|k| (i64::from(a[k]) - i64::from(b[k])).abs()).sum()
}

fn chessboard(a: [i32; 2], b: [i32; 2]) -> i64 {
    (i64::from(a[0]) - i64::from(b[0])).abs().max((i64::from(a[1]) - i64::from(b[1])).abs())
}

/// Does `t` stay off `r`'s bar? A spectator receiver sees every transmitter; a spectator
/// transmitter, or one `r` rides, shows to nobody else; otherwise out of the smaller range.
fn ignores(t: &Body, r: &Player) -> bool {
    if r.body.spectator {
        return false;
    }
    if t.spectator || r.vehicles.contains(&t.id) {
        return true;
    }
    f64::from(distance(t.pos, r.body.pos)) >= t.transmit_range.min(r.receive_range)
}

/// The connection `t` offers `r`, by distance and view; none before `t`'s first tick or when `t`
/// stays off `r`'s bar.
fn make_connection(t: &Body, r: &Player) -> Option<Connection> {
    if !t.ticked || t.id == r.body.id || ignores(t, r) {
        return None;
    }
    let told = if distance(t.pos, r.body.pos) > REALLY_FAR {
        Told::Azimuth(azimuth(t.pos, r.body.pos))
    } else if !r.sees_chunk(chunk_of(t.pos)) {
        Told::Chunk(chunk_of(t.pos))
    } else {
        Told::Block(crate::mobs::mth::block_of(t.pos))
    };
    Some(Connection { transmitter: t.uuid, icon: t.icon, told })
}

/// Must the connection be made again? A direction once the transmitter comes near or into
/// view; a block after a jump of more than one block (never for the distance: a slow walk away
/// keeps the exact block, like the game); a chunk after a jump of more than one chunk or once
/// the chunk told comes into view.
fn is_broken(c: &Connection, t: &Body, r: &Player) -> bool {
    match c.told {
        Told::Azimuth(_) => {
            ignores(t, r) || r.sees_chunk(chunk_of(t.pos)) || distance(t.pos, r.body.pos) <= REALLY_FAR
        }
        Told::Block(last) => manhattan(last, crate::mobs::mth::block_of(t.pos)) > 1 || ignores(t, r),
        Told::Chunk(last) => chessboard(last, chunk_of(t.pos)) > 1 || ignores(t, r) || r.sees_chunk(last),
    }
}

/// New place to tell (an update), if it changed enough.
fn changed_place(c: &Connection, t: &Body, r: &Player) -> Option<Told> {
    match c.told {
        Told::Azimuth(last) => {
            let now = azimuth(t.pos, r.body.pos);
            ((now - last).abs() > AZIMUTH_STEP).then_some(Told::Azimuth(now))
        }
        Told::Block(last) => {
            let now = crate::mobs::mth::block_of(t.pos);
            (now != last).then_some(Told::Block(now))
        }
        Told::Chunk(last) => {
            let now = chunk_of(t.pos);
            (now != last).then_some(Told::Chunk(now))
        }
    }
}

fn packet(op: WaypointOp, c: &Connection) -> Waypoint {
    let pos = match c.told {
        Told::Block(b) => WaypointPos::Block(b),
        Told::Chunk(ch) => WaypointPos::Chunk(ch),
        Told::Azimuth(a) => WaypointPos::Azimuth(a),
    };
    // The host is handed the packet without encoding: the 24 bits its decoder would read.
    let color = c.icon.color.map(rgb_of);
    Waypoint { op, id: WaypointId::Uuid(c.transmitter), style: c.icon.style.to_owned(), color, pos }
}

/// End of a connection: the game's empty waypoint (default icon, no place).
fn untrack_packet(transmitter: u128) -> Waypoint {
    Waypoint {
        op: WaypointOp::Untrack,
        id: WaypointId::Uuid(transmitter),
        style: DEFAULT_STYLE.to_owned(),
        color: None,
        pos: WaypointPos::Empty,
    }
}

impl Server {
    /// The waypoint connections brought up to date with the world and their packets sent: at the
    /// end of the tick, and before `/waypoint` answers. Skipped while the server works in another
    /// world than the host's (`execute in`, a LAN guest's own world): its entities are not those
    /// of the host's world, the one the connections follow.
    pub(crate) fn waypoints_sync(&mut self) {
        if self.muted || self.dims.home.is_some() || !self.works_in_host_world() {
            return;
        }
        // The snapshot's memory is kept from tick to tick: no allocation in the usual pass.
        let mut snapshot = std::mem::take(&mut self.waypoints.spare_snapshot);
        self.fill_waypoint_snapshot(&mut snapshot);
        let mut out = Vec::new();
        self.waypoints.sync(&snapshot, &mut out);
        self.waypoints.spare_snapshot = snapshot;
        for o in out {
            self.send_to(o.seat, rc_client::Event::Waypoint(Box::new(o.packet)));
        }
    }

    /// The players of the world (the host, then the guests in the game, each read with its state
    /// installed) and the mobs the rules look at.
    pub(crate) fn fill_waypoint_snapshot(&mut self, snap: &mut Snapshot) {
        snap.locator_bar = self.rule("minecraft:locator_bar", true);
        snap.players.clear();
        snap.players.push(self.as_player(None, |sv| sv.current_player_waypoint(None)));
        let guests = self.lan.as_ref().map_or(0, |l| l.guests.len());
        let mut taken = std::mem::take(&mut self.waypoints.guest_views);
        // Only a view of this very tick, outside any swap (a command's pass reads everyone again).
        let swapped = self.lan.as_ref().is_some_and(|l| l.active.is_some());
        let tick = self.tick;
        for k in 0..guests {
            if !self.guest_in_game(k) {
                continue;
            }
            let fresh = taken.iter().position(|(at, seat, _)| *at == tick && *seat == k && !swapped);
            let view = match fresh {
                Some(i) => taken.swap_remove(i).2,
                None => self.as_player(Some(k), |sv| sv.current_player_waypoint(Some(k))),
            };
            snap.players.push(view);
        }
        taken.clear();
        self.waypoints.guest_views = taken;
        snap.others.clear();
        self.waypoint_mobs(&mut snap.others);
        self.waypoint_decorations(&mut snap.others);
    }

    /// Living decorations the rules look at (armor stands, mannequins): a living entity of the
    /// game, each transmits once its range is above zero, and `/waypoint modify` tracks one
    /// without any range.
    fn waypoint_decorations(&self, bodies: &mut Vec<Body>) {
        let rangeless: Vec<i32> = self.waypoints.tracked_without_range().collect();
        self.deco_waypoint_transmitters(
            |id| rangeless.contains(&id),
            |d| {
                let uuid = crate::nbtio::uuid(d.compound.get("UUID")).unwrap_or(0);
                let team = team_color(self.cmd.scores.team_of_uuid(uuid));
                bodies.push(Body {
                    id: d.id,
                    uuid,
                    pos: crate::nbtio::f64s::<3>(d.compound.get("Pos")).unwrap_or_default(),
                    ticked: d.ticked,
                    spectator: false,
                    transmit_range: d.transmit_range,
                    icon: Icon::of_entity(Some(d.compound)).with_team_color(team),
                });
            },
        );
    }

    /// End of a guest's own tick, its state installed: its view kept for this tick's pass, which
    /// then installs it no more.
    pub(crate) fn note_guest_waypoint_view(&mut self) {
        let Some(seat) = self.lan.as_ref().and_then(|l| l.active) else { return };
        if !self.waypoints.guest_view_from_tick {
            return;
        }
        let view = self.current_player_waypoint(Some(seat));
        let tick = self.tick;
        self.waypoints.guest_views.retain(|(_, s, _)| *s != seat);
        self.waypoints.guest_views.push((tick, seat, view));
    }

    /// Guest `k` entered the game, has not left (the installed one counts) and stands in this
    /// world: each world keeps its own waypoints, like the game's.
    fn guest_in_game(&self, k: usize) -> bool {
        let Some(l) = &self.lan else { return false };
        let in_game = Some(k) == l.active || l.guests.get(k).is_some_and(|g| g.joined && !g.gone);
        in_game && self.seat_client_world(l, k).is_some_and(|world| self.stands_here(world))
    }

    /// The installed player as the waypoint rules see it.
    fn current_player_waypoint(&self, seat: Option<usize>) -> Player {
        let block = crate::mobs::mth::block_of(self.pos);
        let own_camera = self.prog.cmds.camera.is_none_or(|e| e == E::Player);
        Player {
            body: Body {
                id: self.player_id,
                uuid: self.uuid,
                pos: self.pos,
                ticked: self.player_ready(),
                spectator: self.game_mode == 3,
                transmit_range: self.player_attribute(TRANSMIT_RANGE),
                icon: self.player_waypoint_icon(),
            },
            seat,
            receive_range: self.player_attribute(RECEIVE_RANGE),
            view_center: (block[0] >> 4, block[2] >> 4),
            view: self.view,
            vehicles: self.player_vehicles(),
            // Looking through another entity: the game moves the player to it every tick.
            sent_movement: self.waypoints.sent_movement_packet(self.player_id) || !own_camera,
            generation: self.walk.player,
        }
    }

    /// The installed player's icon as its new connections copy it (team colour included).
    fn player_waypoint_icon(&self) -> Icon {
        Icon::of_entity(Some(&self.pdata.root)).with_team_color(team_color(self.cmd.scores.team_of(&self.name)))
    }

    /// Entities the installed player rides, nearest first.
    fn player_vehicles(&self) -> Vec<i32> {
        let mut chain = Vec::new();
        let mut next = self.mobs.mounts.riding.vehicle;
        while let Some(id) = next {
            // A loop of riders (broken data) is walked once.
            if chain.contains(&id) {
                break;
            }
            chain.push(id);
            next = self.mobs.get(id).and_then(|m| m.mount.vehicle);
        }
        chain
    }

    /// Mobs the rules look at: those that transmit, then those `/waypoint modify` tracked
    /// without any range.
    pub(crate) fn waypoint_mobs(&self, bodies: &mut Vec<Body>) {
        // No mob of the process holds a range (the usual world): the list has nothing to give.
        let idle = self.waypoints.skip_idle_mob_scan && crate::mobs::mob::mobs_with_waypoint_range() == 0;
        if !idle {
            for m in self.mobs.list.iter() {
                let range = m.attrs.waypoint_transmit_range();
                if range > 0.0 && !m.removed {
                    bodies.push(self.mob_waypoint_body(m, range));
                }
            }
        }
        for id in self.waypoints.tracked_without_range() {
            if let Some(m) = self.mobs.get(id).filter(|m| !m.removed) {
                let range = m.attrs.waypoint_transmit_range();
                if range <= 0.0 {
                    bodies.push(self.mob_waypoint_body(m, range));
                }
            }
        }
    }

    fn mob_waypoint_body(&self, m: &crate::mobs::mob::Mob, transmit_range: f64) -> Body {
        let team = team_color(self.cmd.scores.team_of_uuid(m.uuid));
        Body {
            id: m.id,
            uuid: m.uuid,
            pos: m.pos(),
            ticked: m.tick_count > 0,
            spectator: false,
            transmit_range,
            icon: Icon::of_entity(m.nbt.as_ref()).with_team_color(team),
        }
    }

    /// Player named `name` (host or guest in the game), by entity id.
    pub(crate) fn player_id_named(&self, name: &str) -> Option<i32> {
        if self.name == name {
            return Some(self.player_id);
        }
        self.lan_others().find(|c| c.name == name).map(|c| c.player_id)
    }

    /// The teams of these score holders changed: the waypoints of those that are players are
    /// made again with their new colour (the game does not do it for other entities).
    pub(crate) fn waypoints_team_changed<'a>(&mut self, holders: impl IntoIterator<Item = &'a String>) {
        for h in holders {
            if let Some(id) = self.player_id_named(h) {
                self.waypoints.remake(id);
            }
        }
    }
}
