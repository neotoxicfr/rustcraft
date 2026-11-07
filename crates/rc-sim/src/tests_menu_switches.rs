//! Equivalence of the menu switches on a running server (`MenuSwitches`, `docs/perf.md`, menus):
//! a chest open for 1,200 ticks while a hopper fills it and another drains it, changed by
//! `/item`, `/data` (patches the decoding reduces: a component equal to its default, a removal
//! without a default, an empty patch, air and empty counts) and `/give`, clicked in every mode
//! with the client's copies announced right and wrong; then a crafting table, the inventory, a
//! burning furnace, an Ender chest, a double chest fed by a hopper and a crafter. Each run is a
//! fresh flat world in a thread of its own (same test seeds) under one combination of the
//! switches; the trace (every event sent to the client, the stored contents, the chest's
//! comparator signal, the ground items) must be the same for all 16 combinations and match a
//! stored fingerprint. Version data mandatory. Micro-benches in `bench_menus.rs`.

use rc_proto::nbt::Nbt;
use rc_proto_v777::container::HashedStack;

use crate::blocks::Dir;
use crate::player::Stack;
use crate::blocks::probe::Probe;
use crate::menus::{Env, FORCED_SWITCHES, MenuSwitches, mode};
use crate::tests_g3f13::{finish, open};

/// Fingerprint of the reference trace (every switch off). Windows only: the libc's sine and
/// cosine of other systems move the thrown items, as for the other fingerprints.
const SERVER_TRACE_FINGERPRINT: u64 = 0x60c5_0346_b23e_4754;

const CHEST: [i32; 3] = [2, -60, 0];
const FEEDER: [i32; 3] = [2, -59, 0];
const DRAIN: [i32; 3] = [2, -61, 0];
const BARREL: [i32; 3] = [3, -61, 0];
const TABLE: [i32; 3] = [0, -60, 3];
const FURNACE: [i32; 3] = [-2, -60, 0];
const ENDER: [i32; 3] = [-2, -60, 2];
const DOUBLE_RIGHT: [i32; 3] = [6, -60, 0];
const DOUBLE_LEFT: [i32; 3] = [5, -60, 0];
const CRAFTER: [i32; 3] = [0, -60, -3];

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn digest(lines: &[String]) -> u64 {
    lines.iter().fold(0xcbf2_9ce4_8422_2325, |h, l| fnv(fnv(h, l.as_bytes()), b"\n"))
}

/// Events sent since the last read, one line each. The advancement updates carry the wall
/// clock time of their criteria: only how many were sent is kept (a reward sent a tick early
/// still shows).
fn record_events(p: &Probe, trace: &mut Vec<String>) {
    let mut advancements = 0;
    for line in p.take_events().into_iter().map(|e| format!("{e:?}")) {
        if line.starts_with("Progress(Advancements(") {
            advancements += 1;
        } else {
            trace.push(line);
        }
    }
    if advancements > 0 {
        trace.push(format!("{advancements} advancement updates"));
    }
}

/// What a comparator and a save would read of a container.
fn record_container(p: &Probe, at: [i32; 3], trace: &mut Vec<String>) {
    let state = p.s.state_at(at).expect("loaded container");
    let signal = p.s.analog_output(state, at, Dir::West);
    trace.push(format!("{at:?} signal {signal} slots {:?}", p.s.slots(at)));
}

fn ticks(p: &mut Probe, trace: &mut Vec<String>, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
        record_events(p, trace);
    }
}

/// What the client says it holds after its click.
#[derive(Clone, Copy)]
enum Announce {
    /// Nothing changed, cursor empty (a client that predicts nothing).
    Nothing,
    /// Slot `k` and the cursor as the server holds them before the click: right for a slot
    /// the click leaves alone, stale for the one it changes.
    Before(i16),
}

/// The server's fingerprints of menu slot `k` and of the cursor.
fn fingerprints(p: &mut Probe, k: i16) -> (Option<HashedStack>, Option<HashedStack>) {
    let open = p.s.life.menu.is_some();
    p.s.with_menu(open, |m, inv, env| (env.hashed(m.get(inv, k as usize)), env.hashed(m.carried.as_ref())))
        .expect("menu")
}

