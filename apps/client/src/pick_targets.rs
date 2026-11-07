//! Entities the crosshair may aim at this frame (`targeting.md` § 2): pickable by type and state,
//! stored in the sections the game searches, with their current box, pick radius and whether
//! they share the camera's root vehicle. Free of `App` so that the proofs and the micro-bench
//! build it from a bare entity table.

use crate::entities::{Entities, Entity, Kind, PickRule, kind_flags_on};
use crate::pick::{Candidate, SectionRange, section_of};
use crate::players::PlayerList;
use crate::stack::ComponentIds;

#[cfg(test)]
mod tests;

/// `RUSTCRAFT_OPT_CACHED_ENTITY_BOXES=0`: every frame recomputes the box of every entity in reach
/// (old path) instead of reading back the one kept while it does not change.
pub(crate) fn cached_entity_boxes_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_CACHED_ENTITY_BOXES")
}

/// Which code answers, from the switches (read once at startup).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TargetPaths {
    /// Pickable and pick radius from the facts stored at spawn (`RUSTCRAFT_OPT_KIND_FLAGS`).
    pub kind_flags: bool,
    /// Box kept between frames (`RUSTCRAFT_OPT_CACHED_ENTITY_BOXES`).
    pub cached_boxes: bool,
}

impl TargetPaths {
    #[cfg(test)]
    pub const OLD: TargetPaths = TargetPaths { kind_flags: false, cached_boxes: false };

    pub fn current() -> TargetPaths {
        TargetPaths { kind_flags: kind_flags_on(), cached_boxes: cached_entity_boxes_on() }
    }
}

/// What the filter reads besides the eyes: the entities, the player list (spectators), the
/// dimension table by type, painting sizes by variant and the component ids (framed maps).
pub(crate) struct TargetSource<'a, P> {
    pub ents: &'a Entities,
    pub players: &'a PlayerList,
    pub table: &'a [[f32; 4]],
    pub painting: P,
    pub component_ids: &'a ComponentIds,
}

/// Where this frame's targeting may find entities: the sections of its widest search box (each
/// targeting then keeps those of its own, shorter, box) and the root vehicle of the camera entity.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TargetArea {
    pub search: rc_data::Aabb,
    pub sections: SectionRange,
    pub camera_root: i32,
}

impl TargetArea {
    /// Area of the search box `search`; `camera_root`: root vehicle of the camera entity (itself
    /// when it rides nothing).
    pub fn of(search: &rc_data::Aabb, camera_root: i32) -> TargetArea {
        TargetArea { search: *search, sections: SectionRange::of(search), camera_root }
    }

    /// The box overlaps the widest search box: otherwise no targeting of the frame can find it.
    fn overlaps(&self, b: &rc_physics::Aabb) -> bool {
        (0..3).all(|i| self.search.min[i] < b.max[i] && self.search.max[i] > b.min[i])
    }
}

/// Targetable entities stored in the sections of `area`.
pub(crate) fn candidates<'a, P>(
    src: TargetSource<'a, P>,
    area: TargetArea,
    paths: TargetPaths,
) -> impl Iterator<Item = Candidate> + 'a
where
    P: Fn(i32) -> Option<(u8, u8)> + 'a,
{
    src.ents.map.iter().filter_map(move |(&id, e)| {
        // Outside the sections searched: discarded before any kind test.
        let section = section_of(e.pos);
        if !area.sections.holds(section) {
            return None;
        }
        let radius = if paths.kind_flags { pick_radius(e, src.players) } else { pick_radius_by_name(e, src.players) }?;
        // Current box (type size, baby, cubes, scale, interaction, shulker…), inflated at
        // targeting by the selection radius.
        let framed_map = |e: &Entity| crate::debug_render::framed_map(e, src.component_ids);
        let b = if paths.cached_boxes {
            e.cached_box(src.table, &src.painting, &framed_map)
        } else {
            e.bounding_box(src.table, &src.painting, framed_map(e)).1
        };
        // Same test as each targeting's own, shorter, search box: what fails it here fails there.
        if !area.overlaps(&b) {
            return None;
        }
        let same_vehicle = root_vehicle(src.ents, id) == area.camera_root;
        Some(Candidate { id, bb: rc_data::Aabb { min: b.min, max: b.max }, radius, same_vehicle, section })
    })
}

/// Root vehicle of entity `id`: up the vehicles it rides (8 levels at most), itself if it rides
/// nothing.
fn root_vehicle(ents: &Entities, mut id: i32) -> i32 {
    for _ in 0..8 {
        match ents.map.get(&id).and_then(|e| e.vehicle).filter(|v| ents.map.contains_key(v)) {
            Some(v) => id = v,
            None => break,
        }
    }
    id
}

/// Pick radius of an entity the crosshair may aim at (1 for redirectable projectiles), `None`
/// for one it may not, from the facts of its type.
fn pick_radius(e: &Entity, players: &PlayerList) -> Option<f64> {
    let pickable = match e.facts.pick {
        PickRule::Never => false,
        PickRule::Always => true,
        PickRule::UnlessSpectator => not_spectator(e, players),
        PickRule::UnlessMarker => !marker(e),
    };
    pickable.then_some(if e.facts.pick_radius_one { 1.0 } else { 0.0 })
}

/// `pick_radius` by the type name (old path, test reference).
fn pick_radius_by_name(e: &Entity, players: &PlayerList) -> Option<f64> {
    let pickable = match e.kind.as_str() {
        "player" => not_spectator(e, players),
        "armor_stand" => !marker(e),
        // Outside the `redirectable_projectile` tag: never aimed at, in flight or stuck.
        "arrow" | "spectral_arrow" | "trident" => false,
        // Entities hanging on a block: always targetable.
        "painting" | "item_frame" | "glow_item_frame" | "leash_knot" => true,
        "end_crystal" | "interaction" | "falling_block" | "tnt" | "shulker_bullet" | "fireball" | "wind_charge"
        | "breeze_wind_charge" => true,
        k if k.ends_with("boat") || k.ends_with("raft") || k.ends_with("minecart") => true,
        _ => e.class != Kind::Other,
    };
    let one = matches!(e.kind.as_str(), "fireball" | "wind_charge" | "breeze_wind_charge");
    pickable.then_some(if one { 1.0 } else { 0.0 })
}

/// A remote player shown in spectator mode by the player list is never aimed at.
fn not_spectator(e: &Entity, players: &PlayerList) -> bool {
    players.map.get(&e.uuid).is_none_or(|p| p.game_mode != 3)
}

/// Marker armor stand (flag 0x10 of data 15): no box to aim at.
fn marker(e: &Entity) -> bool {
    e.meta_byte(15) & 0x10 != 0
}
