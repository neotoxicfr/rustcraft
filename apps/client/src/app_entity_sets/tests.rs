//! Proofs of the entity sets against the old code (`every_entity.rs`): name tags (order, text,
//! anchor, "below name" lines), glowing and ghost sets, rare kinds, player sets and holder names,
//! over a scene that changes on every frame; rules of the tags; micro-benches.

use super::*;
use crate::debug_render::tests::test_entity;
use crate::entities::{Entities, EntitySubsets};
use rc_client::EntityEvent;
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::players::{InfoEntry, InfoUpdate};
use rc_proto_v777::scores::{ScoreEvent, TeamParams};

/// Fixed generator (xorshift 64): the scene depends only on the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    /// In [-r, r), with fractions (distances never tie by chance).
    fn coord(&mut self, r: f64) -> f64 {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * r
    }
}

/// Kinds of the scene, by type id.
const KINDS: [&str; 13] = [
    "player",
    "zombie",
    "cow",
    "armor_stand",
    "item_frame",
    "glow_item_frame",
    "minecart",
    "mannequin",
    "guardian",
    "fishing_bobber",
    "enderman",
    "elder_guardian",
    "illusioner",
];

const NAMES: [&str; 6] = ["deadmau5", "Dinnerbone", "Grumm", "Alex", "Steve", "Zoé"];

/// Light of the fake world, from the probe position.
fn light(x: i32, y: i32, z: i32) -> (u8, u8) {
    ((x & 15) as u8, ((y ^ z) & 15) as u8)
}

/// A tag as the renderer receives it.
#[derive(Debug, PartialEq)]
struct Recorded {
    id: i32,
    source: String,
    text: Vec<rc_text::StyledChar>,
    anchor: [u32; 3],
    discrete: bool,
    light: (u8, u8),
}

/// Records every tag with its text (the layout is always built here).
#[derive(Default)]
struct Recorder {
    lang: rc_text::Lang,
    tags: Vec<Recorded>,
}

impl TagSink for Recorder {
    fn lang(&self) -> &rc_text::Lang {
        &self.lang
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
        let text = chars(&self.lang);
        let anchor = anchor.to_array().map(f32::to_bits);
        self.tags.push(Recorded { id, source: format!("{source:?}"), text, anchor, discrete, light });
    }
}

/// Benches: tags whose layout is kept (the usual frame), only their origin read.
#[derive(Default)]
struct KeptLayouts {
    lang: rc_text::Lang,
    seen: usize,
}

impl TagSink for KeptLayouts {
    fn lang(&self) -> &rc_text::Lang {
        &self.lang
    }

    fn push(
        &mut self,
        id: i32,
        source: NameRef<'_>,
        _chars: impl FnOnce(&rc_text::Lang) -> Vec<rc_text::StyledChar>,
        anchor: glam::Vec3,
        _discrete: bool,
        _light: (u8, u8),
    ) {
        let text = match source {
            NameRef::Text(s) | NameRef::Type(s) => s.len(),
            NameRef::Nbt(_) => 1,
        };
        self.seen += text + id as usize + anchor.x as usize;
    }
}

/// Every kind of tag holder (players with profiles, named and unnamed mobs, frames with a named
/// item, armor stands, minecarts, mannequins, guardians, bobbers), teams with every name-tag rule,
/// a "below name" objective; [`Scene::step`] changes it on every frame.
struct Scene {
    ents: Entities,
    players: players::PlayerList,
    scores: scoreboard::Scoreboard,
    ids: stack::ComponentIds,
    custom_name: Option<u16>,
    rng: Rng,
    next_id: i32,
    frame: TagFrame,
    local_name: String,
    /// Holder names of the scene (team members, scores).
    holders: Vec<String>,
    /// Two armor stands at the same place, for a few frames.
    tie_stands: Vec<i32>,
}

const LOCAL_ID: i32 = 9_000;

