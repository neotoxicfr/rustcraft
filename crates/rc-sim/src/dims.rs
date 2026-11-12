//! Several dimensions loaded at the same time (S6): the player's dimension lives in the
//! server fields (`dim`, `chunks`, `bs`…), the others are "parked" here with their
//! chunks, tick queues, mobs, items, furnaces and generator, and exchanged by
//! `mem::swap` as needed (save, entities going through a portal, player dimension
//! change). Player dimension change like the official 26.3 server: `respawn`
//! (data kept), difficulty, operator level, position, abilities, world info
//! (border, time, spawn point, rain, chunk wait), inventory, sound.

use hashbrown::HashMap;
use std::collections::BTreeSet;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

use crate::blocks::P;
use crate::chunk::Chunk;
use crate::nbtio;
use crate::server::Server;
use crate::world::Dimension;

pub(crate) const OVERWORLD: &str = "minecraft:overworld";
pub(crate) const NETHER: &str = "minecraft:the_nether";
pub(crate) const END: &str = "minecraft:the_end";

/// Relative flags of the position packet.
pub(crate) mod rel {
    pub const Y_ROT: i32 = 1 << 3;
    pub const X_ROT: i32 = 1 << 4;
    pub const ROTATION: i32 = Y_ROT | X_ROT;
    /// Velocity kept (three axes) and rotated with the yaw.
    pub const DELTA: i32 = (1 << 5) | (1 << 6) | (1 << 7) | (1 << 8);
}

/// Entity an entity packet is about; `None` for a removal list (entities of a world).
fn entity_event_subject(e: &EntityEvent) -> Option<i32> {
    match e {
        EntityEvent::Add(a) => Some(a.id),
        EntityEvent::Remove(_) => None,
        EntityEvent::Move(m) => Some(m.id),
        EntityEvent::Teleport(t) => Some(t.id),
        EntityEvent::Sync(s) => Some(s.id),
        EntityEvent::Passengers { vehicle: id, .. }
        | EntityEvent::Head { id, .. }
        | EntityEvent::Motion { id, .. }
        | EntityEvent::Data { id, .. }
        | EntityEvent::Equipment { id, .. }
        | EntityEvent::Event { id, .. }
        | EntityEvent::Swing { id, .. }
        | EntityEvent::Hurt { id, .. }
        | EntityEvent::Damage { id, .. }
        | EntityEvent::Effect { id, .. }
        | EntityEvent::RemoveEffect { id, .. }
        | EntityEvent::Animate { id, .. } => Some(*id),
    }
}

/// World of a dimension, parked while the player is elsewhere.
pub(crate) struct World {
    pub dim: Dimension,
    pub chunks: HashMap<(i32, i32), Option<Box<Chunk>>>,
    pub entities: HashMap<(i32, i32), Vec<(i32, Nbt)>>,
    /// Hints into `entities`, parked with them (one index for two dimensions' lists kept the
    /// entries of both and rebuilt for each).
    pub kept_index: crate::commands::kept_entity_index::KeptEntityIndex,
    pub bs: crate::blocks::level::Sim,
    pub generator: Option<crate::generation::Gen>,
    /// Mobs (only the list, the arrows, the chunks read and the world's random draw are used).
    pub mobs: Box<crate::mobs::Mobs>,
    /// Simulated decorations of the dimension.
    pub deco: Box<crate::decoration::Decos>,
    pub drops: Vec<crate::drops::ItemEnt>,
    pub drop_chunks: BTreeSet<(i32, i32)>,
    pub orbs: Vec<crate::drops::Orb>,
    pub furnaces: HashMap<[i32; 3], crate::containers::Furnace>,
    /// G3-F13: brewing stands of the dimension (their tickers are in `bs`).
    pub brewers: HashMap<[i32; 3], crate::brewing::Brewer>,
    pub changed: Vec<P>,
    pub pending_light_updates: Vec<Event>,
    pub relight: Vec<P>,
    /// `relight` positions that touch the sky (C2-M04).
    pub relight_sky: Vec<P>,
    /// Remaining ticks of the portal ticket (300 at each arrival): items and falling blocks
    /// are simulated there during that time.
    pub ticket: i32,
    /// S8: projectiles and clouds of the dimension.
    pub proj: Box<crate::projectiles::Projs>,
    /// G5-F06: minecarts of the dimension.
    pub carts: Box<crate::minecart::Carts>,
    /// L9-TICK: evoker fangs of the dimension (before, they followed the player and bit at the
    /// same coordinates in the arrival dimension).
    pub fangs: Vec<crate::mobs::illagers::spells::Fangs>,
    /// L9-TICK: signs of the dimension being edited (player allowed to send their text): the
    /// game keeps the editor on the sign itself, so a sign at the same coordinates elsewhere
    /// refuses the text.
    pub sign_editors: HashMap<[i32; 3], u128>,
    /// L10-ORDER: entities of the dimension outside the entity list (`walk::Walk::left`).
    pub left: Vec<(crate::walk::Fam, u64)>,
    /// L10-ORDER: families swept out of the list while parked (`walk::Walk::swept`).
    pub swept: u16,
}

