//! The solid entities in the server's free-room tests (spawn boxes, random teleports, the player's
//! pose and move check, a shulker's peek, an armor stand's placement…) and movements. Besides the
//! blocks, the game counts there the boxes of the entities others collide with as with a block (a
//! living shulker, a living adult happy ghast held still, a boat), as the entity the test is made
//! for sees them, and the world border only when the test is made for an entity standing close to
//! it. The two mob species are found through the per-species index of the mob list, the boats in
//! the vehicle list.

use rc_physics::Aabb;

use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::play::SimWorld;
use crate::server::Server;

/// Species whose mobs can be solid to the others ([`blocks_other_entities`]).
const SOLID_SPECIES: [Kind; 2] = [Kind::Shulker, Kind::HappyGhast];

/// The entity a free-room test or a movement is made for, as the game hands it to its entity
/// search: it decides which solid entities block and whether the world border counts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RoomFor {
    /// No entity (spawn boxes, random teleports, a shulker box lid, an arrow's fall): every solid
    /// entity blocks, the world border does not count.
    NoEntity,
    /// An entity: the border counts when that entity stands close to it, and neither the entity
    /// itself nor what shares its root vehicle blocks it.
    Entity(Asker),
}

impl RoomFor {
    /// The same test made for its entity standing at `pos` (where the game has already moved it,
    /// or where a test is made before the entity is placed): the border counts from there.
    pub(crate) fn standing_at(self, pos: [f64; 3]) -> RoomFor {
        match self {
            RoomFor::NoEntity => RoomFor::NoEntity,
            RoomFor::Entity(a) => RoomFor::Entity(Asker { position: [pos[0], pos[2]], ..a }),
        }
    }
}

/// The entity behind a [`RoomFor::Entity`] test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Asker {
    /// Its own id: a mob still in the list is not solid to itself.
    id: i32,
    /// Its root vehicle (its own id when it rides nothing).
    root: i32,
    /// A happy ghast: a ridden happy ghast is solid to it whether held still or not.
    happy_ghast: bool,
    /// Where it stands (x, z): the world border counts when that point is close to it, whatever
    /// box is tested.
    position: [f64; 2],
}

/// A mob other entities collide with as with a block, whoever asks: a living shulker, a living
/// adult happy ghast held still. (A ridden happy ghast is also solid to another happy ghast, see
/// [`SolidEntities`].)
pub(crate) fn blocks_other_entities(m: &Mob) -> bool {
    match m.kind {
        Kind::Shulker => m.alive(),
        Kind::HappyGhast => adult_alive(m) && crate::mobs::recent::happy_ghast::on_still_timeout(m),
        _ => false,
    }
}

fn adult_alive(m: &Mob) -> bool {
    !m.is_baby() && m.alive()
}

/// False only when the box of the solid mob `o` cannot touch `zone`. A shulker's box is the unit
/// box at its position grown by its lid's opening, one block at most toward one side; a happy ghast
/// has its species' bound. `RUSTCRAFT_OPT_PUSHFILTER=0` turns both bounds off, like the other
/// species bounds.
pub(crate) fn box_may_touch(o: &Mob, zone: &Aabb) -> bool {
    if o.kind != Kind::Shulker {
        return o.may_touch(zone);
    }
    if !crate::mobs::mob::push_filter() {
        return true;
    }
    let p = o.pos();
    let around = Aabb { min: [p[0] - 1.5, p[1] - 1.0, p[2] - 1.5], max: [p[0] + 1.5, p[1] + 2.0, p[2] + 1.5] };
    around.intersects(zone)
}

/// `RUSTCRAFT_OPT_SOLID_MOBS_BY_SPECIES=0`: the searches walk the whole mob list, kind read first
/// (the former lead, measured slower on a loaded machine only: kept until the quiet bench).
fn solid_mobs_by_species() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SOLID_MOBS_BY_SPECIES")
}

/// Something rides `m` (the player, a mob, or a passenger of a species we do not simulate).
fn carries_passengers(m: &Mob) -> bool {
    !m.mount.passengers.is_empty() || !m.mount.foreign_passengers.is_empty()
}

#[cfg(test)]
thread_local! {
    /// Solid entity searches made on this thread (the bench's deterministic count).
    static SEARCHES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Solid entity searches made on this thread so far (tests and benches).
#[cfg(test)]
pub(crate) fn searches() -> u64 {
    SEARCHES.with(std::cell::Cell::get)
}

/// The solid entities a free-room test meets: what `SimWorld::entity_colliders` gives.
#[derive(Clone, Copy)]
pub(crate) struct SolidEntities<'a> {
    server: &'a Server,
    room: RoomFor,
}