impl Scene {
    fn new(data: &rc_data::VersionData, seed: u64, mobs: usize) -> Scene {
        let ids = stack::ComponentIds::new(data);
        let custom_name = ids.of("custom_name");
        let ents = Entities {
            type_names: KINDS.iter().map(|k| format!("minecraft:{k}")).collect(),
            local: LOCAL_ID,
            dims: (0..KINDS.len()).map(|i| [0.6, 1.0 + i as f32 * 0.1, 0.8 + i as f32 * 0.05, 1.1 + i as f32 * 0.1]).collect(),
            ..Default::default()
        };
        let frame = TagFrame { eye: [0.3, 64.7, 0.1], pt: 0.25, target: None, hud_hidden: false, spectator: false };
        let mut s = Scene {
            ents,
            players: Default::default(),
            scores: Default::default(),
            ids,
            custom_name,
            rng: Rng(seed),
            next_id: 1,
            frame,
            local_name: "Local".into(),
            holders: Vec::new(),
            tie_stands: Vec::new(),
        };
        s.ents.spawn_me(0xffff, 0, [0.0, 64.0, 0.0], 0.0);
        s.profile(0xffff, "Local", 0);
        for _ in 0..mobs {
            s.spawn_random();
        }
        s.scores.apply(ScoreEvent::Objective {
            name: "hp".into(),
            mode: 0,
            display: Some(Nbt::String("HP".into())),
            hearts: false,
            format: None,
        });
        s.scores.apply(ScoreEvent::Display { slot: 2, objective: "hp".into() });
        for (value, holder) in s.holders.clone().into_iter().enumerate() {
            s.scores.apply(ScoreEvent::Score { holder, objective: "hp".into(), value: value as i32, display: None, format: None });
        }
        for team in ["red", "blue", "green"] {
            s.team_event(team, 0);
        }
        s
    }

    fn profile(&mut self, uuid: u128, name: &str, game_mode: i32) {
        let entry = InfoEntry {
            uuid,
            profile: Some((name.to_owned(), Vec::new())),
            game_mode: Some(game_mode),
            ..Default::default()
        };
        self.players.apply(InfoUpdate { actions: 0xff, entries: vec![entry] });
    }

    /// A new entity of a random kind, with random data; a player gets a profile (but not always).
    fn spawn_random(&mut self) {
        let kind = self.rng.below(KINDS.len());
        let id = self.next_id;
        self.next_id += 1;
        self.spawn(id, kind);
    }

    fn spawn(&mut self, id: i32, kind: usize) {
        let uuid = 0x1234_5678_9abc_def0_0000_0000_0000_0000u128 | (id as u128) << 20 | self.rng.below(1000) as u128;
        let pos = [self.rng.coord(50.0), 64.0 + self.rng.coord(6.0), self.rng.coord(50.0)];
        let add = rc_proto_v777::entity::AddEntity {
            id,
            uuid,
            kind: kind as i32,
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: if KINDS[kind] == "fishing_bobber" { LOCAL_ID } else { 0 },
        };
        self.ents.apply(EntityEvent::Add(Box::new(add)));
        if let Some(e) = self.ents.map.get_mut(&id) {
            // Moving between ticks: the interpolated position differs from the current one.
            e.pos_o = [pos[0] - 0.4, pos[1], pos[2] + 0.3];
            e.tag_dist = [64.0, 64.0, 32.0, 100.0][self.rng.below(4)];
            e.below_dist = [10.0, 40.0][self.rng.below(2)];
        }
        if KINDS[kind] == "player" {
            if !self.rng.chance(6) {
                let name = format!("{}{}", NAMES[self.rng.below(NAMES.len())], if self.rng.chance(2) { "" } else { "_" });
                let game_mode = if self.rng.chance(5) { 3 } else { 1 };
                self.profile(uuid, &name, game_mode);
                self.holders.push(name);
            }
        } else {
            self.holders.push(UuidText::new(uuid).as_str().to_owned());
        }
        self.random_data(id);
    }

    /// Flags, custom name, visible name, frame item, mannequin description.
    fn random_data(&mut self, id: i32) {
        let mut values = Vec::new();
        if !self.rng.chance(3) {
            let flags = [0x02, 0x20, 0x40, 0x60, 0x22][self.rng.below(5)] & [0x7f, 0][self.rng.below(2)];
            values.push((0, Meta::Byte(flags)));
        }
        if self.rng.chance(2) {
            values.push((2, Meta::Text(Some(Nbt::String(format!("Name {id}"))))));
        }
        if self.rng.chance(2) {
            values.push((3, Meta::Bool(self.rng.chance(4) || id % 3 == 0)));
        }
        let kind = self.ents.map.get(&id).map(|e| e.kind.clone()).unwrap_or_default();
        if kind.ends_with("item_frame") {
            values.push((9, Meta::Item(Some(self.named_stack(id)))));
        }
        if kind == "mannequin" && self.rng.chance(2) {
            let text = if self.rng.chance(2) { Some(Nbt::String("Vendor".into())) } else { None };
            values.push((19, Meta::Text(text)));
        }
        self.ents.apply(EntityEvent::Data { id, values });
    }