/// World generation settings (`data/minecraft/world_gen_settings.dat`): seed, and for
/// each dimension its type and generator. Read once when the world is opened (and re-read on
/// `/reload`) instead of at each question: the file is only written at world creation, so the
/// answer is the same as if it were re-read.
#[derive(Default)]
pub(crate) struct GenSettings {
    root: Option<Nbt>,
    /// Hashed seed of the biome zoom (the one the client receives), computed once.
    pub zoom_seed: i64,
}

impl GenSettings {
    pub fn read(world: &std::path::Path) -> GenSettings {
        let mut g = GenSettings {
            root: nbtio::read_dat(&world.join("data").join("minecraft").join("world_gen_settings.dat")).ok(),
            zoom_seed: 0,
        };
        g.zoom_seed = crate::server::sha256_seed(g.seed());
        g
    }

    /// `data` compound.
    pub fn data(&self) -> Option<&Nbt> {
        self.root.as_ref()?.get("data")
    }

    /// World seed (0 if unreadable).
    pub fn seed(&self) -> i64 {
        self.data().and_then(|d| d.get("seed")?.as_i64()).unwrap_or(0)
    }

    /// Dimensions of the world: the game's three, then those of the settings (data packs).
    pub fn dimension_names(&self) -> Vec<String> {
        let mut out: Vec<String> = crate::server::DIMENSIONS.iter().map(|d| (*d).to_owned()).collect();
        if let Some(Nbt::Compound(f)) = self.data().and_then(|d| d.get("dimensions")) {
            let mut extra: Vec<String> = f.iter().map(|(k, _)| k.clone()).filter(|k| !out.contains(k)).collect();
            extra.sort();
            out.extend(extra);
        }
        out
    }

    /// Compound of a dimension (`dimensions.<name>`).
    fn dimension(&self, name: &str) -> Option<&Nbt> {
        self.data()?.get("dimensions")?.get(name)
    }

    /// Dimension type (its name by default).
    pub fn dimension_type<'a>(&'a self, name: &'a str) -> &'a str {
        self.dimension(name).and_then(|d| d.get("type")?.as_str()).unwrap_or(name)
    }

    /// Flat generator (`minecraft:flat`).
    pub fn is_flat(&self, name: &str) -> bool {
        self.dimension(name).and_then(|d| d.get("generator")?.get("type")?.as_str()) == Some("minecraft:flat")
    }
}

/// Portal traversed (the game compares the block object: one process per kind of portal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortalKind {
    Nether,
    End,
    Gateway,
}

/// Portal process of an entity.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Proc {
    pub kind: PortalKind,
    pub entry: P,
    pub time: i32,
    pub inside: bool,
}

impl Proc {
    /// True when the delay is reached.
    pub fn process(&mut self, allowed: bool, delay: i32) -> bool {
        if !self.inside {
            self.time = (self.time - 4).max(0);
            return false;
        }
        self.inside = false;
        if !allowed {
            return false;
        }
        let t = self.time;
        self.time += 1;
        t >= delay
    }

    pub fn expired(&self) -> bool {
        self.time <= 0
    }
}

/// Portal state of an entity (return delay and process).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EntPortal {
    pub cooldown: i32,
    pub proc: Option<Proc>,
}

impl EntPortal {
    /// The "in a portal" mark.
    pub fn set_inside(&mut self, kind: PortalKind, p: P, delay_after: i32) {
        if self.cooldown > 0 {
            self.cooldown = delay_after;
            return;
        }
        match &mut self.proc {
            Some(pr) if pr.kind == kind => {
                if !pr.inside {
                    pr.entry = p;
                    pr.inside = true;
                }
            }
            _ => self.proc = Some(Proc { kind, entry: p, time: 0, inside: true }),
        }
    }
}

/// Destination of a passage.
#[derive(Debug, Clone)]
pub(crate) struct Transition {
    pub dim: String,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// `rel::*` flags: rotation added, velocity kept.
    pub relative: i32,
    /// Travel sound (event 1032) to the player.
    pub sound: bool,
    /// Portal ticket placed on arrival.
    pub ticket: bool,
    /// Respawn: respawn block missing (event 0).
    pub missing_respawn: bool,
}

