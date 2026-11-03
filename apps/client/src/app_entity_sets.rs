//! Sets derived from the entities for the renderer: name tags, glowing, ghost and spectator avatars.

use crate::render::Renderer;
use crate::{World, camera, entities, entity_render, environ, nametag, players, scoreboard, stack};

/// Name tags (`entities.md` part 2 § 1.8), from the farthest to the nearest.
#[allow(clippy::too_many_arguments)]
pub(crate) fn name_tags(
    r: &mut Renderer,
    ents: &entities::Entities,
    players: &players::PlayerList,
    world: &World,
    eye: [f64; 3],
    (yaw, pitch): (f32, f32),
    pt: f32,
    hidden: bool,
    target: Option<i32>,
    (scores, local_name, spectator): (&scoreboard::Scoreboard, &str, bool),
    ids: &stack::ComponentIds,
) {
    r.tags.clear();
    r.tags.sync(scores.version);
    let Some(er) = &r.entities else { return };
    if rc_data::env_flag!("RUSTCRAFT_NOENT") {
        return;
    }
    let mut list: Vec<(f64, i32, &entities::Entity)> = ents
        .map
        .iter()
        .filter_map(|(&id, e)| {
            let p = e.render_pos(pt);
            let d2: f64 = (0..3).map(|i| (p[i] - eye[i]).powi(2)).sum();
            let frame_named = || frame_item_name(e, ids).is_some();
            // Team rules (`scoreboard-teams.md` § 8): final result if the entity has a team.
            let team = |invisible: bool| {
                let holder = holder_name(e, players, !scores.members.is_empty());
                let (name, t) = holder.as_deref().and_then(|h| scores.team_named(h))?;
                let mine = scores.team_named(local_name).map(|(n, _)| n);
                let allied = mine == Some(name);
                let sees = spectator || (allied && t.params.options & 2 != 0);
                let visible = !invisible || sees;
                Some(match t.params.nametag {
                    1 => false,
                    2 if mine.is_some() => allied && (t.params.options & 2 != 0 || !invisible),
                    3 if mine.is_some() => !allied && !invisible,
                    _ => visible,
                })
            };
            let ctx = TagCtx { picked: target == Some(id), hud_hidden: hidden, spectator };
            tag_shown(e, d2, ctx, frame_named, team).then_some((d2, id, e))
        })
        .collect();
    if list.is_empty() {
        return;
    }
    list.sort_by(|a, b| b.0.total_cmp(&a.0));
    let view = environ::WorldView { grid: &world.grid, min_y: world.min_y, height: world.height };
    let origin = [eye[0].floor() as i32, eye[2].floor() as i32];
    let (right, up, _) = camera::axes(yaw, pitch);
    let q = r.queue_handle();
    r.tags.begin(right, up);
    let dims: Vec<[f32; 3]> = list.iter().map(|(_, _, e)| e.dims(&er.dims)).collect();
    let frame_names: Vec<Option<rc_proto::nbt::Nbt>> = list.iter().map(|(_, _, e)| frame_item_name(e, ids)).collect();
    for (((_, id, e), d), frame_name) in list.iter().zip(dims).zip(&frame_names) {
        use nametag::NameRef;
        let source = match e.meta.get(&2) {
            _ if let Some(n) = frame_name => NameRef::Nbt(n),
            _ if e.kind == "player" => NameRef::Text(players.name(e.uuid).unwrap_or_default()),
            Some(rc_proto_v777::entity::Meta::Text(Some(n))) => NameRef::Nbt(n),
            _ => NameRef::Type(&e.kind),
        };
        let chars = |lang: &rc_text::Lang| {
            let name = match source {
                NameRef::Text(s) => rc_text::Component::text(s),
                NameRef::Nbt(n) => rc_text::from_nbt(n).unwrap_or_else(|| rc_text::Component::text("")),
                NameRef::Type(k) => rc_text::Component::translate(format!("entity.minecraft.{k}"), vec![]),
            };
            // Formatting by the holder's team (`scoreboard-teams.md` § 4).
            let name = match holder_name(e, players, !scores.members.is_empty()) {
                Some(h) => scores.decorate(&h, name),
                None => name,
            };
            rc_text::styled_chars(&rc_text::flatten(&name, lang))
        };
        let p = e.render_pos(pt);
        let mut anchor = glam::Vec3::new(
            (p[0] - f64::from(origin[0])) as f32,
            (p[1] + f64::from(d[2]) + 0.5 + entity_render::crouch_offset(e)) as f32,
            (p[2] - f64::from(origin[1])) as f32,
        );
        // "deadmau5" player (ears): the two lines raised by 10 px of the text.
        if e.kind == "player" && players.name(e.uuid) == Some("deadmau5") {
            anchor.y += EARS_TAG_RAISE;
        }
        // Light at the entity's probe point (eyes).
        let light = view.light_pair(p[0].floor() as i32, (p[1] + f64::from(d[1])).floor() as i32, p[2].floor() as i32);
        let discrete = e.flags() & 0x02 != 0;
        // "Below name" line (`scoreboard-teams.md` § 7) under the attribute distance (10 by
        // default, fixed for non-living), name raised above.
        let dist2 = list.iter().find(|(_, i, _)| i == id).map_or(f64::MAX, |x| x.0);
        // Mannequin: its description replaces the scoreboard line.
        let below = || match e.kind.as_str() {
            "mannequin" => mannequin_description(e),
            _ if scores.slots[2].is_some() => holder_name(e, players, true).and_then(|h| scores.below_name(&h)),
            _ => None,
        };
        if dist2 < e.below_dist * e.below_dist
            && let Some(line) = below()
        {
            // Cache key of the text: an object counts there by its font (sprite, profile), not by
            // its carrier character, common to all the objects.
            let key = rc_text::flatten(&line, &r.gui.lang)
                .iter()
                .map(|s| match s.style.font.as_deref() {
                    Some(f) if rc_text::object_font(f).is_some() => f,
                    _ => s.text.as_str(),
                })
                .collect::<String>();
            let line_chars = |lang: &rc_text::Lang| rc_text::styled_chars(&rc_text::flatten(&line, lang));
            r.tags.push(&mut r.gui, &q, *id ^ 0x4000_0000, NameRef::Text(&key), line_chars, anchor, discrete, light);
            anchor.y += 9.0 * 1.15 * 0.025;
        }
        r.tags.push(&mut r.gui, &q, *id, source, chars, anchor, discrete, light);
    }
}