/// One click as the client sends it, with the menu's current state number.
fn click(p: &mut Probe, trace: &mut Vec<String>, slot: i16, button: i8, mode: i32, announce: Announce) {
    let window = p.s.current_window();
    let state = p.s.life.menu.as_ref().map_or(p.s.life.inv_menu.state, |m| m.state);
    let (changed, carried) = match announce {
        Announce::Nothing => (Vec::new(), None),
        Announce::Before(k) => {
            let (slot_copy, cursor) = fingerprints(p, k);
            (vec![(k, slot_copy)], cursor)
        }
    };
    p.s.on_click(window, state, slot, button, mode, changed, carried);
    record_events(p, trace);
}

fn open_menu(p: &mut Probe, trace: &mut Vec<String>, at: [i32; 3]) {
    assert!(p.s.use_block(at), "no menu at {at:?}");
    record_events(p, trace);
}

fn close_menu(p: &mut Probe, trace: &mut Vec<String>) {
    p.s.close_menu(false);
    record_events(p, trace);
}

/// Feeder hopper refilled every 60 ticks, cycling through plain and damaged stacks.
fn refill_feeder(p: &mut Probe, t: i32) {
    if t % 60 != 0 {
        return;
    }
    let stacks = ["cobblestone 16", "iron_ingot 3", "diamond_sword[minecraft:damage=1] 1", "oak_log 7", "iron_ingot 2"];
    let k = (t / 60) as usize % stacks.len();
    cmd(p, &format!("item replace block 2 -59 0 container.{k} with minecraft:{}", stacks[k]));
}

/// Stored entries the decoding rewrites or refuses, and command writes, while the menu is open.
fn chest_commands(p: &mut Probe, t: i32) {
    let append = |slot: i32, entry: &str| format!("data modify block 2 -60 0 Items append value {{Slot:{slot}b,{entry}}}");
    match t {
        200 => cmd(p, &append(20, "id:\"minecraft:diamond_sword\",count:1,components:{\"minecraft:max_stack_size\":1}")),
        300 => cmd(p, &append(21, "id:\"minecraft:stone\",count:2,components:{\"!minecraft:fire_resistant\":{}}")),
        400 => cmd(p, &append(22, "id:\"minecraft:stone\",count:3,components:{}")),
        450 => {
            cmd(p, &append(23, "id:\"minecraft:air\",count:3"));
            cmd(p, &append(24, "id:\"minecraft:stone\",count:0"));
        }
        500 => cmd(p, "item replace block 2 -60 0 container.25 with minecraft:golden_apple 3"),
        550 => cmd(p, "give @p minecraft:bread 5"),
        560 => cmd(p, "item replace entity @p hotbar.4 with minecraft:iron_ingot 9"),
        _ => {}
    }
}