/// Dimensions and "portals" state of the player.
#[derive(Default)]
pub(crate) struct Dims {
    pub parked: Vec<World>,
    /// Player: portal process, return delay (`PortalCooldown`).
    pub player: EntPortal,
    pub seen_credits: bool,
    pub won_game: bool,
    pub entered_nether: Option<[f64; 3]>,
    /// Other entities (items, falling blocks, mobs) by identifier.
    pub ents: HashMap<i32, EntPortal>,
    /// End gateway delays: (dimension, position) → remaining ticks.
    pub gateways: HashMap<(String, P), i32>,
    /// Player fields read.
    pub read: bool,
    /// During the tick of a parked dimension: real dimension and position of the player.
    pub home: Option<String>,
    pub home_pos: [f64; 3],
    /// While the server works in a parked world (`in_world`): the world the players stand in.
    pub players_world: Option<String>,
    /// While a command run in another world hurts the player ([`Server::hurt_player_from_command`]):
    /// that world, which the game hands to the hit.
    pub hit_from_world: Option<String>,
    /// Player pearls that fell into a parked dimension: (dimension, arrival point, pearl on
    /// return delay), teleportations done on return to its dimension.
    pub pearl_jumps: Vec<(String, [f64; 3], bool)>,
}

impl Server {
    /// Swaps the active world with `w`.
    pub(crate) fn exchange(&mut self, w: &mut World) {
        use std::mem::swap;
        self.end_on_park(); // F15: dragon bar removed when leaving the dimension
        self.raids_on_park(); // L10-ENT: same for the raid bars
        self.swap_active_border(&w.dim.name); // each dimension's own border
        swap(&mut self.dim, &mut w.dim);
        swap(&mut self.chunks, &mut w.chunks);
        self.chunk_map_swaps += 1;
        swap(&mut self.entities, &mut w.entities);
        swap(&mut self.kept_index, &mut w.kept_index);
        swap(&mut self.bs, &mut w.bs);
        swap(&mut self.generator, &mut w.generator);
        swap(&mut self.mobs.list, &mut w.mobs.list);
        swap(&mut self.mobs.index, &mut w.mobs.index);
        swap(&mut self.mobs.adopted, &mut w.mobs.adopted);
        swap(&mut self.mobs.rng, &mut w.mobs.rng);
        // Mobs (F04-F06): projectiles and lightning bolts specific to the world, points of interest.
        swap(&mut self.mobs.mounts.spits, &mut w.mobs.mounts.spits);
        swap(&mut self.mobs.mounts.bolts, &mut w.mobs.mounts.bolts);
        swap(&mut self.mobs.poi, &mut w.mobs.poi);
        swap(&mut self.mobs.end, &mut w.mobs.end); // F15: crystals, dragon fight
        swap(&mut self.mobs.ill.raids, &mut w.mobs.ill.raids); // L10-ENT: raids of the dimension
        // G3-F13: state of the block entities whose tickers live in `bs` (rung bells, vaults,
        // ominous item spawners), kept with their dimension.
        swap(&mut self.mobs.bells, &mut w.mobs.bells);
        swap(&mut self.mobs.spawners.blocks, &mut w.mobs.spawners.blocks);
        swap(&mut self.life.drops, &mut w.drops);
        swap(&mut self.life.drop_chunks, &mut w.drop_chunks);
        swap(&mut self.life.orbs, &mut w.orbs);
        swap(&mut self.life.furnaces, &mut w.furnaces);
        swap(&mut self.life.brewers, &mut w.brewers); // G3-F13
        swap(&mut self.changed, &mut w.changed);
        swap(&mut self.pending_light_updates, &mut w.pending_light_updates);
        swap(&mut self.relight, &mut w.relight);
        swap(&mut self.relight_sky, &mut w.relight_sky);
        swap(&mut self.proj, &mut w.proj); // S8: projectiles
        swap(&mut self.carts, &mut w.carts); // G5-F06: minecarts
        swap(&mut self.deco, &mut w.deco); // decorations
        swap(&mut self.mobs.ill.fangs, &mut w.fangs); // L9-TICK
        swap(&mut self.life.sign_editors, &mut w.sign_editors); // L9-TICK
        swap(&mut self.walk.left, &mut w.left); // L10-ORDER
        swap(&mut self.walk.swept, &mut w.swept);
    }

    pub(crate) fn world_seed(&self) -> i64 {
        self.wgs.seed()
    }

