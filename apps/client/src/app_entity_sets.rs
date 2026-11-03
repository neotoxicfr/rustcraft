//! Sets derived from the entities for the renderer: name tags, glowing, ghost and spectator avatars.

use std::borrow::Cow;

use crate::render::Renderer;
use crate::{World, camera, entities, entity_render, environ, nametag, players, scoreboard, stack};
use nametag::NameRef;

mod every_entity;
#[cfg(test)]
mod tests;

/// `RUSTCRAFT_OPT_NAME_TAG_EARLY_REJECT=0`: the old name tags, every entity through the full rules
/// (distance and team before the name), frame names looked up twice, the distance of the "below
/// name" line searched in the list.
pub(crate) fn name_tag_early_reject_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_NAME_TAG_EARLY_REJECT")
}

/// `RUSTCRAFT_OPT_HOLDER_NAME_ON_STACK=0`: holder names formatted into a new string on every call.
pub(crate) fn holder_name_on_stack_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_HOLDER_NAME_ON_STACK")
}

/// `RUSTCRAFT_OPT_BELOW_NAME_KEY_CACHE=0`: the "below name" line built and flattened on every frame
/// to find its cache key (acts with `RUSTCRAFT_OPT_NAME_TAG_EARLY_REJECT=1`).
pub(crate) fn below_name_key_cache_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_BELOW_NAME_KEY_CACHE")
}

/// `RUSTCRAFT_OPT_PLAYER_SET_REUSE=0`: spectator, upside-down and eared players collected into new
/// sets on every frame.
pub(crate) fn player_set_reuse_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_PLAYER_SET_REUSE")
}

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
    below_keys: &mut BelowLineKeys,
) {
    r.tags.clear();
    r.tags.sync(scores.version);
    let Some(er) = &r.entities else { return };
    if rc_data::env_flag!("RUSTCRAFT_NOENT") {
        return;
    }
    let frame = TagFrame { eye, pt, target, hud_hidden: hidden, spectator };
    let early = name_tag_early_reject_on();
    let rules = TagRules::new(players, scores, ids, local_name);
    let picked = if early {
        pick_tags(ents, &rules, &frame, entities::entity_subsets_on())
    } else {
        every_entity::pick_tags(ents, players, (scores, local_name), ids, &frame)
    };
    if picked.is_empty() {
        return;
    }
    let view = environ::WorldView { grid: &world.grid, min_y: world.min_y, height: world.height };
    let light = |x, y, z| view.light_pair(x, y, z);
    let (right, up, _) = camera::axes(yaw, pitch);
    let queue = r.queue_handle();
    r.tags.begin(right, up);
    let mut sink = RendererTags { tags: &mut r.tags, gui: &mut r.gui, queue: &queue };
    if early {
        let keys = below_name_key_cache_on().then_some(below_keys);
        push_tags(&mut sink, &picked, &rules, &er.dims, &light, &frame, keys);
    } else {
        every_entity::push_tags(&mut sink, &picked, players, &er.dims, &light, &frame, scores, ids);
    }
}

/// What the frame's tags depend on besides the entities and the scoreboard.
#[derive(Clone, Copy)]
struct TagFrame {
    eye: [f64; 3],
    pt: f32,
    /// Aimed entity.
    target: Option<i32>,
    /// Interface hidden (F1).
    hud_hidden: bool,
    /// Local player in spectator mode.
    spectator: bool,
}

/// A name tag shown this frame.
struct PickedTag<'a> {
    /// Square of the distance to the camera.
    dist2: f64,
    id: i32,
    entity: &'a entities::Entity,
    /// Custom name of a frame's item, kept from the rules (new path; the old one looks it up again).
    frame_name: Option<rc_proto::nbt::Nbt>,
}

/// Where a frame's name tags go: the renderer, or a recorder in the proofs.
trait TagSink {
    fn lang(&self) -> &rc_text::Lang;

    /// One tag (`NameTags::push`): `chars` is called only if its layout must be redone.
    fn push(
        &mut self,
        id: i32,
        source: NameRef<'_>,
        chars: impl FnOnce(&rc_text::Lang) -> Vec<rc_text::StyledChar>,
        anchor: glam::Vec3,
        discrete: bool,
        light: (u8, u8),
    );
}

