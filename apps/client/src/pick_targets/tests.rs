//! Proofs of the crosshair targets against the old code: pick rule of every type of the registry,
//! then 10,000 frames of a scene that changes between frames (camera, entities, data, scale,
//! blocks, held item): same candidates and same hit, bit for bit; the box cache sabotaged must be
//! caught. Micro-bench of a frame of targeting.

use super::*;
use crate::entities::{BOX_KEY_FORGETS_SCALE, Entities, KindFacts};
use crate::pick::{self, Hit};
use crate::players::PlayerEntry;
use rc_client::EntityEvent;
use rc_data::Aabb;
use rc_proto_v777::entity::{AddEntity, Meta};
use rc_proto_v777::item::ItemStack;

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

    /// In [-r, r).
    fn coord(&mut self, r: f64) -> f64 {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * r
    }
}

/// Kinds of the scene: every way a box or a pick rule is decided.
const KINDS: &[&str] = &[
    "cow",
    "zombie",
    "piglin",
    "player",
    "mannequin",
    "armor_stand",
    "slime",
    "magma_cube",
    "phantom",
    "pufferfish",
    "salmon",
    "shulker",
    "polar_bear",
    "goat",
    "camel",
    "sniffer",
    "warden",
    "happy_ghast",
    "ender_dragon",
    "interaction",
    "area_effect_cloud",
    "painting",
    "item_frame",
    "glow_item_frame",
    "leash_knot",
    "fireball",
    "wind_charge",
    "breeze_wind_charge",
    "arrow",
    "trident",
    "oak_boat",
    "bamboo_raft",
    "minecart",
    "tnt",
    "falling_block",
    "end_crystal",
    "item",
    "experience_orb",
    "bat",
];

const FULL: [Aabb; 1] = [Aabb { min: [0.0; 3], max: [1.0; 3] }];
const SLAB: [Aabb; 1] = [Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] }];

fn data() -> std::sync::Arc<rc_data::VersionData> {
    std::sync::Arc::new(rc_testdata::version_data().expect("version data missing (extraction)"))
}

fn type_id(names: &[String], kind: &str) -> i32 {
    let at = names.iter().position(|n| n.strip_prefix("minecraft:") == Some(kind));
    at.unwrap_or_else(|| panic!("kind {kind} missing from the 26.3 registry")) as i32
}

/// Scene: entities around the camera, players (some in spectator), solid blocks.
struct Scene {
    ents: Entities,
    players: PlayerList,
    ids: ComponentIds,
    blocks: hashbrown::HashMap<[i32; 3], bool>,
    eye: [f64; 3],
    dir: [f64; 3],
    rng: Rng,
    next_id: i32,
    map_item: Option<ItemStack>,
}

impl Scene {
    fn new(data: &rc_data::VersionData, seed: u64, count: usize) -> Scene {
        let names = data.registry("minecraft:entity_type").expect("type registry").entries.clone();
        let ents = Entities { type_names: names, dims: data.entity_dims.clone(), ..Default::default() };
        let ids = ComponentIds::new(data);
        // A filled map held by item frames: varint id 3 in the `map_id` component.
        let map_item = ids.map_id.map(|c| ItemStack { item: 1, count: 1, added: vec![(c, vec![3u8].into())], removed: vec![] });
        let mut s = Scene {
            ents,
            players: PlayerList::default(),
            ids,
            blocks: Default::default(),
            eye: [0.5, 65.62, 0.5],
            dir: [0.0, 0.0, 1.0],
            rng: Rng(seed),
            next_id: 1,
            map_item,
        };
        for _ in 0..count {
            s.spawn();
        }
        s
    }

    fn spawn(&mut self) {
        let kind = KINDS[self.rng.below(KINDS.len())];
        let id = self.next_id;
        self.next_id += 1;
        let pos = [self.eye[0] + self.rng.coord(7.0), 64.0 + self.rng.coord(2.5), self.eye[2] + self.rng.coord(7.0)];
        let add = AddEntity {
            id,
            uuid: 0x7000 + id as u128,
            kind: type_id(&self.ents.type_names, kind),
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: self.rng.below(6) as i32,
        };
        self.ents.apply(EntityEvent::Add(Box::new(add)));
        if kind == "player" {
            let game_mode = [0, 1, 3][self.rng.below(3)];
            self.players.map.insert(0x7000 + id as u128, PlayerEntry { game_mode, ..Default::default() });
        }
    }

