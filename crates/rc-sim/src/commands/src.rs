//! Source of a command (the game's, in our terms): who receives the feedback,
//! executing entity, position, rotation, dimension, permission level, anchor.

use rc_text::Component;

use super::ents::E;
use crate::server::Server;

/// Recipient of the feedback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Origin {
    Player,
    Server,
    Block([i32; 3]),
    Minecart(i32),
}

/// Failure of a command (message shown in red).
#[derive(Debug, Clone)]
pub struct Fail(pub Component);

impl From<Component> for Fail {
    fn from(c: Component) -> Fail {
        Fail(c)
    }
}

/// Memorisation of the result (`execute store`).
#[derive(Debug, Clone)]
pub enum Store {
    Score { targets: Vec<String>, objective: String },
    Bossbar { id: String, max: bool },
    Block { pos: [i32; 3], path: String, kind: String, scale: f64 },
    Entity { ent: E, path: String, kind: String, scale: f64 },
    Storage { id: String, path: String, kind: String, scale: f64 },
}

#[derive(Debug, Clone)]
pub struct Src {
    pub origin: Origin,
    pub entity: Option<E>,
    pub pos: [f64; 3],
    /// Pitch, yaw (the game's `Vec2(x, y)`).
    pub rot: [f32; 2],
    pub dim: String,
    pub level: u8,
    pub name: String,
    pub display: Component,
    /// Anchor at the eyes (`execute anchored eyes`).
    pub eyes: bool,
    pub silent: bool,
    /// (result?, target): `execute store result|success`.
    pub stores: Vec<(bool, Store)>,
}

impl Src {
    /// Anchor position (feet or eyes of the executing entity).
    pub fn anchor(&self, s: &Server) -> [f64; 3] {
        match (self.eyes, self.entity) {
            (true, Some(e)) => {
                let mut p = self.pos;
                p[1] += s.ent_eye_height(e);
                p
            }
            _ => self.pos,
        }
    }

    pub fn is_player(&self) -> bool {
        self.entity == Some(E::Player)
    }
}

/// Source kept past the end of its command (test runs, profile lookups answered later): the
/// player in it is found again by their profile at each report, since LAN seats move when a guest
/// leaves.
#[derive(Debug, Clone)]
pub(crate) struct KeptSource {
    pub src: Src,
    /// Profile of the source's player (the one who typed it, or the executing player); `None`
    /// without one.
    pub player: Option<u128>,
    /// Last known position of that player (kept after they leave, like the game's player object).
    pub last: [f64; 3],
}

impl KeptSource {
    /// The command is over: a command block (or minecart) closes its output, which then keeps
    /// nothing and informs nobody; a player or the server keep theirs.
    pub(crate) fn command_ended(&mut self) {
        if matches!(self.src.origin, Origin::Block(_) | Origin::Minecart(_)) {
            self.src.silent = true;
        }
    }
}

impl Server {
    /// Source of the player who types a command.
    pub(crate) fn player_source(&self) -> Src {
        Src {
            origin: Origin::Player,
            entity: Some(E::Player),
            pos: self.pos,
            rot: [self.pitch, self.yaw],
            dim: self.dim.name.clone(),
            level: self.cmd.op,
            name: self.name.clone(),
            display: self.player_display_name(),
            eyes: false,
            silent: false,
            stores: Vec::new(),
        }
    }

    /// "Server" source (scheduled functions, `tick`): lower corner of the world's
    /// spawn point, in its dimension (the main world if it does not exist).
    pub(crate) fn server_source(&self) -> Src {
        let sp = self.level.spawn_pos;
        let dim = &self.level.spawn_dimension;
        // The game's dimensions first: no list built at each function tick.
        let known = crate::server::DIMENSIONS.contains(&dim.as_str()) || self.world_dimensions().contains(dim);
        let dim = if known { dim.clone() } else { "minecraft:overworld".into() };
        Src {
            origin: Origin::Server,
            entity: None,
            pos: [f64::from(sp[0]), f64::from(sp[1]), f64::from(sp[2])],
            rot: [0.0, 0.0],
            dim,
            level: 2,
            name: "Server".into(),
            display: Component::text("Server"),
            eyes: false,
            silent: true,
            stores: Vec::new(),
        }
    }

    /// The server's own source when it reports (sprint report, test runs): owner level, output
    /// kept (its successes reach the operators; the server's log is not kept).
    pub(crate) fn server_reply_source(&self) -> Src {
        let mut s = self.server_source();
        s.level = 4;
        s.silent = false;
        s
    }

    /// Source kept past its command; its player is the current one (the only player a command
    /// can name as its executing entity).
    pub(crate) fn keep_source(&self, s: &Src) -> KeptSource {
        let player = (s.origin == Origin::Player || s.entity == Some(E::Player)).then_some(self.uuid);
        KeptSource { src: s.clone(), player, last: self.pos }
    }

    /// Runs `f` as a kept source: its dimension, its player installed. A player who left gets
    /// nothing (their messages are lost), but their relayed successes still reach the operators.
    pub(crate) fn as_kept_source(&mut self, ks: &KeptSource, f: impl FnOnce(&mut Server, &Src)) {
        let mut src = ks.src.clone();
        let seat = ks.player.and_then(|u| self.seat_of_uuid(u));
        if ks.player.is_some() && seat.is_none() {
            if src.origin == Origin::Player {
                src.origin = Origin::Server;
            }
            if src.entity == Some(E::Player) {
                src.entity = None;
            }
        }
        let dim = src.dim.clone();
        self.in_dimension(&dim, |sv| match seat {
            Some(who) => sv.as_player(who, |sv| f(sv, &src)),
            None => f(sv, &src),
        });
    }

    /// Source with the same output, become entity `e` (`execute as`).
    pub(crate) fn with_entity(&self, src: &Src, e: E) -> Src {
        let mut s = src.clone();
        s.entity = Some(e);
        s.name = self.ent_score_name(e);
        s.display = self.ent_display(e);
        s
    }
}