/// The renderer's name tags.
struct RendererTags<'a> {
    tags: &'a mut nametag::NameTags,
    gui: &'a mut crate::gui::Gui,
    queue: &'a wgpu::Queue,
}

impl TagSink for RendererTags<'_> {
    fn lang(&self) -> &rc_text::Lang {
        &self.gui.lang
    }

    fn push(
        &mut self,
        id: i32,
        source: NameRef<'_>,
        chars: impl FnOnce(&rc_text::Lang) -> Vec<rc_text::StyledChar>,
        anchor: glam::Vec3,
        discrete: bool,
        light: (u8, u8),
    ) {
        self.tags.push(self.gui, self.queue, id, source, chars, anchor, discrete, light);
    }
}

/// What the tag rules read besides the entity, looked up once per frame.
struct TagRules<'a> {
    players: &'a players::PlayerList,
    scores: &'a scoreboard::Scoreboard,
    ids: &'a stack::ComponentIds,
    /// Teams exist: without them no holder is looked up.
    teams: bool,
    /// Team of the local player.
    local_team: Option<&'a str>,
}

impl<'a> TagRules<'a> {
    fn new(
        players: &'a players::PlayerList,
        scores: &'a scoreboard::Scoreboard,
        ids: &'a stack::ComponentIds,
        local_name: &str,
    ) -> Self {
        let local_team = scores.team_named(local_name).map(|(n, _)| n);
        TagRules { players, scores, ids, teams: !scores.members.is_empty(), local_team }
    }

    /// Team rules (`scoreboard-teams.md` § 8): the final answer when the entity has a team.
    fn team_shows(&self, e: &entities::Entity, invisible: bool, spectator: bool) -> Option<bool> {
        let holder = holder_name(e, self.players, self.teams)?;
        let (name, t) = self.scores.team_named(&holder)?;
        let mine = self.local_team;
        let allied = mine == Some(name);
        let sees = spectator || (allied && t.params.options & 2 != 0);
        let visible = !invisible || sees;
        Some(match t.params.nametag {
            1 => false,
            2 if mine.is_some() => allied && (t.params.options & 2 != 0 || !invisible),
            3 if mine.is_some() => !allied && !invisible,
            _ => visible,
        })
    }
}

/// Tags shown this frame, from the farthest to the nearest; at equal distance in the order of the
/// entity table. With `from_subsets`, only the candidates kept by `Entities` are read.
fn pick_tags<'a>(
    ents: &'a entities::Entities,
    rules: &TagRules<'_>,
    f: &TagFrame,
    from_subsets: bool,
) -> Vec<PickedTag<'a>> {
    if from_subsets && let Some(list) = pick_tags_among_candidates(ents, rules, f) {
        return list;
    }
    by_distance(ents.map.iter().filter_map(|(&id, e)| pick_tag(id, e, rules, f)).collect())
}

/// [`pick_tags`] from the name-tag candidates (players, always-visible names) and the aimed
/// entity, without walking every entity. `None` when two tags are at the same distance: their order
/// is that of the entity table, which only the full walk knows.
fn pick_tags_among_candidates<'a>(
    ents: &'a entities::Entities,
    rules: &TagRules<'_>,
    f: &TagFrame,
) -> Option<Vec<PickedTag<'a>>> {
    let candidates = &ents.subsets.tag_candidates;
    let aimed = f.target.filter(|id| !candidates.contains(id));
    let ids = candidates.iter().copied().chain(aimed);
    let list = by_distance(ids.filter_map(|id| pick_tag(id, ents.map.get(&id)?, rules, f)).collect());
    let tie = list.windows(2).any(|w| w[0].dist2.total_cmp(&w[1].dist2).is_eq());
    (!tie).then_some(list)
}

