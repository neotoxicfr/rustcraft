//! State specific to a player, exchanged with the server's fields: the singleplayer game code (written
//! for a single player) runs as is for a local network guest whose state is
//! installed while their packets and their tick are handled.

use hashbrown::HashSet;
use std::collections::BTreeSet;
use std::mem::swap;

use crate::player::{PlayerData, Stack};
use crate::server::Server;

/// Fields of a player (see `Server`: player, chunk tracking, survival, combat, portals,
/// rights, progress).
pub struct PlayerCtx {
    pub player_id: i32,
    pub uuid: u128,
    pub name: String,
    pub pdata: PlayerData,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub game_mode: i32,
    pub flying: bool,
    pub held: i32,
    pub slots: Vec<Option<Stack>>,
    pub loaded: bool,
    pub joined_tick: u64,
    pub mine_ticks: i64,
    pub demo: crate::demo::Demo,
    pub teleport: Option<(i32, [f64; 3], u64)>,
    pub teleport_id: i32,
    pub moved_this_tick: bool,
    pub view: i32,
    pub center: Option<(i32, i32)>,
    pub sent: HashSet<(i32, i32)>,
    pub queue: Vec<(i32, i32)>,
    pub ack: Option<i32>,
    pub mining: Option<crate::play::Mining>,
    /// Displayed skin parts and main arm (`client_information`).
    pub model: (u8, i32),
    pub last_good: [f64; 3],
    pub sneaking: bool,
    // Survival.
    pub inv_menu: crate::menus::Menu,
    pub menu: Option<crate::menus::Menu>,
    pub known: BTreeSet<String>,
    pub highlight: BTreeSet<String>,
    pub last_recipe: Option<usize>,
    pub pending: crate::life::Pending,
    pub vit: crate::vitals::Vitals,
    pub fx: crate::effects::Effects,
    /// Hits received (death messages): specific to each player.
    pub combat: crate::vitals::Combat,
    /// Mounted minecart and related states: specific to each player.
    pub pcart: crate::minecart::PlayerCart,
    /// Cushion the player is sitting on: specific to each player.
    pub seat: Option<i32>,
    // Combat.
    pub player_hurt_by: Option<(crate::mobs::mob::Ent, i32)>,
    pub player_hit: Option<(crate::mobs::mob::Ent, i32)>,
    pub player_ticks: i32,
    pub attack_ticker: i32,
    pub last_main_hand: Option<(String, i32)>,
    pub player_vel: [f64; 3],
    // Player body as seen by mobs: parrots, swimming, stingers, mount, cube contact.
    pub shoulder: [Option<rc_proto::nbt::Nbt>; 2],
    pub shoulder_time: i64,
    pub shoulder_loaded: bool,
    pub swimming: bool,
    /// Steps (distance walked, next step) and feet water at the last movement.
    pub steps: (f32, f32, bool),
    pub stingers: (i32, i32),
    pub ride: crate::mobs::mounts::riding::PlayerRide,
    pub cube_last: Option<[f64; 3]>,
    pub cube_speed: [f64; 3],
    // Portals.
    pub(crate) portal: crate::dims::EntPortal,
    pub seen_credits: bool,
    pub entered_nether: Option<[f64; 3]>,
    pub dims_read: bool,
    // Rights, progress.
    pub op: u8,
    pub pending_suggest: Option<(i32, String)>,
    pub prog: Box<crate::progress::Progress>,
    /// S8: shooting state (use, cooldowns, stuck arrows, gliding).
    pub proj_player: Box<crate::projectiles::PlayerProj>,
}

