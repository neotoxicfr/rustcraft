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

    /// Source with the same output, become entity `e` (`execute as`).
    pub(crate) fn with_entity(&self, src: &Src, e: E) -> Src {
        let mut s = src.clone();
        s.entity = Some(e);
        s.name = self.ent_score_name(e);
        s.display = self.ent_display(e);
        s
    }
}