/// From the farthest to the nearest (stable: equal distances keep their order).
fn by_distance(mut list: Vec<PickedTag<'_>>) -> Vec<PickedTag<'_>> {
    list.sort_by(|a, b| b.dist2.total_cmp(&a.dist2));
    list
}

/// Tag of entity `id` if it shows this frame.
fn pick_tag<'a>(id: i32, e: &'a entities::Entity, rules: &TagRules<'_>, f: &TagFrame) -> Option<PickedTag<'a>> {
    let picked = f.target == Some(id);
    // Every rule of `tag_shown` needs the aimed entity, an always-visible name or a player: the
    // others (most mobs) leave before the distance, the frame item and the team lookups.
    if !(picked || e.meta_bool(3) || e.kind == "player") {
        return None;
    }
    let p = e.render_pos(f.pt);
    let dist2: f64 = (0..3).map(|i| (p[i] - f.eye[i]).powi(2)).sum();
    let mut frame_name = None;
    let frame_named = || {
        frame_name = frame_item_name(e, rules.ids);
        frame_name.is_some()
    };
    let ctx = TagCtx { picked, hud_hidden: f.hud_hidden, spectator: f.spectator };
    let shown = tag_shown(e, dist2, ctx, frame_named, |invisible| rules.team_shows(e, invisible, f.spectator));
    shown.then_some(PickedTag { dist2, id, entity: e, frame_name })
}

/// The picked tags handed to `sink`, each with its "below name" line; `below_keys`: keys of these
/// lines kept between frames (`None`: built on every frame).
fn push_tags(
    sink: &mut impl TagSink,
    picked: &[PickedTag<'_>],
    rules: &TagRules<'_>,
    dims: &[[f32; 4]],
    light_at: &impl Fn(i32, i32, i32) -> (u8, u8),
    f: &TagFrame,
    mut below_keys: Option<&mut BelowLineKeys>,
) {
    let origin = [f.eye[0].floor() as i32, f.eye[2].floor() as i32];
    if let Some(keys) = below_keys.as_deref_mut() {
        keys.begin(BelowLineStamp::of(rules, sink.lang()));
    }
    for t in picked {
        let e = t.entity;
        let profile = if e.kind == "player" { rules.players.name(e.uuid) } else { None };
        let source = tag_source(e, t.frame_name.as_ref(), profile);
        let chars = |lang: &rc_text::Lang| {
            // Formatting by the holder's team (`scoreboard-teams.md` § 4).
            let name = match rules.teams.then(|| entity_holder(e, profile)).flatten() {
                Some(h) => rules.scores.decorate(&h, source_component(source)),
                None => source_component(source),
            };
            rc_text::styled_chars(&rc_text::flatten(&name, lang))
        };
        let d = e.dims(dims);
        let p = e.render_pos(f.pt);
        let mut anchor = glam::Vec3::new(
            (p[0] - f64::from(origin[0])) as f32,
            (p[1] + f64::from(d[2]) + 0.5 + entity_render::crouch_offset(e)) as f32,
            (p[2] - f64::from(origin[1])) as f32,
        );
        // "deadmau5" player (ears): the two lines raised by 10 px of the text.
        if profile == Some("deadmau5") {
            anchor.y += EARS_TAG_RAISE;
        }
        // Light at the entity's probe point (eyes).
        let light = light_at(p[0].floor() as i32, (p[1] + f64::from(d[1])).floor() as i32, p[2].floor() as i32);
        let discrete = e.flags() & 0x02 != 0;
        // "Below name" line (`scoreboard-teams.md` § 7) under the attribute distance (10 by
        // default, fixed for non-living), name raised above.
        if t.dist2 < e.below_dist * e.below_dist {
            let lang = sink.lang();
            let key = match below_keys.as_deref_mut() {
                // A mannequin's line comes from its own data, which no stamp follows.
                Some(keys) if e.kind != "mannequin" => keys.key(e, || below_line(e, rules, profile), lang).map(Cow::Borrowed),
                _ => below_line(e, rules, profile).map(|line| Cow::Owned(below_line_key(&line, lang))),
            };
            if let Some(key) = key {
                let line_chars = |lang: &rc_text::Lang| {
                    let line = below_line(e, rules, profile).unwrap_or_else(|| rc_text::Component::text(""));
                    rc_text::styled_chars(&rc_text::flatten(&line, lang))
                };
                sink.push(t.id ^ 0x4000_0000, NameRef::Text(&key), line_chars, anchor, discrete, light);
                anchor.y += 9.0 * 1.15 * 0.025;
            }
        }
        sink.push(t.id, source, chars, anchor, discrete, light);
    }
    if let Some(keys) = below_keys {
        keys.end();
    }
}

/// Origin of a tag's name: the item's name for a frame, the profile name for a player, the
/// custom name, otherwise the translated kind.
fn tag_source<'a>(
    e: &'a entities::Entity,
    frame_name: Option<&'a rc_proto::nbt::Nbt>,
    profile: Option<&'a str>,
) -> NameRef<'a> {
    match e.meta.get(&2) {
        _ if let Some(n) = frame_name => NameRef::Nbt(n),
        _ if e.kind == "player" => NameRef::Text(profile.unwrap_or_default()),
        Some(rc_proto_v777::entity::Meta::Text(Some(n))) => NameRef::Nbt(n),
        _ => NameRef::Type(&e.kind),
    }
}

