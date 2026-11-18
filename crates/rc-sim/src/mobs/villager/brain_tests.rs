//! Equivalence proofs of the villager optimisations on the village of the bench model world
//! (`RUSTCRAFT_OPT_WORLD`, otherwise `target/perf/model`; without it the tests skip, explicit failure
//! under `RUSTCRAFT_OPT_REQUIRE=1`): twenty villagers of every kind (farmers carrying bread, wheat and
//! seeds, jobless adults, a nitwit, babies, other professions), a cat and a zombie, the player holding
//! wheat among them, three stretches of 200 ticks across the work, meeting and rest hours.
//!
//! Each tick is reduced to one SHA-256 line: every mob (memories, behaviour nodes, navigation,
//! inventory, offers), the items on the ground and, every 50 ticks, the blocks of the village. The
//! reduction names what the player and a server can see or what the next ticks read, never how it
//! is stored, so a new representation gives the same line. Each run has its own thread: the test
//! seeds restart there, so two runs differ only by their switches.

use std::fmt::Write as _;

use rc_data::PropKey;
use rc_proto::nbt::Nbt;

use super::super::jrand::JRand;
use super::super::mob::Mob;
use super::super::nav::search_tests::village_server;
use super::super::species::Kind;
use super::behave::Beh;
use super::brain::{Brain, FarmlandSearch, GateChild, Mem, Node};
use super::switches::{FORCED, VillagerSwitches};
use crate::player::Stack;
use crate::server::Server;

/// Village of the bench model world (same point as the path search proofs).
const VILLAGE: [i32; 2] = [-1408, 368];
/// Day time of each stretch (the model world's clock stands still): work, meeting, rest.
pub(super) const STRETCHES: [i64; 3] = [2100, 9100, 12100];
const TICKS_PER_STRETCH: usize = 200;

/// Fingerprint of the whole run (600 tick lines), recorded on the code before this work
/// (integ/optim3-v3 293122a5). An optimisation never changes it.
/// L17-DONNEES: the hurt sensor ages the last hit on the world's game time like the game (it read
/// the session's tick count, so a hit villager of this old world stayed hurt): the value below,
/// the only cause of the change (the old sensor put back alone: 4720b587…, measured).
const EXPECTED_RUN: &str = "b419118bb80e9d7a05ae0eaeea8646234227d4323ec7fae4770871e8a91e5f67";

/// What each villager is: profession, level, baby, starting inventory.
struct Role {
    profession: &'static str,
    level: i32,
    baby: bool,
    inventory: &'static [(&'static str, i32)],
}

const FARMER_GOODS: &[(&str, i32)] = &[("bread", 6), ("wheat", 40), ("wheat_seeds", 20), ("bone_meal", 8)];