impl SolidEntities<'_> {
    /// Boxes of the solid entities that touch `area` (grown by 1e-7 like the game's search), into
    /// `out`.
    pub(crate) fn boxes_touching(&self, area: &Aabb, out: &mut Vec<Aabb>) {
        #[cfg(test)]
        SEARCHES.with(|n| n.set(n.get() + 1));
        let Some(zone) = rc_physics::entity_search_zone(area) else { return };
        if solid_mobs_by_species() {
            for o in self.server.mobs.of_kinds(SOLID_SPECIES) {
                self.push_if_blocking(o, &zone, out);
            }
        } else {
            for o in self.server.mobs.list.iter().filter(|o| SOLID_SPECIES.contains(&o.kind)) {
                self.push_if_blocking(o, &zone, out);
            }
        }
        // Boats, whoever asks (the one being ticked is out of the list: never solid to itself).
        // Every move of every entity searches here: a vehicle list without a boat is not walked.
        let carts = &self.server.carts;
        if !carts.any_boat && crate::minecart::skip_vehicles_without_boats() {
            return;
        }
        for c in carts.list.iter().flatten() {
            if !c.kind.is_boat() || c.removed {
                continue;
            }
            let b = c.bbox();
            if b.intersects(&zone) && self.boat_blocks_asker(c.id) {
                out.push(b);
            }
        }
    }

    /// The boat `id` is solid to the entity the test is made for: neither the boat itself nor one
    /// of its passengers (a boat rides nothing: it is its own root vehicle).
    fn boat_blocks_asker(&self, id: i32) -> bool {
        match self.room {
            RoomFor::NoEntity => true,
            RoomFor::Entity(a) => a.id != id && a.root != id,
        }
    }

    /// The box of the solid mob `o` into `out` when it meets `zone` and blocks the asker.
    fn push_if_blocking(&self, o: &Mob, zone: &Aabb, out: &mut Vec<Aabb>) {
        // A solid mob away from the test is the usual case: a bound read from its position first
        // (a shulker's exact box costs a sine), the rule last since it may walk vehicles.
        if !box_may_touch(o, zone) {
            // Every search of the test suite checks the bound (the benches turn it off).
            #[cfg(test)]
            if crate::mobs::index::verify() {
                assert!(!o.bbox().intersects(zone), "{:?} box outside its bound", o.kind);
            }
            return;
        }
        let b = o.bbox();
        if b.intersects(zone) && self.blocked_by(o) {
            out.push(b);
        }
    }

    /// `o` is solid to the entity the test is made for.
    fn blocked_by(&self, o: &Mob) -> bool {
        match self.room {
            RoomFor::NoEntity => blocks_other_entities(o),
            RoomFor::Entity(a) => {
                let solid = blocks_other_entities(o)
                    || (a.happy_ghast && o.kind == Kind::HappyGhast && adult_alive(o) && carries_passengers(o));
                o.id != a.id && solid && self.server.root_of(o.id, o.mount.vehicle) != a.root
            }
        }
    }
}

impl Server {
    /// A free-room test made for the mob `m`, whether in the list, taken out for its tick, or not
    /// spawned yet.
    pub(crate) fn room_for_mob(&self, m: &Mob) -> RoomFor {
        let root = self.root_of(m.id, m.mount.vehicle);
        let position = [m.pos()[0], m.pos()[2]];
        RoomFor::Entity(Asker { id: m.id, root, happy_ghast: m.kind == Kind::HappyGhast, position })
    }

    /// A free-room test made for the installed player, where it stands.
    pub(crate) fn room_for_player(&self) -> RoomFor {
        let root = self.root_of(self.player_id, self.mobs.mounts.riding.vehicle);
        let position = [self.pos[0], self.pos[2]];
        RoomFor::Entity(Asker { id: self.player_id, root, happy_ghast: false, position })
    }

    /// A free-room test made for the entity `id` standing at `pos`, which rides nothing and is no
    /// mob (a dropped item, an experience orb, a primed TNT, a projectile).
    pub(crate) fn room_for_object(id: i32, pos: [f64; 3]) -> RoomFor {
        RoomFor::Entity(Asker { id, root: id, happy_ghast: false, position: [pos[0], pos[2]] })
    }

    /// The server world as the free-room tests and the movements of `room` see it: the solid
    /// entities it collides with count, and the world border unless the test is made for no entity.
    pub(crate) fn world_for_room(&self, room: RoomFor) -> SimWorld<'_> {
        let asker_position = match room {
            RoomFor::NoEntity => None,
            RoomFor::Entity(a) => Some(a.position),
        };
        SimWorld {
            solid_entities: Some(SolidEntities { server: self, room }),
            border_counts: asker_position.is_some(),
            asker_position,
            ..self.world()
        }
    }

    /// The game's free-room test of `b`: no block, no solid entity `room` collides with, and no
    /// world border for an entity.
    pub(crate) fn room_free(&self, room: RoomFor, b: &Aabb) -> bool {
        rc_physics::is_free(&self.world_for_room(room), b)
    }

    /// The free-room test of `room` made in the region a new chunk is generated in, which holds no
    /// entity: the blocks, and the world border for an entity close to it.
    pub(crate) fn generation_room_free(&self, room: RoomFor, b: &Aabb) -> bool {
        rc_physics::is_free(&SimWorld { solid_entities: None, ..self.world_for_room(room) }, b)
    }
}