    /// Dimensions of the world: the game's three, then those of the settings (data packs).
    pub(crate) fn world_dimensions(&self) -> Vec<String> {
        self.wgs.dimension_names()
    }

    /// Dimension type of a dimension (world settings, otherwise its name).
    pub(crate) fn dimension_type_of(&self, name: &str) -> String {
        self.wgs.dimension_type(name).to_owned()
    }

    /// Dimension type file (jar pack).
    fn dim_type_json(&self, name: &str) -> Option<std::sync::Arc<serde_json::Value>> {
        self.life.pack.get("dimension_type", self.wgs.dimension_type(name))
    }

    /// Field of a dimension type (jar pack).
    pub(crate) fn dim_type_field(&self, name: &str, key: &str) -> Option<serde_json::Value> {
        self.dim_type_json(name).and_then(|v| v.get(key).cloned())
    }

    /// Game attribute of a dimension (`attributes`), e.g. `minecraft:gameplay/respawn_anchor_works`.
    pub(crate) fn dim_attribute(&self, name: &str, key: &str) -> Option<serde_json::Value> {
        self.dim_type_json(name).and_then(|v| v.get("attributes")?.get(key).cloned())
    }

    /// `coordinate_scale` of the dimension type.
    pub(crate) fn coordinate_scale(&self, name: &str) -> f64 {
        self.dim_type_field(name, "coordinate_scale").and_then(|v| v.as_f64()).unwrap_or(match name {
            NETHER => 8.0,
            _ => 1.0,
        })
    }

    /// Highest Y to place a portal: `min(max_y, min_y + logical_height - 1)`.
    pub(crate) fn max_placeable_y(&self) -> i32 {
        let h = self.dim.height;
        let max_y = h.min_y + h.sections as i32 * 16 - 1;
        let logical = self
            .dim_type_field(&self.dim.name, "logical_height")
            .and_then(|v| v.as_i64())
            .map_or(h.sections as i32 * 16, |v| v as i32);
        max_y.min(h.min_y + logical - 1)
    }

    fn new_world(&self, name: &str) -> World {
        let dim = Dimension::new(&self.world, name, crate::dimension_height(&self.data, name));
        World {
            generator: crate::generation::start_for(&self.data, &self.world, name, &self.regs),
            chunks: HashMap::new(),
            entities: HashMap::new(),
            kept_index: Default::default(),
            bs: crate::blocks::level::Sim::new(&self.data),
            mobs: Box::default(),
            deco: Box::default(),
            drops: Vec::new(),
            drop_chunks: BTreeSet::new(),
            orbs: Vec::new(),
            furnaces: HashMap::new(),
            brewers: HashMap::new(),
            changed: Vec::new(),
            pending_light_updates: Vec::new(),
            relight: Vec::new(),
            relight_sky: Vec::new(),
            ticket: 0,
            dim,
            proj: Box::default(),
            carts: Box::default(),
            fangs: Vec::new(),
            sign_editors: HashMap::new(),
            left: Vec::new(),
            swept: 0,
        }
    }

    /// Makes dimension `name` active (parked, otherwise opened); the current one is parked.
    pub(crate) fn enter_world(&mut self, name: &str) {
        if self.dim.name == name {
            return;
        }
        let mut w = match self.dims.parked.iter().position(|w| w.dim.name == name) {
            Some(i) => self.dims.parked.swap_remove(i),
            None => self.new_world(name),
        };
        self.exchange(&mut w);
        self.dims.parked.push(w);
    }

    /// Runs `f` in dimension `name`, then returns to the active dimension. No player stands in
    /// that world: what happens there reaches nobody, the rest still leaves
    /// ([`Server::belongs_to_world`]). A nested visit that comes back to the players' own world
    /// (`execute in the_nether run execute in overworld run …`) sends open, like a command run
    /// there.
    pub(crate) fn in_world<R>(&mut self, name: &str, f: impl FnOnce(&mut Server) -> R) -> R {
        let home = self.dim.name.clone();
        if home == name {
            return f(self);
        }
        let muted = self.muted;
        // Leaving the players' world (not a visit nested in another one): remembered for
        // the nested visits that come back to it.
        let outer_players_world = (!muted).then(|| self.dims.players_world.replace(home.clone()));
        let back_in_players_world = muted && self.dims.players_world.as_deref() == Some(name);
        // Muted during both exchanges: they must not take the players out of their world's boss
        // bars (`end_on_park`, `raids_on_park`).
        self.muted = true;
        self.enter_world(name);
        self.muted = !back_in_players_world;
        let r = f(self);
        self.muted = true;
        self.enter_world(&home);
        self.muted = muted;
        if let Some(previous) = outer_players_world {
            self.dims.players_world = previous;
        }
        r
    }

