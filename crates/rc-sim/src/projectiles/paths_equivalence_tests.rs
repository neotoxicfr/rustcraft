//! Equivalence of the projectile switches (`ProjectilePaths`). Each scenario runs on a fresh flat
//! world in a thread of its own (the test seeds follow the thread, so every run draws the same
//! values), once per set of paths; its trace (every projectile after every tick, the events sent
//! to the client, the saved compound of every projectile, parked worlds included) must be the
//! same on every path and match a stored fingerprint. Exhaustive tests compare the per-state rules
//! over every block state and the target search over random boxes in a crowd. Each proof also
//! runs a deliberate fault and must see it. Version data mandatory.

use rc_physics::Aabb;

use super::paths::{FORCED_PROJECTILE_PATHS, ProjectilePaths, Sabotage};
use super::{PKind, Proj};
use crate::blocks::probe::Probe;
use crate::mobs::jrand::JRand;
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::tests_g3f13::{finish, open, reopen};

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn tick(p: &mut Probe) {
    p.s.game_tick();
    p.s.tick += 1;
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn digest(lines: &[String]) -> u64 {
    lines.iter().fold(0xcbf2_9ce4_8422_2325, |h, l| fnv(fnv(h, l.as_bytes()), b"\n"))
}

/// `[I;a,b,c,d]` of a UUID, for a compound typed in a command.
fn uuid_ints(u: u128) -> String {
    format!("[I;{},{},{},{}]", (u >> 96) as i32, (u >> 64) as i32, (u >> 32) as i32, u as i32)
}

/// Runs `f` with `paths` forced on this thread.
fn with_paths<R>(paths: ProjectilePaths, f: impl FnOnce() -> R) -> R {
    let before = FORCED_PROJECTILE_PATHS.replace(Some(paths));
    let r = f();
    FORCED_PROJECTILE_PATHS.set(before);
    r
}

/// Events sent to the client since the last read, one line each (the advancement updates carry
/// the wall clock time of the recipes unlocked at the first tick: left out).
fn record_events(p: &Probe, trace: &mut Vec<String>) {
    let lines = p.take_events().into_iter().map(|e| format!("{e:?}"));
    trace.extend(lines.filter(|l| !l.starts_with("Progress(Advancements(")));
}

/// Every projectile of the active list as the tick left it.
fn record_projectiles(p: &Probe, trace: &mut Vec<String>) {
    for q in &p.s.proj.list {
        trace.push(format!(
            "proj {:x} {} t{} {:?} {:?} removed {} ground {} fire {} pierced {:?}",
            q.uuid,
            q.kind.name(),
            q.tick_count,
            q.pos,
            q.vel,
            q.removed,
            q.in_ground,
            q.fire,
            q.pierced
        ));
    }
}

/// What stays at the end: the saved compound of every projectile, active and parked worlds, the
/// mobs and the player.
fn record_end(p: &Probe, trace: &mut Vec<String>) {
    trace.push(format!("dimension {}", p.s.dim.name));
    for q in &p.s.proj.list {
        trace.push(format!("nbt {} removed {}: {:?}", p.s.dim.name, q.removed, p.s.proj_to_nbt(q)));
    }
    for w in &p.s.dims.parked {
        for q in &w.proj.list {
            trace.push(format!("parked nbt {} removed {}: {:?}", w.dim.name, q.removed, p.s.proj_to_nbt(q)));
        }
    }
    for m in p.s.mobs.list.iter() {
        let held = m.equipment[0].as_ref().map(|s| s.id.as_str());
        trace.push(format!(
            "mob {} {:?} {:?} health {} removed {} target {:?} holds {held:?}",
            m.kind.info().name,
            m.pos(),
            m.vel,
            m.health,
            m.removed,
            m.target
        ));
    }
    trace.push(format!("player {:?} health {} dead {}", p.s.pos, p.s.life.vit.health, p.s.life.vit.dead));
}

type Scene = fn(&mut Probe, &mut Vec<String>);

/// Files of a data pack written into the world before it opens.
type Pack = &'static [(&'static str, &'static str)];

/// Runs `scene` on a fresh flat world (with `pack` installed as `datapacks/t`), in its own
/// thread, with `paths` forced there.
fn run_scene(tag: String, paths: ProjectilePaths, pack: Pack, scene: Scene) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        FORCED_PROJECTILE_PATHS.set(Some(paths));
        let (p, dir) = open(&tag);
        let mut p = if pack.is_empty() {
            p
        } else {
            let data = p.s.data.clone();
            drop(p);
            for (path, text) in pack {
                let file = dir.join("datapacks").join("t").join(path);
                std::fs::create_dir_all(file.parent().expect("pack folder")).expect("pack folder");
                std::fs::write(file, text).expect("pack file");
            }
            reopen(&dir, data)
        };
        let mut trace = Vec::new();
        scene(&mut p, &mut trace);
        record_end(&p, &mut trace);
        finish(p, dir);
        trace
    })
}