/// Clicks on the open chest (slots 0..27 chest, 27..54 bag, 54..63 hotbar), by tick.
fn chest_clicks(p: &mut Probe, trace: &mut Vec<String>, t: i32) {
    use Announce::{Before, Nothing};
    let plan: &[(i32, i16, i8, i32, Announce)] = &[
        (600, 0, 0, mode::QUICK_MOVE, Before(1)),
        (610, 1, 0, mode::PICKUP, Before(1)),
        (620, 40, 0, mode::PICKUP, Before(40)),
        (630, 40, 0, mode::PICKUP_ALL, Nothing),
        (640, 41, 1, mode::PICKUP, Before(5)),
        (650, 2, 3, mode::SWAP, Nothing),
        (660, 3, 0, mode::THROW, Before(3)),
        (670, 4, 1, mode::THROW, Nothing),
        (680, 54, 0, mode::PICKUP, Nothing),
        (690, -999, 0, mode::QUICK_CRAFT, Nothing),
        (691, 10, 1, mode::QUICK_CRAFT, Nothing),
        (692, 11, 1, mode::QUICK_CRAFT, Nothing),
        (693, 12, 1, mode::QUICK_CRAFT, Nothing),
        (694, -999, 2, mode::QUICK_CRAFT, Nothing),
        (700, 12, 0, mode::PICKUP_ALL, Before(12)),
        (710, -999, 1, mode::PICKUP, Nothing),
        (720, -999, 0, mode::PICKUP, Nothing),
        (730, 58, 0, mode::PICKUP, Nothing),
        (740, 13, 0, mode::PICKUP, Before(13)),
        (750, 58, 0, mode::PICKUP, Nothing),
        (800, 30, 0, mode::QUICK_MOVE, Before(30)),
        (850, 0, 0, mode::QUICK_MOVE, Nothing),
        (900, 5, 0, mode::QUICK_MOVE, Nothing),
    ];
    for &(at, slot, button, mode, announce) in plan {
        if at == t {
            click(p, trace, slot, button, mode, announce);
        }
    }
    if t == 760 {
        // A stale state number: the server sends everything again.
        let window = p.s.current_window();
        p.s.on_click(window, 12_345, 31, 0, mode::PICKUP, Vec::new(), None);
        record_events(p, trace);
    }
    if t == 780 {
        p.s.game_mode = 1;
        click(p, trace, 6, 2, mode::CLONE, Nothing);
        click(p, trace, -999, 0, mode::PICKUP, Nothing);
        p.s.game_mode = 0;
    }
}

/// The chest phase: 1,200 ticks, hoppers in and out, commands, clicks.
fn chest_phase(p: &mut Probe, trace: &mut Vec<String>) {
    cmd(p, "setblock 2 -61 0 minecraft:hopper[facing=east]");
    cmd(p, "setblock 3 -61 0 minecraft:barrel");
    cmd(p, "setblock 2 -60 0 minecraft:chest[facing=west]");
    cmd(p, "setblock 2 -59 0 minecraft:hopper[facing=down]");
    for item in ["cobblestone 40", "iron_ingot 5", "oak_planks 16", "bundle 1", "diamond_sword[minecraft:damage=3] 1"] {
        cmd(p, &format!("give @p minecraft:{item}"));
    }
    open_menu(p, trace, CHEST);
    for t in 0..1200 {
        refill_feeder(p, t);
        chest_commands(p, t);
        chest_clicks(p, trace, t);
        ticks(p, trace, 1);
        if t % 50 == 0 {
            record_container(p, CHEST, trace);
        }
    }
    close_menu(p, trace);
    for at in [CHEST, FEEDER, DRAIN, BARREL] {
        record_container(p, at, trace);
    }
}

/// Crafting table (planks dragged into the grid, result shift-clicked), then the inventory's grid.
fn crafting_phase(p: &mut Probe, trace: &mut Vec<String>) {
    use Announce::Nothing;
    cmd(p, "setblock 0 -60 3 minecraft:crafting_table");
    cmd(p, "item replace entity @p inventory.20 with minecraft:oak_planks 12");
    open_menu(p, trace, TABLE);
    // Bag slot 20 is inventory slot 29, menu slot 29 - 9 + 10.
    click(p, trace, 30, 0, mode::PICKUP, Nothing);
    for (slot, button) in [(-999, 0), (1, 1), (2, 1), (4, 1), (5, 1), (-999, 2)] {
        click(p, trace, slot, button, mode::QUICK_CRAFT, Nothing);
    }
    click(p, trace, 0, 0, mode::QUICK_MOVE, Nothing);
    ticks(p, trace, 5);
    close_menu(p, trace);
    // Inventory (window 0): planks into the 2×2 grid, result taken.
    for (slot, button, mode) in [(29, 0, mode::PICKUP), (1, 1, mode::PICKUP), (29, 0, mode::PICKUP), (0, 0, mode::QUICK_MOVE)] {
        click(p, trace, slot, button, mode, Nothing);
    }
    ticks(p, trace, 5);
}