    fn random_id(&mut self) -> Option<i32> {
        let mut ids: Vec<i32> = self.ents.map.keys().copied().collect();
        ids.sort_unstable();
        (!ids.is_empty()).then(|| ids[self.rng.below(ids.len())])
    }

    /// One random change of what a box or a pick rule reads.
    fn mutate(&mut self) {
        let Some(id) = self.random_id() else { return self.spawn() };
        let value = match self.rng.below(14) {
            0 => {
                let e = self.ents.map.get_mut(&id).expect("listed");
                e.pos = [e.pos[0] + self.rng.coord(0.4), e.pos[1] + self.rng.coord(0.2), e.pos[2] + self.rng.coord(0.4)];
                return;
            }
            1 => {
                let e = self.ents.map.get_mut(&id).expect("listed");
                e.scale = [0.5, 1.0, 2.0, 3.7, 16.0][self.rng.below(5)];
                return;
            }
            2 => {
                let e = self.ents.map.get_mut(&id).expect("listed");
                e.peek = [0.0, 0.3, 1.0][self.rng.below(3)];
                e.extra[0] = [0.0, 2.5, 6.0][self.rng.below(3)];
                return;
            }
            3 => (6, Meta::Pose([0, 1, 2, 3, 5, 6, 7, 10, 13][self.rng.below(9)])),
            4 => (16, Meta::Bool(self.rng.below(2) == 0)),
            5 => (17, Meta::Bool(self.rng.below(2) == 0)),
            6 => (15, Meta::Byte([0, 1, 0x10, 0x11][self.rng.below(4)])),
            7 => (18, Meta::Int(self.rng.below(5) as i32)),
            8 => (16, Meta::Int(self.rng.below(8) as i32)),
            9 => (17, Meta::Int(self.rng.below(3) as i32)),
            10 => (8, Meta::Float(0.5 + self.rng.below(4) as f32 * 0.75)),
            11 => (9, Meta::Float(0.5 + self.rng.below(4) as f32 * 0.75)),
            12 => (8, Meta::Direction(self.rng.below(6) as i32)),
            _ => match self.rng.below(3) {
                0 => (9, Meta::Variant(self.rng.below(4) as i32, 0)),
                1 => (9, Meta::Item(self.map_item.clone())),
                _ => (9, Meta::Item(None)),
            },
        };
        self.ents.apply(EntityEvent::Data { id, values: vec![value] });
    }

    /// A frame: a few changes, camera moved and turned, blocks changed.
    fn step(&mut self) {
        for _ in 0..self.rng.below(4) {
            self.mutate();
        }
        if self.rng.below(40) == 0
            && let Some(id) = self.random_id()
        {
            self.ents.apply(EntityEvent::Remove(vec![id]));
            self.spawn();
        }
        self.eye = [self.eye[0] + self.rng.coord(0.2), 64.0 + 1.62 + self.rng.coord(1.0), self.eye[2] + self.rng.coord(0.2)];
        let d = [self.rng.coord(1.0), self.rng.coord(0.6), self.rng.coord(1.0)];
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
        self.dir = d.map(|v| v / n);
        let b = [self.rng.below(11) as i32 - 5, 63 + self.rng.below(4) as i32, self.rng.below(11) as i32 - 5];
        let _ = match self.rng.below(3) {
            0 => self.blocks.remove(&b),
            1 => self.blocks.insert(b, true),
            _ => self.blocks.insert(b, false),
        };
    }