fn first_difference(a: &[String], b: &[String]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y).or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// Every path of `paths` gives the trace of the first; the first's fingerprint is `stored`
/// (Windows only: the libc's sine and cosine differ elsewhere, as for the other fingerprints).
/// Every sabotage of `faults` must change the trace.
fn assert_paths_agree(
    name: &str,
    pack: Pack,
    scene: Scene,
    paths: &[(&str, ProjectilePaths)],
    faults: &[(&str, ProjectilePaths)],
    stored: u64,
) {
    let runs: Vec<_> = paths
        .iter()
        .chain(faults)
        .map(|(label, s)| (*label, run_scene(format!("{name}-{label}"), *s, pack, scene)))
        .collect();
    let traces: Vec<(&str, Vec<String>)> =
        runs.into_iter().map(|(l, h)| (l, h.join().expect("scene thread"))).collect();
    let (reference_label, reference) = &traces[0];
    assert!(reference.len() > 20, "{name}: trace too short ({} lines), the scene did nothing", reference.len());
    for (label, trace) in &traces[1..paths.len()] {
        let k = first_difference(reference, trace);
        assert!(
            k.is_none(),
            "{name}: path {label} differs from {reference_label} (lengths {} / {}, first difference at line {k:?}: {:?} / {:?})",
            reference.len(),
            trace.len(),
            k.and_then(|k| reference.get(k)),
            k.and_then(|k| trace.get(k)),
        );
    }
    for (label, trace) in &traces[paths.len()..] {
        assert!(first_difference(reference, trace).is_some(), "{name}: sabotage {label} not seen by the proof");
    }
    let got = digest(reference);
    eprintln!("{name}: {} lines, fingerprint {got:#018x}", reference.len());
    // To read what a scene did: the reference trace written into that folder.
    if let Some(folder) = rc_data::env_os!("RUSTCRAFT_PROJECTILE_TRACE_DIR") {
        let _ = std::fs::write(std::path::Path::new(&folder).join(format!("{name}.txt")), reference.join("\n"));
    }
    if cfg!(windows) {
        assert_eq!(got, stored, "{name}: fingerprint changed (behaviour differs from the stored reference)");
    }
}

/// Each new path alone over the old ones, then every new path.
fn one_by_one() -> Vec<(&'static str, ProjectilePaths)> {
    let old = ProjectilePaths::OLD;
    vec![
        ("old", old),
        ("take-heavy-fields", ProjectilePaths { take_heavy_fields: true, ..old }),
        ("borrowed-shapes", ProjectilePaths { borrowed_shapes: true, ..old }),
        ("owner-index", ProjectilePaths { owner_index: true, ..old }),
        ("target-prefilter", ProjectilePaths { target_prefilter: true, ..old }),
        ("targets-in-place", ProjectilePaths { targets_in_place: true, ..old }),
        ("hit-without-mob-clone", ProjectilePaths { hit_without_mob_clone: true, ..old }),
        ("new", ProjectilePaths::NEW),
    ]
}

fn sabotaged(s: Sabotage) -> ProjectilePaths {
    ProjectilePaths { sabotage: s, ..ProjectilePaths::NEW }
}

// ---------- Archers ----------

