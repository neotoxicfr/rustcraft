//! Proofs that the network sync of the mobs sends the same packets whatever path computes them
//! (`docs/perf.md`, mob network sync):
//!
//! - a crowd of 1,040 mobs of 28 species lives 400 ticks on the course of `body_movement_tests`
//!   while it is renamed, given effects, set on fire, hurt, killed, crammed in a pen, and taken
//!   out of the tracking and back (one chunk is never sent, and for 50 ticks none is). Every
//!   entity packet sent, in order, goes into one stored fingerprint, the same when the tracking
//!   refreshes the synced data itself and when 1, 2, 8 or 32 runs refresh it ahead;
//! - every species defines each synced index once, at the position its index gives;
//! - the network id of every species is its position in the registry;
//! - an attribute without modifier reads like the modifier passes;
//! - the mob and each family's state keep their recorded sizes;
//! - a large crowd left to a single thread is refreshed by the tracking;
//! - a bench started beside the proofs leaves their cross-checks on.
//!
//! Besides, every test of the crate compares each shortcut with the former computation when it
//! is taken (`mob::cross_checks`).

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::body_movement_tests::{COURSE, CROWD, cmd, game_ticks};
use super::mob::{Attr, Mob, cross_checks};
use super::species::Kind;
use super::track::{ForcedRefresh, RefreshAheadRule, force_refresh, mobs_per_thread_from};
use crate::blocks::probe::Probe;
use crate::server::Server;

/// Species added to the course crowd: synced values of their own shapes (owner, block position,
/// variants, long, optional int), slots defined out of order (wolf, cat), a tracking interval
/// of 2 ticks (allay), no velocity tracking (bat).
const OWN_SHAPES: [Kind; 16] = [
    Kind::Villager,
    Kind::Cat,
    Kind::Bee,
    Kind::Creeper,
    Kind::Enderman,
    Kind::Slime,
    Kind::Horse,
    Kind::Allay,
    Kind::Bat,
    Kind::Camel,
    Kind::Parrot,
    Kind::Panda,
    Kind::Drowned,
    Kind::Axolotl,
    Kind::Llama,
    Kind::IronGolem,
];

/// Chunk of the course never sent to the client: a mob that walks into it leaves the tracking,
/// and is paired again on the way out.
const NEVER_SENT_CHUNK: (i32, i32) = (1, 1);

/// Ticks during which no chunk is sent: every mob leaves the tracking and keeps living unseen.
const UNSEEN_TICKS: std::ops::Range<i32> = 150..200;

/// Rectangle of blocks (x from, x to, z from, z to) a crowd is scattered on.
pub(super) type Area = [f64; 4];

/// The course of `body_movement_tests`.
pub(super) const COURSE_AREA: Area = [0.0, 44.0, -2.0, 32.0];

/// What the stream contained, to prove the scenario exercised every kind of packet.
#[derive(Debug, Default, PartialEq, Eq)]
struct Coverage {
    pairings: usize,
    removals: usize,
    moves: usize,
    full_syncs: usize,
    metadata: usize,
    names_sent: usize,
    names_cleared: usize,
}