    fn source(&self) -> TargetSource<'_, impl Fn(i32) -> Option<(u8, u8)>> {
        TargetSource {
            ents: &self.ents,
            players: &self.players,
            table: &self.ents.dims,
            painting: |v: i32| (v >= 0).then(|| ((v % 4 + 1) as u8, (v / 2 + 1) as u8)),
            component_ids: &self.ids,
        }
    }

    /// Area of the frame: normal targeting over 4.5 and spear targeting (2 to 4.5), camera riding
    /// nothing.
    fn area(&self) -> TargetArea {
        let at = |k: f64| -> [f64; 3] { std::array::from_fn(|i| self.eye[i] + self.dir[i] * k) };
        let normal = pick::gaze_search_box(&camera_box(self.eye), self.dir, 4.5);
        let widest = pick::hull(&normal, &pick::attack_search_box(at(2.0), at(4.5), 0.125));
        TargetArea::of(&widest, LOCAL)
    }

    fn candidates(&self, paths: TargetPaths) -> Vec<Snapshot> {
        candidates(self.source(), self.area(), paths).map(|c| snapshot(&c)).collect()
    }

    /// Normal targeting, then targeting with an attack reach (spear), over these candidates.
    fn hits(&self, list: &[Snapshot]) -> [Hit; 2] {
        let shape = |x: i32, y: i32, z: i32| match self.blocks.get(&[x, y, z]) {
            Some(true) => pick::plain(&FULL),
            Some(false) => pick::plain(&SLAB),
            None => pick::plain(&[]),
        };
        let cam = camera_box(self.eye);
        let normal = pick::pick(self.eye, self.dir, &cam, 4.5, 3.0, &shape, &pick::no_interaction, rebuild(list));
        let reach = pick::AttackReach { min: 2.0, max: 4.5, margin: 0.125 };
        let open = [-3.0e7, 3.0e7, -3.0e7, 3.0e7];
        let attack =
            pick::pick_attack(self.eye, self.dir, reach, 0.0, 4.5, open, &shape, &pick::no_interaction, rebuild(list));
        [normal, attack]
    }
}

/// Id of the local player (never in the entity table).
const LOCAL: i32 = -7;

/// Box of a standing player whose eyes are at `eye`.
fn camera_box(eye: [f64; 3]) -> Aabb {
    Aabb { min: [eye[0] - 0.3, eye[1] - 1.62, eye[2] - 0.3], max: [eye[0] + 0.3, eye[1] + 0.18, eye[2] + 0.3] }
}

/// A candidate as bits: id, box, radius, same vehicle as the camera, section.
type Snapshot = (i32, [u64; 6], u64, bool, [i32; 3]);

fn snapshot(c: &Candidate) -> Snapshot {
    let b = [c.bb.min[0], c.bb.min[1], c.bb.min[2], c.bb.max[0], c.bb.max[1], c.bb.max[2]];
    (c.id, b.map(f64::to_bits), c.radius.to_bits(), c.same_vehicle, c.section)
}

fn rebuild(list: &[Snapshot]) -> impl Iterator<Item = Candidate> + '_ {
    list.iter().map(|&(id, b, r, same_vehicle, section)| {
        let b = b.map(f64::from_bits);
        Candidate {
            id,
            bb: Aabb { min: [b[0], b[1], b[2]], max: [b[3], b[4], b[5]] },
            radius: f64::from_bits(r),
            same_vehicle,
            section,
        }
    })
}

/// Frames whose candidates or hits differ between the old code and `paths`.
fn differing_frames(seed: u64, frames: usize, paths: TargetPaths) -> (usize, usize) {
    let data = data();
    let mut s = Scene::new(&data, seed, 120);
    let (mut differ, mut aimed) = (0, 0);
    for _ in 0..frames {
        s.step();
        let old = s.candidates(TargetPaths::OLD);
        let new = s.candidates(paths);
        let hits = s.hits(&old);
        aimed += usize::from(hits.iter().any(|h| matches!(h, Hit::Entity { .. })));
        if old != new || hits != s.hits(&new) {
            differ += 1;
        }
    }
    (differ, aimed)
}

/// Every path and their combination: 10,000 frames, same candidates and same hit as the old code.
#[test]
fn targets_match_the_old_code_frame_after_frame() {
    for (seed, paths) in [
        (11, TargetPaths { kind_flags: true, cached_boxes: false }),
        (12, TargetPaths { kind_flags: false, cached_boxes: true }),
        (13, TargetPaths { kind_flags: true, cached_boxes: true }),
    ] {
        let (differ, aimed) = differing_frames(seed, 10_000, paths);
        assert_eq!(differ, 0, "{paths:?}: frames that differ");
        assert!(aimed > 1_000, "{paths:?}: the scene aims at too few entities ({aimed})");
    }
}