impl Server {
    /// Swaps the current player's state with `c` (two calls restore the state).
    pub(crate) fn swap_player(&mut self, c: &mut PlayerCtx) {
        swap(&mut self.player_id, &mut c.player_id);
        swap(&mut self.uuid, &mut c.uuid);
        swap(&mut self.name, &mut c.name);
        swap(&mut self.pdata, &mut c.pdata);
        swap(&mut self.pos, &mut c.pos);
        swap(&mut self.yaw, &mut c.yaw);
        swap(&mut self.pitch, &mut c.pitch);
        swap(&mut self.on_ground, &mut c.on_ground);
        swap(&mut self.game_mode, &mut c.game_mode);
        swap(&mut self.flying, &mut c.flying);
        swap(&mut self.held, &mut c.held);
        swap(&mut self.slots, &mut c.slots);
        swap(&mut self.loaded, &mut c.loaded);
        swap(&mut self.joined_tick, &mut c.joined_tick);
        swap(&mut self.mine_ticks, &mut c.mine_ticks);
        swap(&mut self.demo, &mut c.demo);
        swap(&mut self.teleport, &mut c.teleport);
        swap(&mut self.teleport_id, &mut c.teleport_id);
        swap(&mut self.moved_this_tick, &mut c.moved_this_tick);
        swap(&mut self.view, &mut c.view);
        swap(&mut self.center, &mut c.center);
        swap(&mut self.sent, &mut c.sent);
        swap(&mut self.queue, &mut c.queue);
        swap(&mut self.ack, &mut c.ack);
        swap(&mut self.mining, &mut c.mining);
        swap(&mut self.model, &mut c.model);
        swap(&mut self.last_good, &mut c.last_good);
        swap(&mut self.sneaking, &mut c.sneaking);
        let l = &mut *self.life;
        swap(&mut l.inv_menu, &mut c.inv_menu);
        swap(&mut l.menu, &mut c.menu);
        swap(&mut l.known, &mut c.known);
        swap(&mut l.highlight, &mut c.highlight);
        swap(&mut l.last_recipe, &mut c.last_recipe);
        swap(&mut l.pending, &mut c.pending);
        swap(&mut l.vit, &mut c.vit);
        swap(&mut l.fx, &mut c.fx);
        swap(&mut l.combat, &mut c.combat);
        swap(&mut self.pcart, &mut c.pcart);
        swap(&mut self.deco.seat, &mut c.seat);
        let m = &mut *self.mobs;
        swap(&mut m.player_hurt_by, &mut c.player_hurt_by);
        swap(&mut m.player_hit, &mut c.player_hit);
        swap(&mut m.player_ticks, &mut c.player_ticks);
        swap(&mut m.attack_ticker, &mut c.attack_ticker);
        swap(&mut m.last_main_hand, &mut c.last_main_hand);
        swap(&mut m.player_vel, &mut c.player_vel);
        swap(&mut m.shoulder, &mut c.shoulder);
        swap(&mut m.shoulder_time, &mut c.shoulder_time);
        swap(&mut m.shoulder_loaded, &mut c.shoulder_loaded);
        swap(&mut m.sea.player_swimming, &mut c.swimming);
        swap(&mut m.rec.player_move_dist, &mut c.steps.0);
        swap(&mut m.rec.player_next_step, &mut c.steps.1);
        swap(&mut m.rec.player_in_water, &mut c.steps.2);
        swap(&mut m.wild.player_stingers, &mut c.stingers.0);
        swap(&mut m.wild.remove_stinger_time, &mut c.stingers.1);
        swap(&mut m.mounts.riding, &mut c.ride);
        swap(&mut m.cubes.last_player, &mut c.cube_last);
        swap(&mut m.cubes.player_speed, &mut c.cube_speed);
        let d = &mut *self.dims;
        swap(&mut d.player, &mut c.portal);
        swap(&mut d.seen_credits, &mut c.seen_credits);
        swap(&mut d.entered_nether, &mut c.entered_nether);
        swap(&mut d.read, &mut c.dims_read);
        swap(&mut self.cmd.op, &mut c.op);
        swap(&mut self.cmd.pending_suggest, &mut c.pending_suggest);
        swap(&mut self.prog, &mut c.prog);
        swap(&mut self.proj_player, &mut c.proj_player); // S8
    }

    /// State of a new player (file `players/data/<uuid>.dat` if it exists).
    pub(crate) fn new_player_ctx(&mut self, name: &str, uuid: u128, view: i32, op: u8, mode: Option<i32>) -> PlayerCtx {
        let pdata = PlayerData::read(&self.world, uuid);
        let spawn = self.level.spawn_pos;
        let pos = pdata
            .pos
            .filter(|_| pdata.dimension.as_deref().is_none_or(|d| d == self.dim.name))
            .unwrap_or_else(|| [f64::from(spawn[0]) + 0.5, f64::from(spawn[1]), f64::from(spawn[2]) + 0.5]);
        let [yaw, pitch] = pdata.rotation.unwrap_or([self.level.spawn_yaw, self.level.spawn_pitch]);
        let game_mode = mode.unwrap_or_else(|| pdata.game_mode.unwrap_or(self.level.game_type));
        let mut known = BTreeSet::new();
        let mut highlight = BTreeSet::new();
        if let Some(b) = pdata.root.get("recipeBook") {
            for r in crate::nbtio::list(b.get("recipes")) {
                if let Some(s) = r.as_str() {
                    known.insert(s.to_owned());
                }
            }
            for r in crate::nbtio::list(b.get("toBeDisplayed")) {
                if let Some(s) = r.as_str() {
                    highlight.insert(s.to_owned());
                }
            }
        }
        self.next_entity += 1;
        let id = self.next_entity;
        let tree = self.prog.tree.clone();
        PlayerCtx {
            player_id: id,
            uuid,
            name: name.to_owned(),
            pos,
            yaw,
            pitch,
            on_ground: false,
            game_mode,
            flying: match game_mode {
                3 => true,
                1 => pdata.flying,
                _ => false,
            },
            held: pdata.selected,
            slots: pdata.slots.clone(),
            loaded: false,
            joined_tick: self.tick,
            mine_ticks: 0,
            demo: crate::demo::Demo::default(),
            teleport: None,
            teleport_id: 0,
            moved_this_tick: false,
            view: view.max(2),
            center: None,
            sent: HashSet::new(),
            queue: Vec::new(),
            ack: None,
            mining: None,
            model: (0x7f, 1),
            last_good: pos,
            sneaking: false,
            inv_menu: crate::menus::Menu::inventory(),
            menu: None,
            known,
            highlight,
            last_recipe: None,
            pending: crate::life::Pending::default(),
            vit: crate::vitals::Vitals::read(&pdata.root),
            fx: crate::effects::Effects::read(&pdata.root),
            combat: Default::default(),
            pcart: Default::default(),
            seat: None,
            player_hurt_by: None,
            player_hit: None,
            player_ticks: 0,
            attack_ticker: 0,
            last_main_hand: None,
            player_vel: [0.0; 3],
            shoulder: [None, None],
            shoulder_time: 0,
            shoulder_loaded: false,
            swimming: false,
            steps: (0.0, 0.0, false),
            stingers: (0, 0),
            ride: Default::default(),
            cube_last: None,
            cube_speed: [0.0; 3],
            portal: Default::default(),
            seen_credits: false,
            entered_nether: None,
            dims_read: false,
            op,
            pending_suggest: None,
            prog: Box::new(crate::progress::Progress::new(tree, &self.world, uuid)),
            proj_player: Box::default(), // S8
            pdata,
        }
    }
}