/// 20 skeletons and a witch around a survival player under a rain, arrows stuck in wooden buttons,
/// pressure plates, slabs and fences, effect and spectral arrows, a piercing arrow through a line
/// of pigs, a loyal trident thrown by the player, a large fireball sent back by the player, a
/// snowball and an egg; 3,000 ticks.
fn archers(p: &mut Probe, trace: &mut Vec<String>) {
    let me = uuid_ints(p.s.uuid);
    // Refused when the world is already in normal difficulty.
    let src = p.s.server_source();
    let answer = p.s.perform(src, "difficulty normal");
    trace.push(format!("difficulty {} ({answer:?})", p.s.level.difficulty_id()));
    for c in [
        // Under a stone roof, like the bench's archers (in the open they seek shade instead).
        "fill -17 -54 -17 17 -54 17 minecraft:stone",
        "weather rain",
        "effect give @a minecraft:resistance infinite 3 true",
        "effect give @a minecraft:regeneration infinite 4 true",
        "fill 20 -60 -4 20 -57 4 minecraft:stone",
        "fill 24 -60 8 28 -60 8 minecraft:oak_pressure_plate",
        "fill 10 -60 -10 14 -60 -10 minecraft:stone_slab",
        "fill 10 -60 -13 14 -60 -13 minecraft:oak_fence",
    ] {
        cmd(p, c);
    }
    for z in -3..=3 {
        cmd(p, &format!("setblock 19 -59 {z} minecraft:oak_button[face=wall,facing=west]"));
    }
    for k in 0..20 {
        let a = f64::from(k) * std::f64::consts::TAU / 20.0;
        let (x, z) = (0.5 + 14.0 * a.cos(), 0.5 + 14.0 * a.sin());
        let bow = "equipment:{mainhand:{id:\"minecraft:bow\",count:1}}";
        cmd(p, &format!("summon minecraft:skeleton {x:.3} -60 {z:.3} {{PersistenceRequired:1b,{bow}}}"));
    }
    let skeletons: Vec<i32> = p.s.mobs.list.iter().filter(|m| m.kind == Kind::Skeleton).map(|m| m.id).collect();
    cmd(p, "summon minecraft:witch 6.5 -60 6.5 {PersistenceRequired:1b}");
    for x in 30..=34 {
        cmd(p, &format!("summon minecraft:pig {x}.5 -60 -6.5 {{NoAI:1b,PersistenceRequired:1b}}"));
    }
    for z in -3..=3 {
        cmd(p, &format!("summon minecraft:arrow 15.5 -58.5 {z}.5 {{Motion:[1.2d,0.05d,0.0d]}}"));
    }
    for x in 24..=28 {
        cmd(p, &format!("summon minecraft:arrow {x}.5 -55 8.5 {{Motion:[0.0d,-0.6d,0.0d]}}"));
    }
    for x in 10..=14 {
        cmd(p, &format!("summon minecraft:arrow {x}.5 -56 -9.5 {{Motion:[0.0d,-0.8d,0.0d]}}"));
        cmd(p, &format!("summon minecraft:arrow {x}.5 -56 -12.5 {{Motion:[0.0d,-0.8d,0.0d]}}"));
    }
    let tipped = "item:{id:\"minecraft:tipped_arrow\",count:1,components:{\"minecraft:potion_contents\":{potion:\"minecraft:poison\"}}}";
    cmd(p, &format!("summon minecraft:arrow 15.5 -58.5 2.5 {{Motion:[1.0d,0.1d,0.0d],{tipped}}}"));
    cmd(p, "summon minecraft:spectral_arrow 15.5 -58.5 -2.5 {Motion:[1.0d,0.1d,0.0d]}");
    cmd(p, &format!("summon minecraft:arrow 27.5 -59.5 -6.5 {{Motion:[1.5d,0.0d,0.0d],PierceLevel:3b,Owner:{me}}}"));
    let trident =
        "item:{id:\"minecraft:trident\",count:1,components:{\"minecraft:enchantments\":{\"minecraft:loyalty\":3}}}";
    cmd(
        p,
        &format!("summon minecraft:trident 1.5 -58.5 0.5 {{Motion:[1.5d,0.2d,0.0d],pickup:1b,Owner:{me},{trident}}}"),
    );
    cmd(p, "summon minecraft:fireball 0.5 -58.3 9.5 {Motion:[0.0d,0.0d,-0.3d],acceleration_power:0.02d}");
    cmd(p, "summon minecraft:snowball 2.5 -58 2.5 {Motion:[0.5d,0.4d,0.5d]}");
    cmd(p, "summon minecraft:egg -2.5 -58 2.5 {Motion:[-0.5d,0.4d,0.5d]}");
    let mut deflected = false;
    for t in 0..3000 {
        // Each skeleton shoots at the player every 40 ticks, in turn (their own aim in this flat
        // test world does not shoot: the shots are driven here, through the mobs' own shot).
        if let Some(&id) = skeletons.get(t % 40) {
            shoot_at_player(p, id);
        }
        tick(p);
        record_projectiles(p, trace);
        record_events(p, trace);
        if deflected {
            continue;
        }
        let eye = [p.s.pos[0], p.s.player_eye_y(), p.s.pos[2]];
        let near = p.s.proj.list.iter().find(|q| {
            q.kind == PKind::LargeFireball
                && !q.removed
                && (0..3).map(|k| (q.pos[k] - eye[k]).powi(2)).sum::<f64>() < 9.0
        });
        if let Some(id) = near.map(|q| q.id) {
            deflected = p.s.fireball_attacked(id);
            trace.push(format!("fireball sent back: {deflected}"));
        }
    }
}