/// Line below a mannequin's name: its description (value 19), the default translated label
/// as long as the server has not sent one, nothing if hidden.
fn mannequin_description(e: &entities::Entity) -> Option<rc_text::Component> {
    match e.meta.get(&19) {
        Some(rc_proto_v777::entity::Meta::Text(Some(n))) => rc_text::from_nbt(n),
        Some(rc_proto_v777::entity::Meta::Text(None)) => None,
        _ => Some(rc_text::Component::translate("entity.minecraft.mannequin.label", vec![])),
    }
}

/// Offset of the name of a player with ears ("deadmau5"): 10 px of the text at scale 0.025.
const EARS_TAG_RAISE: f32 = 10.0 * 0.025;

/// Image context of the tags: targeted entity, hidden interface (F1), local spectator.
#[derive(Clone, Copy)]
struct TagCtx {
    picked: bool,
    hud_hidden: bool,
    spectator: bool,
}

/// Name tag retained at `d2` (square of the distance to the camera), rules of the kind:
/// - armor stand: only "always visible name", at the distance of its attribute (the
///   invisible holograms stay displayed, even with the interface hidden);
/// - frame: named item, aimed frame, interface visible;
/// - non-living: always visible name, or aimed custom name, at 64 blocks;
/// - living: attribute distance, crouching within 32, team rules (`team`, depending on
///   invisibility) otherwise visible, interface visible and without a passenger; then player, or the same
///   name conditions as the non-living.
fn tag_shown(
    e: &entities::Entity,
    d2: f64,
    c: TagCtx,
    frame_named: impl FnOnce() -> bool,
    team: impl FnOnce(bool) -> Option<bool>,
) -> bool {
    let named = matches!(e.meta.get(&2), Some(rc_proto_v777::entity::Meta::Text(Some(_))));
    let by_name = e.meta_bool(3) || (named && c.picked);
    match e.kind.as_str() {
        "armor_stand" => d2 < e.tag_dist * e.tag_dist && e.meta_bool(3),
        "item_frame" | "glow_item_frame" => d2 < 64.0 * 64.0 && !c.hud_hidden && c.picked && frame_named(),
        _ if e.class == entities::Kind::Other => d2 < 64.0 * 64.0 && by_name,
        _ => {
            if d2 >= e.tag_dist * e.tag_dist || (e.flags() & 0x02 != 0 && d2 >= 1024.0) {
                return false;
            }
            let invisible = e.flags() & 0x20 != 0;
            let living = match team(invisible) {
                Some(v) => v,
                None => !c.hud_hidden && (!invisible || c.spectator) && e.passengers.is_empty(),
            };
            living && (e.kind == "player" || by_name)
        }
    }
}