impl Coverage {
    fn count(&mut self, event: &EntityEvent) {
        match event {
            EntityEvent::Add(_) => self.pairings += 1,
            EntityEvent::Remove(_) => self.removals += 1,
            EntityEvent::Move(_) => self.moves += 1,
            EntityEvent::Sync(_) => self.full_syncs += 1,
            EntityEvent::Data { values, .. } => {
                self.metadata += 1;
                for (_, value) in values {
                    match value {
                        Meta::Text(Some(_)) => self.names_sent += 1,
                        Meta::Text(None) => self.names_cleared += 1,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// Fingerprint of the entity packets of a run (SHA-256 of each packet in order), their number,
/// and what they covered.
#[derive(Debug, PartialEq, Eq)]
struct StreamPrint {
    sha256: String,
    packets: usize,
    coverage: Coverage,
}

/// Every mob whose network id is `first` modulo `stride`, taken out of the list for `change`
/// (ids, unlike slots, do not shift when a mob dies).
fn change_every(p: &mut Probe, first: i32, stride: i32, mut change: impl FnMut(&mut Server, &mut Mob)) {
    for slot in 0..p.s.mobs.list.len() {
        let Some(mut m) = p.s.mobs.list.take(slot) else { continue };
        if m.id % stride == first {
            change(&mut p.s, &mut m);
        }
        p.s.mobs.list.put(slot, m);
    }
}

fn name(text: &str, m: &Mob) -> Option<Nbt> {
    Some(Nbt::String(format!("{text} {}", m.id)))
}

/// `mobs` mobs of 28 species scattered on `area`, one in 25 named with its name always shown,
/// one in 7 a baby.
pub(super) fn spawn_crowd(p: &mut Probe, mobs: usize, area: Area) {
    let kinds: Vec<Kind> = CROWD.into_iter().chain(OWN_SHAPES).collect();
    let mut r: u64 = 0xC6_07;
    for i in 0..mobs {
        r ^= r << 13;
        r ^= r >> 7;
        r ^= r << 17;
        let (fx, fz) = ((r & 0xFFFF) as f64 / 65536.0, ((r >> 16) & 0xFFFF) as f64 / 65536.0);
        let at =
            [area[0] + fx * (area[1] - area[0]), -58.0 + ((r >> 32) % 3) as f64, area[2] + fz * (area[3] - area[2])];
        let mut m = p.s.new_mob(kinds[i % kinds.len()], at, ((r >> 40) % 360) as f32);
        m.persistent = true;
        if i % 7 == 0 {
            m.age = -24000;
        }
        if i % 25 == 0 {
            m.custom_name = name("Elder", &m);
            m.x.name_visible = true;
        }
        p.s.add_mob(m);
    }
}

/// 40 cows in a pen of one block: they hurt each other by cramming until few are left.
fn spawn_crammed_pen(p: &mut Probe) {
    cmd(p, "fill 50 -60 20 52 -59 22 minecraft:stone");
    cmd(p, "fill 51 -60 21 51 -59 21 minecraft:air");
    for _ in 0..40 {
        let mut m = p.s.new_mob(Kind::Cow, [51.5, -60.0, 21.5], 0.0);
        m.persistent = true;
        p.s.add_mob(m);
    }
}

/// Every chunk the flat test world loads, sent to the client, but `NEVER_SENT_CHUNK`.
pub(super) fn send_loaded_chunks(p: &mut Probe) {
    let loaded = (-2..=9).flat_map(|x| (-2..=2).map(move |z| (x, z)));
    p.s.sent.extend(loaded.filter(|c| *c != NEVER_SENT_CHUNK));
}

/// What happens to the crowd at the start of tick `tick`.
fn scripted_changes(p: &mut Probe, tick: i32, sent_chunks: &mut Vec<(i32, i32)>) {
    match tick {
        30 => change_every(p, 0, 9, |_, m| m.custom_name = name("Renamed", m)),
        // The same text again (nothing to send), then another one.
        45 => change_every(p, 0, 9, |_, m| m.custom_name = name("Renamed", m)),
        46 => change_every(p, 0, 9, |_, m| m.custom_name = name("Renamed twice", m)),
        60 => change_every(p, 0, 18, |_, m| m.custom_name = None),
        80 => {
            change_every(p, 1, 11, |s, m| {
                let _ = s.give_mob_effect(m, "minecraft:invisibility", 60, 0);
            });
            change_every(p, 2, 13, |s, m| {
                let _ = s.give_mob_effect(m, "minecraft:glowing", 100, 0);
            });
        }
        100 => {
            change_every(p, 3, 17, |_, m| m.x.silent = true);
            change_every(p, 4, 19, |_, m| m.surf.no_gravity = true);
        }
        120 => change_every(p, 5, 23, |_, m| m.fire = 80),
        tick if tick == UNSEEN_TICKS.start => *sent_chunks = p.s.sent.drain().collect(),
        // Unseen: a name and an effect that only the next pairing shows.
        165 => {
            change_every(p, 6, 9, |_, m| m.custom_name = name("Renamed unseen", m));
            change_every(p, 7, 29, |s, m| {
                let _ = s.give_mob_effect(m, "minecraft:glowing", 200, 0);
            });
        }
        tick if tick == UNSEEN_TICKS.end => p.s.sent.extend(sent_chunks.drain(..)),
        250 => change_every(p, 8, 15, |s, m| {
            let _ = s.mob_hurt(m, "minecraft:generic", 3.0, None, None);
        }),
        300 => change_every(p, 9, 31, |s, m| {
            let _ = s.mob_hurt(m, "minecraft:generic", 2000.0, None, None);
        }),
        _ => {}
    }
}

/// The scenario of the module: fingerprint of every entity packet of its 400 ticks. `world` names
/// the world folder (one per test: they run side by side).
fn tracked_crowd_stream(world: &str) -> StreamPrint {
    let (mut p, _dir) = crate::tests_g3f13::open(world);
    cmd(&mut p, crate::tests_opt::PLAYER_TAKES_NO_DAMAGE);
    for c in COURSE {
        cmd(&mut p, c);
    }
    spawn_crowd(&mut p, 1000, COURSE_AREA);
    spawn_crammed_pen(&mut p);
    send_loaded_chunks(&mut p);
    let _ = p.take_events();
    let mut sha = ring::digest::Context::new(&ring::digest::SHA256);
    let (mut packets, mut coverage) = (0, Coverage::default());
    let mut sent_chunks = Vec::new();
    for tick in 0..400 {
        scripted_changes(&mut p, tick, &mut sent_chunks);
        game_ticks(&mut p, 1);
        for event in p.take_events() {
            if let Event::Entity(event) = event {
                packets += 1;
                coverage.count(&event);
                sha.update(format!("{event:?}").as_bytes());
            }
        }
    }
    assert!(!p.s.sent.contains(&NEVER_SENT_CHUNK), "the chunk left out was sent");
    let sha256 = sha.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    StreamPrint { sha256, packets, coverage }
}

/// Expected stream of the scenario, recorded on the paths before this module's optimisations
/// (synced slot searched on every write behind a call, name copied on every refresh, unsent
/// changes copied, refresh by the tracking, attribute through the modifier passes): 71e9a6de…,
/// 329,142 packets. Two fidelity lots then change what the crowd sends, each the only cause of
/// its step (the value before still holds just before its merge): the navigation fixes of G6-F20
/// move the mobs (52544dff…, 328,062 packets), then the effect particle list and the ambient flag
/// sent apart (30d6a2a8…, 328,062 packets), then the mobs' "on fire" flag set at their base tick
/// like the game, not after the crossed blocks and the sun (the only change of
/// that fix: 115 packets fewer: cb196f32…d230, 327,947 packets). Then L15-ENTITES: the crowd killed
/// the player, who now leaves the world 20 ticks after its death (its client forgets every mob and
/// nothing is simulated: 9e700946…0e13, 180,776 packets), so the player takes no damage now: the
/// value before (3c4fa7a6…1c4e, 328,465 packets), the same with the removal neutralised (checked).
/// Then the boats work: the push between two mobs multiplies in the game's order (× pow, then
/// × 0.05), bit for bit: 6d0932d4…5719, 329,261 packets; the former order alone gives back the
/// value before (checked). Then L16-RAMASSAGE: a fox that takes one piece of a
/// stack no longer leaves the rest twice (the rest leaves as a new item with a random push and the
/// original is removed): alone on the value before it gave b06f0a33…356c, 324,837 packets, and
/// with the old fox put back the boats value comes back (checked). Both together give the value
/// below, measured right after their merge and unchanged by the later merges of the wave. Then
/// L17-LAN-CREATURES: a stared-at enderman of the crowd keeps waiting, its body turned toward
/// the player, then becomes hostile, instead of dropping its stare objective at once (d82fec0f…
/// 31d8, 324,298 packets before; with the old stare objective put back the value before comes
/// back, checked; the target search over every player gives the same stream in singleplayer).
/// Every path of this module gives it alike.
const TRACKED_CROWD_STREAM: (&str, usize) =
    ("5a76b2f3bb2f49526e0fb15c2fa08045977c3aa5d14a136dd4fa508b1f2b2f62", 324_927);

/// The scenario in a fresh thread (the per-thread test seed restarts there) refreshing its
/// synced data as `refresh` says (`None`: the game's rule), checked against the stored stream.
fn assert_stored_stream(what: &'static str, refresh: Option<ForcedRefresh>) {
    let run = move || {
        force_refresh(refresh);
        tracked_crowd_stream(&format!("mob-stream-{}", what.replace(' ', "-")))
    };
    let got = std::thread::spawn(run).join().expect("simulation");
    eprintln!("{what}: stream {} ({} packets), {:?}", got.sha256, got.packets, got.coverage);
    let c = &got.coverage;
    assert!(
        c.pairings > 2000 && c.removals > 1000 && c.moves > 10_000 && c.full_syncs > 100 && c.metadata > 5000,
        "{what}: the scenario no longer covers every packet: {c:?}"
    );
    assert!(c.names_sent > 200 && c.names_cleared > 30, "{what}: names sent and cleared: {c:?}");
    assert_eq!(
        (got.sha256.as_str(), got.packets),
        TRACKED_CROWD_STREAM,
        "{what}: the packets sent changed: an optimisation must never change them; an intended \
         behaviour fix replaces TRACKED_CROWD_STREAM with ({:?}, {})",
        got.sha256,
        got.packets
    );
}

#[test]
fn tracked_crowd_sends_the_stored_stream() {
    assert_stored_stream("the rule of the game", None);
}

#[test]
fn stream_is_the_same_refreshed_by_the_tracking() {
    assert_stored_stream("refreshed by the tracking", Some(ForcedRefresh::InTracking));
}

#[test]
fn stream_is_the_same_refreshed_ahead_in_1_run() {
    assert_stored_stream("refreshed ahead in 1 run", Some(ForcedRefresh::Ahead { runs: 1 }));
}

#[test]
fn stream_is_the_same_refreshed_ahead_in_2_runs() {
    assert_stored_stream("refreshed ahead in 2 runs", Some(ForcedRefresh::Ahead { runs: 2 }));
}

#[test]
fn stream_is_the_same_refreshed_ahead_in_8_runs() {
    assert_stored_stream("refreshed ahead in 8 runs", Some(ForcedRefresh::Ahead { runs: 8 }));
}

#[test]
fn stream_is_the_same_refreshed_ahead_in_32_runs() {
    assert_stored_stream("refreshed ahead in 32 runs", Some(ForcedRefresh::Ahead { runs: 32 }));
}

/// The refresh ahead visits every mob once, whatever the cut: more runs than mobs, one run,
/// an empty list, an emptied slot.
#[test]
fn update_in_parallel_visits_every_mob_once() {
    let (mut p, _dir) = crate::tests_g3f13::open("mob-parallel-update");
    p.s.mobs.list.update_in_parallel(4, |m| m.air += 1); // empty list
    spawn_crowd(&mut p, 37, COURSE_AREA);
    let taken = p.s.mobs.list.take(5).expect("mob of slot 5");
    for runs in [0, 1, 2, 7, 37, 64] {
        p.s.mobs.list.iter_mut().for_each(|m| m.air = 0);
        p.s.mobs.list.update_in_parallel(runs, |m| m.air += 1);
        let visits: Vec<i32> = p.s.mobs.list.iter().map(|m| m.air).collect();
        assert_eq!(visits, vec![1; 36], "{runs} runs");
    }
    p.s.mobs.list.put(5, taken);
}

/// Every species: each synced index is defined once and sits at the position its index gives,
/// so the slot read directly is the slot a search finds; the refresh then reads and writes
/// every slot of the species through both (cross-check in `Mob::synced_position`).
#[test]
fn every_species_defines_each_synced_index_once() {
    let (mut p, _dir) = crate::tests_g3f13::open("mob-synced-slots");
    let mut searched = Vec::new();
    for kind in Kind::ALL {
        let mut m = p.s.new_mob(kind, [8.5, -60.0, 8.5], 0.0);
        let indices: Vec<u8> = m.data.iter().map(|s| s.index).collect();
        let mut unique = indices.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), indices.len(), "{kind:?} defines a synced index twice: {indices:?}");
        if indices.iter().enumerate().any(|(at, index)| usize::from(*index) != at) {
            searched.push((kind, indices.clone()));
        }
        m.sync_data();
        for index in indices {
            let by_search = m.data.iter().find(|s| s.index == index).map(|s| &s.value);
            assert_eq!(m.get(index), by_search, "{kind:?}, synced index {index}");
        }
        assert_eq!(m.get(200), None, "{kind:?}: undefined index");
    }
    assert!(
        searched.is_empty(),
        "species whose synced values are not at their index (served by the search): {searched:?}"
    );
}

/// The network id of every species is the position of its name in the entity type registry;
/// an unknown name gives 0.
#[test]
fn network_id_of_every_species_is_its_registry_position() {
    let (p, _dir) = crate::tests_g3f13::open("mob-kind-ids");
    let types = p.s.data.registry("minecraft:entity_type").expect("entity type registry");
    for kind in Kind::ALL {
        let name = kind.info().name;
        let position = types.entries.iter().position(|n| n == name).unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(p.s.entity_kind_id(name), position as i32, "{name}");
    }
    assert_eq!(p.s.entity_kind_id("minecraft:no_such_entity"), 0);
}

/// An attribute without modifier: the shortcut gives the base brought back into the bounds, bit
/// for bit what the modifier passes give (in bounds, above, below, undefined with and without a
/// finite minimum, infinite, negative zero). The shortcut is called directly, so the proof
/// holds whatever its switch says.
#[test]
fn attribute_without_modifier_reads_like_the_modifier_passes() {
    let nan = f64::NAN;
    let cases = [
        (0.7, 0.0, 1024.0, 0.7),
        (5000.0, 0.0, 1024.0, 1024.0),
        (-3.0, 0.0, 1024.0, 0.0),
        (nan, 1.0, 1024.0, 1.0),
        (nan, f64::NEG_INFINITY, f64::INFINITY, nan),
        (f64::INFINITY, 0.0, 2048.0, 2048.0),
        (-0.0, f64::NEG_INFINITY, f64::INFINITY, -0.0),
    ];
    for (base, lo, hi, expected) in cases {
        let attr = Attr::ranged(base, lo, hi);
        let shortcut = attr.value_without_modifier().to_bits();
        assert_eq!(shortcut, expected.to_bits(), "base {base} in [{lo}, {hi}]");
        assert_eq!(shortcut, attr.value_through_modifiers().to_bits(), "base {base} in [{lo}, {hi}]");
        assert_eq!(shortcut, attr.value().to_bits(), "base {base} in [{lo}, {hi}]");
    }
    // With modifiers the passes run; without them again, the base comes back.
    let mut speed = Attr::ranged(0.25, 0.0, 1024.0);
    speed.set_mod("test:added", 0.1, 0);
    speed.set_mod("test:share_of_base", 0.5, 1);
    speed.set_mod("test:factor", -0.2, 2);
    let added = 0.25 + 0.1;
    assert_eq!(speed.value().to_bits(), ((added + added * 0.5) * (1.0 + -0.2f64)).to_bits());
    for id in ["test:added", "test:share_of_base", "test:factor"] {
        speed.remove_mod(id);
    }
    assert_eq!(speed.value().to_bits(), 0.25f64.to_bits());
    assert_eq!(speed.value_without_modifier().to_bits(), 0.25f64.to_bits());
}

/// The mob and the state each family adds to it: name, size in bytes, size recorded (64-bit
/// target). Every mob carries the state of every family: a field added to one of them is paid by
/// all the mobs of the world.
#[cfg(target_pointer_width = "64")]
fn mob_struct_sizes() -> Vec<(&'static str, usize, usize)> {
    use std::mem::size_of;
    vec![
        // A2-W1: +16, the boxed locator bar range (absent on almost every mob) and the padding
        // to the mob's 16-byte alignment.
        ("Mob", size_of::<Mob>(), 6992),
        ("one synced value (Synced)", size_of::<super::mob::Synced>(), 208),
        ("a synced value's content (Meta)", size_of::<Meta>(), 96),
        ("Attrs", size_of::<super::mob::Attrs>(), 1632),
        ("Attr", size_of::<Attr>(), 80),
        ("Tracker", size_of::<super::track::Tracker>(), 184),
        ("Nav", size_of::<super::nav::Nav>(), 224),
        ("goal Selector", size_of::<super::goals::Selector>(), 96),
        ("variants::Extra", size_of::<super::variants::Extra>(), 208),
        ("cubes::CubeState", size_of::<super::cubes::CubeState>(), 248),
        ("surface::Surf", size_of::<super::surface::Surf>(), 208),
        ("mounts::Mount", size_of::<super::mounts::Mount>(), 272),
        ("golems::Golem", size_of::<super::golems::Golem>(), 624),
        ("illagers::Ill", size_of::<super::illagers::Ill>(), 176),
        ("aquatic::Aqua", size_of::<super::aquatic::Aqua>(), 36),
        ("sealife::Aqua", size_of::<super::sealife::Aqua>(), 176),
        ("brainy::Bm", size_of::<super::brainy::Bm>(), 168),
        ("wild::Wild", size_of::<super::wild::Wild>(), 304),
        ("nether::Neth", size_of::<super::nether::Neth>(), 32),
        ("piglins::Pig", size_of::<super::piglins::Pig>(), 192),
        ("recent::Rec", size_of::<super::recent::Rec>(), 432),
        ("end::EndMob", size_of::<super::end::EndMob>(), 32),
        ("wither::Wither", size_of::<super::wither::Wither>(), 144),
        ("villager::Npc (boxed)", size_of::<super::villager::Npc>(), 192),
        ("leash::Leash (boxed)", size_of::<super::leash::Leash>(), 80),
    ]
}

/// A struct of the mob that changes size fails here. Grown: shrink it (box what few mobs use) or
/// record the new size knowingly. Shrunk: record the new size, so that it cannot grow back unseen.
#[test]
#[cfg(target_pointer_width = "64")]
fn mob_structs_keep_their_recorded_sizes() {
    let sizes = mob_struct_sizes();
    let measured: Vec<(&str, usize)> = sizes.iter().map(|(name, bytes, _)| (*name, *bytes)).collect();
    let recorded: Vec<(&str, usize)> = sizes.iter().map(|(name, _, recorded)| (*name, *recorded)).collect();
    assert_eq!(measured, recorded, "size in bytes of the structs of the mob (left: now, right: recorded)");
}

/// The refresh ahead of the tracking: the server thread alone under 1,024 mobs, then one run per
/// 512 mobs and at most one per thread; a large crowd left to a single thread goes back to the
/// refresh by the tracking (walking it twice loses, `docs/perf.md`) unless its switch keeps it.
#[test]
fn large_crowd_with_a_single_thread_is_refreshed_by_the_tracking() {
    let rule = |threads, large_crowd_alone| RefreshAheadRule { threads, mobs_per_thread: 512, large_crowd_alone };
    let runs = |mobs, threads| rule(threads, false).runs(mobs);
    for threads in [1, 2, 8, 32] {
        assert_eq!([runs(0, threads), runs(740, threads), runs(1023, threads)], [Some(1); 3], "{threads} threads");
    }
    assert_eq!([runs(1024, 32), runs(1535, 32), runs(1536, 32), runs(4249, 32)], [Some(2), Some(2), Some(3), Some(8)]);
    assert_eq!([runs(4249, 2), runs(4249, 4), runs(100_000, 32)], [Some(2), Some(4), Some(32)]);
    // One thread (one core, `RUSTCRAFT_THREADS=1`, or the sharing switched off).
    assert_eq!([runs(1024, 1), runs(4249, 1), runs(4249, 0)], [None; 3]);
    let kept_ahead = |mobs| rule(1, true).runs(mobs);
    assert_eq!([kept_ahead(740), kept_ahead(1024), kept_ahead(4249)], [Some(1); 3]);
    // Another count per thread (quiet-machine bench): the helpers and the fallback follow it.
    let per_256 = |threads| RefreshAheadRule { threads, mobs_per_thread: 256, large_crowd_alone: false };
    assert_eq!([per_256(32).runs(511), per_256(32).runs(1277), per_256(1).runs(512)], [Some(1), Some(4), None]);
    assert_eq!([mobs_per_thread_from(None), mobs_per_thread_from(Some(" 256 "))], [512, 256]);
    assert_eq!([mobs_per_thread_from(Some("0")), mobs_per_thread_from(Some("many"))], [512, 512]);
}

/// A bench started beside the proofs (`--include-ignored`) leaves their cross-checks on; started
/// alone (`--ignored`) it may turn them off.
#[test]
fn bench_beside_the_proofs_keeps_their_cross_checks() {
    let arguments = |flag: &str| ["rc_sim.exe", "bench_mob_network_sync", flag, "--nocapture"].map(String::from);
    assert!(cross_checks::proofs_run_beside_benches(arguments("--include-ignored")));
    assert!(!cross_checks::proofs_run_beside_benches(arguments("--ignored")));
    assert!(!cross_checks::turn_off_unless_proofs_run(arguments("--include-ignored")), "turned off beside the proofs");
    assert!(cross_checks::on(), "cross-checks off in a process that runs the proofs");
}