    /// Runs `f` in the world the players stand in, sends open. A command run in another
    /// dimension (`execute in`) still acts on a player in the player's own world, like the
    /// game: the item `give` drops at the player's feet and its sound.
    pub(crate) fn in_players_world<R>(&mut self, f: impl FnOnce(&mut Server) -> R) -> R {
        match self.players_world_while_away() {
            Some(players_world) => self.in_world(&players_world, f),
            None => f(self),
        }
    }

    /// Damage dealt to the player by a command run in another world (`execute in`, a command
    /// block or function ticking in a parked world). The game hands the command's world to the
    /// hit, and that world only decides where the hurt packet and the shield's sound go (nowhere:
    /// it tracks no player) and where an infestation's silverfish appear; the rest of the hit
    /// (sounds, death drops, place of death) happens in the player's own world.
    pub(crate) fn hurt_player_from_command<R>(&mut self, f: impl FnOnce(&mut Server) -> R) -> R {
        let Some(players_world) = self.players_world_while_away() else { return f(self) };
        let command_world = Some(self.dim.name.clone());
        self.in_world(&players_world, |sv| {
            let outer = std::mem::replace(&mut sv.dims.hit_from_world, command_world);
            let r = f(sv);
            sv.dims.hit_from_world = outer;
            r
        })
    }

    /// The world the players stand in while the server works in another one; `None` when it
    /// already works there.
    fn players_world_while_away(&self) -> Option<String> {
        if self.muted { self.dims.players_world.clone() } else { None }
    }

    /// Whether `e` describes the active world itself: its blocks, light and chunks, the
    /// entities living there (not the players), the sounds, particles, explosions and boss
    /// bars heard there, its border. The game sends such an event only to the players
    /// standing in that world. Everything else is addressed to players or to the whole server
    /// (chat, titles, difficulty, inventory, clocks, scores, the players' own entities…) and
    /// leaves whatever dimension a command runs in.
    pub(crate) fn belongs_to_world(&self, e: &Event) -> bool {
        match e {
            Event::LightUpdate { .. }
            | Event::BlocksAheadOfLight { .. }
            | Event::Chunk(_)
            | Event::ForgetChunk { .. }
            | Event::ChunkBiomes(_)
            | Event::BlockUpdate { .. }
            | Event::SectionUpdate { .. }
            | Event::BlockDestruction { .. }
            | Event::BlockEvent { .. }
            | Event::BlockEntityData { .. }
            | Event::Sound(_)
            | Event::Particles(_)
            | Event::LevelEvent { .. }
            | Event::Explode(_)
            | Event::Border(_)
            | Event::Boss(_)
            | Event::Waypoint(_) => true,
            Event::Entity(ee) => !entity_event_subject(ee).is_some_and(|id| self.is_player_entity(id)),
            Event::Attributes { entity, .. } => !self.is_player_entity(*entity),
            Event::Misc(m) => !matches!(
                m,
                rc_client::misc::Misc::PostEffects(_)
                    | rc_client::misc::Misc::LowDiskSpace
                    | rc_client::misc::Misc::ServerData { .. }
            ),
            Event::Bundle(list) => list.iter().any(|e| self.belongs_to_world(e)),
            _ => false,
        }
    }

    /// Entity id of a player in game (the host or a LAN guest).
    fn is_player_entity(&self, id: i32) -> bool {
        id == self.player_id || self.lan_others().any(|c| c.player_id == id)
    }

    /// Reading of the player file's "portals" fields (once).
    pub(crate) fn dims_read_player(&mut self) {
        if self.dims.read {
            return;
        }
        self.dims.read = true;
        let root = &self.pdata.root;
        self.dims.player.cooldown = nbtio::int(root, "PortalCooldown").unwrap_or(0) as i32;
        self.dims.seen_credits = nbtio::int(root, "seenCredits").is_some_and(|v| v != 0);
        self.dims.entered_nether = nbtio::f64s::<3>(root.get("entered_nether_pos"));
    }

    /// Portal return delay of the simulated entity `id`, as written in its NBT.
    pub(crate) fn portal_cooldown(&self, id: i32) -> i32 {
        self.dims.ents.get(&id).map_or(0, |p| p.cooldown)
    }

    /// `PortalCooldown` of an entity read from its NBT (chunk or summon): a running delay is
    /// restored under its new identifier, the game reads it for every entity.
    pub(crate) fn load_portal_cooldown(&mut self, id: i32, e: &Nbt) {
        if let Some(cd) = nbtio::int(e, "PortalCooldown").filter(|&v| v > 0) {
            self.dims.ents.insert(id, EntPortal { cooldown: cd as i32, proc: None });
        }
    }

