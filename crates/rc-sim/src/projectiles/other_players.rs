//! Projectiles and the other players of a game open to the local network. A projectile of a
//! player standing in its world ticks with that player installed (`proj_visit`): its owner is
//! "the player" (credit of its hits, the pearl's teleport, the firework's boost). The other
//! players of the world are among its targets; a hit on one of them is resolved on that
//! player's side as a hit on the installed player, the shooting player known there as the
//! author of the damage, then the shooter gets what the game gives it: the hit sound of an
//! arrow, the `player_hurt_entity` and kill credits.

use rc_physics::Aabb;

use super::physics::{EntityHit, Target};
use super::{PKind, Proj};
use crate::commands::ents::E;
use crate::mobs::combat::HurtParts;
use crate::mobs::explode::PlayerAuthor;
use crate::mobs::mob::Ent;
use crate::progress::pred::Who;
use crate::server::Server;

/// A hit on another player while it is resolved on that player's side.
#[derive(Debug, Default)]
pub struct OtherPlayerHit {
    /// The shooting player (the installed one before the victim's side), author of the damage;
    /// `None` for a mob's or an ownerless projectile.
    pub(crate) author: Option<PlayerAuthor>,
    /// What the victim's side reports back: its hurt (`player_hurt_entity`) with the damage type,
    /// its name if it died of it, whether the damage landed.
    pub(crate) hurt: Option<(HurtParts, String)>,
    pub(crate) killed: Option<String>,
    pub(crate) landed: bool,
}

/// Game event the shooting player's client answers with the arrow's hit sound.
const ARROW_HIT_PLAYER_EVENT: u8 = 6;

impl Server {
    /// Seat of the owner of a projectile when it is another player standing in this world.
    pub(crate) fn other_owner_seat_here(&self, owner: Option<u128>) -> Option<Option<usize>> {
        let owner = owner.filter(|&u| u != self.uuid && self.lan.is_some())?;
        self.kept_players().find(|(_, c)| c.uuid == owner && self.stands_here(&c.client_world)).map(|(seat, _)| seat)
    }

    /// The living players of this world other than the installed one that projectile `p` may
    /// hit in `area`, each given to `visit` with its box and position: not a spectator, not
    /// pierced already, not its owner before it left them; an arrow of a player passes through a
    /// player that player may not hurt (`pvp` rule, team without friendly fire).
    pub(crate) fn for_each_other_player_target(
        &self,
        p: &Proj,
        area: &Aabb,
        mut visit: impl FnMut(Target, Aabb, [f64; 3]),
    ) {
        if self.lan.is_none() {
            return;
        }
        let shooter = (p.kind.arrow_like() && p.owner == Some(self.uuid)).then_some(self.name.as_str());
        for (_, c) in self.kept_players() {
            if c.vit.dead || c.game_mode == 3 || !self.stands_here(&c.client_world) || p.pierced.contains(&c.player_id)
            {
                continue;
            }
            if (!p.left_owner && p.owner == Some(c.uuid))
                || shooter.is_some_and(|s| self.player_refuses_damage_from(&c.name, s))
            {
                continue;
            }
            let b = crate::mobs::guest_box(c);
            if b.intersects(area) {
                visit(Target::OtherPlayer(c.player_id), b, c.pos);
            }
        }
    }

    /// Position of the other player of entity `id` (a projectile's target).
    pub(crate) fn other_player_pos(&self, id: i32) -> Option<[f64; 3]> {
        self.lan_player(id).map(|o| o.1)
    }

    /// The other living players of this world, spectators aside, whose box touches `area`:
    /// entity id, position and box (an area effect's victims).
    pub(crate) fn other_players_in(&self, area: &Aabb) -> Vec<(i32, [f64; 3], Aabb)> {
        if self.lan.is_none() {
            return Vec::new();
        }
        let here =
            self.kept_players().filter(|(_, c)| !c.vit.dead && c.game_mode != 3 && self.stands_here(&c.client_world));
        here.map(|(_, c)| (c.player_id, c.pos, crate::mobs::guest_box(c)))
            .filter(|(_, _, b)| b.intersects(area))
            .collect()
    }

    /// An arrow's or a trident's hit on the other player of entity `id` (`proj_hit_entity`).
    pub(crate) fn arrow_hits_other_player(&mut self, p: &mut Proj, id: i32, loc: [f64; 3]) -> bool {
        let on_victim = EntityHit { ent: Target::Living(Ent::Player), loc };
        self.hit_other_player(p, id, |sv, p| sv.proj_hit_entity_now(p, on_victim)).unwrap_or(false)
    }

    /// Another projectile's own hit on the other player of entity `id` (`proj_hit`); what follows
    /// it (landing, the kind's impact) stays on the shooter's side. A rocket only explodes and a
    /// wind charge bursts where it is: those reach every player from the shooter's side.
    pub(crate) fn thrown_hits_other_player(&mut self, p: &mut Proj, id: i32, loc: [f64; 3]) {
        match p.kind {
            PKind::Firework => self.firework_explode(p),
            PKind::WindCharge | PKind::BreezeWindCharge => {
                self.hit_other_player(p, id, |sv, p| sv.wind_hurts_entity(p, Ent::Player, true));
                let c = p.pos;
                self.wind_explode(p, c);
            }
            _ => {
                let on_victim = EntityHit { ent: Target::Living(Ent::Player), loc };
                self.hit_other_player(p, id, |sv, p| sv.proj_on_hit_entity(p, on_victim));
            }
        }
    }