/// The mob `id`, out of its list as during its own tick, shoots its bow at the player.
fn shoot_at_player(p: &mut Probe, id: i32) {
    let Some(i) = p.s.mob_index(id) else { return };
    let Some(mut m) = p.s.mobs.list.take(i) else { return };
    if m.alive() && !m.removed {
        p.s.shoot_arrow(&mut m, crate::mobs::mob::Ent::Player, 1.0);
    }
    p.s.mobs.list.put(i, m);
}

/// Sabotages seen failing (by hand): the heavy fields not given back to the tick copy (path
/// `take-heavy-fields` differs at line 102); the owner scan resumed after the hinted slot (path
/// `owner-index` differs at line 411).
#[test]
fn archers_trace_is_the_same_on_every_path() {
    assert_paths_agree("proj-archers", &[], archers, &one_by_one(), &[], 0x4bdc_5a94_e1ab_b897);
}

// ---------- Dimension change during a projectile's tick ----------

const PACK_FORMAT: &str = r#"{"pack":{"description":"t","pack_format":121,"min_format":121,"max_format":121}}"#;

/// Hitting a target block takes the player to the Nether, during the tick of the arrow that hits
/// it. (A mob hurt by the player's arrow would not do: the mob is out of its list for the time of
/// its hurt, and putting it back after the reward changed the dimension panics, on every path.)
const AWAY_PACK: Pack = &[
    ("pack.mcmeta", PACK_FORMAT),
    ("data/t/function/away.mcfunction", "execute in minecraft:the_nether run tp @s 0.5 80 0.5\n"),
    (
        "data/t/advancement/away.json",
        r#"{"criteria":{"hit":{"trigger":"minecraft:target_hit"}},"rewards":{"function":"t:away"}}"#,
    ),
];

/// The player's critical, piercing effect arrow hits a target block: the reward takes the player to
/// the Nether in the middle of the arrow's tick, which parks the overworld with the arrow's slot in
/// it. Arrows stuck around stay there too. The player then comes back, and the arrows are paired
/// again with the client.
fn away_mid_tick(p: &mut Probe, trace: &mut Vec<String>) {
    p.s.view = 2;
    let me = uuid_ints(p.s.uuid);
    cmd(p, "setblock 8 -59 0 minecraft:target");
    cmd(p, "fill 4 -60 6 4 -57 9 minecraft:stone");
    for z in 6..=9 {
        cmd(p, &format!("summon minecraft:arrow 1.5 -58.5 {z}.5 {{Motion:[1.0d,0.0d,0.0d]}}"));
    }
    let tipped = "item:{id:\"minecraft:tipped_arrow\",count:1,components:{\"minecraft:potion_contents\":{potion:\"minecraft:healing\"}}}";
    let shot = format!("Motion:[2.0d,0.1d,0.0d],crit:1b,PierceLevel:2b,Owner:{me},{tipped}");
    cmd(p, &format!("summon minecraft:arrow 3.5 -58.5 0.5 {{{shot}}}"));
    assert!(p.s.trig_active("minecraft:target_hit"), "advancement of the pack not loaded");
    assert!(p.s.cmd.functions.contains_key("t:away"), "function of the pack not loaded");
    for _ in 0..60 {
        tick(p);
        record_projectiles(p, trace);
        record_events(p, trace);
    }
    assert_eq!(p.s.dim.name, "minecraft:the_nether", "the reward did not take the player away during the hit");
    let parked = p.s.dims.parked.iter().flat_map(|w| w.proj.list.iter());
    check_hit_kept(parked.clone().find(|q| q.item.id == "minecraft:tipped_arrow"), "parked", trace);
    cmd(p, "execute in minecraft:overworld run tp @a 0.5 -60 0.5");
    // The probe streams no chunk: the arrows' chunk counts as sent, so that they are paired again.
    p.s.sent.insert((0, 0));
    for _ in 0..40 {
        tick(p);
        record_projectiles(p, trace);
        record_events(p, trace);
    }
    assert_eq!(p.s.dim.name, "minecraft:overworld", "the player did not come back");
    check_hit_kept(p.s.proj.list.iter().find(|q| q.item.id == "minecraft:tipped_arrow"), "back", trace);
}