    /// "Portals" fields written in the player file.
    pub(crate) fn dims_player_nbt(&self, root: &mut Nbt) {
        nbtio::set(root, "PortalCooldown", Nbt::Int(self.dims.player.cooldown));
        nbtio::set(root, "seenCredits", Nbt::Byte(i8::from(self.dims.seen_credits)));
        match self.dims.entered_nether {
            Some(p) => nbtio::set(root, "entered_nether_pos", Nbt::List(p.iter().map(|&d| Nbt::Double(d)).collect())),
            None => nbtio::remove(root, "entered_nether_pos"),
        }
    }

    /// Save of the parked dimensions: modified chunks, furnaces, items.
    pub(crate) fn save_parked(&mut self, jobs: &mut Vec<crate::save::Job>, wait: bool) -> usize {
        let names: Vec<String> = self.dims.parked.iter().map(|w| w.dim.name.clone()).collect();
        let mut n = 0;
        for name in names {
            n += self.in_world(&name, |s| {
                // Proto-chunks of the parked dimension's generator: written before `jobs`.
                s.flush_gen(wait);
                s.flush_furnaces();
                s.flush_brewers(); // G3-F13: brewing stands kept with their dimension
                s.drops_save_jobs(jobs);
                s.poi_save_jobs(jobs); // F05: points of interest of the parked dimension
                s.end_save_jobs(jobs); // F15: dragon fight
                s.raids_save_jobs(jobs); // L10-ENT: raids of the parked dimension
                // Random sequences: a single set for the server, saved with the survival data.
                s.chunk_jobs(jobs)
            });
        }
        n
    }

    /// Closing: generators of the parked dimensions stopped, outputs resumed.
    pub(crate) fn stop_parked(&mut self) {
        let names: Vec<String> = self.dims.parked.iter().map(|w| w.dim.name.clone()).collect();
        for name in names {
            self.in_world(&name, |s| s.stop_gen());
        }
    }

    /// "Dimensions" tick (after the mobs): portals of the player and entities, parked worlds
    /// (generator, portal ticket).
    pub(crate) fn dims_tick(&mut self) {
        self.dims_read_player();
        let runs = self.cmd.ticking.runs(); // frozen (`tick freeze`): only the player ticks
        if runs {
            self.gateways_tick();
            self.drops_portal_tick();
            self.mobs_portal_tick();
            self.carts_portal_tick(); // G5-F06
        }
        self.player_portal_tick();
        self.parked_tick(runs);
        if runs {
            self.parked_raids_tick(); // L10-ENT: every world ticks its own raids
        }
    }

    /// Parked dimensions: generation always collected; entities and tickets only if the world
    /// ticks (`runs` false under `tick freeze`).
    fn parked_tick(&mut self, runs: bool) {
        // L9-TICK: a pearl ticket (of the host or of a guest, still flying or expiring) keeps
        // the world active like the game's.
        let names: Vec<(String, i32, bool, u16)> = self
            .dims
            .parked
            .iter()
            .map(|w| (w.dim.name.clone(), w.ticket, !w.proj.tickets.is_empty(), w.swept))
            .collect();
        let home = self.dim.name.clone();
        for (name, ticket, pearl, swept) in names {
            let has_gen = self.dims.parked.iter().any(|w| w.dim.name == name && w.generator.is_some());
            // L10-ORDER: the families its own walk does not visit leave the list where their
            // chunk no longer ticks, once until that walk visits them again.
            let active = (ticket > 0 || pearl) && runs;
            let walked = if active { crate::walk::parked_walk(ticket > 0) } else { 0 };
            let sweep = crate::walk::SWEPT & !walked & !swept;
            if ticket <= 0 && !has_gen && !pearl && sweep == 0 {
                continue;
            }
            self.dims.home = Some(home.clone());
            self.dims.home_pos = self.pos;
            self.in_world(&name, |s| {
                s.poll_gen();
                if !active && sweep == 0 {
                    return;
                }
                // Player absent.
                let (pos, ready, joined) = (s.pos, s.loaded, s.joined_tick);
                s.pos = [f64::from(i32::MAX), -1.0e9, f64::from(i32::MAX)];
                s.loaded = false;
                s.joined_tick = s.tick + 1;
                if active {
                    // Tickets of the previous tick applied, then the chunk source's purge.
                    s.proj.pearl_tickets_apply();
                    s.proj.pearl_tickets_purge(s.time);
                }
                s.walk.swept = (s.walk.swept & !walked) | sweep;
                s.walk_park_sweep(sweep);
                if active {
                    // Portal ticket: items, falling blocks and projectiles continue; a player's
                    // pearl keeps its own while it flies (the game's pearl ticket), only its
                    // projectiles tick then.
                    s.tick_parked_entities(ticket > 0);
                    s.flush_block_changes();
                }
                s.pos = pos;
                s.loaded = ready;
                s.joined_tick = joined;
            });
            self.dims.home = None;
            if runs && let Some(w) = self.dims.parked.iter_mut().find(|w| w.dim.name == name) {
                w.ticket = (w.ticket - 1).max(0);
            }
        }
        self.pearl_jumps();
    }