    /// The installed player as the author of the damage of what it owns (`owner`: a
    /// projectile's or a cloud's owner).
    pub(crate) fn author_of(&self, owner: Option<u128>) -> Option<PlayerAuthor> {
        if owner == Some(self.uuid) { self.player_author(crate::commands::ents::E::Player) } else { None }
    }

    /// Runs `hit` with the other player of entity `id` installed, the installed player known
    /// there as the author when it owns `p`; then, back on the shooter's side, the credits.
    fn hit_other_player<R>(
        &mut self,
        p: &mut Proj,
        id: i32,
        hit: impl FnOnce(&mut Server, &mut Proj) -> R,
    ) -> Option<R> {
        let author = self.author_of(p.owner);
        let shot_by_player = author.is_some();
        let (r, report) = self.on_other_player(id, author, |sv| hit(sv, p))?;
        if shot_by_player {
            self.credit_arrow_shooter(p, id, &report);
            self.credit_author(&report);
        }
        Some(r)
    }

    /// Runs `f` with the other player of entity `id` installed (`Ent::Player` there), `author`
    /// known as the author of the damage it takes; returns what `f` gave and what the hit did.
    pub(crate) fn on_other_player<R>(
        &mut self,
        id: i32,
        author: Option<PlayerAuthor>,
        f: impl FnOnce(&mut Server) -> R,
    ) -> Option<(R, OtherPlayerHit)> {
        let (victim, ..) = self.lan_player(id)?;
        let resolving = OtherPlayerHit { author, ..Default::default() };
        Some(self.as_player(victim, |sv| {
            let outer = sv.proj.other_player_hit.replace(Box::new(resolving));
            let r = f(sv);
            let report = std::mem::replace(&mut sv.proj.other_player_hit, outer);
            (r, report.map(|b| *b).unwrap_or_default())
        }))
    }

    /// The author's part of a hit on another player, on its own side: the `player_hurt_entity`
    /// criterion and the kill.
    pub(crate) fn credit_author(&mut self, report: &OtherPlayerHit) {
        if let Some(((original, taken, blocked), ty)) = &report.hurt {
            self.s9_player_hurt_living(None, ty, *original, *taken, *blocked);
        }
        if let (Some(victim), Some((_, ty))) = (&report.killed, &report.hurt) {
            self.s9_player_killed_player(victim, ty);
        }
    }

    /// The shooter's part of its arrow's hit on the other player of entity `id`: the hit sound
    /// of an arrow that hurt them (to its own client), and `killed_by_arrow`.
    fn credit_arrow_shooter(&mut self, p: &Proj, id: i32, report: &OtherPlayerHit) {
        let arrow = matches!(p.kind, PKind::Arrow | PKind::SpectralArrow);
        if arrow && report.landed && !p.silent {
            let _ = self.send(rc_client::Event::GameEvent { kind: ARROW_HIT_PLAYER_EVENT, param: 0.0 });
        }
        if arrow && report.landed && (p.pierce_killed.is_some() || report.killed.is_some()) {
            let mut victims: Vec<Who<'_>> = Vec::new();
            for &e in p.pierce_killed.iter().flatten() {
                if let Ent::Mob(m) = e
                    && self.mobs.mob_ref_any(m).is_some()
                {
                    victims.push(Who::E(E::Mob(m)));
                }
            }
            if report.killed.is_some() {
                victims.push(Who::E(E::OtherPlayer(id)));
            }
            self.trig_killed_by_arrow(&victims, p.weapon.as_ref());
        }
    }

    /// Damage of a projectile on the installed player while it is another player's victim
    /// (`proj_damage`): refused like a hit from that player (`pvp` rule, team), its author known
    /// to the death message; what it did is noted for the shooter. `None`: no such hit is being
    /// resolved, or no player shot it.
    pub(crate) fn player_hurt_by_other_players_projectile(
        &mut self,
        ty: &str,
        amount: f32,
        kb: Option<(f64, f64)>,
    ) -> Option<bool> {
        let author = self.proj.other_player_hit.as_ref()?.author.as_ref()?;
        if self.refuses_player_damage(&author.name) {
            return Some(false);
        }
        self.prog.attacker_player = Some((author.display.clone(), author.named_item.clone()));
        self.prog.hurt_parts = None;
        let alive = !self.life.vit.dead;
        let landed = self.player_hurt_kb(ty, amount, kb);
        self.prog.attacker_player = None;
        let killed = (alive && self.life.vit.dead).then(|| self.name.clone());
        let hurt = self.prog.hurt_parts.take().map(|parts| (parts, ty.to_owned()));
        if let Some(h) = self.proj.other_player_hit.as_mut() {
            // A second hurt of the same hit (a rocket's blast after its hit) keeps the last report.
            h.landed |= landed;
            if hurt.is_some() {
                h.hurt = hurt;
            }
            if killed.is_some() {
                h.killed = killed;
            }
        }
        Some(landed)
    }

    /// Whether the installed player is being hit as another player's victim (the credits are
    /// the shooter's, given back on its side).
    pub(crate) fn hit_as_other_players_victim(&self) -> bool {
        self.proj.other_player_hit.is_some()
    }
}