const fn role(profession: &'static str, level: i32, baby: bool, inventory: &'static [(&'static str, i32)]) -> Role {
    Role { profession, level, baby, inventory }
}

const ROLES: [Role; 20] = [
    role("farmer", 1, false, FARMER_GOODS),
    role("farmer", 1, false, FARMER_GOODS),
    role("farmer", 1, false, FARMER_GOODS),
    role("farmer", 1, false, FARMER_GOODS),
    role("farmer", 1, false, FARMER_GOODS),
    role("farmer", 1, false, FARMER_GOODS),
    role("farmer", 2, false, &[("wheat", 10), ("beetroot_seeds", 12)]),
    role("farmer", 2, false, &[("wheat", 10), ("beetroot_seeds", 12)]),
    role("none", 1, false, &[("bread", 3)]),
    role("none", 1, false, &[("bread", 3)]),
    role("none", 1, false, &[("bread", 3)]),
    role("none", 1, false, &[]),
    role("nitwit", 1, false, &[("bread", 4)]),
    role("none", 1, true, &[]),
    role("none", 1, true, &[]),
    role("none", 1, true, &[]),
    role("librarian", 1, false, &[("bread", 3)]),
    role("armorer", 1, false, &[]),
    role("cleric", 1, false, &[("bread", 3)]),
    role("fletcher", 1, false, &[]),
];

pub(super) fn set_day_time(s: &mut Server, t: i64) {
    match s.clocks.iter_mut().find(|(n, _)| n == "minecraft:overworld") {
        Some(c) => c.1 = t,
        None => s.clocks.push(("minecraft:overworld".into(), t)),
    }
    s.time = t;
}

/// A spot on open ground (sand, grass or path) within 14 blocks of the village centre.
fn ground_spot(s: &Server, draw: &mut JRand) -> [f64; 3] {
    let mut seen = Vec::new();
    for _ in 0..500 {
        let x = VILLAGE[0] + draw.next_int_bound(29) - 14;
        let z = VILLAGE[1] + draw.next_int_bound(29) - 14;
        let y = s.hm_no_leaves(x, z);
        let below = s.bname([x, y - 1, z]);
        // A desert village: sand and paths between the houses (never a roof).
        if matches!(below, "minecraft:sand" | "minecraft:grass_block" | "minecraft:dirt_path") {
            return [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
        }
        seen.push(format!("{below}@{y}"));
    }
    panic!("no open ground around the village: {:?}", &seen[..20]);
}

fn add_mob(s: &mut Server, kind: Kind, pos: [f64; 3]) -> i32 {
    let mut m = s.new_mob(kind, pos, 0.0);
    m.persistent = true;
    let id = m.id;
    s.add_mob(m);
    id
}

/// A villager of role `r`, working at `job_site` if given (the farmers' composters: they would
/// otherwise spend the work stretch looking for one).
fn add_villager(s: &mut Server, pos: [f64; 3], r: &Role, job_site: Option<[i32; 3]>) {
    let mut m = s.new_mob(Kind::Villager, pos, 0.0);
    m.persistent = true;
    let prof = super::super::variants::PROFESSIONS.iter().position(|p| *p == r.profession).expect("profession") as i32;
    let desert = super::super::variants::VILLAGER_TYPES.iter().position(|t| *t == "desert").expect("type") as i32;
    m.villager = [desert, prof, r.level];
    m.x.vdata_final = true;
    if r.baby {
        m.age = -24000;
    }
    // The brain depends on the profession and the age: built again for this villager.
    s.npc_init(&mut m);
    for &(item, count) in r.inventory {
        let rest = s.npc_add(&mut m, Stack { id: format!("minecraft:{item}"), count, components: None });
        assert!(rest.is_none(), "inventory full");
    }
    if let Some(p) = job_site {
        let g = s.gpos_here(p);
        if let Some(b) = m.npc.as_mut().and_then(|n| n.brain.as_mut()) {
            b.mem.job_site = Some(g);
        }
    }
    s.add_mob(m);
}

/// A field of farmland sown with wheat of every age on the sand around `at` (the columns whose
/// surface is at the same height), a composter in its middle: the village of the model world has
/// none near its centre. Farmland blocks and the composter.
fn sow_field(s: &mut Server, at: [f64; 3]) -> (Vec<[i32; 3]>, [i32; 3]) {
    let (x0, y, z0) = (at[0].floor() as i32, at[1] as i32 - 1, at[2].floor() as i32);
    let farmland = s.view().default_state(s.bs.t.id.farmland);
    let wheat = s.view().default_state(s.bs.t.block_id("wheat"));
    let mut out = Vec::new();
    for dx in -3..=3 {
        for dz in -3..=3 {
            let p = [x0 + dx, y, z0 + dz];
            if s.hm_no_leaves(p[0], p[2]) != y + 1 || s.bname(p) != "minecraft:sand" {
                continue;
            }
            s.set_block(p, farmland, 3);
            let ripe = s.view().set_int(wheat, PropKey::Age, (dx + dz).rem_euclid(8));
            s.set_block([p[0], y + 1, p[2]], ripe, 3);
            out.push(p);
        }
    }
    let composter = [x0, y + 1, z0];
    let block = s.view().default_state(s.bs.t.block_id("composter"));
    s.set_block(composter, block, 3);
    (out, composter)
}

/// The villagers (farmers on a field of their own), a cat, a zombie a little away, the player
/// holding wheat next to the first farmer.
fn populate(s: &mut Server) {
    s.game_mode = 0;
    let mut draw = JRand::new(0xC6_0011);
    let mut fields = Vec::new();
    let mut composters = Vec::new();
    for _ in 0..2 {
        let at = ground_spot(s, &mut draw);
        let (field, composter) = sow_field(s, at);
        fields.extend(field);
        composters.push(composter);
    }
    assert!(fields.len() >= 16, "village fields: {} farmland blocks", fields.len());
    for (i, r) in ROLES.iter().enumerate() {
        if r.profession == "farmer" {
            let f = fields[(i * 5) % fields.len()];
            let at = [f64::from(f[0]) + 0.5, f64::from(f[1]) + 1.0, f64::from(f[2]) + 0.5];
            add_villager(s, at, r, Some(composters[i % 2]));
        } else {
            let at = ground_spot(s, &mut draw);
            add_villager(s, at, r, None);
        }
    }
    let cat = ground_spot(s, &mut draw);
    add_mob(s, Kind::Cat, cat);
    let zombie = ground_spot(s, &mut draw);
    add_mob(s, Kind::Zombie, zombie);
    let f = fields[0];
    s.pos = [f64::from(f[0]) + 2.0, f64::from(f[1]) + 1.0, f64::from(f[2]) + 0.5];
    let held = 36 + s.held as usize;
    s.slots[held] = Some(Stack { id: "minecraft:wheat".into(), count: 20, components: None });
}

/// The village server with the scene's mobs and fields (`None` without the model world).
pub(super) fn scene(tag: &str) -> Option<Server> {
    let mut s = village_server(tag)?;
    populate(&mut s);
    Some(s)
}

/// One run of the scenario with the switches `switches`, on its own thread: one line per tick.
fn run_scenario(tag: &str, switches: VillagerSwitches) -> Option<Vec<String>> {
    let tag = tag.to_owned();
    let run = move || {
        FORCED.set(Some(switches));
        let mut s = scene(&tag)?;
        let mut lines = Vec::new();
        let mut seen = std::collections::BTreeMap::new();
        for &day in &STRETCHES {
            set_day_time(&mut s, day);
            for t in 0..TICKS_PER_STRETCH {
                s.game_tick();
                s.tick += 1;
                lines.push(tick_line(&s, t % 50 == 49));
                count_situations(&s, &mut seen);
            }
        }
        eprintln!("[villager] {tag}: villager-ticks per situation {seen:?}");
        Some(lines)
    };
    std::thread::Builder::new().stack_size(32 << 20).spawn(run).expect("scenario thread").join().expect("scenario run")
}

/// Coverage of the scenario: how many villager-ticks met each situation the optimised code handles.
fn count_situations(s: &Server, seen: &mut std::collections::BTreeMap<&'static str, u32>) {
    let mut bump = |k: &'static str, on: bool| {
        if on {
            *seen.entry(k).or_insert(0) += 1;
        }
    };
    for m in s.mobs.of_kind(Kind::Villager) {
        let Some(n) = &m.npc else { continue };
        bump("sleeping", n.sleeping.is_some());
        bump("trading_with_player", n.trading);
        bump("item_in_hand", m.equipment[0].is_some());
        let Some(b) = &n.brain else { continue };
        let mem = &b.mem;
        bump("path", mem.path);
        bump("doors", mem.doors.is_some());
        bump("interaction_player", mem.interaction == Some(super::super::mob::Ent::Player));
        bump("interaction_mob", matches!(mem.interaction, Some(super::super::mob::Ent::Mob(_))));
        bump("breed", mem.breed.is_some());
        bump("babies", mem.babies.is_some());
        bump("secondary", mem.secondary.is_some());
        bump("hostile", mem.hostile.is_some());
        bump("job_site", mem.job_site.is_some());
        bump("home", mem.home.is_some());
        bump("meeting_point", mem.meeting_point.is_some());
        bump("wanted_item", mem.wanted_item.is_some());
        bump("look", mem.look.is_some());
        bump("panic", b.active == super::brain::Act::Panic);
        bump("meet", b.active == super::brain::Act::Meet);
        bump("rest", b.active == super::brain::Act::Rest);
        bump("work", b.active == super::brain::Act::Work);
    }
}

fn hex(d: ring::digest::Digest) -> String {
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Digest of one tick: every mob, every item on the ground, the village blocks if `blocks`.
fn tick_line(s: &Server, blocks: bool) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut line = String::new();
    for m in s.mobs.list.iter() {
        line.clear();
        mob_state(&mut line, m);
        ctx.update(line.as_bytes());
        ctx.update(b"\n");
    }
    for d in &s.life.drops {
        line.clear();
        let _ = write!(line, "item {} {:?} {:?} {:?} {}", d.id, d.stack, d.pos, d.vel, d.pickup_delay);
        ctx.update(line.as_bytes());
        ctx.update(b"\n");
    }
    if blocks {
        village_blocks(&mut ctx, s);
    }
    hex(ctx.finish())
}

/// The states of the blocks of the village (crops, doors, beds, composters).
fn village_blocks(ctx: &mut ring::digest::Context, s: &Server) {
    let y0 = s.hm_no_leaves(VILLAGE[0], VILLAGE[1]);
    for x in VILLAGE[0] - 32..=VILLAGE[0] + 32 {
        for z in VILLAGE[1] - 32..=VILLAGE[1] + 32 {
            for y in y0 - 6..=y0 + 10 {
                ctx.update(&s.st([x, y, z]).to_le_bytes());
            }
        }
    }
}

fn mob_state(out: &mut String, m: &Mob) {
    let _ = write!(
        out,
        "{} {:?} {:?} {:?} {:?} {:?} {:?} {} {} {:?} {:?} {:?} {}",
        m.id,
        m.kind,
        m.pos(),
        m.vel,
        m.yaw,
        m.pitch,
        m.health,
        m.age,
        m.dead,
        m.villager,
        m.equipment[0],
        m.nav,
        m.x.villager_xp
    );
    let Some(n) = &m.npc else { return };
    let _ = write!(
        out,
        " | {:?} {} {:?} {} {} {:?} {} {} {} {} {} {} {:?} {}",
        n.offers,
        n.trading,
        n.inv,
        n.unhappy,
        n.food,
        n.gossips,
        n.last_gossip,
        n.last_gossip_decay,
        n.last_restock,
        n.restocks,
        n.restock_day,
        n.last_traded,
        n.sleeping,
        n.was_baby
    );
    if let Some(b) = &n.brain {
        brain_state(out, b);
    }
}

fn brain_state(out: &mut String, b: &Brain) {
    let _ = write!(
        out,
        " | {:?} {:?} {:?} {} {} {:?} {:?} {} {} {}",
        b.active,
        b.default,
        b.available,
        b.baby,
        b.last_schedule,
        b.sensors,
        b.bed_cache,
        b.bed_tried,
        b.bed_last,
        b.refresh
    );
    mem_state(out, &b.mem);
    for (priority, activity, node) in &b.nodes {
        let _ = write!(out, " | {priority} {activity:?} ");
        node_state(out, node);
    }
}

/// Every memory by its content (the visible entities as entity and visibility cache).
fn mem_state(out: &mut String, m: &Mem) {
    let visible: Option<Vec<String>> =
        m.visible.as_ref().map(|v| v.iter().map(|x| format!("{:?}/{:?}", x.ent, x.seen)).collect());
    let _ = write!(
        out,
        " | {:?} {:?} {} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
        m.walk,
        m.look,
        m.path,
        m.cant_reach_since,
        m.home,
        m.job_site,
        m.potential_job_site,
        m.meeting_point,
        m.secondary,
        m.living,
        visible,
        m.players,
        m.visible_player,
        m.wanted_item,
        m.nearest_bed,
        m.hurt_by,
        m.hurt_by_entity,
        m.hostile,
        m.babies,
        m.golem,
        m.interaction,
        m.breed,
        m.last_slept,
        m.last_woken,
        m.last_worked,
        m.heard_bell,
        m.hiding_place,
        m.doors
    );
}

fn node_state(out: &mut String, n: &Node) {
    match n {
        Node::B { b, run, end } => {
            let _ = write!(out, "B {run} {end} ");
            behavior_state(out, b);
        }
        Node::O { t, run } => {
            let _ = write!(out, "O {run} {t:?}");
        }
        Node::Idle { min, max, run, end } => {
            let _ = write!(out, "Idle {min} {max} {run} {end}");
        }
        Node::G(g) => {
            let _ = write!(out, "G {:?} {:?} {} {} {:?} {} [", g.entry, g.erase, g.shuffled, g.run_one, g.rng, g.run);
            for GateChild { node: kid, weight, shuffle_key: key } in &g.kids {
                node_state(out, kid);
                let _ = write!(out, " {weight} {key:?}; ");
            }
            out.push(']');
        }
    }
}

/// A behaviour; the walk keeps only whether it holds a path (the path itself is the navigation's).
fn behavior_state(out: &mut String, b: &Beh) {
    match b {
        Beh::MoveToTarget { min, max, cooldown, has_path, last, speed, .. } => {
            let _ = write!(out, "MoveToTarget {min} {max} {cooldown} kept={has_path} {last:?} {speed:?}");
        }
        _ => {
            let _ = write!(out, "{b:?}");
        }
    }
}

fn whole_run(lines: &[String]) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for l in lines {
        ctx.update(l.as_bytes());
        ctx.update(b"\n");
    }
    hex(ctx.finish())
}

/// `None` for the same lines, otherwise the first tick that differs.
fn first_difference(old: &[String], new: &[String], what: &str) -> Option<String> {
    if old.len() != new.len() {
        return Some(format!("{what}: {} ticks instead of {}", new.len(), old.len()));
    }
    let t = old.iter().zip(new).position(|(a, b)| a != b)?;
    Some(format!(
        "{what}: first difference at tick {t} (stretch {}, tick {})",
        t / TICKS_PER_STRETCH,
        t % TICKS_PER_STRETCH
    ))
}

/// Each switch alone, then all of them.
fn cases() -> Vec<(&'static str, VillagerSwitches)> {
    let old = VillagerSwitches::OLD;
    vec![
        // The scenario itself is reproducible: otherwise no comparison below would prove anything.
        ("old_again", old),
        ("visible_by_index", VillagerSwitches { visible_by_index: true, ..old }),
        ("sensed_kind", VillagerSwitches { sensed_kind: true, ..old }),
        ("no_tick_allocations", VillagerSwitches { no_tick_allocations: true, ..old }),
        ("move_paths", VillagerSwitches { move_paths: true, ..old }),
        ("food_cache", VillagerSwitches { food_cache: true, ..old }),
        ("trade_borrow", VillagerSwitches { trade_borrow: true, ..old }),
        ("block_ids", VillagerSwitches { block_ids: true, ..old }),
        ("farmland_box_scan", VillagerSwitches { farmland_box_scan: true, ..old }),
        ("all_new", VillagerSwitches::ALL_NEW),
    ]
}

#[test]
fn every_villager_switch_gives_the_old_ticks() {
    let start = std::time::Instant::now();
    let Some(old) = run_scenario("villager-brain-old", VillagerSwitches::OLD) else { return };
    let summary = whole_run(&old);
    eprintln!("[villager] old path: {summary} ({:.1} s)", start.elapsed().as_secs_f64());
    // The C library's trigonometry differs between systems: the recorded value is Windows'.
    if cfg!(windows) {
        assert_eq!(summary, EXPECTED_RUN, "old path against the recorded fingerprint");
    }
    // Every case is run, so that one failure does not hide another.
    let failures: Vec<String> = cases()
        .into_iter()
        .filter_map(|(name, switches)| {
            let new = run_scenario(&format!("villager-brain-{name}"), switches).expect("second run");
            first_difference(&old, &new, name)
        })
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// `f` on a new thread with a stack large enough for a server (the test seeds restart there).
fn on_own_thread(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(32 << 20).spawn(f).expect("test thread").join().expect("test run");
}

fn trade(a: &str, n: i32, b: Option<(&str, i32)>, out: &str, xp: i32) -> super::super::trades::Offer {
    let cost = |id: &str, count| super::super::trades::Cost { id: format!("minecraft:{id}"), count, components: None };
    super::super::trades::Offer {
        cost_a: cost(a, n),
        cost_b: b.map(|(id, count)| cost(id, count)),
        result: Stack { id: format!("minecraft:{out}"), count: 1, components: None },
        uses: 0,
        max_uses: 16,
        reward_exp: true,
        special: 0,
        demand: 3,
        mult: 0.05,
        xp,
    }
}

/// A trading session on the test world (`RUSTCRAFT_SOLO_WORLD`) with the switches `switches`: a
/// level 1 farmer one trade away from level 2, opened by the player, offer chosen, bought twice
/// (the level up draws new offers), another offer chosen and bought. Every event sent to the
/// player, then the farmer's offers and experience and the player's inventory.
fn trading_session(tag: &'static str, switches: VillagerSwitches) -> Option<Vec<String>> {
    let run = move || {
        FORCED.set(Some(switches));
        let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
        let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
        let mut p = crate::blocks::probe::Probe::open(&dir, std::sync::Arc::new(data));
        p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
        p.s.slots[36] = Some(Stack { id: "minecraft:wheat".into(), count: 64, components: None });
        p.s.slots[37] = Some(Stack { id: "minecraft:emerald".into(), count: 3, components: None });
        let mut m = p.s.new_mob(Kind::Villager, [216.5, -59.0, 204.5], 180.0);
        m.persistent = true;
        let farmer = super::super::variants::PROFESSIONS.iter().position(|p| *p == "farmer").expect("farmer") as i32;
        m.villager = [0, farmer, 1];
        m.x.vdata_final = true;
        m.x.villager_xp = 9;
        p.s.npc_init(&mut m);
        if let Some(n) = m.npc.as_mut() {
            n.offers =
                Some(vec![trade("wheat", 20, None, "emerald", 2), trade("wheat", 6, Some(("emerald", 1)), "bread", 1)]);
        }
        let id = m.id;
        p.s.add_mob(m);
        let _ = p.take_events();
        let mut out: Vec<String> = Vec::new();
        let i = p.s.mob_index(id)?;
        let mut vm = p.s.mobs.list.take(i)?;
        p.s.npc_interact(&mut vm, "");
        p.s.mobs.list.put(i, vm);
        out.extend(p.take_events().iter().map(|e| format!("{e:?}")));
        for (offer, purchases) in [(0, 2), (1, 1)] {
            p.s.merchant_select(offer);
            out.extend(p.take_events().iter().map(|e| format!("{e:?}")));
            for _ in 0..purchases {
                // Quick move of the result slot: one purchase.
                let (window, state) = (p.s.current_window(), p.s.life.menu.as_ref().map_or(0, |m| m.state));
                p.s.on_click(window, state, 2, 0, 1, Vec::new(), None);
                out.extend(p.take_events().iter().map(|e| format!("{e:?}")));
            }
        }
        let v = p.s.mobs.get(id)?;
        let n = v.npc.as_ref()?;
        out.push(format!("farmer {:?} {} {:?} {:?}", v.villager, v.x.villager_xp, n.offers, n.inv));
        out.push(format!("player {:?}", p.s.slots));
        Some(out)
    };
    std::thread::Builder::new().stack_size(32 << 20).spawn(run).expect("trading thread").join().expect("trading run")
}

/// The trading menu sends the same offers and makes the same trades with the offers lent: the
/// whole stream of events to the player (offers, slots, experience) and the end state.
#[test]
fn trading_with_lent_offers_sends_the_same_packets() {
    let Some(old) = trading_session("villager-trade-old", VillagerSwitches::OLD) else { return };
    // Opening, then the new offers of level 2 after the first purchase.
    let offers_sent = old.iter().filter(|l| l.starts_with("MerchantOffers")).count();
    assert!(offers_sent >= 2, "only {offers_sent} offer lists sent: the session did not trade");
    let farmer = old.iter().find(|l| l.starts_with("farmer")).expect("end state");
    assert!(farmer.starts_with("farmer [0, 5, 2]"), "no level up: {}", &farmer[..40]);
    for (name, switches) in [
        ("trade_borrow", VillagerSwitches { trade_borrow: true, ..VillagerSwitches::OLD }),
        ("all_new", VillagerSwitches::ALL_NEW),
    ] {
        let new =
            trading_session(if name == "all_new" { "villager-trade-new" } else { "villager-trade-lent" }, switches)
                .expect("second session");
        assert_eq!(old.len(), new.len(), "{name}: event count");
        if let Some(i) = old.iter().zip(&new).position(|(a, b)| a != b) {
            panic!("{name}: event {i} differs:\nold {}\nnew {}", old[i], new[i]);
        }
    }
}

/// The price of a payment reads the cost's stack limit the same way with or without a stack built,
/// patches included (limit set, limit removed).
#[test]
fn payment_price_reads_the_same_stack_limit() {
    let Some(data) = rc_testdata::version_data() else { return };
    let items = crate::items::Items::new(std::sync::Arc::new(data), &Vec::new());
    let patches = [
        None,
        Some(Nbt::Compound(vec![("minecraft:max_stack_size".into(), Nbt::Int(16))])),
        Some(Nbt::Compound(vec![("!minecraft:max_stack_size".into(), Nbt::Compound(Vec::new()))])),
    ];
    on_own_thread(move || {
        for id in ["wheat", "emerald", "ender_pearl", "diamond_sword"] {
            for components in &patches {
                for special in [-50, 0, 40, 90] {
                    let mut o = trade(id, 20, None, "emerald", 1);
                    o.cost_a.components = components.clone();
                    o.special = special;
                    FORCED.set(Some(VillagerSwitches::OLD));
                    let old = o.cost_a_count(&items);
                    FORCED.set(Some(VillagerSwitches::ALL_NEW));
                    assert_eq!(o.cost_a_count(&items), old, "{id} {components:?} {special}");
                }
            }
        }
    });
}

/// Like the game, a villager closing the doors it passed only spares one that a fellow villager it
/// perceives (its living entities memory) is crossing: an unperceived one crossing the door does not
/// hold it open.
#[test]
fn only_perceived_villagers_hold_a_door_open() {
    on_own_thread(|| {
        let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
        let dir = super::super::tests::prep(&world, "villager-door-hold", 0, "normal", &[]);
        let mut p = crate::blocks::probe::Probe::open(&dir, std::sync::Arc::new(data));
        p.run(
            "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\n\
             player 210.5 -59 202.5 0 0\n\
             setblock 216 -59 206 minecraft:oak_door[half=lower,open=true]\n\
             setblock 216 -58 206 minecraft:oak_door[half=upper,open=true]\n",
        );
        let door = [216, -59, 206];
        let add = |p: &mut crate::blocks::probe::Probe, at: [f64; 3]| {
            let mut m = p.s.new_mob(Kind::Villager, at, 0.0);
            m.persistent = true;
            let id = m.id;
            p.s.add_mob(m);
            id
        };
        let closer = add(&mut p, [216.5, -59.0, 204.5]);
        let crosser = add(&mut p, [216.5, -59.0, 207.5]);
        let i = p.s.mob_index(crosser).expect("crossing villager");
        // Its path goes through the door: next node on it.
        p.s.mobs.list.get_mut(i).expect("crossing villager").nav.path = Some(super::super::nav::Path {
            nodes: [[216, -59, 208], door, [216, -59, 205]].map(|n| (n, super::super::nav::PathType::Walkable)).to_vec(),
            next: 1,
            target: [216, -59, 205],
            dist_to_target: 0.0,
            reached: true,
        });
        let open = |p: &crate::blocks::probe::Probe| p.s.view().get_bool(p.s.st(door), PropKey::Open);
        assert!(open(&p), "the door is not placed open");
        for perceived in [true, false] {
            let i = p.s.mob_index(closer).expect("villager");
            let mut m = p.s.mobs.list.take(i).expect("villager");
            let mut b = m.npc.as_mut().and_then(|n| n.brain.take()).expect("brain");
            b.mem.doors = Some(vec![door]);
            b.mem.living = perceived.then(|| vec![super::super::mob::Ent::Mob(crosser)]);
            p.s.close_doors(&m, &mut b, None, None);
            if let Some(n) = m.npc.as_mut() {
                n.brain = Some(b);
            }
            p.s.mobs.list.put(i, m);
            assert_eq!(open(&p), perceived, "perceived crossing villager: {perceived}");
        }
    });
}

/// The species tables give the name tables' answers for every species.
#[test]
fn species_tables_match_the_names() {
    for k in Kind::ALL {
        let name = k.info().name;
        assert_eq!(super::brain::hostile_alert_distance(k), super::brain::hostile_distance(name), "{k:?}");
        assert_eq!(k == Kind::IronGolem, name == "minecraft:iron_golem", "{k:?}");
    }
}

/// Blocks read by identifier and the food table give the old answers: every block state for the
/// crops, 200 cubes drawn around the farmers for the farmland, every item for the food (bare, with
/// the component removed by its patch, with an unrelated patch).
#[test]
fn villager_block_and_food_tables_give_the_old_answers() {
    on_own_thread(|| {
        let Some(s) = scene("villager-tables") else { return };
        for st in 0..s.data.states.len() as u32 {
            assert_eq!(s.crop_max_age_with(st, true), s.crop_max_age_with(st, false), "crop of state {st}");
        }
        let farmers: Vec<[i32; 3]> = s
            .mobs
            .of_kind(Kind::Villager)
            .filter(|m| m.villager[1] == super::PROF_FARMER)
            .map(|m| m.block_pos())
            .collect();
        let mut draw = JRand::new(0xC6_0012);
        let (mut by_name, mut other, mut met) = (Vec::new(), Vec::new(), 0);
        for i in 0..200 {
            let f = farmers[i % farmers.len()];
            let c = [
                f[0] + draw.next_int_bound(13) - 6,
                f[1] + draw.next_int_bound(5) - 2,
                f[2] + draw.next_int_bound(13) - 6,
            ];
            by_name.clear();
            s.farmland_around(c, FarmlandSearch::ByName, &mut by_name);
            for how in [FarmlandSearch::ById, FarmlandSearch::BoxScan] {
                // Something before the cube's cells: each search only appends.
                other.clear();
                other.push([0; 3]);
                s.farmland_around(c, how, &mut other);
                assert_eq!(other[1..], by_name[..], "farmland around {c:?} ({how:?})");
            }
            met += by_name.len();
        }
        assert!(met > 0, "no farmland met: the cubes miss the fields");
        let removed = Nbt::Compound(vec![("!minecraft:villager_food".into(), Nbt::Compound(Vec::new()))]);
        let other = Nbt::Compound(vec![("minecraft:max_stack_size".into(), Nbt::Int(16))]);
        let mut foods = 0;
        for name in s.life.items.names.clone() {
            for components in [None, Some(removed.clone()), Some(other.clone())] {
                let st = Stack { id: name.clone(), count: 1, components };
                FORCED.set(Some(VillagerSwitches::OLD));
                let old = s.villager_food(&st);
                FORCED.set(Some(VillagerSwitches::ALL_NEW));
                assert_eq!(s.villager_food(&st), old, "food of {st:?}");
                foods += usize::from(old.is_some());
            }
        }
        assert!(foods >= 8, "only {foods} foods: the default components are not read");
    });
}

/// With every switch on, the sensing of the living entities and the nine gaze searches allocate
/// nothing once the buffers have grown; the old path allocates at each of them.
#[test]
fn sensing_and_gaze_searches_do_not_allocate() {
    use super::behave::{LookKind, Trig};
    on_own_thread(|| {
        let Some(mut s) = scene("villager-allocations") else { return };
        set_day_time(&mut s, STRETCHES[1]);
        for _ in 0..40 {
            s.game_tick();
            s.tick += 1;
        }
        let slot = (0..s.mobs.list.len())
            .find(|&i| s.mobs.list.get(i).is_some_and(|m| m.kind == Kind::Villager && !m.is_baby()))
            .expect("an adult villager");
        let mut m = s.mobs.list.take(slot).expect("villager");
        let mut b = m.npc.as_mut().and_then(|n| n.brain.take()).expect("brain");
        let t = s.time;
        let gazes = [LookKind::Cat, LookKind::Villager, LookKind::Player, LookKind::Category(0), LookKind::Category(5)];
        let seen = b.mem.visible.as_ref().map_or(0, Vec::len);
        assert!(seen >= 5, "only {seen} entities perceived");
        let allocations = crate::test_alloc::allocations;
        // Allocations of 10 sensings, then of 10 rounds of gaze searches (visibility already
        // evaluated: the line of sight is not this code's), buffers and caches warmed first.
        let mut count = |s: &mut Server, switches: VillagerSwitches| {
            FORCED.set(Some(switches));
            s.sense_living(&mut m, &mut b);
            let before = allocations();
            for _ in 0..10 {
                s.sense_living(&mut m, &mut b);
            }
            let sensing = allocations() - before;
            let mut gaze_round = |s: &mut Server| {
                for k in gazes {
                    b.mem.look = None;
                    let _ = s.trig(&mut m, &mut b, &mut Trig::Look(k), t);
                }
            };
            gaze_round(s);
            let before = allocations();
            for _ in 0..10 {
                gaze_round(s);
            }
            (sensing, allocations() - before)
        };
        let old = count(&mut s, VillagerSwitches::OLD);
        let new = count(&mut s, VillagerSwitches::ALL_NEW);
        let gaze_copies = 10 * gazes.len() as u64;
        assert!(old.0 >= 30 && old.1 >= gaze_copies, "old path: {old:?} allocations, the counter misses them");
        assert_eq!(new, (0, 0), "new path allocates (sensing, gaze searches)");
    });
}