    /// Player pearls that fell into another dimension: if the player can
    /// still go through a portal (alive, awake), they are taken there, their fall forgotten, then
    /// hurt by 5 (pearl); return delay set if the pearl itself was on delay.
    fn pearl_jumps(&mut self) {
        for (dim, pos, cooling) in std::mem::take(&mut self.dims.pearl_jumps) {
            if self.life.vit.dead || self.life.vit.sleeping.is_some() {
                continue;
            }
            if cooling {
                self.dims.player.cooldown = crate::portal::PLAYER_COOLDOWN;
            }
            let t = Transition {
                dim,
                pos,
                yaw: 0.0,
                pitch: 0.0,
                relative: rel::ROTATION,
                sound: false,
                ticket: false,
                missing_respawn: false,
            };
            self.teleport_player(t);
            self.life.vit.fall = 0.0;
            self.life.vit.impulse_reset();
            self.mobs.player_vel = [0.0; 3];
            // Player in the middle of a dimension change (not yet "loaded"): the game lets
            // only the pearl's damage through.
            let loaded = std::mem::replace(&mut self.loaded, true);
            self.hurt("minecraft:ender_pearl", 5.0);
            self.loaded = loaded;
            self.play_sound("minecraft:entity.player.teleport", 7, pos, 1.0, 1.0);
        }
    }

    /// Portal ticket (300 ticks) placed in dimension `name`.
    pub(crate) fn place_portal_ticket(&mut self, name: &str) {
        if let Some(w) = self.dims.parked.iter_mut().find(|w| w.dim.name == name) {
            w.ticket = 300;
        }
    }

    /// Height and flags of the active dimension type (`login`/`respawn` packets).
    pub(crate) fn current_height(&self) -> rc_proto_v777::config::DimensionHeight {
        let t = self.dimension_type_of(&self.dim.name);
        rc_proto_v777::config::DimensionHeight {
            min_y: self.dim.height.min_y,
            height: self.dim.height.sections as i32 * 16,
            nether_light: self.data.dimension_types.iter().any(|d| d.name == t && d.nether_light),
        }
    }

    /// The world has just changed for the player (portal, respawn): client view
    /// reset, chunks resent, items re-announced.
    pub(crate) fn reset_view(&mut self) {
        self.sent.clear();
        self.queue.clear();
        self.center = None;
        self.loaded = false;
        self.joined_tick = self.tick;
        self.mining = None;
    }

    /// Mobs of the world left forgotten by the client (it clears its entities at `respawn`).
    pub(crate) fn forget_world_entities(&mut self, name: &str) {
        if let Some(w) = self.dims.parked.iter_mut().find(|w| w.dim.name == name) {
            for m in w.mobs.list.iter_mut() {
                m.track.seen = false;
            }
        }
        for m in self.mobs.list.iter_mut() {
            m.track.seen = false;
        }
        // F16: the player leaves the wither bars.
        let mut bars = Vec::new();
        let parked = self.dims.parked.iter_mut().filter(|w| w.dim.name == name).flat_map(|w| w.mobs.list.iter_mut());
        for m in parked.chain(self.mobs.list.iter_mut()) {
            if std::mem::take(&mut m.wither.bar_shown) {
                bars.push(m.wither.bar);
            }
        }
        for b in bars {
            crate::mobs::wither::boss::bar_removed(self, b);
        }
        // S8: projectiles of both worlds.
        if let Some(w) = self.dims.parked.iter_mut().find(|w| w.dim.name == name) {
            for p in &mut w.proj.list {
                p.track.seen = false;
            }
            for c in &mut w.proj.clouds {
                c.seen = false;
            }
            // L9-TICK: fangs parked with the world left (`enter_world` already swapped them out).
            for f in &mut w.fangs {
                f.seen = false;
            }
            // L9-TICK: the bobber left behind no longer finds its player in its world: the game
            // discards it at its next tick, before the rod can be used again.
            w.proj.hook = None;
            // G5-F06: minecarts of the world left.
            for c in w.carts.list.iter_mut().flatten() {
                c.track.seen = false;
            }
            // L10-ORDER: its entities leave the list on its next tick (`walk_park_sweep`).
            w.swept = 0;
        }
        self.proj_forget_tracking();
        self.carts_forget_tracking(); // G5-F06
    }