/// Component of a tag's name.
fn source_component(source: NameRef<'_>) -> rc_text::Component {
    match source {
        NameRef::Text(s) => rc_text::Component::text(s),
        NameRef::Nbt(n) => rc_text::from_nbt(n).unwrap_or_else(|| rc_text::Component::text("")),
        NameRef::Type(k) => rc_text::Component::translate(format!("entity.minecraft.{k}"), vec![]),
    }
}

/// "Below name" line of a tag: a mannequin's description, otherwise the holder's score in the
/// objective of slot 2.
fn below_line(e: &entities::Entity, rules: &TagRules<'_>, profile: Option<&str>) -> Option<rc_text::Component> {
    match e.kind.as_str() {
        "mannequin" => mannequin_description(e),
        _ if rules.scores.slots[2].is_some() => entity_holder(e, profile).and_then(|h| rules.scores.below_name(&h)),
        _ => None,
    }
}

/// Cache key of a line's text: an object counts there by its font (sprite, profile), not by its
/// carrier character, common to all the objects.
fn below_line_key(line: &rc_text::Component, lang: &rc_text::Lang) -> String {
    rc_text::flatten(line, lang)
        .iter()
        .map(|s| match s.style.font.as_deref() {
            Some(f) if rc_text::object_font(f).is_some() => f,
            _ => s.text.as_str(),
        })
        .collect()
}

/// What every "below name" line except a mannequin's is made of: the scores of the slot, the
/// player profiles (holder names) and the text context of the flattening.
#[derive(Clone, Copy, PartialEq)]
struct BelowLineStamp {
    scores: players::Revision,
    players: players::Revision,
    lang: u64,
    keybinds: u64,
}

impl BelowLineStamp {
    fn of(rules: &TagRules<'_>, lang: &rc_text::Lang) -> Self {
        BelowLineStamp {
            scores: rules.scores.below_name_revision(),
            players: rules.players.revision(),
            lang: lang.generation,
            keybinds: rc_text::keybinds_generation(),
        }
    }
}

/// Keys of the "below name" lines kept between frames (`RUSTCRAFT_OPT_BELOW_NAME_KEY_CACHE`):
/// building a line and flattening it to compare with the kept layout costs several allocations per
/// tag and per frame, while the key only changes with its stamp.
#[derive(Default)]
pub(crate) struct BelowLineKeys {
    stamp: Option<BelowLineStamp>,
    frame: u64,
    /// Entity id → key of its line.
    lines: hashbrown::HashMap<i32, KeptBelowLine>,
    /// Lines built (test count).
    #[cfg(test)]
    built: usize,
}

struct KeptBelowLine {
    /// Holder of the line (a new entity under the same id is another holder: a player is named by
    /// its profile, another kind by its UUID).
    uuid: u128,
    kind_id: i32,
    /// Last frame that read it.
    frame: u64,
    /// `None`: the entity has no line.
    key: Option<String>,
}

impl BelowLineKeys {
    fn begin(&mut self, stamp: BelowLineStamp) {
        self.frame += 1;
        if self.stamp != Some(stamp) {
            self.stamp = Some(stamp);
            self.lines.clear();
        }
    }

