//! Integrated server lead: per-mob state, attachment to the player, to another mob or to a fence
//! knot, transfer, shears, damped spring (pull), snapping beyond range, saving (`leash`) and the
//! `set_entity_link` packet.
//!
//! The fence knot is a decoration (`decoration`, kind `Knot`) that is never saved: a mob reloaded
//! with a lead set on a block recreates it.
//!
//! Cost: a mob without a lead only pays one `Option` test per tick (boxed state, rare).
//! The "animals held by X" sweeps only serve clicks and snaps.

use rc_client::Event;
use rc_physics::Aabb;
use rc_proto::nbt::Nbt;

use super::goals::{Goal, MOVE};
use super::mob::{Ent, Mob};
use super::species::Kind;
use crate::decoration::{DKind, base_fields};
use crate::nbtio;
use crate::player::Stack;
use crate::server::Server;

pub const LEAD: &str = "minecraft:lead";
const TIED: &str = "minecraft:item.lead.tied";
const UNTIED: &str = "minecraft:item.lead.untied";
const BREAK: &str = "minecraft:item.lead.break";
/// "Neutral" sound category.
const NEUTRAL: i32 = 6;
/// Half-side of the zone searched for held animals (32-block cube).
const AREA: f64 = 16.0;

/// Who holds the lead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    Player,
    Mob(i32),
    /// Fence knot (decoration id).
    Knot(i32),
}

/// Saved form of the attachment: UUID of an entity or block of the knot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Uuid(u128),
    Pos([i32; 3]),
}

/// Lead state of a mob. Empty `holder`: reloaded attachment, waiting for its holder.
#[derive(Debug, Clone, PartialEq)]
pub struct Leash {
    pub holder: Option<Holder>,
    pub anchor: Anchor,
    /// Angular momentum (degrees removed from the yaw each tick, damped).
    pub angular: f64,
    /// Home zone set by the lead (centre, radius), cleared with it.
    pub home: Option<([i32; 3], i32)>,
}

/// Holder as seen by the lead physics.
#[derive(Debug, Clone, Copy)]
struct Hold {
    pos: [f64; 3],
    center: [f64; 3],
    yaw: f32,
    w: f32,
    h: f32,
    known: [f64; 3],
    /// Alive, present and not a spectator.
    ok: bool,
    /// Carries a four-point lead (ghasts).
    quad: bool,
}

pub fn is_leashed(m: &Mob) -> bool {
    m.leash.as_ref().is_some_and(|l| l.holder.is_some())
}

pub fn holder(m: &Mob) -> Option<Holder> {
    m.leash.as_ref().and_then(|l| l.holder)
}

/// Home zone set by the lead.
pub fn home(m: &Mob) -> Option<([i32; 3], i32)> {
    m.leash.as_ref().and_then(|l| l.home)
}