/// The game keeps the result of the tick that hit the target: the arrow is stuck there, no longer
/// critical nor piercing, and its entity data says the same as its fields. Checked on the parked
/// slot (on return the arrow would hit the target again at its first tick, hiding a lost result).
/// The sabotage leaves the slot as before the tick: for it only the trace must differ.
fn check_hit_kept(arrow: Option<&Proj>, place: &str, trace: &mut Vec<String>) {
    use rc_proto_v777::entity::Meta;
    let Some(arrow) = arrow else {
        trace.push(format!("{place}: the arrow that hit the target is gone"));
        return;
    };
    let state = (arrow.in_ground, arrow.crit, arrow.pierce, arrow.pos);
    trace.push(format!("{place}: arrow {state:?} data {:?}", arrow.meta));
    if ProjectilePaths::current().sabotage != Sabotage::None {
        return;
    }
    assert!(state.0 && !state.1 && state.2 == 0, "{place}: the arrow's slot lost the result of its tick: {state:?}");
    let data = |i: u8| arrow.meta.iter().find(|m| m.0 == i).map(|m| m.1.clone());
    assert_eq!(data(10), Some(Meta::Bool(true)), "{place}: in-ground data out of step with the slot");
    assert_eq!(data(8), Some(Meta::Byte(0)), "{place}: critical data out of step with the slot");
}

/// Sabotage seen at every run: the slot not written back after the dimension change. Seen failing
/// by hand before the fix: the new path gave the slot the heavy fields only (in-ground data sent at
/// the pairing while the slot flew), the old path dropped the result of the tick (arrow in flight).
/// The stored trace holds the two full time packets of the dimension changes, with every clock
/// of the registry (the End's since the world clocks follow the registry: the trace without it
/// gives the fingerprint stored before, 0xca2b_01cf_6b4a_7917). It also holds the pairing data of
/// the players, where the effect particle list and the ambient flag are now sent apart: with the
/// clocks alone 0x3c90_1a42_eedf_3a46, with the pairing data alone 0xa1ea_a80d_54a1_214d, with both
/// the value below (the same on two runs).
#[test]
fn dimension_change_during_a_tick_keeps_the_parked_slot_whole() {
    let paths = [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)];
    let faults = [("slot-not-written-back", sabotaged(Sabotage::SlotNotWrittenBackAfterListChange))];
    assert_paths_agree("proj-away", AWAY_PACK, away_mid_tick, &paths, &faults, 0x1bae_cd29_ee20_7d50);
}

// ---------- Targets and owner in a crowd ----------

/// 1,000 mobs of eight species on a 60 × 60 square, every 50th one doubled at the same place (equal
/// distances), a few marked removed, one UUID carried twice; the arrow's shooter among them.
fn crowd(p: &mut Probe, r: &mut JRand) -> Vec<(i32, u128)> {
    let kinds =
        [Kind::Zombie, Kind::Skeleton, Kind::Pig, Kind::Slime, Kind::Chicken, Kind::Spider, Kind::Vex, Kind::Cow];
    let mut ids = Vec::new();
    for k in 0..1000 {
        let pos = [r.next_double() * 60.0 - 30.0, -60.0 + r.next_double() * 3.0, r.next_double() * 60.0 - 30.0];
        let kind = kinds[k % kinds.len()];
        let m = p.s.new_mob(kind, pos, 0.0);
        ids.push((m.id, m.uuid));
        p.s.add_mob(m);
        if k % 50 == 0 {
            let twin = p.s.new_mob(kind, pos, 0.0);
            ids.push((twin.id, twin.uuid));
            p.s.add_mob(twin);
        }
    }
    for &(id, _) in ids.iter().step_by(97) {
        if let Some(m) = p.s.mobs.get_mut(id) {
            m.removed = true;
        }
    }
    // A removed mob and a living one sharing a UUID: the first living one is the owner.
    let shared = ids[5].1;
    if let Some(m) = p.s.mobs.get_mut(ids[300].0) {
        m.uuid = shared;
    }
    ids
}

fn random_box(r: &mut JRand) -> Aabb {
    let c = [r.next_double() * 70.0 - 35.0, -61.0 + r.next_double() * 6.0, r.next_double() * 70.0 - 35.0];
    let h = [0.25 + r.next_double() * 4.0, 0.25 + r.next_double() * 2.0, 0.25 + r.next_double() * 4.0];
    Aabb { min: [c[0] - h[0], c[1] - h[1], c[2] - h[2]], max: [c[0] + h[0], c[1] + h[1], c[2] + h[2]] }
}