    /// Key of the line of `e` (`line` builds it when not kept), `None` without a line.
    fn key(
        &mut self,
        e: &entities::Entity,
        line: impl FnOnce() -> Option<rc_text::Component>,
        lang: &rc_text::Lang,
    ) -> Option<&str> {
        let frame = self.frame;
        let kept = self.lines.get(&e.id).is_some_and(|k| (k.uuid, k.kind_id) == (e.uuid, e.kind_id));
        if !kept {
            #[cfg(test)]
            {
                self.built += 1;
            }
            let key = line().map(|line| below_line_key(&line, lang));
            self.lines.insert(e.id, KeptBelowLine { uuid: e.uuid, kind_id: e.kind_id, frame, key });
        }
        let k = self.lines.get_mut(&e.id)?;
        k.frame = frame;
        k.key.as_deref()
    }

    /// Lines of the entities whose tag was not shown this frame are forgotten.
    fn end(&mut self) {
        let frame = self.frame;
        self.lines.retain(|_, k| k.frame == frame);
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
/// - living: player, or the same name conditions as the non-living; attribute distance,
///   crouching within 32; team rules (`team`, depending on invisibility) otherwise visible,
///   interface visible and without a passenger.
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
            let flags = e.flags();
            // The name first: the team rules can only hide a tag, and finding the team formats
            // the holder name.
            if !(e.kind == "player" || by_name) || d2 >= e.tag_dist * e.tag_dist || (flags & 0x02 != 0 && d2 >= 1024.0)
            {
                return false;
            }
            let invisible = flags & 0x20 != 0;
            match team(invisible) {
                Some(v) => v,
                None => !c.hud_hidden && (!invisible || c.spectator) && e.passengers.is_empty(),
            }
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
/// (that of their team, otherwise white), written into `out`.
pub(crate) fn fill_glow_colors(
    out: &mut hashbrown::HashMap<u128, u32>,
    ents: &entities::Entities,
    players: &players::PlayerList,
    scores: &scoreboard::Scoreboard,
) {
    if !entities::entity_subsets_on() {
        *out = every_entity::glow_colors(ents, players, scores);
        return;
    }
    out.clear();
    let teams = !scores.members.is_empty();
    let kept = ents.subsets.glowing.iter().filter_map(|id| ents.map.get(id));
    for e in kept.chain(ents.me.as_ref()).filter(|e| entities::EntitySubsets::is_glowing(e)) {
        let rgb = holder_name(e, players, teams).and_then(|h| scores.team_rgb(&h)).unwrap_or(0xff_ffff);
        out.insert(e.uuid, rgb);
    }
}

/// Invisible entities that the local player sees as a ghost: all of them in spectator, otherwise those of
/// their team if it sees its invisible allies (option 0x02); written into `out`.
pub(crate) fn fill_ghost_set(
    out: &mut hashbrown::HashSet<u128>,
    ents: &entities::Entities,
    players: &players::PlayerList,
    (scores, spectator): (&scoreboard::Scoreboard, bool),
) {
    if !entities::entity_subsets_on() {
        *out = every_entity::ghost_set(ents, players, scores, spectator);
        return;
    }
    out.clear();
    let kept = ents.subsets.invisible.iter().filter_map(|id| ents.map.get(id));
    let invisible = kept.chain(ents.me.as_ref()).filter(|e| entities::EntitySubsets::is_invisible(e));
    if spectator {
        out.extend(invisible.map(|e| e.uuid));
        return;
    }
    let teams = !scores.members.is_empty();
    let team_of = |e: &entities::Entity| holder_name(e, players, teams).and_then(|h| scores.members.get(&*h));
    let Some(mine) = ents.me.as_ref().and_then(team_of) else { return };
    if scores.teams.get(mine).is_none_or(|t| t.params.options & 2 == 0) {
        return;
    }
    out.extend(invisible.filter(|e| team_of(e) == Some(mine)).map(|e| e.uuid));
}

/// Inputs the player sets were built from: player list, local player, its spectator mode.
pub(crate) type PlayerSetsStamp = (players::Revision, Option<u128>, bool);

/// Player sets of the entity renderer — `[spectators, upside_down, extra_ears]`: avatars in
/// spectator mode (mode 3 in the player list, and the local player according to its own mode),
/// "Dinnerbone" and "Grumm" drawn upside down, "deadmau5" with ears. Refilled in place only when
/// `stamp` (kept by the renderer) no longer matches.
pub(crate) fn refresh_player_sets(
    [spectators, upside_down, extra_ears]: [&mut hashbrown::HashSet<u128>; 3],
    stamp: &mut Option<PlayerSetsStamp>,
    ents: &entities::Entities,
    players: &players::PlayerList,
    local_spectator: bool,
) {
    if !player_set_reuse_on() {
        *spectators = every_entity::spectator_set(ents, players, local_spectator);
        *upside_down = every_entity::upside_down(players);
        *extra_ears = every_entity::extra_ears(players);
        return;
    }
    let now = (players.revision(), ents.me.as_ref().map(|m| m.uuid), local_spectator);
    if *stamp == Some(now) {
        return;
    }
    *stamp = Some(now);
    for set in [&mut *spectators, &mut *upside_down, &mut *extra_ears] {
        set.clear();
    }
    for (&uuid, p) in &players.map {
        if p.game_mode == 3 {
            spectators.insert(uuid);
        }
        if matches!(p.name.as_str(), "Dinnerbone" | "Grumm") {
            upside_down.insert(uuid);
        }
        if p.name == "deadmau5" {
            extra_ears.insert(uuid);
        }
    }
    if let Some(me) = ents.me.as_ref() {
        if local_spectator {
            spectators.insert(me.uuid);
        } else {
            spectators.remove(&me.uuid);
        }
    }
}

/// Score holder of an entity: profile name of a player, UUID as text otherwise; `None` without
/// teams (nothing to format).
pub(crate) fn holder_name<'a>(
    e: &entities::Entity,
    players: &'a players::PlayerList,
    teams: bool,
) -> Option<HolderName<'a>> {
    if !teams {
        return None;
    }
    let profile = if e.kind == "player" { players.name(e.uuid) } else { None };
    entity_holder(e, profile)
}