    /// Player passage to `t` (same dimension: simple teleportation).
    pub(crate) fn teleport_player(&mut self, t: Transition) {
        self.player_remove_vehicle(); // no longer a passenger
        if t.dim == self.dim.name {
            self.send_relative_teleport(t.pos, t.yaw, t.pitch, t.relative);
            if t.sound {
                let _ = self.send(Event::LevelEvent { kind: 1032, pos: [0, 0, 0], data: 0, global: false });
            }
            return;
        }
        self.change_dimension(t);
    }

    /// Absolute position with relative flags (rotation added, velocity kept).
    pub(crate) fn send_relative_teleport(&mut self, pos: [f64; 3], yaw: f32, pitch: f32, relative: i32) {
        let ayaw = if relative & rel::Y_ROT != 0 { self.yaw + yaw } else { yaw };
        let apitch = if relative & rel::X_ROT != 0 { self.pitch + pitch } else { pitch };
        self.begin_teleport(pos);
        self.yaw = ayaw;
        self.pitch = apitch;
        let _ = self.send(Event::Teleport(rc_proto_v777::play::PlayerPosition {
            id: self.teleport_id,
            pos,
            delta: [0.0; 3],
            yaw,
            pitch,
            relative,
        }));
    }

    /// Player dimension change (other dimension).
    pub(crate) fn change_dimension(&mut self, t: Transition) {
        let from = self.dim.name.clone();
        // Before the worlds are swapped: the mount and the cushion stay in the old one.
        self.player_remove_vehicle();
        self.flush_block_changes();
        if self.life.menu.is_some() {
            self.close_menu(true);
        }
        let old_pos = self.pos;
        self.inside_forget_player(); // G5-F02a: no sweep from one world to the other
        self.enter_world(&t.dim);
        // The player joins the end of the arrival world's entity list.
        self.walk.player = crate::walk::next_rank();
        self.forget_world_entities(&from);
        if t.missing_respawn {
            let _ = self.send(Event::GameEvent { kind: 0, param: 0.0 });
        }
        let flat = self.is_flat();
        let height = self.current_height();
        let dimension = self.dimension_type_of(&self.dim.name);
        let _ = self.send(Event::Respawn { height, dimension, flat, keep: 3, last_death: self.life.vit.last_death.clone() });
        let _ = self.send(Event::Difficulty { id: self.level.difficulty_id(), locked: self.level.difficulty_locked });
        let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: crate::lan::op_event(self.cmd.op) }));
        if from == OVERWORLD && t.dim == NETHER {
            self.dims.entered_nether = Some(old_pos);
        }
        self.reset_view();
        self.send_relative_teleport(t.pos, t.yaw, t.pitch, t.relative);
        self.last_good = t.pos;
        self.stop_using();
        let _ = self.send(Event::Abilities(self.abilities()));
        self.send_level_info();
        self.life_send_inventory();
        let _ = self.send(Event::HeldSlot(self.held));
        if t.sound {
            let _ = self.send(Event::LevelEvent { kind: 1032, pos: [0, 0, 0], data: 0, global: false });
        }
        self.life.vit.force_resend();
        self.send_all_effects(); // S7
        self.update_tracking();
        let drops = std::mem::take(&mut self.life.drops);
        for d in &drops {
            self.reannounce_item(d);
        }
        self.life.drops = drops;
        self.s9_dimension(&from, &t.dim); // S9: `changed_dimension`, `nether_travel`
        // Nether entry forgotten only after `nether_travel`, which reads it.
        if t.dim != NETHER {
            self.dims.entered_nether = None;
        }
        eprintln!("singleplayer: {} → {} at {:.1} {:.1} {:.1}", from, self.dim.name, t.pos[0], t.pos[1], t.pos[2]);
    }

    /// Border, clocks, spawn point, rain, chunk wait.
    pub(crate) fn send_level_info(&mut self) {
        self.send_border();
        self.send_time(true);
        let _ = self
            .send(Event::SpawnPosition { dimension: self.level.spawn_dimension.clone(), pos: self.level.spawn_pos });
        if self.dim.name == OVERWORLD {
            self.send_weather_state();
        }
        let _ = self.send(Event::GameEvent { kind: 13, param: 0.0 });
        self.send_ticking_state(); // current rate (`tick`)
        self.send_ticking_step();
    }
}
