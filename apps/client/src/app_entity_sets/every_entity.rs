//! Old paths of the entity sets, kept behind their switches (`=0`) and as the reference of the
//! proofs: every entity walked on every frame, the full tag rules for each one, new collections
//! every frame. Code as it was, only its output handed to a [`TagSink`].

use super::{EARS_TAG_RAISE, PickedTag, TagFrame, TagSink, frame_item_name, holder_name, mannequin_description};
use crate::{entities, entity_render, nametag, players, scoreboard, stack};

/// Name tags shown this frame (`RUSTCRAFT_OPT_NAME_TAG_EARLY_REJECT=0`): every entity through
/// the rules, from the farthest to the nearest.
pub(super) fn pick_tags<'a>(
    ents: &'a entities::Entities,
    players: &players::PlayerList,
    (scores, local_name): (&scoreboard::Scoreboard, &str),
    ids: &stack::ComponentIds,
    f: &TagFrame,
) -> Vec<PickedTag<'a>> {
    let (eye, pt, target, hidden, spectator) = (f.eye, f.pt, f.target, f.hud_hidden, f.spectator);
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
    list.sort_by(|a, b| b.0.total_cmp(&a.0));
    list.into_iter().map(|(dist2, id, entity)| PickedTag { dist2, id, entity, frame_name: None }).collect()
}

/// The picked tags handed to `sink` (`RUSTCRAFT_OPT_NAME_TAG_EARLY_REJECT=0`): frame names looked up
/// again, the distance of the "below name" line searched in the list.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_tags(
    sink: &mut impl TagSink,
    list: &[PickedTag<'_>],
    players: &players::PlayerList,
    dims_table: &[[f32; 4]],
    light_at: &impl Fn(i32, i32, i32) -> (u8, u8),
    f: &TagFrame,
    scores: &scoreboard::Scoreboard,
    ids: &stack::ComponentIds,
) {
    let origin = [f.eye[0].floor() as i32, f.eye[2].floor() as i32];
    let dims: Vec<[f32; 3]> = list.iter().map(|t| t.entity.dims(dims_table)).collect();
    let frame_names: Vec<Option<rc_proto::nbt::Nbt>> = list.iter().map(|t| frame_item_name(t.entity, ids)).collect();
    for ((t, d), frame_name) in list.iter().zip(dims).zip(&frame_names) {
        let (id, e) = (&t.id, t.entity);
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
        let p = e.render_pos(f.pt);
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
        let light = light_at(p[0].floor() as i32, (p[1] + f64::from(d[1])).floor() as i32, p[2].floor() as i32);
        let discrete = e.flags() & 0x02 != 0;
        // "Below name" line (`scoreboard-teams.md` § 7) under the attribute distance (10 by
        // default, fixed for non-living), name raised above.
        let dist2 = list.iter().find(|t| t.id == *id).map_or(f64::MAX, |x| x.dist2);
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
            let key = rc_text::flatten(&line, sink.lang())
                .iter()
                .map(|s| match s.style.font.as_deref() {
                    Some(f) if rc_text::object_font(f).is_some() => f,
                    _ => s.text.as_str(),
                })
                .collect::<String>();
            let line_chars = |lang: &rc_text::Lang| rc_text::styled_chars(&rc_text::flatten(&line, lang));
            sink.push(*id ^ 0x4000_0000, NameRef::Text(&key), line_chars, anchor, discrete, light);
            anchor.y += 9.0 * 1.15 * 0.025;
        }
        sink.push(*id, source, chars, anchor, discrete, light);
    }
}

/// Image context of the tags: targeted entity, hidden interface (F1), local spectator.
#[derive(Clone, Copy)]
struct TagCtx {
    picked: bool,
    hud_hidden: bool,
    spectator: bool,
}

/// Old order of the rules: distance and team before the name.
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

/// Glowing entities (`RUSTCRAFT_OPT_ENTITY_SUBSETS=0`): every entity walked, a new table.
pub(super) fn glow_colors(
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

/// Ghosts (`RUSTCRAFT_OPT_ENTITY_SUBSETS=0`): every entity walked, team names copied, a new set.
pub(super) fn ghost_set(
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
    let team_of = |e: &entities::Entity| holder_name(e, players, teams).and_then(|h| scores.members.get(&*h).cloned());
    let Some(mine) = ents.me.as_ref().and_then(team_of) else { return Default::default() };
    if scores.teams.get(&mine).is_none_or(|t| t.params.options & 2 == 0) {
        return Default::default();
    }
    invisible.filter(|e| team_of(e).as_ref() == Some(&mine)).map(|e| e.uuid).collect()
}

/// Spectator avatars (`RUSTCRAFT_OPT_PLAYER_SET_REUSE=0`): a new set every frame.
pub(super) fn spectator_set(
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

/// Players drawn upside down (`RUSTCRAFT_OPT_PLAYER_SET_REUSE=0`): a new set every frame.
pub(super) fn upside_down(players: &players::PlayerList) -> hashbrown::HashSet<u128> {
    (players.map.iter()).filter(|(_, p)| p.name == "Dinnerbone" || p.name == "Grumm").map(|(&u, _)| u).collect()
}

/// Players with ears (`RUSTCRAFT_OPT_PLAYER_SET_REUSE=0`): a new set every frame.
pub(super) fn extra_ears(players: &players::PlayerList) -> hashbrown::HashSet<u128> {
    players.map.iter().filter(|(_, p)| p.name == "deadmau5").map(|(&u, _)| u).collect()
}