/// Holder of `e` whose profile name (players) is already known.
fn entity_holder<'a>(e: &entities::Entity, profile: Option<&'a str>) -> Option<HolderName<'a>> {
    if !holder_name_on_stack_on() {
        return old_holder_name(e, profile).map(HolderName::Formatted);
    }
    if e.kind == "player" {
        return profile.map(HolderName::Profile);
    }
    Some(HolderName::Uuid(UuidText::new(e.uuid)))
}

/// Old path (`RUSTCRAFT_OPT_HOLDER_NAME_ON_STACK=0`): a copied name, or two strings formatted.
fn old_holder_name(e: &entities::Entity, profile: Option<&str>) -> Option<String> {
    if e.kind == "player" {
        return profile.map(str::to_owned);
    }
    let h = format!("{:032x}", e.uuid);
    Some(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

/// A score holder's name, read as `&str`.
pub(crate) enum HolderName<'a> {
    /// Profile name of a player, borrowed from the player list.
    Profile(&'a str),
    /// UUID of another entity, written in a fixed buffer (no allocation).
    Uuid(UuidText),
    /// Old path: a new string.
    Formatted(String),
}

impl std::ops::Deref for HolderName<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        match self {
            HolderName::Profile(s) => s,
            HolderName::Uuid(u) => u.as_str(),
            HolderName::Formatted(s) => s,
        }
    }
}

/// A UUID written like the game, lowercase hexadecimal in 8-4-4-4-12 groups.
pub(crate) struct UuidText([u8; 36]);

impl UuidText {
    pub(crate) fn new(uuid: u128) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = [b'-'; 36];
        let mut digit = 0;
        for (i, slot) in out.iter_mut().enumerate() {
            if matches!(i, 8 | 13 | 18 | 23) {
                continue;
            }
            *slot = HEX[((uuid >> (124 - 4 * digit)) & 0xf) as usize];
            digit += 1;
        }
        UuidText(out)
    }

    pub(crate) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("hexadecimal digits and hyphens are ASCII")
    }
}