/// Sabotage: a box key that forgets the scale attribute keeps a stale box; the proof sees it.
#[test]
fn box_cache_forgetting_the_scale_is_caught() {
    BOX_KEY_FORGETS_SCALE.set(true);
    let (differ, _) = differing_frames(14, 3_000, TargetPaths { kind_flags: true, cached_boxes: true });
    BOX_KEY_FORGETS_SCALE.set(false);
    assert!(differ > 0, "the sabotaged box cache went unnoticed");
}

/// Pick rule and radius of every type of the 26.3 registry, as the facts give them, equal to the
/// old name rules, in each state that decides them (marker armor stand, spectator player).
#[test]
fn pick_rule_of_every_registry_type_matches_the_names() {
    let data = data();
    let names = data.registry("minecraft:entity_type").expect("type registry").entries.clone();
    assert!(names.len() > 100, "type registry too short: {}", names.len());
    let mut all = Entities { type_names: names.clone(), ..Default::default() };
    let mut players = PlayerList::default();
    players.map.insert(9, PlayerEntry { game_mode: 3, ..Default::default() });
    let mismatches = |all: &Entities, players: &PlayerList| {
        all.map.values().filter(|e| pick_radius(e, players) != pick_radius_by_name(e, players)).count()
    };
    for (i, _) in names.iter().enumerate() {
        for (n, uuid) in [(0, 1), (1, 9)] {
            let id = (i * 2 + n) as i32 + 1;
            let add = AddEntity {
                id,
                uuid,
                kind: i as i32,
                pos: [0.0; 3],
                velocity: [0.0; 3],
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            };
            all.apply(EntityEvent::Add(Box::new(add)));
            if n == 1 {
                all.apply(EntityEvent::Data { id, values: vec![(15, Meta::Byte(0x10))] });
            }
        }
    }
    assert_eq!(mismatches(&all, &players), 0);
    // Sabotage: a trident made pickable is seen.
    let trident = all.map.values_mut().find(|e| e.kind == "trident").expect("trident");
    trident.facts = KindFacts { pick: crate::entities::PickRule::Always, ..trident.facts };
    assert_eq!(mismatches(&all, &players), 1);
}

/// Normal targeting of one entity of kind `kind` at `pos` (scale `scale`, riding `vehicle` when
/// given) from eyes at (0.5, 65.62, 0.5) looking toward +z, no block; `camera_root`: root vehicle
/// of the camera.
fn aim_at(kind: &str, pos: [f64; 3], scale: f32, camera_root: i32, local_vehicle: Option<i32>) -> Hit {
    let data = data();
    let mut s = Scene::new(&data, 1, 0);
    s.ents.local = LOCAL;
    let add = AddEntity {
        id: 1,
        uuid: 1,
        kind: type_id(&s.ents.type_names, kind),
        pos,
        velocity: [0.0; 3],
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        data: 0,
    };
    s.ents.apply(EntityEvent::Add(Box::new(add)));
    s.ents.map.get_mut(&1).expect("spawned").scale = scale;
    if let Some(v) = local_vehicle {
        s.ents.apply(EntityEvent::Passengers { vehicle: v, passengers: vec![LOCAL] });
    }
    s.eye = [0.5, 65.62, 0.5];
    s.dir = [0.0, 0.0, 1.0];
    let cam = camera_box(s.eye);
    let area = TargetArea::of(&pick::gaze_search_box(&cam, s.dir, 4.5), camera_root);
    let list: Vec<Snapshot> = candidates(s.source(), area, TargetPaths::current()).map(|c| snapshot(&c)).collect();
    let empty = |_: i32, _: i32, _: i32| pick::plain(&[]);
    pick::pick(s.eye, s.dir, &cam, 4.5, 3.0, &empty, &pick::no_interaction, rebuild(&list))
}