/// Centre of a box (middle of the segment, like the game).
fn center(b: &Aabb) -> [f64; 3] {
    std::array::from_fn(|a| b.min[a] + 0.5 * (b.max[a] - b.min[a]))
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn area(c: [f64; 3]) -> Aabb {
    Aabb { min: [c[0] - AREA, c[1] - AREA, c[2] - AREA], max: [c[0] + AREA, c[1] + AREA, c[2] + AREA] }
}

/// Species a lead can hold (fixed rule; the state-dependent cases are in
/// `can_be_leashed`): hostiles, fish, bat, villagers, turtle and panda refused.
fn base_leashable(k: Kind) -> bool {
    use Kind::*;
    !matches!(
        k,
        Zombie
            | Skeleton
            | Creeper
            | Spider
            | Enderman
            | Husk
            | Stray
            | Bogged
            | Parched
            | ZombieVillager
            | Giant
            | CaveSpider
            | Silverfish
            | Endermite
            | Phantom
            | Slime
            | MagmaCube
            | Pillager
            | Vindicator
            | Evoker
            | Vex
            | Illusioner
            | Ravager
            | Witch
            | Drowned
            | Guardian
            | ElderGuardian
            | Blaze
            | Ghast
            | WitherSkeleton
            | Piglin
            | PiglinBrute
            | ZombifiedPiglin
            | Warden
            | Breeze
            | Creaking
            | EnderDragon
            | Shulker
            | Wither
            | Cod
            | Salmon
            | Pufferfish
            | TropicalFish
            | Tadpole
            | Bat
            | Villager
            | WanderingTrader
            | Turtle
            | Panda
    )
}

/// Snap distance (centre to centre).
pub fn snap_distance(k: Kind) -> f64 {
    if matches!(k, Kind::Ghast | Kind::HappyGhast) { 16.0 } else { 12.0 }
}

/// Distance beyond which the spring pulls.
pub fn elastic_distance(k: Kind) -> f64 {
    if matches!(k, Kind::Ghast | Kind::HappyGhast) { 10.0 } else { 6.0 }
}

/// Held at four points when the holder allows it (equines other than llamas, camels, sniffer).
fn quad_leashee(k: Kind) -> bool {
    matches!(
        k,
        Kind::Horse
            | Kind::Donkey
            | Kind::Mule
            | Kind::SkeletonHorse
            | Kind::ZombieHorse
            | Kind::Camel
            | Kind::CamelHusk
            | Kind::Sniffer
    )
}

/// Pathfinding mob (follows its holder closely, home around him).
fn pathfinder(k: Kind) -> bool {
    !matches!(k, Kind::Ghast | Kind::Phantom | Kind::EnderDragon | Kind::Bat)
}

/// Joins its holder when close (the allay and the happy ghast do not care to).
fn stays_close(k: Kind) -> bool {
    !matches!(k, Kind::Allay | Kind::HappyGhast)
}

fn follow_speed(k: Kind) -> f64 {
    if matches!(k, Kind::Llama | Kind::TraderLlama) { 2.0 } else { 1.0 }
}

/// Angry nautilus (target or grudge in memory).
fn aggravated(m: &Mob) -> bool {
    m.sea.brain.as_ref().is_some_and(|b| b.angry.is_some() || b.attack.is_some())
}

/// Panic in progress (brain memory or panic goal).
fn panicking(m: &Mob) -> bool {
    super::brainy::is_panicking(m)
        || m.sea.brain.as_ref().is_some_and(|b| b.panicking)
        || m.goals.running(|g| matches!(g, Goal::Panic { .. }))
}

// ---------- Saving ----------

/// Reads `leash`: `{UUID}` compound (entity) or three integers (knot block).
pub fn load(m: &mut Mob, e: &Nbt) {
    let anchor = match e.get("leash") {
        Some(Nbt::IntArray(a)) if a.len() == 3 => Some(Anchor::Pos([a[0], a[1], a[2]])),
        Some(Nbt::List(l)) if l.len() == 3 => {
            let v: Vec<i32> = l.iter().filter_map(|x| x.as_i64()).map(|x| x as i32).collect();
            (v.len() == 3).then(|| Anchor::Pos([v[0], v[1], v[2]]))
        }
        Some(c @ Nbt::Compound(_)) => nbtio::uuid(c.get("UUID")).map(Anchor::Uuid),
        _ => None,
    };
    m.leash = anchor.map(|anchor| Box::new(Leash { holder: None, anchor, angular: 0.0, home: None }));
    // Common home (`home_radius` ≥ 0, `home_pos` at the origin if missing): when no species
    // has taken it in its own field, it goes into the generic field. An animal reloaded on a
    // lead thus keeps the lead's zone until its holder is found, and keeps it for good if
    // the holder stays missing (the game does not clear it in that case).
    let r = nbtio::int(e, "home_radius").map_or(-1, |v| v as i32);
    if r >= 0 && !species_home(m) {
        let p = match e.get("home_pos") {
            Some(Nbt::IntArray(v)) if v.len() == 3 => [v[0], v[1], v[2]],
            _ => [0, 0, 0],
        };
        m.rec.restriction = Some((p, r));
    }
}

/// Home held by a species-specific field (nautilus, elder guardian, trader).
fn species_home(m: &Mob) -> bool {
    m.sea.home.is_some() || m.aqua.home.is_some() || m.npc.as_ref().is_some_and(|n| n.home.is_some())
}

/// Writes `leash` (absent without a lead), then the common home: the lead's takes precedence
/// (the game has a single field, which the lead rewrites); without a lead, the generic field.
/// The turtle and the creaking write their own `home_pos` after the mob's in the game: only
/// `home_radius` is written here for them.
pub fn save(m: &Mob, e: &mut Nbt) {
    match m.leash.as_ref().map(|l| l.anchor) {
        Some(Anchor::Uuid(u)) => nbtio::set(e, "leash", Nbt::Compound(vec![("UUID".into(), nbtio::uuid_nbt(u))])),
        Some(Anchor::Pos(p)) => nbtio::set(e, "leash", Nbt::IntArray(p.to_vec())),
        None => nbtio::remove(e, "leash"),
    }
    let own_pos = matches!(m.kind, Kind::Turtle | Kind::Creaking);
    match home(m).or(m.rec.restriction) {
        Some((p, r)) => {
            nbtio::set(e, "home_radius", Nbt::Int(r));
            if !own_pos {
                nbtio::set(e, "home_pos", Nbt::IntArray(p.to_vec()));
            }
        }
        None if !species_home(m) => {
            nbtio::remove(e, "home_radius");
            if !own_pos {
                nbtio::remove(e, "home_pos");
            }
        }
        None => {}
    }
}

impl Server {
    // ---------- Rules ----------

    /// The mob accepts a lead (species rule and current state).
    pub(crate) fn can_be_leashed(&self, m: &Mob) -> bool {
        match m.kind {
            Kind::Wolf => !(m.anger_end > 0 && m.anger_end - self.time > 0),
            Kind::SulfurCube => m.has_body_item(),
            Kind::Nautilus => !aggravated(m),
            Kind::ZombieNautilus => !aggravated(m) && !matches!(m.mount.passengers.first(), Some(Ent::Mob(_))),
            Kind::CamelHusk => !super::mounts::mob_controlled(m),
            Kind::ZombieHorse => super::mounts::is_tamed(m) || !super::mounts::mob_controlled(m),
            k => base_leashable(k),
        }
    }

    /// `m` can switch to holder `h` (not itself, within snap range, willing species).
    fn can_attach(&self, m: &Mob, h: Holder, hi: &Hold) -> bool {
        h != Holder::Mob(m.id) && dist(center(&m.bbox()), hi.center) <= snap_distance(m.kind) && self.can_be_leashed(m)
    }

    // ---------- Holders ----------

    fn hold_mob(&self, o: &Mob) -> Hold {
        let known = if o.no_ai {
            [0.0; 3]
        } else if Self::player_controls(o) {
            self.proj_player.known_move
        } else {
            o.vel
        };
        Hold {
            pos: o.pos(),
            center: center(&o.bbox()),
            yaw: o.yaw,
            w: o.width(),
            h: o.height(),
            known,
            ok: o.alive(),
            quad: matches!(o.kind, Kind::Ghast | Kind::HappyGhast),
        }
    }

    fn hold_player(&self) -> Hold {
        let b = self.player_aabb();
        Hold {
            pos: self.pos,
            center: center(&b),
            yaw: self.yaw,
            w: (b.max[0] - b.min[0]) as f32,
            h: (b.max[1] - b.min[1]) as f32,
            known: self.proj_player.known_move,
            ok: !self.life.vit.dead && self.game_mode != 3,
            quad: false,
        }
    }

    fn hold_knot(&self, id: i32) -> Option<Hold> {
        let e = self.deco_nbt(id)?;
        let b = self.deco_box(e)?;
        Some(Hold {
            pos: nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default(),
            center: center(&b),
            yaw: nbtio::f32s::<2>(e.get("Rotation")).map_or(0.0, |r| r[0]),
            w: 0.375,
            h: 0.5,
            known: [0.0; 3],
            ok: true,
            quad: false,
        })
    }

    /// Holder present in the world (mob of the list, registered knot, player).
    fn hold(&self, h: Holder) -> Option<Hold> {
        match h {
            Holder::Player => Some(self.hold_player()),
            Holder::Mob(id) => self.mobs.get(id).map(|o| self.hold_mob(o)),
            Holder::Knot(id) => self.hold_knot(id),
        }
    }

    fn holder_net_id(&self, h: Holder) -> i32 {
        match h {
            Holder::Player => self.player_id,
            Holder::Mob(i) | Holder::Knot(i) => i,
        }
    }

    /// `set_entity_link` to the players who track the mob.
    fn send_link(&self, m: &Mob, h: Option<Holder>) {
        if m.track.seen {
            let holder = h.map_or(0, |h| self.holder_net_id(h));
            let _ = self.send(Event::Misc(rc_client::misc::Misc::Link { held: m.id, holder }));
        }
    }

    /// Link sent when a held mob is paired.
    pub(crate) fn pair_leash(&self, m: &Mob) {
        if let Some(h) = holder(m) {
            let holder = self.holder_net_id(h);
            let _ = self.send(Event::Misc(rc_client::misc::Misc::Link { held: m.id, holder }));
        }
    }

    /// Animals held by `h` whose box touches the 32-block cube centred on `c`.
    fn leashed_in_area(&self, c: [f64; 3], h: Holder) -> Vec<i32> {
        let a = area(c);
        self.mobs.list.iter().filter(|o| !o.removed && holder(o) == Some(h) && o.touches(&a)).map(|o| o.id).collect()
    }

    // ---------- Attach and detach ----------

    /// Sets (or changes) the holder; the old knot, if it no longer holds any, disappears; a
    /// ridden mob dismounts.
    pub(crate) fn leash_set(&mut self, m: &mut Mob, h: Holder, anchor: Anchor, sync: bool) {
        let old = match m.leash.as_deref_mut() {
            Some(l) => {
                let old = l.holder;
                l.holder = Some(h);
                l.anchor = anchor;
                old
            }
            None => {
                m.leash = Some(Box::new(Leash { holder: Some(h), anchor, angular: 0.0, home: None }));
                None
            }
        };
        if let Some(o) = old
            && o != h
        {
            self.leashee_removed(o);
        }
        if sync {
            self.send_link(m, Some(h));
        }
        if let Some(vid) = m.mount.vehicle
            && let Some(i) = self.mob_index(vid)
            && let Some(mut v) = self.mobs.list.take(i)
        {
            self.mob_dismount_from(m, &mut v);
            self.mobs.list.put(i, v);
        }
    }

    /// Removes the lead: `drop_lead` drops the item, `sync` notifies the client.
    pub(crate) fn leash_drop(&mut self, m: &mut Mob, sync: bool, drop_lead: bool) {
        let Some(h) = holder(m) else { return };
        m.leash = None;
        // Home cleared with the lead (all those of the mob, like the game).
        super::goals::clear_home(m);
        if drop_lead {
            self.spawn_at_location(m, Stack { id: LEAD.into(), count: 1, components: None }, 0.0);
        }
        if sync {
            self.send_link(m, None);
        }
        self.leashee_removed(h);
    }

    /// A holder lost an animal: a knot that holds none any more disappears.
    fn leashee_removed(&mut self, h: Holder) {
        if let Holder::Knot(k) = h
            && let Some(hk) = self.hold_knot(k)
            && self.leashed_in_area(hk.center, h).is_empty()
        {
            self.deco_remove(k);
        }
    }

    /// Reloaded attachment: holder found (entity by UUID, knot recreated on its block), otherwise
    /// abandoned with the item after 100 ticks.
    fn leash_restore(&mut self, m: &mut Mob) {
        let Some(anchor) = m.leash.as_ref().map(|l| l.anchor) else { return };
        match anchor {
            Anchor::Uuid(u) => {
                let found = if u == self.uuid {
                    Some(Holder::Player)
                } else {
                    self.mobs
                        .index_of_uuid(u)
                        .and_then(|i| self.mobs.list.get(i))
                        .filter(|o| !o.removed)
                        .map(|o| Holder::Mob(o.id))
                };
                if let Some(h) = found {
                    self.leash_set(m, h, anchor, true);
                    return;
                }
            }
            Anchor::Pos(p) => {
                let k = self.knot_at(p).unwrap_or_else(|| self.knot_create(p));
                self.leash_set(m, Holder::Knot(k), anchor, true);
                return;
            }
        }
        if m.tick_count > 100 {
            self.spawn_at_location(m, Stack { id: LEAD.into(), count: 1, components: None }, 0.0);
            m.leash = None;
        }
    }

    // ---------- Tick ----------

    /// Lead tick (end of the entity's base tick).
    pub(crate) fn leash_tick(&mut self, m: &mut Mob) {
        if m.leash.as_ref().is_some_and(|l| l.holder.is_none()) {
            self.leash_restore(m);
        }
        let Some(h) = holder(m) else { return };
        let mut angular = m.leash.as_ref().map_or(0.0, |l| l.angular);
        let info = self.hold(h).filter(|i| i.ok);
        let Some(hi) = info.filter(|_| m.alive()) else {
            let drops = self.rule("minecraft:entity_drops", true);
            self.leash_drop(m, true, drops);
            return;
        };
        let d = dist(hi.center, center(&m.bbox()));
        self.when_leashed_to(m, h, &hi);
        if d > snap_distance(m.kind) {
            self.world_sound(BREAK, NEUTRAL, hi.pos, 1.0, 1.0);
            self.leash_drop(m, true, true);
            m.goals.disabled |= MOVE;
        } else if d > elastic_distance(m.kind) - f64::from(hi.w) - f64::from(m.width())
            && self.leash_elastic(m, &hi, &mut angular)
        {
            self.elastic_pull(m);
        } else {
            self.close_range(m, &hi);
        }
        m.yaw = (f64::from(m.yaw) - angular) as f32;
        let next = angular * f64::from(self.angular_friction(m));
        if let Some(l) = m.leash.as_deref_mut() {
            l.angular = next;
        }
    }

    /// Home around the holder (pathfinding mob); a happy ghast holding a four-point animal
    /// shows its attachments for 5 ticks.
    fn when_leashed_to(&mut self, m: &mut Mob, h: Holder, hi: &Hold) {
        if pathfinder(m.kind)
            && let Some(l) = m.leash.as_deref_mut()
        {
            let c = super::mth::block_of(hi.pos);
            l.home = Some((c, elastic_distance(m.kind) as i32 - 1));
        }
        if let Holder::Mob(id) = h
            && quad_leashee(m.kind)
            && let Some(o) = self.mobs.get_mut(id)
            && o.kind == Kind::HappyGhast
        {
            o.rec.leash_time = 5;
        }
    }

    /// Damped spring between attachment points; false if the lead is slack.
    fn leash_elastic(&mut self, m: &mut Mob, hi: &Hold, angular: &mut f64) -> bool {
        const SHARED: [[f64; 3]; 4] = [[-0.5, 0.5, 0.5], [-0.5, 0.5, -0.5], [0.5, 0.5, -0.5], [0.5, 0.5, 0.5]];
        let quad = hi.quad && quad_leashee(m.kind);
        let (ent_pts, hold_pts): (&[[f64; 3]], &[[f64; 3]]) =
            if quad { (&SHARED, &SHARED) } else { (&[[0.0, 0.5, 0.5]], &[[0.0, 0.5, 0.0]]) };
        let slack = elastic_distance(m.kind);
        let own = self.known_movement_of(m);
        let motion = if m.no_ai { [0.0; 3] } else { own };
        let rad = std::f32::consts::PI / 180.0;
        let (ey, hy) = (m.yaw * rad, hi.yaw * rad);
        let edim = [f64::from(m.width()), f64::from(m.height()), f64::from(m.width())];
        let hdim = [f64::from(hi.w), f64::from(hi.h), f64::from(hi.w)];
        let mut sum = [0.0f64; 3];
        let mut torque = 0.0f64;
        let mut any = false;
        for i in 0..ent_pts.len() {
            let ev = y_rot(mul(ent_pts[i], edim), -ey);
            let ep = [m.pos()[0] + ev[0], m.pos()[1] + ev[1], m.pos()[2] + ev[2]];
            let hv = y_rot(mul(hold_pts[i], hdim), -hy);
            let hp = [hi.pos[0] + hv[0], hi.pos[1] + hv[1], hi.pos[2] + hv[2]];
            let distance = dist(ep, hp);
            if distance < slack {
                continue;
            }
            let n = normalize([hp[0] - ep[0], hp[1] - ep[1], hp[2] - ep[2]]);
            let mut disp = n.map(|c| c * (distance - slack));
            let t = ev[2] * disp[0] - ev[0] * disp[2];
            if motion[0] * disp[0] + motion[1] * disp[1] + motion[2] * disp[2] >= 0.0 {
                disp = disp.map(|c| c * f64::from(0.3f32));
            }
            for a in 0..3 {
                sum[a] += disp[a];
            }
            torque += t;
            any = true;
        }
        if !any {
            return false;
        }
        let scale = if quad { 0.25 } else { 1.0 };
        let force = sum.map(|c| c * scale);
        *angular += 10.0 * (torque * scale);
        let rel = [hi.known[0] - own[0], hi.known[1] - own[1], hi.known[2] - own[2]];
        let k = [0.8, 0.2, 0.8];
        for a in 0..3 {
            m.vel[a] += force[a] * k[a] + rel[a] * 0.11;
        }
        true
    }

    /// Known movement of a mob: that of the player who steers it, otherwise its velocity.
    fn known_movement_of(&self, m: &Mob) -> [f64; 3] {
        if m.alive() && Self::player_controls(m) { self.proj_player.known_move } else { m.vel }
    }

    /// Pull: bounded fall, and species reactions (meal interrupted, camel stands up,
    /// happy ghast waits).
    fn elastic_pull(&mut self, m: &mut Mob) {
        if m.vel[1] > -0.5 && m.fall_distance > 1.0 {
            m.fall_distance = 1.0;
        }
        if super::mounts::handles(m.kind) {
            m.mount.set_flag(super::mounts::FLAG_EATING, false);
        }
        if super::mounts::is_camel(m.kind) {
            use super::mounts::camel;
            if camel::sitting(m) && !camel::in_transition(self, m) && camel::can_change_pose(self, m) {
                camel::stand_up(self, m);
            }
        }
        if m.kind == Kind::HappyGhast {
            m.move_ctl.op = super::mob::MoveOp::Wait;
        }
    }

    /// Close to the holder: the pathfinding mob joins him at 2 blocks (unless panicking).
    fn close_range(&mut self, m: &mut Mob, hi: &Hold) {
        if !pathfinder(m.kind) || !stays_close(m.kind) || panicking(m) {
            return;
        }
        m.goals.disabled &= !MOVE;
        let f = |v: f64| v as f32;
        let (xd, yd, zd) = (f(m.pos()[0] - hi.pos[0]), f(m.pos()[1] - hi.pos[1]), f(m.pos()[2] - hi.pos[2]));
        let distance = f64::from(xd * xd + yd * yd + zd * zd).sqrt() as f32;
        let n = normalize([hi.pos[0] - m.pos()[0], hi.pos[1] - m.pos()[1], hi.pos[2] - m.pos()[2]]);
        let s = f64::from((distance - 2.0).max(0.0));
        let to = [m.pos()[0] + n[0] * s, m.pos()[1] + n[1] * s, m.pos()[2] + n[2] * s];
        self.move_to_pos(m, to, follow_speed(m.kind));
    }

    /// Angular momentum friction: ground (slipperiness × 0.91), liquid 0.8, air 0.91.
    fn angular_friction(&self, m: &Mob) -> f32 {
        if m.on_ground {
            self.friction_at(self.mob_below(m, 0.500_001)) * 0.91
        } else if m.in_water || m.lava_height > 0.0 {
            0.8
        } else {
            0.91
        }
    }

    // ---------- Interactions ----------

    /// "Lead" part of a player's click on a mob (before its own interaction): while sneaking,
    /// the animals he holds switch to it; shears cut all its leads, then shears on the
    /// equipment; detaches if the player holds it; a lead in hand attaches it.
    /// `Some(with item)` if the click is consumed.
    pub(crate) fn leash_interact(&mut self, m: &mut Mob, slot: usize) -> Option<bool> {
        if self.sneaking && self.can_be_leashed(m) && m.alive() && !m.is_baby() {
            let ids = self.leashed_in_area(center(&m.bbox()), Holder::Player);
            if !ids.is_empty() {
                let hi = self.hold_mob(m);
                let h = Holder::Mob(m.id);
                let mut any = false;
                for id in ids {
                    any |= self.leash_transfer(id, h, &hi, Anchor::Uuid(m.uuid));
                }
                if any {
                    self.game_event_block("minecraft:entity_action", m.block_pos(), Some(Ent::Player), None);
                    self.mob_sound(m, TIED, 1.0, 1.0);
                    return Some(false);
                }
            }
        }
        let held = self.slots[slot].as_ref().map(|s| s.id.clone()).unwrap_or_default();
        if held == "minecraft:shears" && self.shear_off_all(Holder::Mob(m.id), Some(m), true) {
            self.hurt_item(slot, 1);
            return Some(true);
        }
        // The sulfur cube keeps its own shears handling (species interaction).
        if m.kind != Kind::SulfurCube && self.shear_equipment(m, slot) {
            return Some(true);
        }
        if !m.alive() {
            return None;
        }
        if holder(m) == Some(Holder::Player) {
            if self.game_mode == 1 {
                self.leash_drop(m, true, false);
            } else {
                self.leash_drop(m, true, true);
            }
            self.game_event_at("minecraft:entity_interact", m.pos(), Some(Ent::Player));
            self.mob_sound(m, UNTIED, 1.0, 1.0);
            return Some(false);
        }
        if held == LEAD && self.can_attach(m, Holder::Player, &self.hold_player()) {
            if is_leashed(m) {
                self.leash_drop(m, true, true);
            }
            let u = self.uuid;
            self.leash_set(m, Holder::Player, Anchor::Uuid(u), true);
            self.mob_sound(m, TIED, 1.0, 1.0);
            self.consume_held(slot);
            self.send_slot(slot);
            return Some(true);
        }
        None
    }

    /// Switches mob `id` to holder `h` if it can.
    fn leash_transfer(&mut self, id: i32, h: Holder, hi: &Hold, anchor: Anchor) -> bool {
        let Some(i) = self.mob_index(id) else { return false };
        let Some(mut o) = self.mobs.list.take(i) else { return false };
        let ok = self.can_attach(&o, h, hi);
        if ok {
            self.leash_set(&mut o, h, anchor, true);
        }
        self.mobs.list.put(i, o);
        ok
    }

    /// Shears on a holder: its own lead then all those it holds fall
    /// (`shear` event, shears sound); true if at least one fell. `by_player`: cut by the player
    /// (event source, sound in his category); otherwise (dispenser) event without a source and
    /// sound in the category of the sheared entity.
    pub(crate) fn shear_off_all(&mut self, h: Holder, me: Option<&mut Mob>, by_player: bool) -> bool {
        let (c, pos) = match (&me, h) {
            (Some(m), _) => (center(&m.bbox()), m.pos()),
            (None, _) => match self.hold(h) {
                Some(hi) => (hi.center, hi.pos),
                None => return false,
            },
        };
        let category = match (&me, h) {
            _ if by_player => 7,
            (Some(m), _) => Self::mob_sound_source(m),
            (None, Holder::Player) => 7,
            (None, _) => NEUTRAL,
        };
        let ids = self.leashed_in_area(c, h);
        let mut dropped = !ids.is_empty();
        if let Some(m) = me
            && is_leashed(m)
        {
            self.leash_drop(m, true, true);
            dropped = true;
        }
        for id in ids {
            if let Some(i) = self.mob_index(id)
                && let Some(mut o) = self.mobs.list.take(i)
            {
                self.leash_drop(&mut o, true, true);
                self.mobs.list.put(i, o);
            }
        }
        if !dropped {
            return false;
        }
        self.game_event_at("minecraft:shear", pos, by_player.then_some(Ent::Player));
        let bp = super::mth::block_of(pos);
        let at = [f64::from(bp[0]) + 0.5, f64::from(bp[1]) + 0.5, f64::from(bp[2]) + 0.5];
        self.world_sound("minecraft:item.shears.snip", category, at, 1.0, 1.0);
        true
    }

    // ---------- Fence knot ----------

    /// New knot on fence `p` (block centre, 0.375 above the bottom).
    pub(crate) fn knot_create(&mut self, p: [i32; 3]) -> i32 {
        let u = self.deco_uuid();
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.375, f64::from(p[2]) + 0.5];
        let mut f = base_fields(DKind::Knot, pos, 0.0, 0.0, u);
        f.push(("block_pos".into(), Nbt::IntArray(p.to_vec())));
        self.deco_spawn(Nbt::Compound(f))
    }

    /// Click on a fence (any item, or a lead while sneaking): the animals the player holds
    /// switch to the block's knot (created, and removed if unused). True if at least one was
    /// attached.
    pub(crate) fn leash_bind_player_mobs(&mut self, p: [i32; 3]) -> bool {
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let ids = self.leashed_in_area(c, Holder::Player);
        if ids.is_empty() {
            return false;
        }
        let existing = self.knot_at(p);
        let k = existing.unwrap_or_else(|| self.knot_create(p));
        let Some(hk) = self.hold_knot(k) else { return false };
        let mut any = false;
        for id in ids {
            any |= self.leash_transfer(id, Holder::Knot(k), &hk, Anchor::Pos(p));
        }
        if any {
            if let Some(e) = self.deco_nbt(k).cloned() {
                self.knot_sound(&e, TIED);
            }
            self.game_event_block("minecraft:block_attach", p, Some(Ent::Player), None);
            return true;
        }
        if existing.is_none() {
            self.deco_remove(k);
        }
        false
    }

    /// Sound of a knot (silent if `Silent`).
    fn knot_sound(&mut self, e: &Nbt, name: &str) {
        if nbtio::int(e, "Silent").is_some_and(|v| v != 0) {
            return;
        }
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.world_sound(name, NEUTRAL, pos, 1.0, 1.0);
    }

    /// Knot destroyed (hit, explosion, fence gone): detached lead sound.
    pub(crate) fn knot_dropped(&mut self, e: &Nbt) {
        self.knot_sound(e, UNTIED);
    }

    /// Player click on a knot: shears cut everything; otherwise the animals he holds are
    /// attached to it, or (without sneaking) those of the knot come back to him. True if consumed.
    pub(crate) fn knot_interact(&mut self, id: i32, slot: usize) -> bool {
        let Some(e) = self.deco_nbt(id).cloned() else { return false };
        let Some(hk) = self.hold_knot(id) else { return false };
        if self.slots[slot].as_ref().is_some_and(|s| s.id == "minecraft:shears")
            && self.shear_off_all(Holder::Knot(id), None, true)
        {
            self.hurt_item(slot, 1);
            return true;
        }
        let bp = crate::decoration::block_pos_of(&e);
        let mut attached = false;
        for o in self.leashed_in_area(self.hold_player().center, Holder::Player) {
            attached |= self.leash_transfer(o, Holder::Knot(id), &hk, Anchor::Pos(bp));
        }
        let mut taken = false;
        if !attached && !self.sneaking {
            let hp = self.hold_player();
            let u = self.uuid;
            for o in self.leashed_in_area(hk.center, Holder::Knot(id)) {
                taken |= self.leash_transfer(o, Holder::Player, &hp, Anchor::Uuid(u));
            }
        }
        if !attached && !taken {
            return false;
        }
        self.game_event_at("minecraft:block_attach", hk.pos, Some(Ent::Player));
        self.knot_sound(&e, TIED);
        true
    }

    /// Holder of `e` for `/execute on leasher`.
    pub(crate) fn relation_leasher(&self, e: crate::commands::ents::E) -> Option<crate::commands::ents::E> {
        use crate::commands::ents::E;
        let E::Mob(i) = e else { return None };
        match holder(self.mobs.mob_ref_any(i)?)? {
            Holder::Player => Some(E::Player),
            Holder::Mob(o) => Some(E::Mob(o)),
            Holder::Knot(k) => Some(E::Other(k)),
        }
    }
}

fn mul(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

/// Rotation around Y through the game's sine tables (single precision).
fn y_rot(v: [f64; 3], a: f32) -> [f64; 3] {
    let (c, s) = (f64::from(rc_physics::cos_t(a)), f64::from(rc_physics::sin_t(a)));
    [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
}

/// Normalisation (zero under 1e-5, single-precision threshold).
fn normalize(v: [f64; 3]) -> [f64; 3] {
    let d = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if d < f64::from(1.0e-5f32) { [0.0; 3] } else { v.map(|c| c / d) }
}