    /// A stack named in two cases out of three (the frame then shows the item's name).
    fn named_stack(&mut self, id: i32) -> rc_proto_v777::item::ItemStack {
        let mut stack = rc_proto_v777::item::ItemStack { item: 1, count: 1, added: Vec::new(), removed: Vec::new() };
        if let Some(ty) = self.custom_name.filter(|_| !self.rng.chance(3)) {
            let mut bytes = Vec::new();
            rc_proto::nbt::write(&mut bytes, Some(&Nbt::String(format!("Map {id}"))));
            stack.added.push((ty, bytes.into()));
        }
        stack
    }

    fn team_event(&mut self, team: &str, mode: i8) {
        let params = TeamParams {
            display: Some(Nbt::String(team.into())),
            prefix: Some(Nbt::String("[".into())),
            suffix: None,
            nametag: self.rng.below(4) as i32,
            collision: 0,
            color: Some(self.rng.below(16) as i32),
            options: [0, 2][self.rng.below(2)],
        };
        let n = self.rng.below(12);
        let mut members: Vec<String> =
            (0..n).filter_map(|_| self.holders.get(self.rng.below(self.holders.len().max(1))).cloned()).collect();
        if self.rng.chance(2) {
            members.push(self.local_name.clone());
        }
        self.scores.apply(ScoreEvent::Team { name: team.into(), mode, params: Some(params), members });
    }

    /// An entity id picked in id order (the table order changes with each process).
    fn random_id(&mut self) -> Option<i32> {
        let mut ids: Vec<i32> = self.ents.map.keys().copied().collect();
        ids.sort_unstable();
        (!ids.is_empty()).then(|| ids[self.rng.below(ids.len())])
    }

    /// One frame of changes; the scores, the slot and the profiles change one frame in eight (kept
    /// "below name" keys between).
    fn step(&mut self, frame: u32) {
        let rare = frame.is_multiple_of(8);
        for _ in 0..6 {
            match self.rng.below(16) {
                9 | 10 | 13 | 14 if !rare => {
                    if let Some(id) = self.random_id() {
                        self.random_data(id);
                    }
                }
                0..=2 => {
                    if let Some(id) = self.random_id() {
                        self.random_data(id);
                    }
                }
                3..=4 => {
                    if let Some(id) = self.random_id() {
                        let p = [self.rng.coord(50.0), 64.0 + self.rng.coord(6.0), self.rng.coord(50.0)];
                        let e = self.ents.map.get_mut(&id).expect("entity");
                        (e.pos_o, e.pos) = (e.pos, p);
                    }
                }
                5 => self.spawn_random(),
                6 => {
                    if let Some(id) = self.random_id() {
                        self.ents.apply(EntityEvent::Remove(vec![id]));
                    }
                }
                // Removed elsewhere than by `apply` (picked-up items, lightning), then sometimes
                // the same id spawned again with another kind.
                7 => {
                    if let Some(id) = self.random_id() {
                        self.ents.map.remove(&id);
                        if self.rng.chance(2) {
                            let kind = self.rng.below(KINDS.len());
                            self.spawn(id, kind);
                        }
                    }
                }
                8 => {
                    let team = ["red", "blue", "green"][self.rng.below(3)];
                    let mode = [2, 3, 3, 4][self.rng.below(4)];
                    self.team_event(team, mode);
                }
                9..=10 => {
                    if !self.holders.is_empty() {
                        let holder = self.holders[self.rng.below(self.holders.len())].clone();
                        let value = self.rng.below(40) as i32;
                        self.scores.apply(ScoreEvent::Score { holder, objective: "hp".into(), value, display: None, format: None });
                    }
                }
                11 => self.frame.target = if self.rng.chance(4) { None } else { self.random_id() },
                12 => {
                    // The local player glows, turns invisible, respawns.
                    let flags = [0, 0x20, 0x40, 0x60][self.rng.below(4)];
                    self.ents.apply(EntityEvent::Data { id: LOCAL_ID, values: vec![(0, Meta::Byte(flags))] });
                    if self.rng.chance(4) {
                        self.ents.respawn_me(self.rng.chance(2), [0.0, 64.0, 0.0], 0.0);
                    }
                }
                13 => {
                    let mut uuids: Vec<u128> = self.players.map.keys().copied().collect();
                    uuids.sort_unstable();
                    if let Some(&uuid) = uuids.get(self.rng.below(uuids.len())) {
                        let (name, game_mode) = (NAMES[self.rng.below(NAMES.len())], self.rng.below(4) as i32);
                        self.profile(uuid, name, game_mode);
                    }
                }
                14 => self.scores.apply(ScoreEvent::Display { slot: 2, objective: ["hp", "hp", ""][self.rng.below(3)].into() }),
                _ => self.ents.tick(),
            }
        }
        self.frame.hud_hidden = frame % 37 == 5;
        self.frame.spectator = frame % 23 < 3;
        self.frame.eye = [self.rng.coord(2.0), 64.0 + self.rng.coord(2.0), self.rng.coord(2.0)];
        // Two always-visible names at the same place for three frames: equal distances, kept in
        // table order.
        if frame.is_multiple_of(20) {
            for _ in 0..2 {
                let id = self.next_id;
                self.next_id += 1;
                self.spawn(id, 3);
                self.ents.apply(EntityEvent::Data { id, values: vec![(3, Meta::Bool(true)), (0, Meta::Byte(0))] });
                let e = self.ents.map.get_mut(&id).expect("armor stand");
                (e.pos, e.pos_o, e.tag_dist) = ([5.25, 65.0, -3.5], [5.25, 65.0, -3.5], 64.0);
                self.tie_stands.push(id);
            }
        } else if frame % 20 == 3 {
            let gone = std::mem::take(&mut self.tie_stands);
            self.ents.apply(EntityEvent::Remove(gone));
        }
    }