/// Custom name of a frame's item (the game then displays the item's name, not the frame's).
fn frame_item_name(e: &entities::Entity, ids: &stack::ComponentIds) -> Option<rc_proto::nbt::Nbt> {
    if !matches!(e.kind.as_str(), "item_frame" | "glow_item_frame") {
        return None;
    }
    match e.meta.get(&9) {
        Some(rc_proto_v777::entity::Meta::Item(Some(s))) if s.count > 0 => ids.custom_name_nbt(s),
        _ => None,
    }
}

/// Glowing entities (flag 6: Glowing effect, spectral arrow…): identity → outline colour
/// (that of their team, otherwise white).
pub(crate) fn glow_colors(
    ents: &entities::Entities,
    players: &players::PlayerList,
    scores: &scoreboard::Scoreboard,
) -> hashbrown::HashMap<u128, u32> {
    let teams = !scores.members.is_empty();
    ents.map
        .values()
        .chain(ents.me.as_ref())
        .filter(|e| e.flags() & 0x40 != 0)
        .map(|e| {
            let rgb = holder_name(e, players, teams).and_then(|h| scores.team_rgb(&h)).unwrap_or(0xff_ffff);
            (e.uuid, rgb)
        })
        .collect()
}

/// Invisible entities that the local player sees as a ghost: all of them in spectator, otherwise those of
/// their team if it sees its invisible allies (option 0x02).
pub(crate) fn ghost_set(
    ents: &entities::Entities,
    players: &players::PlayerList,
    scores: &scoreboard::Scoreboard,
    spectator: bool,
) -> hashbrown::HashSet<u128> {
    let invisible = ents.map.values().chain(ents.me.as_ref()).filter(|e| e.flags() & 0x20 != 0);
    if spectator {
        return invisible.map(|e| e.uuid).collect();
    }
    let teams = !scores.members.is_empty();
    let team_of = |e: &entities::Entity| holder_name(e, players, teams).and_then(|h| scores.members.get(&h).cloned());
    let Some(mine) = ents.me.as_ref().and_then(team_of) else { return Default::default() };
    if scores.teams.get(&mine).is_none_or(|t| t.params.options & 2 == 0) {
        return Default::default();
    }
    invisible.filter(|e| team_of(e).as_ref() == Some(&mine)).map(|e| e.uuid).collect()
}