/// 10,000 random boxes: the targets (order included) and the nearest hit are those of the walk;
/// the owner lookup gives the walk's answer for every UUID of the crowd, removed, shared or absent.
/// Sabotage seen at every run: the last target at equal distance chosen instead of the first.
/// Seen failing by hand: the prefilter on the area shrunk by 0.3 (box 0); the owner scan resumed
/// after the hinted slot.
#[test]
fn targets_and_owner_match_the_walk_in_a_crowd() {
    let (mut p, dir) = open("proj-crowd");
    let mut r = JRand::new(0x5EED);
    let ids = crowd(&mut p, &mut r);
    let mut arrow = Proj::new(PKind::Arrow, p.s.proj_new_id(), [0.5, -59.0, 0.5], arrow_stack());
    arrow.left_owner = true;
    arrow.pierced = vec![ids[20].0, ids[30].0];
    let (old, new, fault) =
        (ProjectilePaths::OLD, ProjectilePaths::NEW, sabotaged(Sabotage::LastTargetAtEqualDistance));
    let mut owners_seen = 0;
    for &(_, u) in ids.iter().chain(&[(0, 0x1234_u128)]) {
        arrow.owner = Some(u);
        let want = with_paths(old, || p.s.proj_owner(&arrow));
        assert_eq!(with_paths(new, || p.s.proj_owner(&arrow)), want, "owner of UUID {u:x}");
        owners_seen += usize::from(want.is_some());
    }
    assert!(owners_seen > 900, "owners found: {owners_seen}");
    arrow.owner = Some(ids[10].1);
    arrow.left_owner = false;
    let (mut hits, mut fault_seen) = (0, 0);
    for k in 0..10_000 {
        let area = random_box(&mut r);
        let items = k % 7 == 0;
        let want = with_paths(old, || p.s.proj_targets(&arrow, &area, items));
        assert_eq!(with_paths(new, || p.s.proj_targets(&arrow, &area, items)), want, "targets of box {k}");
        // A segment across the box, sometimes from a twin's position (equal distances).
        let from =
            if k % 3 == 0 { want.first().map_or(area.min, |t| [t.2[0], t.2[1] + 0.5, t.2[2] - 2.0]) } else { area.min };
        let to = [area.max[0], area.max[1], area.max[2] + 2.0];
        let m = super::physics::margin(k % 12);
        let want_hit = with_paths(old, || p.s.entity_hit_nearest(&arrow, from, to, &area, m, items));
        assert_eq!(
            with_paths(new, || p.s.entity_hit_nearest(&arrow, from, to, &area, m, items)),
            want_hit,
            "nearest of box {k}"
        );
        hits += usize::from(want_hit.is_some());
        fault_seen +=
            usize::from(with_paths(fault, || p.s.entity_hit_nearest(&arrow, from, to, &area, m, items)) != want_hit);
    }
    assert!(hits > 1000, "too few hits to prove anything: {hits}");
    assert!(fault_seen > 0, "sabotage (last target at equal distance) not seen");
    eprintln!("proj-crowd: {owners_seen} owners, {hits} nearest hits, sabotage seen {fault_seen} times");
    finish(p, dir);
}

fn arrow_stack() -> Stack {
    Stack { id: "minecraft:arrow".into(), count: 1, components: None }
}

// ---------- Per-state rules ----------

/// Every block state: the chorus tags (both forms, one of them given a short-named copy), the
/// bobber's open-water class and water test, and the borrowed collision shape give the answer of
/// their text versions. Sabotages seen: the tag read for the next block; the short form dropped.
#[test]
fn state_rules_match_their_text_versions() {
    let (mut p, dir) = open("proj-states");
    // A tag known only under its short name: the lookup must try both forms.
    let some: Vec<u32> = (0..p.s.data.blocks.len() as u32).step_by(7).collect();
    p.s.block_tags.insert("entities_can_teleport_to".into(), some);
    p.s.tag_bits = crate::tag_bits::TagBits::new(&p.s.block_tags, &p.s.data);
    let tags = ["minecraft:entities_can_teleport_to", "minecraft:consumable_does_not_teleport_to"];
    let (old, new) = (ProjectilePaths::OLD, ProjectilePaths::NEW);
    let lily_pad = p.s.tag_bits.block_of_name("minecraft:lily_pad");
    assert!(lily_pad.is_some(), "lily pad missing from the block list");
    let mode = crate::tag_bits::mode();
    let (mut tagged, mut next_block_differs, mut short_dropped_differs) = (0, 0, 0);
    let mut classes = [0usize; 3];
    for s in 0..p.s.data.states.len() as u32 {
        for tag in tags {
            let want = with_paths(old, || p.s.chorus_block_tag(s, tag));
            assert_eq!(with_paths(new, || p.s.chorus_block_tag(s, tag)), want, "tag {tag} of state {s}");
            tagged += usize::from(want);
            let block = p.s.data.state(s).map_or(u32::MAX, |i| u32::from(i.block));
            next_block_differs += usize::from(p.s.tag_u32(tag, block.wrapping_add(1), mode) != want);
            short_dropped_differs += usize::from(p.s.tag_u32(tag, block, mode) != want);
        }
        let cell = p.s.open_water_cell(s, lily_pad);
        assert_eq!(cell, p.s.open_water_cell_by_name(s), "open-water class of state {s}");
        classes[cell as usize] += 1;
        let water = with_paths(old, || p.s.water_block_state(s));
        assert_eq!(with_paths(new, || p.s.water_block_state(s)), water, "water block of state {s}");
        let copied = with_paths(old, || p.s.with_collision_shape(s, <[Aabb]>::to_vec));
        assert_eq!(with_paths(new, || p.s.with_collision_shape(s, <[Aabb]>::to_vec)), copied, "shape of state {s}");
    }
    assert!(tagged > 100 && classes.iter().all(|&n| n > 0), "states too uniform: {tagged} tagged, classes {classes:?}");
    assert!(
        next_block_differs > 0 && short_dropped_differs > 0,
        "sabotages not seen: {next_block_differs} / {short_dropped_differs}"
    );
    finish(p, dir);
}