    fn old_tags(&self) -> Vec<Recorded> {
        let picked =
            every_entity::pick_tags(&self.ents, &self.players, (&self.scores, &self.local_name), &self.ids, &self.frame);
        let mut rec = Recorder::default();
        every_entity::push_tags(&mut rec, &picked, &self.players, &self.ents.dims, &light, &self.frame, &self.scores, &self.ids);
        rec.tags
    }

    fn new_tags(&self, from_subsets: bool, keys: Option<&mut BelowLineKeys>) -> Vec<Recorded> {
        let rules = TagRules::new(&self.players, &self.scores, &self.ids, &self.local_name);
        let picked = pick_tags(&self.ents, &rules, &self.frame, from_subsets);
        let mut rec = Recorder::default();
        push_tags(&mut rec, &picked, &rules, &self.ents.dims, &light, &self.frame, keys);
        rec.tags
    }

    /// Two tags at the same distance: the candidates hand over to the full walk.
    fn tags_tie(&self) -> bool {
        let rules = TagRules::new(&self.players, &self.scores, &self.ids, &self.local_name);
        pick_tags_among_candidates(&self.ents, &rules, &self.frame).is_none()
    }
}

/// Over 400 frames of a changing scene, the name tags of the old code, of the full walk with the
/// early rejection and of the kept candidates (with kept "below name" keys) are the same tags in
/// the same order, with the same text, anchor, light and discretion. Without the game data,
/// skipped (`RUSTCRAFT_OPT_REQUIRE=1`: failure). Sabotages verified (failure): the distance of
/// another entity for the "below name" line; the name tested after the team; the visible name not
/// noted on data 3; a "below name" key kept across a score change; the aimed entity left out of
/// the candidates.
#[test]
fn name_tags_match_the_old_code() {
    let Some(data) = rc_testdata::version_data() else { return };
    let mut s = Scene::new(&data, 0x9E37_79B9_7F4A_7C15, 160);
    let mut keys = BelowLineKeys::default();
    let (mut tags, mut below, mut frames_named, mut ties, mut teams_hiding, mut stamps) = (0, 0, 0, 0, 0, 0);
    for frame in 0..400 {
        s.step(frame);
        let want = s.old_tags();
        assert_eq!(s.new_tags(false, None), want, "frame {frame}: full walk with early rejection");
        let stamp = keys.stamp;
        assert_eq!(s.new_tags(true, Some(&mut keys)), want, "frame {frame}: kept candidates and keys");
        stamps += usize::from(keys.stamp != stamp);
        tags += want.len();
        below += want.iter().filter(|t| t.id & 0x4000_0000 != 0).count();
        frames_named += want.iter().filter(|t| t.source.contains("Map ")).count();
        ties += usize::from(s.tags_tie());
        // The team rules hid a tag that their absence would show.
        let members = std::mem::take(&mut s.scores.members);
        let without_teams = s.old_tags().len();
        s.scores.members = members;
        teams_hiding += usize::from(without_teams > want.len());
    }
    assert!(tags > 4_000 && below > 400, "too few tags to prove anything: {tags} tags, {below} below lines");
    assert!(frames_named > 0 && ties > 10 && teams_hiding > 20, "{frames_named} frame names, {ties} ties, {teams_hiding} hidden by teams");
    assert!(stamps > 20 && keys.built > 0, "{stamps} stamp changes, {} lines built", keys.built);
}

