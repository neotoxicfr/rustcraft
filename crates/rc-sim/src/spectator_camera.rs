//! A spectator's camera set on an entity (`spectator_action` click, `/spectate`). Like the game,
//! the player rides along with the watched entity: each of their ticks puts them on its position
//! and rotation, so the chunks around it load (the tracked area follows the player). The camera
//! comes back to the player's own eyes when the entity is gone or dead, when the player sneaks,
//! when they leave the spectator mode, and before a teleport by `/tp`, `/spreadplayers`, the
//! spectator menu or a portal clicked as a spectator.

use rc_client::Event;

use crate::commands::ents::E;
use crate::server::Server;

impl Server {
    /// Entity whose eyes the installed player looks through, `None` for their own.
    pub(crate) fn spectated_entity(&self) -> Option<E> {
        self.prog.cmds.camera.filter(|&e| e != E::Player)
    }

    /// Moves the installed player's camera to `target` (`None` or the player: their own eyes).
    /// A change puts the player where the new camera stands (their own rotation kept, a teleport
    /// the client answers) and tells the client which entity it looks through; asking for the
    /// camera already in place sends nothing, like the game.
    pub(crate) fn set_camera(&mut self, target: Option<E>) {
        let target = target.filter(|&e| e != E::Player);
        if target == self.spectated_entity() {
            return;
        }
        self.prog.cmds.camera = target;
        let stand_at = target.map_or(self.pos, |e| self.e_pos(e));
        self.teleport_to(stand_at);
        let looked_through = target.map_or(self.player_id, |e| self.ent_id(e));
        let _ = self.send(Event::SetCamera(looked_through));
    }

    /// The camera of player `e` (the installed one or a LAN guest) back to their own eyes.
    pub(crate) fn reset_camera_of(&mut self, e: E) {
        match e {
            E::Player => self.set_camera(None),
            E::OtherPlayer(_) => {
                let _ = self.on_player(e, |sv| sv.set_camera(None));
            }
            _ => {}
        }
    }

    /// The camera's part of the player's own tick: back to their eyes once the watched entity is
    /// gone or dead; otherwise the player stands on its position and rotation, and holding the
    /// sneak key brings the camera back.
    pub(crate) fn spectator_camera_tick(&mut self) {
        let Some(watched) = self.spectated_entity() else { return };
        if !self.camera_target_alive(watched) {
            self.set_camera(None);
            return;
        }
        self.pos = self.e_pos(watched);
        [self.yaw, self.pitch] = self.ent_rot(watched);
        if self.sneaking {
            self.set_camera(None);
        }
    }

    /// The watched entity still stands in the world and lives (a dying mob no longer does).
    fn camera_target_alive(&self, e: E) -> bool {
        match e {
            E::Mob(id) => self.mobs.mob_ref_any(id).is_some_and(|m| m.alive()) && self.ent_exists(e),
            _ => self.e_alive(e),
        }
    }
}