// ---------- Allocation of the tick copy ----------

/// The tick copy of a projectile carrying an effect stack, a weapon, data and pierced lists takes
/// its heavy fields without one allocation, and gives them back to its slot; the old full copy
/// allocates (the counter sees it).
#[test]
fn heavy_fields_move_without_allocating() {
    if cfg!(feature = "memstats") {
        return; // the bench allocator replaces the per-thread counter
    }
    let (mut p, dir) = open("proj-alloc");
    let mut a = Proj::new(PKind::Arrow, p.s.proj_new_id(), [0.5, -59.0, 0.5], arrow_stack());
    a.item.components = Some(rc_proto::nbt::Nbt::Compound(vec![(
        "minecraft:potion_contents".into(),
        rc_proto::nbt::Nbt::String("x".into()),
    )]));
    a.weapon = Some(Stack { id: "minecraft:crossbow".into(), count: 1, components: None });
    a.pierced = vec![1, 2, 3];
    a.pierce_killed = Some(vec![crate::mobs::mob::Ent::Player]);
    let before = format!("{a:?}");
    p.s.proj.list.push(a);
    let allocs = crate::test_alloc::allocations();
    let q = p.s.take_for_tick(0, ProjectilePaths::NEW);
    assert_eq!(crate::test_alloc::allocations() - allocs, 0, "the tick copy allocated");
    assert_eq!(format!("{q:?}"), before, "the tick copy is not the projectile");
    let shell = &p.s.proj.list[0];
    assert!(shell.item.id.is_empty() && shell.meta.is_empty() && shell.pierced.is_empty() && shell.weapon.is_none());
    assert_eq!((shell.id, shell.uuid, shell.pos, shell.kind), (q.id, q.uuid, q.pos, q.kind), "light copy");
    p.s.proj.list[0] = q;
    let allocs = crate::test_alloc::allocations();
    let full = p.s.take_for_tick(0, ProjectilePaths::OLD);
    assert!(crate::test_alloc::allocations() - allocs >= 4, "the counter does not see the full copy");
    assert_eq!(format!("{full:?}"), before);
    finish(p, dir);
}

// ---------- Chorus fruit ----------

/// 16 fruits eaten on the flat ground, then 16 in a closed stone room (most attempts fail): same
/// destinations, same draws, same events on every path.
fn chorus(p: &mut Probe, trace: &mut Vec<String>) {
    for k in 0..16 {
        let ok = p.s.chorus_teleport(16.0, true);
        trace.push(format!("open {k}: {ok} {:?} rng {:?}", p.s.pos, p.s.life.rng.parts()));
        record_events(p, trace);
    }
    cmd(p, "fill -12 -63 -12 12 -40 12 minecraft:stone");
    cmd(p, "fill -1 -60 -1 1 -58 1 minecraft:air");
    cmd(p, "fill 6 -60 6 8 -58 8 minecraft:air");
    cmd(p, "setblock 7 -60 7 minecraft:water");
    p.s.pos = [0.5, -60.0, 0.5];
    for k in 0..16 {
        let ok = p.s.chorus_teleport(16.0, true);
        trace.push(format!("cave {k}: {ok} {:?} rng {:?}", p.s.pos, p.s.life.rng.parts()));
        record_events(p, trace);
    }
}