/// A "below name" key is built once, then kept while its stamp holds; a score of the slot, a
/// profile, the language, or another entity under the same id builds it again, and a frame
/// without the tag forgets it. Sabotage verified (failure): the UUID left out of the kept key.
#[test]
fn below_line_keys_are_kept_while_their_stamp_holds() {
    let Some(data) = rc_testdata::version_data() else { return };
    let mut s = Scene::new(&data, 3, 0);
    s.scores = Default::default();
    s.scores.apply(ScoreEvent::Objective { name: "hp".into(), mode: 0, display: None, hearts: false, format: None });
    s.scores.apply(ScoreEvent::Display { slot: 2, objective: "hp".into() });
    add_at_grid_place(&mut s, 20, 0, Vec::new());
    s.profile(0x5000 + 20, "Alex", 1);
    // A second profile of the same name, for another entity under the same id later.
    s.profile(0x77, "Alex", 1);
    let score = |s: &mut Scene, value| {
        s.scores.apply(ScoreEvent::Score { holder: "Alex".into(), objective: "hp".into(), value, display: None, format: None });
    };
    score(&mut s, 7);
    let place = |s: &mut Scene, p: [f64; 3]| {
        let e = s.ents.map.get_mut(&20).expect("player");
        (e.pos, e.pos_o, e.below_dist) = (p, p, 20.0);
    };
    place(&mut s, [3.5, 64.0, 2.5]);
    s.frame.eye = [0.0, 64.0, 0.0];
    let mut keys = BelowLineKeys::default();
    let frame = |s: &Scene, keys: &mut BelowLineKeys, lang: &rc_text::Lang| {
        let rules = TagRules::new(&s.players, &s.scores, &s.ids, &s.local_name);
        let picked = pick_tags(&s.ents, &rules, &s.frame, true);
        let mut rec = Recorder { lang: lang.clone(), tags: Vec::new() };
        push_tags(&mut rec, &picked, &rules, &s.ents.dims, &light, &s.frame, Some(keys));
        let mut plain = Recorder { lang: lang.clone(), tags: Vec::new() };
        push_tags(&mut plain, &picked, &rules, &s.ents.dims, &light, &s.frame, None);
        assert_eq!(rec.tags, plain.tags);
        rec.tags.len()
    };
    let lang = rc_text::Lang::default();
    for _ in 0..10 {
        assert_eq!(frame(&s, &mut keys, &lang), 2, "name and line");
    }
    assert_eq!(keys.built, 1, "kept over ten frames");
    score(&mut s, 8);
    frame(&s, &mut keys, &lang);
    s.profile(0x5000 + 20, "Alex", 2);
    frame(&s, &mut keys, &lang);
    let other_lang = rc_text::Lang::load(b"{}", None, None);
    frame(&s, &mut keys, &other_lang);
    assert_eq!(keys.built, 4, "score, profile, language");
    frame(&s, &mut keys, &other_lang);
    assert_eq!(keys.built, 4);
    // Another player under the same id (same name, nothing else moved): its own key.
    s.ents.map.get_mut(&20).expect("player").uuid = 0x77;
    frame(&s, &mut keys, &other_lang);
    s.ents.map.get_mut(&20).expect("player").uuid = 0x5000 + 20;
    frame(&s, &mut keys, &other_lang);
    assert_eq!(keys.built, 6, "another player, then the first one again");
    // Too far for the line: forgotten, built again on the way back.
    place(&mut s, [43.5, 64.0, 2.5]);
    assert_eq!(frame(&s, &mut keys, &other_lang), 1, "name only");
    place(&mut s, [3.5, 64.0, 2.5]);
    frame(&s, &mut keys, &other_lang);
    assert_eq!(keys.built, 7, "forgotten while away");
}