/// Burning furnace: the menu re-reads its live slots every tick; the result shift-clicked.
fn furnace_phase(p: &mut Probe, trace: &mut Vec<String>) {
    cmd(p, "setblock -2 -60 0 minecraft:furnace[facing=east]");
    cmd(p, "item replace block -2 -60 0 container.0 with minecraft:raw_iron 3");
    cmd(p, "item replace block -2 -60 0 container.1 with minecraft:coal 1");
    open_menu(p, trace, FURNACE);
    ticks(p, trace, 450);
    click(p, trace, 2, 0, mode::QUICK_MOVE, Announce::Before(2));
    ticks(p, trace, 5);
    close_menu(p, trace);
}

/// Ender chest changed by a command while open, then clicked.
fn ender_phase(p: &mut Probe, trace: &mut Vec<String>) {
    cmd(p, "setblock -2 -60 2 minecraft:ender_chest");
    open_menu(p, trace, ENDER);
    ticks(p, trace, 3);
    cmd(p, "item replace entity @p enderchest.0 with minecraft:emerald 5");
    ticks(p, trace, 3);
    click(p, trace, 0, 0, mode::QUICK_MOVE, Announce::Nothing);
    ticks(p, trace, 3);
    close_menu(p, trace);
}

/// Double chest whose second half is fed by a hopper, then a crafter filled by a command.
fn double_chest_and_crafter_phase(p: &mut Probe, trace: &mut Vec<String>) {
    cmd(p, "setblock 5 -60 0 minecraft:chest[facing=north,type=left]");
    cmd(p, "setblock 6 -60 0 minecraft:chest[facing=north,type=right]");
    cmd(p, "setblock 6 -59 0 minecraft:hopper[facing=down]");
    cmd(p, "item replace block 6 -59 0 container.0 with minecraft:redstone 20");
    cmd(p, "setblock 5 -59 0 minecraft:hopper[facing=down]");
    cmd(p, "item replace block 5 -59 0 container.0 with minecraft:glowstone_dust 20");
    open_menu(p, trace, DOUBLE_RIGHT);
    ticks(p, trace, 60);
    click(p, trace, 27, 0, mode::QUICK_MOVE, Announce::Nothing);
    ticks(p, trace, 3);
    close_menu(p, trace);
    record_container(p, DOUBLE_LEFT, trace);
    cmd(p, "setblock 0 -60 -3 minecraft:crafter");
    open_menu(p, trace, CRAFTER);
    cmd(p, "item replace block 0 -60 -3 container.0 with minecraft:oak_log 2");
    ticks(p, trace, 3);
    click(p, trace, 0, 0, mode::PICKUP, Announce::Nothing);
    click(p, trace, 4, 0, mode::PICKUP, Announce::Nothing);
    ticks(p, trace, 3);
    close_menu(p, trace);
    record_container(p, CRAFTER, trace);
}

fn scenario(p: &mut Probe, trace: &mut Vec<String>) {
    p.s.game_mode = 0;
    chest_phase(p, trace);
    crafting_phase(p, trace);
    furnace_phase(p, trace);
    ender_phase(p, trace);
    double_chest_and_crafter_phase(p, trace);
    trace.push(format!("player {:?}", p.s.slots));
    trace.extend(p.s.life.drops.iter().map(|d| format!("drop {:?} {:?}", d.stack, d.pos)));
}

fn run(tag: String, switches: MenuSwitches) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        FORCED_SWITCHES.with(|f| f.set(Some(switches)));
        let (mut p, dir) = open(&tag);
        let mut trace = Vec::new();
        scenario(&mut p, &mut trace);
        finish(p, dir);
        trace
    })
}

/// Every combination of the four switches.
fn all_combinations() -> Vec<MenuSwitches> {
    (0..16u8)
        .map(|b| MenuSwitches {
            broadcast_in_place: b & 1 != 0,
            remote_copy_reuse: b & 2 != 0,
            pull_without_decoding: b & 4 != 0,
            fewer_allocations: b & 8 != 0,
        })
        .collect()
}