#[test]
fn chorus_destinations_are_the_same_on_every_path() {
    let paths = [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)];
    assert_paths_agree("proj-chorus", &[], chorus, &paths, &[], 0x9af6_7ca9_3feb_4a03);
}

// ---------- Fishing ----------

/// A 9 × 9 pond two blocks deep: open water for 1,000 ticks, then roofed with stone one block
/// above the water for 1,000 more; the player reels in when a fish bites and casts again.
fn fishing(p: &mut Probe, trace: &mut Vec<String>) {
    cmd(p, "fill 2 -62 -4 10 -61 4 minecraft:water");
    p.s.slots[36 + p.s.held as usize] = Some(Stack { id: "minecraft:fishing_rod".into(), count: 1, components: None });
    (p.s.yaw, p.s.pitch) = (-90.0, 20.0);
    for t in 0..2000 {
        if t == 1000 {
            cmd(p, "fill 2 -59 -4 10 -59 4 minecraft:stone");
        }
        let biting = p.s.proj.hook.as_ref().is_some_and(|h| h.biting);
        if p.s.proj.hook.is_none() || biting {
            let used = p.s.fishing_rod_use(0);
            trace.push(format!("rod used at {t}: {used} (biting {biting})"));
            // The catch is noted then cleared, so that the next cast does not hook it.
            for d in &p.s.life.drops {
                trace.push(format!("caught {} {}", d.stack.id, d.stack.count));
            }
            let src = p.s.server_source();
            let _ = p.s.perform(src, "kill @e[type=minecraft:item]");
        }
        tick(p);
        if let Some(h) = &p.s.proj.hook {
            trace.push(format!(
                "hook {:?} {:?} open {} nibble {} lured {} hooked {}",
                h.state, h.p.pos, h.open_water, h.nibble, h.until_lured, h.until_hooked
            ));
        }
        record_events(p, trace);
    }
}

#[test]
fn fishing_is_the_same_on_every_path() {
    let paths = [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)];
    assert_paths_agree("proj-fishing", &[], fishing, &paths, &[], 0x87e4_fc29_7704_5ace);
}

// ---------- Mace smash of a mob ----------

/// A vindicator falling with a mace smashes a zombie among pigs and pillagers (its allies are not
/// pushed): same pushes and events on every path.
fn mob_mace_smash(p: &mut Probe, trace: &mut Vec<String>) {
    let (mut pigs, mut allies) = (Vec::new(), Vec::new());
    for k in 0..6 {
        let x = 3.0 + f64::from(k) * 0.8;
        let pig = p.s.new_mob(Kind::Pig, [x, -60.0, 2.0], 0.0);
        pigs.push(pig.id);
        p.s.add_mob(pig);
        let ally = p.s.new_mob(Kind::Pillager, [x, -60.0, -1.0], 0.0);
        allies.push(ally.id);
        p.s.add_mob(ally);
    }
    let target = p.s.new_mob(Kind::Zombie, [5.0, -60.0, 0.5], 0.0);
    let struck = crate::mace::Struck::mob(&p.s, &target);
    p.s.add_mob(target);
    for fall in [2.0, 4.0, 6.0, 9.0] {
        let mut attacker = p.s.new_mob(Kind::Vindicator, [5.0, -59.0, 1.5], 0.0);
        attacker.equipment[0] = Some(Stack { id: "minecraft:mace".into(), count: 1, components: None });
        attacker.fall_distance = fall;
        // Out of the list, as during its own tick.
        p.s.mob_smash(&mut attacker, &struck);
        record_events(p, trace);
        for id in pigs.iter().chain(&allies) {
            let v = p.s.mob_ref(*id).map(|m| m.vel);
            trace.push(format!("after a fall of {fall}: mob {id} velocity {v:?}"));
        }
    }
    let speed = |id: &i32| p.s.mob_ref(*id).map_or(0.0, |m| m.vel[0].abs() + m.vel[2].abs());
    assert!(pigs.iter().any(|id| speed(id) > 0.0), "no pig pushed");
    assert!(allies.iter().all(|id| speed(id) == 0.0), "an ally of the attacker was pushed");
}

#[test]
fn mob_mace_smash_is_the_same_on_every_path() {
    let paths = [("old", ProjectilePaths::OLD), ("new", ProjectilePaths::NEW)];
    assert_paths_agree("proj-mace", &[], mob_mace_smash, &paths, &[], 0x8bed_b110_d64d_71d4);
}