/// The glowing outlines, the ghosts (spectator or not) and the rare kinds read from the kept
/// subsets equal the walk of every entity, frame after frame, through spawns, data, removals by
/// `apply` and elsewhere, ids spawned again with another kind, respawn of the local player, ticks.
/// Sabotages verified (failure): glowing not noted on data 0; the local player left out.
#[test]
fn kept_subsets_match_the_walk_of_every_entity() {
    let Some(data) = rc_testdata::version_data() else { return };
    let mut s = Scene::new(&data, 0xD1B5_4A32_D192_ED03, 160);
    let (mut glowing, mut ghosts, mut guardians) = (0, 0, 0);
    let (mut glow, mut ghost) = (hashbrown::HashMap::new(), hashbrown::HashSet::new());
    for frame in 0..400 {
        s.step(frame);
        fill_glow_colors(&mut glow, &s.ents, &s.players, &s.scores);
        assert_eq!(glow, every_entity::glow_colors(&s.ents, &s.players, &s.scores), "frame {frame}: glowing");
        glowing += glow.len();
        for spectator in [false, true] {
            fill_ghost_set(&mut ghost, &s.ents, &s.players, (&s.scores, spectator));
            let want = every_entity::ghost_set(&s.ents, &s.players, &s.scores, spectator);
            assert_eq!(ghost, want, "frame {frame}: ghosts (spectator {spectator})");
            ghosts += ghost.len();
        }
        let subsets = &s.ents.subsets;
        for (kept, rule) in [
            (&subsets.guardians, EntitySubsets::is_guardian as fn(&entities::Entity) -> bool),
            (&subsets.bobbers, EntitySubsets::is_bobber),
            (&subsets.effect_mobs, EntitySubsets::is_effect_mob),
        ] {
            let walked = id_set(s.ents.map.values().filter(|e| rule(e)));
            assert_eq!(id_set(s.ents.of_rare_kind(kept, rule)), walked, "frame {frame}: rare kind");
            guardians += walked.len();
        }
    }
    assert!(glowing > 2_000 && ghosts > 2_000 && guardians > 2_000, "{glowing} glowing, {ghosts} ghosts, {guardians} rare");
}

fn id_set<'a>(entities: impl Iterator<Item = &'a entities::Entity>) -> hashbrown::HashSet<i32> {
    entities.map(|e| e.id).collect()
}

/// The player sets refreshed on their stamp equal the old sets rebuilt on every frame, through
/// profile changes (names that turn players upside down or give them ears), game modes, local
/// spectator mode, a new local player, a new player list and a new renderer (empty sets, no
/// stamp); between changes nothing is rebuilt. Sabotage verified (failure): the local spectator
/// mode out of the stamp.
#[test]
fn player_sets_match_the_old_code() {
    let Some(data) = rc_testdata::version_data() else { return };
    let mut s = Scene::new(&data, 0x0BAD_5EED_1234_5678, 60);
    let profiles = |s: &mut Scene| {
        s.profile(0xffff, "Local", 0);
        s.profile(0xabc, "deadmau5", 1);
        s.profile(0xabd, "Dinnerbone", 3);
    };
    profiles(&mut s);
    let mut sets: [hashbrown::HashSet<u128>; 3] = Default::default();
    let mut stamp = None;
    let (mut rebuilt, mut kept, mut filled) = (0, 0, [0; 3]);
    for frame in 0..400 {
        if frame % 4 == 0 {
            s.step(frame);
        }
        if frame % 97 == 0 {
            s.players = Default::default();
            profiles(&mut s);
        }
        if frame % 131 == 0 {
            (sets, stamp) = (Default::default(), None);
        }
        let local_spectator = frame % 29 < 4;
        let before = stamp;
        let [a, b, c] = &mut sets;
        refresh_player_sets([a, b, c], &mut stamp, &s.ents, &s.players, local_spectator);
        if before == stamp { kept += 1 } else { rebuilt += 1 }
        let want = [
            every_entity::spectator_set(&s.ents, &s.players, local_spectator),
            every_entity::upside_down(&s.players),
            every_entity::extra_ears(&s.players),
        ];
        assert_eq!(sets, want, "frame {frame}");
        for (n, set) in filled.iter_mut().zip(&sets) {
            *n += usize::from(!set.is_empty());
        }
    }
    assert!(kept > 200 && rebuilt > 20, "{kept} frames kept, {rebuilt} rebuilt");
    assert!(filled.iter().all(|&n| n > 20), "frames with spectators, upside down, ears: {filled:?}");
}

/// The UUID written in its fixed buffer is the old formatted text, for any value.
#[test]
fn uuid_text_writes_the_old_form() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let fixed = [0, u128::MAX, 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef];
    let random = (0..10_000).map(|_| u128::from(rng.next()) << 64 | u128::from(rng.next()));
    let mut e = test_entity("zombie");
    for uuid in fixed.into_iter().chain(random) {
        e.uuid = uuid;
        assert_eq!(UuidText::new(uuid).as_str(), old_holder_name(&e, None).expect("uuid"), "{uuid:032x}");
    }
    assert_eq!(UuidText::new(fixed[2]).as_str(), "01234567-89ab-cdef-0123-456789abcdef");
    // A player is named by its profile, borrowed.
    let p = test_entity("player");
    assert!(matches!(entity_holder(&p, Some("Steve")), Some(HolderName::Profile("Steve"))));
    assert!(entity_holder(&p, None).is_none());
}