fn compound(fields: Vec<(&str, Nbt)>) -> Nbt {
    Nbt::Compound(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn string(s: &str) -> Nbt {
    Nbt::String(s.to_owned())
}

/// A stored item entry: each field optional, as a command or an old save may leave it.
fn entry(id: Option<Nbt>, count: Option<Nbt>, components: Option<Nbt>) -> Nbt {
    let mut f = Vec::new();
    f.extend(id.map(|v| ("id", v)));
    f.extend(count.map(|v| ("count", v)));
    f.extend(components.map(|v| ("components", v)));
    compound(f)
}

/// Stored entries the decoding reads in every way: counts of every width (one past 32 bits),
/// missing or empty fields, air, patches kept, reduced, emptied, removals with and without a
/// default, a patch that is not a compound, an unknown item.
fn tricky_entries() -> Vec<Nbt> {
    let (sword, stone) = (|| Some(string("minecraft:diamond_sword")), || Some(string("minecraft:stone")));
    let at_default = compound(vec![("minecraft:max_stack_size", Nbt::Int(1))]);
    let damaged = compound(vec![("minecraft:damage", Nbt::Int(3))]);
    let mixed = compound(vec![("minecraft:custom_name", string("x")), ("minecraft:max_stack_size", Nbt::Int(1))]);
    vec![
        entry(stone(), Some(Nbt::Int(5)), None),
        entry(stone(), Some(Nbt::Byte(5)), None),
        entry(stone(), None, None),
        entry(stone(), Some(Nbt::Int(0)), None),
        entry(stone(), Some(Nbt::Int(-1)), None),
        entry(stone(), Some(Nbt::Long((1 << 32) + 3)), None),
        entry(Some(string("minecraft:air")), Some(Nbt::Int(2)), None),
        entry(Some(Nbt::Int(3)), Some(Nbt::Int(2)), None),
        entry(None, Some(Nbt::Int(2)), None),
        entry(stone(), Some(Nbt::Int(1)), Some(compound(Vec::new()))),
        entry(sword(), Some(Nbt::Int(1)), Some(damaged)),
        entry(sword(), Some(Nbt::Int(1)), Some(at_default)),
        entry(sword(), Some(Nbt::Int(1)), Some(mixed)),
        entry(Some(string("minecraft:diamond")), Some(Nbt::Int(4)), Some(compound(vec![("!minecraft:fire_resistant", compound(Vec::new()))]))),
        entry(sword(), Some(Nbt::Int(1)), Some(compound(vec![("!minecraft:damage", compound(Vec::new()))]))),
        entry(stone(), Some(Nbt::Int(1)), Some(string("not a compound"))),
        entry(Some(string("minecraft:no_such_item")), Some(Nbt::Int(1)), Some(compound(Vec::new()))),
    ]
}

/// What a menu may hold against a stored entry: nothing, the decoding's stack, the raw stack
/// (patch not reduced), one more, another item.
fn held_candidates(raw: &Nbt) -> Vec<Option<Stack>> {
    let decoded = Stack::from_nbt(raw);
    let mut out = vec![None, decoded.clone()];
    if let Some(id) = raw.get("id").and_then(Nbt::as_str) {
        let count = crate::nbtio::int(raw, "count").unwrap_or(1) as i32;
        out.push(Some(Stack { id: id.to_owned(), count, components: raw.get("components").cloned() }));
    }
    if let Some(d) = decoded {
        out.push(Some(Stack { count: d.count + 1, ..d.clone() }));
        out.push(Some(Stack { id: "minecraft:dirt".into(), ..d }));
    }
    out
}

/// `Stack::decodes_to` gives the decoding's answer, or hands over (`None`) only for a patch the
/// decoding reduces; `stored_slots_match` agrees with the decoded chest, duplicates and
/// out-of-range slots included.
#[test]
fn in_place_reading_matches_the_decoding() {
    let (mut p, dir) = open("menu-in-place-reading");
    assert!(
        !p.s.life.items.defaults("minecraft:diamond_sword")["minecraft:max_stack_size"].is_null(),
        "item default reports missing: the reduced-patch cases would prove nothing"
    );
    let (mut answered, mut handed_over) = (0, 0);
    for raw in tricky_entries() {
        for held in held_candidates(&raw) {
            let truth = Stack::from_nbt(&raw) == held;
            match Stack::decodes_to(&raw, held.as_ref()) {
                Some(got) => {
                    assert_eq!(got, truth, "{raw:?} against {held:?}");
                    answered += 1;
                }
                None => handed_over += 1,
            }
        }
    }
    assert!(answered > 40 && handed_over >= 3, "answered {answered}, handed over {handed_over}");
    stored_slots_agree(&mut p);
    finish(p, dir);
}

/// A chest holding the tricky entries (plus a duplicate slot and one out of range) compared with
/// the decoded contents and with copies changed one slot at a time.
fn stored_slots_agree(p: &mut Probe) {
    cmd(p, "setblock 2 -60 0 minecraft:chest");
    let mut items: Vec<Nbt> = tricky_entries().into_iter().enumerate().map(|(k, e)| with_slot(e, k as i8)).collect();
    items.push(with_slot(entry(Some(string("minecraft:dirt")), Some(Nbt::Int(9)), None), 0));
    items.push(with_slot(entry(Some(string("minecraft:dirt")), Some(Nbt::Int(9)), None), 40));
    let be = p.s.be_get_mut(CHEST).expect("chest block entity");
    crate::nbtio::set(be, "Items", Nbt::List(items));
    let decoded: Vec<Option<Stack>> = p.s.slots(CHEST).iter().map(|it| it.as_ref().and_then(Stack::from_nbt)).collect();
    let mut menus = vec![decoded.clone()];
    for k in 0..decoded.len() {
        let mut changed = decoded.clone();
        changed[k] = if changed[k].is_some() { None } else { Some(Stack { id: "minecraft:dirt".into(), count: 1, components: None }) };
        menus.push(changed);
    }
    let mut handed_over = 0;
    for menu in &menus {
        let (len, same) = p.s.stored_slots_match(CHEST, menu);
        assert_eq!(len, decoded.len());
        match same {
            Some(same) => assert_eq!(same, decoded[..] == menu[..len], "{menu:?}"),
            None => handed_over += 1,
        }
    }
    assert!(handed_over < menus.len(), "every comparison handed over to the decoding");
}

fn with_slot(mut e: Nbt, slot: i8) -> Nbt {
    if let Nbt::Compound(f) = &mut e {
        f.insert(0, ("Slot".into(), Nbt::Byte(slot)));
    }
    e
}

#[test]
fn menu_switches_keep_every_server_packet() {
    let mut combinations = vec![MenuSwitches::ALL_OFF];
    combinations.extend(all_combinations().into_iter().filter(|s| *s != MenuSwitches::ALL_OFF));
    let runs: Vec<_> = combinations.iter().enumerate().map(|(k, s)| (*s, run(format!("menu-switches-{k}"), *s))).collect();
    let traces: Vec<(MenuSwitches, Vec<String>)> = runs.into_iter().map(|(s, h)| (s, h.join().expect("scenario thread"))).collect();
    let reference = &traces[0].1;
    assert!(reference.len() > 500, "trace too short ({} lines), the scenario did nothing", reference.len());
    assert!(reference.iter().any(|l| l.contains("Inventory(Slot")), "no slot packet in the trace");
    for (switches, trace) in &traces[1..] {
        let first_difference = reference.iter().zip(trace).position(|(a, b)| a != b);
        assert!(
            trace == reference,
            "{switches:?} differs (lengths {} / {}) at line {first_difference:?}:\n old {:?}\n new {:?}",
            reference.len(),
            trace.len(),
            first_difference.map(|k| &reference[k]),
            first_difference.map(|k| &trace[k]),
        );
    }
    let got = digest(reference);
    eprintln!("menu switches: {} lines, fingerprint {got:#018x}", reference.len());
    if cfg!(windows) {
        assert_eq!(got, SERVER_TRACE_FINGERPRINT, "fingerprint changed (behaviour differs from the stored reference)");
    }
}