/// A ghast scaled 16 times (64 blocks wide), its centre 13 blocks aside: its box holds the eyes
/// and its section is searched, so the game aims at it. The old filter (12.5 blocks around the
/// eyes on each axis) dropped it.
#[test]
fn huge_entity_found_by_its_section() {
    assert!(matches!(aim_at("ghast", [13.5, 64.0, 1.0], 16.0, LOCAL, None), Hit::Entity { id: 1, .. }));
    // Same ghast 20 blocks aside: its section is no longer searched (2 blocks of margin).
    assert_eq!(aim_at("ghast", [20.5, 64.0, 1.0], 16.0, LOCAL, None), Hit::Miss);
}

/// Riding a boat, the gaze crossing it: the game never aims at an entity of the camera's own
/// root vehicle from outside its box (the old code aimed at the boat).
#[test]
fn own_vehicle_not_aimed_at() {
    // Boat box from y = 65.3 to 65.86: the level gaze at 65.62 crosses it.
    let boat = [0.5, 65.3, 2.5];
    assert!(matches!(aim_at("oak_boat", boat, 1.0, LOCAL, None), Hit::Entity { id: 1, .. }), "boat of someone else");
    // The camera's root vehicle is the boat (id 1): skipped.
    assert_eq!(aim_at("oak_boat", boat, 1.0, 1, Some(1)), Hit::Miss);
}

/// Scene of the bench: `count` cows on a grid `offset` blocks in front of the camera (0: around it;
/// 22: the camera of the crowd scenes, every cow out of reach).
fn cow_grid(data: &rc_data::VersionData, count: i32, offset: f64) -> Scene {
    let mut s = Scene::new(data, 5, 0);
    let cow = type_id(&s.ents.type_names, "cow");
    for id in 0..count {
        let pos = [f64::from(id % 32) * 0.7 - 11.0, 64.0, f64::from(id / 32) * 0.7 - 11.0 + offset];
        let add = AddEntity {
            id,
            uuid: id as u128,
            kind: cow,
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        s.ents.apply(EntityEvent::Add(Box::new(add)));
    }
    s
}

/// Micro-bench (ignored; `cargo test --release -p rc-app bench_pick_targets -- --ignored
/// --nocapture`): one frame of targeting (candidates then normal targeting) among 200 and 1,000
/// cows, around the camera or 22 blocks away (crowd scenes), camera turning every frame, cows
/// moving every third frame (60 frames for 20 ticks). Paths measured in alternating rounds, best
/// round kept. Main thread work of a few µs: one core only.
#[test]
#[ignore]
fn bench_pick_targets() {
    let data = data();
    println!("entity: {} bytes", std::mem::size_of::<crate::entities::Entity>());
    let shape = |_: i32, y: i32, _: i32| pick::plain(if y < 64 { &FULL[..] } else { &[][..] });
    let paths = [
        ("old code", TargetPaths::OLD),
        ("kind flags", TargetPaths { kind_flags: true, cached_boxes: false }),
        ("cached boxes", TargetPaths { kind_flags: false, cached_boxes: true }),
        ("both", TargetPaths { kind_flags: true, cached_boxes: true }),
    ];
    for (count, offset) in [(200, 0.0), (1_000, 0.0), (1_000, 22.0)] {
        let mut s = cow_grid(&data, count, offset);
        let mut best = [f64::MAX; 4];
        for _round in 0..9 {
            for (slot, (_, p)) in paths.iter().enumerate() {
                let t0 = std::time::Instant::now();
                for frame in 0..600u32 {
                    let yaw = f64::from(frame) * 0.05;
                    s.dir = [-yaw.sin() * 0.9, -0.3, yaw.cos() * 0.9];
                    if frame % 3 == 0 {
                        for e in s.ents.map.values_mut() {
                            e.pos[0] += if frame % 6 == 0 { 0.01 } else { -0.01 };
                        }
                    }
                    let c = candidates(s.source(), s.area(), *p);
                    let cam = camera_box(s.eye);
                    std::hint::black_box(pick::pick(s.eye, s.dir, &cam, 4.5, 3.0, &shape, &pick::no_interaction, c));
                }
                best[slot] = best[slot].min(t0.elapsed().as_secs_f64() * 1e6 / 600.0);
            }
        }
        for ((label, _), t) in paths.iter().zip(best) {
            println!("pick, {count} cows {offset} blocks away, {label}: {t:.2} µs/frame");
        }
    }
}