/// Mannequin: default label, description sent, nothing if hidden.
#[test]
fn mannequin_shows_its_description_below_the_name() {
    let mut e = test_entity("mannequin");
    let label = rc_text::Component::translate("entity.minecraft.mannequin.label", vec![]);
    assert_eq!(mannequin_description(&e), Some(label));
    e.meta.insert(19, Meta::Text(Some(Nbt::String("Vendor".into()))));
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
    e.meta.insert(2, Meta::Text(Some(Nbt::String("Welcome".into()))));
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

/// Microseconds per call of `f` (best of 5 rounds of `n` calls) and bytes allocated per call.
fn measure(n: u32, mut f: impl FnMut()) -> (f64, u64) {
    let best = (0..5)
        .map(|_| {
            let t0 = std::time::Instant::now();
            (0..n).for_each(|_| f());
            t0.elapsed().as_secs_f64() * 1e6 / f64::from(n)
        })
        .fold(f64::MAX, f64::min);
    let bytes = crate::debug_sys::allocated_bytes(|| (0..n).for_each(|_| f())) / u64::from(n);
    (best, bytes)
}

/// Entity `id` of type `kind` at a fixed place of a 40-wide grid, with `values`.
fn add_at_grid_place(s: &mut Scene, id: i32, kind: usize, values: Vec<(u8, Meta)>) {
    let pos = [f64::from(id % 40) - 20.25, 64.0, f64::from(id / 40 % 40) - 12.5];
    let add = rc_proto_v777::entity::AddEntity {
        id,
        uuid: 0x5000 + id as u128,
        kind: kind as i32,
        pos,
        velocity: [0.0; 3],
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        data: 0,
    };
    s.ents.apply(EntityEvent::Add(Box::new(add)));
    s.ents.apply(EntityEvent::Data { id, values });
}

/// Crowd of `mobs` unnamed cows, 20 players with profiles, 10 always-visible names, 1 cow in 10
/// glowing and 1 in 20 invisible, no scoreboard; with `teams`, every holder in a team (the
/// server case).
fn crowd(data: &rc_data::VersionData, mobs: i32, teams: bool) -> Scene {
    let mut s = Scene::new(data, 7, 0);
    s.scores = Default::default();
    for id in 0..mobs {
        let flags = if id % 10 == 0 { 0x40 } else if id % 20 == 5 { 0x20 } else { 0 };
        add_at_grid_place(&mut s, id, 2, vec![(0, Meta::Byte(flags))]);
    }
    for i in 0..20 {
        add_at_grid_place(&mut s, 10_000 + i, 0, Vec::new());
        s.profile(0x5000 + 10_000 + i as u128, &format!("Player{i}"), 1);
    }
    for i in 0..10 {
        let name = Meta::Text(Some(Nbt::String(format!("Boss {i}"))));
        add_at_grid_place(&mut s, 20_000 + i, 1, vec![(2, name), (3, Meta::Bool(true))]);
    }
    // A health line below the players' names (those within 10 blocks show it).
    s.scores.apply(ScoreEvent::Objective { name: "hp".into(), mode: 0, display: Some(Nbt::String("❤".into())), hearts: false, format: None });
    s.scores.apply(ScoreEvent::Display { slot: 2, objective: "hp".into() });
    for i in 0..20 {
        s.scores.apply(ScoreEvent::Score { holder: format!("Player{i}"), objective: "hp".into(), value: 20 - i, display: None, format: None });
    }
    if teams {
        let holder = |e: &entities::Entity| match s.players.name(e.uuid) {
            Some(n) => n.to_owned(),
            None => UuidText::new(e.uuid).as_str().to_owned(),
        };
        let mut members: Vec<String> = s.ents.map.values().map(holder).collect();
        members.push(s.local_name.clone());
        let params = TeamParams {
            display: None,
            prefix: Some(Nbt::String("[C] ".into())),
            suffix: None,
            nametag: 0,
            collision: 0,
            color: Some(12),
            options: 2,
        };
        s.scores.apply(ScoreEvent::Team { name: "crowd".into(), mode: 0, params: Some(params), members });
    }
    s
}

/// Micro-bench (ignored; `cargo test --release -p rc-app bench_entity_sets -- --ignored --nocapture`):
/// one frame of name tags (pick + texts), glowing outlines, ghosts and player sets, old code
/// against each new path, 1,000 mobs, without then with teams. Main thread work of a few µs: one
/// core only (spreading it over threads costs more than the work).
#[test]
#[ignore]
fn bench_entity_sets() {
    let Some(data) = rc_testdata::version_data() else { return };
    for teams in [false, true] {
        let s = crowd(&data, 1_000, teams);
        let mut keys = BelowLineKeys::default();
        let rules = TagRules::new(&s.players, &s.scores, &s.ids, &s.local_name);
        let pick_old = || every_entity::pick_tags(&s.ents, &s.players, (&s.scores, &s.local_name), &s.ids, &s.frame);
        let black_box = |v: Vec<_>| {
            std::hint::black_box(v);
        };
        let rows = [
            ("pick, old code", measure(200, || black_box(pick_old()))),
            ("pick, early rejection", measure(200, || black_box(pick_tags(&s.ents, &rules, &s.frame, false)))),
            ("pick, kept candidates", measure(200, || black_box(pick_tags(&s.ents, &rules, &s.frame, true)))),
        ];
        // A frame of tags whose layouts are kept: pick, origins, anchors, light, "below name" keys.
        let mut sink = KeptLayouts::default();
        let old_frame = |sink: &mut KeptLayouts| {
            let picked = pick_old();
            every_entity::push_tags(sink, &picked, &s.players, &s.ents.dims, &light, &s.frame, &s.scores, &s.ids);
        };
        let tag_rows = [
            ("frame of tags, old code", measure(200, || old_frame(&mut sink))),
            ("frame of tags, keys built", measure(200, || {
                let picked = pick_tags(&s.ents, &rules, &s.frame, true);
                push_tags(&mut sink, &picked, &rules, &s.ents.dims, &light, &s.frame, None);
            })),
            ("frame of tags, keys kept", measure(200, || {
                let picked = pick_tags(&s.ents, &rules, &s.frame, true);
                push_tags(&mut sink, &picked, &rules, &s.ents.dims, &light, &s.frame, Some(&mut keys));
            })),
        ];
        std::hint::black_box(sink.seen);
        let (mut glow, mut ghost) = (hashbrown::HashMap::new(), hashbrown::HashSet::new());
        let sets_rows = [
            ("glow, old code", measure(500, || std::hint::black_box(every_entity::glow_colors(&s.ents, &s.players, &s.scores)).clear())),
            ("glow, kept subset", measure(500, || fill_glow_colors(&mut glow, &s.ents, &s.players, &s.scores))),
            ("ghosts, old code", measure(500, || std::hint::black_box(every_entity::ghost_set(&s.ents, &s.players, &s.scores, true)).clear())),
            ("ghosts, kept subset", measure(500, || fill_ghost_set(&mut ghost, &s.ents, &s.players, (&s.scores, true)))),
        ];
        let mut sets: [hashbrown::HashSet<u128>; 3] = Default::default();
        let mut stamp = None;
        let player_rows = [
            ("player sets, old code", measure(2_000, || {
                std::hint::black_box((
                    every_entity::spectator_set(&s.ents, &s.players, false),
                    every_entity::upside_down(&s.players),
                    every_entity::extra_ears(&s.players),
                ));
            })),
            ("player sets, kept", measure(2_000, || {
                let [a, b, c] = &mut sets;
                refresh_player_sets([a, b, c], &mut stamp, &s.ents, &s.players, false);
            })),
        ];
        let cows: Vec<&entities::Entity> = s.ents.map.values().filter(|e| e.kind == "cow").take(1_000).collect();
        let holder_rows = [
            ("1,000 holder names, formatted", measure(200, || cows.iter().for_each(|e| drop(std::hint::black_box(old_holder_name(e, None)))))),
            ("1,000 holder names, on the stack", measure(200, || cows.iter().for_each(|e| {
                std::hint::black_box(UuidText::new(e.uuid));
            }))),
        ];
        eprintln!("--- 1,000 mobs, teams: {teams}, ties: {} (loaded machine)", s.tags_tie());
        for (name, (us, bytes)) in rows.iter().chain(&tag_rows).chain(&sets_rows).chain(&player_rows).chain(&holder_rows) {
            eprintln!("{name:34} {us:9.3} µs  {bytes:7} B/frame");
        }
    }
}