/// Avatars in spectator mode: mode 3 in the player list, and the local player according to their
/// own mode.
pub(crate) fn spectator_set(
    ents: &entities::Entities,
    players: &players::PlayerList,
    local_spectator: bool,
) -> hashbrown::HashSet<u128> {
    let mut set: hashbrown::HashSet<u128> =
        players.map.iter().filter(|(_, p)| p.game_mode == 3).map(|(&u, _)| u).collect();
    if let Some(me) = ents.me.as_ref() {
        if local_spectator {
            set.insert(me.uuid);
        } else {
            set.remove(&me.uuid);
        }
    }
    set
}

/// Score holder of an entity: profile name of a player, UUID as text otherwise; `None` without
/// teams (nothing to format, no allocation).
pub(crate) fn holder_name(e: &entities::Entity, players: &players::PlayerList, teams: bool) -> Option<String> {
    if !teams {
        return None;
    }
    if e.kind == "player" {
        return players.name(e.uuid).map(str::to_owned);
    }
    let h = format!("{:032x}", e.uuid);
    Some(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug_render::tests::test_entity;
    use rc_proto_v777::entity::Meta;

    /// Mannequin: default label, description sent, nothing if hidden.
    #[test]
    fn mannequin_shows_its_description_below_the_name() {
        let mut e = test_entity("mannequin");
        let label = rc_text::Component::translate("entity.minecraft.mannequin.label", vec![]);
        assert_eq!(mannequin_description(&e), Some(label));
        e.meta.insert(19, Meta::Text(Some(rc_proto::nbt::Nbt::String("Vendor".into()))));
        assert_eq!(mannequin_description(&e), Some(rc_text::Component::text("Vendor")));
        e.meta.insert(19, Meta::Text(None));
        assert_eq!(mannequin_description(&e), None);
    }

    const CTX: TagCtx = TagCtx { picked: false, hud_hidden: false, spectator: false };

    fn shown(e: &entities::Entity, d2: f64, c: TagCtx) -> bool {
        tag_shown(e, d2, c, || false, |_| None)
    }

    /// Hologram: invisible armor stand with always-visible name -> displayed (even with the interface
    /// hidden); without "visible name", never, even aimed and named.
    #[test]
    fn invisible_armor_stand_hologram_keeps_its_name() {
        let mut e = test_entity("armor_stand");
        e.meta.insert(0, Meta::Byte(0x20));
        e.meta.insert(2, Meta::Text(Some(rc_proto::nbt::Nbt::String("Welcome".into()))));
        e.meta.insert(3, Meta::Bool(true));
        assert!(shown(&e, 100.0, CTX));
        assert!(shown(&e, 100.0, TagCtx { hud_hidden: true, ..CTX }));
        // Team that would hide the name: no effect on the armor stand.
        assert!(tag_shown(&e, 100.0, CTX, || false, |_| Some(false)));
        e.meta.insert(3, Meta::Bool(false));
        assert!(!shown(&e, 100.0, TagCtx { picked: true, ..CTX }));
    }

    /// Tag distances of living entities: attribute `name_tag_distance` (64 by default); the non-
    /// living stay at 64.
    #[test]
    fn name_tag_distance_comes_from_attribute() {
        let mut zombie = test_entity("zombie");
        zombie.meta.insert(3, Meta::Bool(true));
        assert!(shown(&zombie, 63.9 * 63.9, CTX) && !shown(&zombie, 64.0 * 64.0, CTX));
        zombie.tag_dist = 100.0;
        assert!(shown(&zombie, 90.0 * 90.0, CTX));
        zombie.tag_dist = 8.0;
        assert!(!shown(&zombie, 9.0 * 9.0, CTX));
        // Invisible living entity without a team: hidden (except spectator); hidden interface: hidden.
        zombie.tag_dist = 64.0;
        zombie.meta.insert(0, Meta::Byte(0x20));
        assert!(!shown(&zombie, 4.0, CTX) && shown(&zombie, 4.0, TagCtx { spectator: true, ..CTX }));
        let mut cart = test_entity("minecart");
        cart.meta.insert(3, Meta::Bool(true));
        cart.tag_dist = 8.0;
        assert!(shown(&cart, 60.0 * 60.0, CTX));
    }
}
