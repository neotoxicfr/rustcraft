//! G3-F11: command item predicate (default components, `|`, `~`, `count`), checked component
//! values, patch brought down to the differences from the prototype, component predicates
//! (potions, wear, attribute modifiers, books, fireworks), selectors (translated `name=`,
//! player name case, empty-list errors) and text components
//! (`nbt`, score format, selector separator). New world created by the generator: only
//! the version data and the jar are required (absent: explicit failure).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rc_proto::nbt::Nbt;

use crate::player::Stack;
use crate::server::Server;

struct T {
    s: Server,
    session: rc_client::Session,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn open(tag: &str) -> T {
    let data = Arc::new(rc_testdata::version_data().expect("version data missing: G3-F11 tests impossible"));
    crate::registries::client_jar(&data).expect("client jar missing: G3-F11 tests impossible");
    let dir = std::env::temp_dir().join(format!("rc-g3f11-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 11;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("G3F11");
    let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
    let opts = crate::Options {
        world: dir.clone(),
        name: "G3F11".into(),
        uuid,
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        sim_distance: 12,
    };
    let mut s = match Server::open(opts, uuid, data, peer, Arc::new(AtomicBool::new(false))) {
        Ok(s) => s,
        Err((_, e)) => panic!("opening: {e}"),
    };
    s.loaded = true;
    s.teleport = None;
    s.game_mode = 1;
    s.load_now(&[(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    T { s, session, dir }
}

fn run(s: &mut Server, c: &str) -> (bool, i32) {
    let mut src = s.player_source();
    src.level = 4;
    s.perform(src, c)
}

fn chats(session: &rc_client::Session) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(e) = session.events.try_recv() {
        if let rc_client::Event::Chat { component, .. } = e {
            out.push(format!("{component:?}"));
        }
    }
    out
}

/// Number of items held in hand matching the predicate (0 if none).
fn held(s: &mut Server, pred: &str) -> i32 {
    let (ok, n) = run(s, &format!("execute if items entity @s weapon.mainhand {pred}"));
    if ok { n } else { 0 }
}

fn hold(s: &mut Server, item: &str) {
    assert!(run(s, &format!("item replace entity @s weapon.mainhand with {item}")).0, "item held: {item}");
}

/// Text of a component (`component`) resolved then flattened in the server's language.
fn text(s: &mut Server, c: &str) -> Result<String, String> {
    let mut src = s.player_source();
    src.level = 4;
    s.arg_component(&src, c, None).map(|c| crate::commands::cmds_ents::plain(&c)).map_err(|e| format!("{:?}", e.0))
}

/// Checks collected without stopping: one run shows all the differences (sabotage proof).
macro_rules! check {
    ($bad:ident, $c:expr) => {
        if !$c {
            $bad.push(format!("l.{}: {}", line!(), stringify!($c)));
        }
    };
    ($bad:ident, $c:expr, $($m:tt)+) => {
        if !$c {
            $bad.push(format!("l.{}: {}", line!(), format!($($m)+)));
        }
    };
}

macro_rules! check_eq {
    ($bad:ident, $a:expr, $b:expr $(, $($m:tt)+)?) => {
        match (&$a, &$b) {
            (a, b) => {
                if a != b {
                    $bad.push(format!("l.{}: {:?} != {:?}", line!(), a, b));
                }
            }
        }
    };
}

#[test]
fn item_predicates_and_components() {
    let mut t = open("items");
    let mut bad: Vec<String> = Vec::new();
    let s = &mut t.s;

    // Default components of the item (stack size, wear) and pseudo-component `count`.
    hold(s, "minecraft:diamond_sword");
    check_eq!(bad, held(s, "*[max_stack_size=1]"), 1, "item default compared");
    check_eq!(bad, held(s, "*[damage]"), 1, "presence of a default component");
    check_eq!(bad, held(s, "*[!food]"), 1);
    check_eq!(bad, held(s, "*[count=1]"), 1, "count pseudo-component");
    check_eq!(bad, held(s, "*[count=2]"), 0);
    check_eq!(bad, held(s, "*[count~{min:1,max:3}]"), 1);
    // Alternatives and partial predicates.
    check_eq!(bad, held(s, "*[food|max_stack_size=1]"), 1, "alternatives");
    check_eq!(bad, held(s, "*[damage~{damage:5}]"), 0, "predicate ~ evaluated");
    check_eq!(bad, held(s, "*[damage~{durability:1561}]"), 1);
    check_eq!(bad, held(s, "*[attribute_modifiers~{modifiers:{contains:[{attribute:\"minecraft:attack_damage\"}]}}]"), 1);
    check_eq!(bad, held(s, "*[attribute_modifiers~{modifiers:{contains:[{attribute:\"minecraft:armor\"}]}}]"), 0);
    // Wear: false for an item without durability.
    hold(s, "minecraft:stick");
    check_eq!(bad, held(s, "*[damage~{}]"), 0, "no damage component");
    // SNBT value with an inner comma.
    hold(s, "minecraft:stick[custom_data={a:1,b:2}]");
    check_eq!(bad, held(s, "*[custom_data={a:1,b:2}]"), 1);
    check_eq!(bad, held(s, "*[custom_data~{b:2}]"), 1);
    // Raw NBT: tags compared with their type.
    hold(s, "minecraft:stick[custom_data={a:1b,id:\"stone\",l:[I;1]}]");
    check_eq!(bad, held(s, "*[custom_data={a:1b,id:\"stone\",l:[I;1]}]"), 1, "identical raw NBT");
    check_eq!(bad, held(s, "*[custom_data={a:1,id:\"stone\",l:[I;1]}]"), 0, "integer ≠ byte");
    check_eq!(bad, held(s, "*[custom_data={a:1b,id:\"minecraft:stone\",l:[I;1]}]"), 0, "exact string");
    check_eq!(bad, held(s, "*[custom_data={a:1b,id:\"stone\",l:[1]}]"), 0, "list ≠ array");
    check_eq!(bad, held(s, "*[custom_data=\"{a:1b,id:'stone',l:[I;1]}\"]"), 1, "text form");

    // Patch: value equal to the default removed (the stack stacks with a new one).
    hold(s, "minecraft:diamond_sword[damage=0]");
    let st = s.slots[36 + s.held as usize].clone().expect("sword held");
    check_eq!(bad, st.components, None, "damage=0 = default: empty patch");
    // Same rule for a stack read from disk and for a stack received from the client.
    let raw = || Some(Nbt::Compound(vec![("minecraft:damage".into(), Nbt::Int(0))]));
    let disk = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:diamond_sword".into())),
        ("count".into(), Nbt::Int(1)),
        ("components".into(), raw().unwrap()),
    ]);
    check_eq!(bad, Stack::from_nbt(&disk).map(|s| s.components), Some(None), "disk: empty patch");
    let sword = Stack { id: "minecraft:diamond_sword".into(), count: 1, components: raw() };
    let net = s.life.codec.to_net(&s.life.items, &sword).expect("network stack");
    let back = s.life.codec.from_net(&s.life.items, &net).expect("stack received");
    check_eq!(bad, back.components, None, "network: empty patch");
    check!(bad, crate::items::same(&back, &st), "stacks with the given sword");
    let mut st = Stack { id: "minecraft:white_banner".into(), count: 1, components: None };
    crate::items::set_component(&mut st, "minecraft:banner_patterns", Nbt::List(Vec::new()));
    check_eq!(bad, st.components, None, "empty patterns = banner default");
    crate::items::set_component(&mut st, "minecraft:rarity", Nbt::String("epic".into()));
    check!(bad, crate::items::component(&st, "minecraft:rarity").is_some());

    // Potions: set of potions and effects.
    hold(s, "minecraft:potion[potion_contents={potion:\"minecraft:swiftness\"}]");
    check_eq!(bad, held(s, "*[potion_contents~{potions:\"minecraft:swiftness\"}]"), 1);
    check_eq!(bad, held(s, "*[potion_contents~{potions:\"minecraft:healing\"}]"), 0);
    check_eq!(bad, held(s, "*[potion_contents~{effects:{contains:[{\"minecraft:speed\":{duration:3600}}]}}]"), 1);
    check_eq!(bad, held(s, "*[potion_contents~{effects:{contains:[{\"minecraft:speed\":{amplifier:1}}]}}]"), 0);

    // Written book: title and pages.
    hold(s, "minecraft:written_book[written_book_content={title:\"T\",author:\"A\",pages:[\"p\"]}]");
    check_eq!(bad, held(s, "*[written_book_content~{title:\"T\"}]"), 1);
    check_eq!(bad, held(s, "*[written_book_content~{title:\"U\"}]"), 0, "title tested");
    check_eq!(bad, held(s, "*[written_book_content~{pages:{contains:[\"p\"]}}]"), 1);
    check_eq!(bad, held(s, "*[written_book_content~{pages:{contains:[\"q\"]}}]"), 0, "pages tested");
    // Firework: explosions.
    hold(s, "minecraft:firework_rocket[fireworks={explosions:[{shape:\"star\"}]}]");
    check_eq!(bad, held(s, "*[fireworks~{explosions:{contains:[{shape:\"star\"}]}}]"), 1);
    check_eq!(bad, held(s, "*[fireworks~{explosions:{contains:[{shape:\"burst\"}]}}]"), 0, "explosions tested");

    // Malformed component value: refused by parsing with the decoder's message.
    chats(&t.session);
    check!(bad, !run(&mut t.s, "give @s minecraft:diamond_sword[damage=\"x\"]").0);
    let c = chats(&t.session);
    check!(bad, c.iter().any(|m| m.contains("arguments.item.component.malformed") && m.contains("Not a number")), "{c:?}");
    check!(bad, !run(&mut t.s, "give @s minecraft:diamond_sword[max_stack_size=100]").0);
    check!(bad, !run(&mut t.s, "give @s minecraft:stick[creative_slot_lock={}]").0, "transient component refused");
    assert!(bad.is_empty(), "differences:\n{}", bad.join("\n"));
}

#[test]
fn selectors_and_text_contents() {
    let mut t = open("text");
    let mut bad: Vec<String> = Vec::new();
    let s = &mut t.s;
    check!(bad, run(s, "summon minecraft:cow ~ ~ ~").0);
    check!(bad, run(s, "summon minecraft:cow ~1 ~ ~").0);

    // `name=`: default name translated; player designated by name ignoring case.
    check_eq!(bad, run(s, "execute if entity @e[type=minecraft:cow,name=Cow]"), (true, 2));
    check!(bad, run(s, "execute if entity g3f11").0, "player name, case ignored");
    check!(bad, run(s, "scoreboard objectives add o dummy").0);
    check!(bad, run(s, "scoreboard players set g3f11 o 5").0);
    check_eq!(bad, s.cmd.scores.get("G3F11", "o"), Some(5), "holder brought back to the online player");
    // Selector without result: entities' error key.
    chats(&t.session);
    check!(bad, !run(&mut t.s, "scoreboard players set @e[type=minecraft:pig] o 1").0);
    let c = chats(&t.session);
    check!(bad, c.iter().any(|m| m.contains("argument.entity.notfound.entity")), "{c:?}");
    let s = &mut t.s;

    // Score: number format of the objective, then that of the score.
    check_eq!(bad, text(s, "{score:{name:\"G3F11\",objective:\"o\"}}").as_deref(), Ok("5"));
    // Simple name read as a selector: online player (case ignored), UUID = the entity.
    check_eq!(bad, text(s, "{score:{name:\"g3f11\",objective:\"o\"}}").as_deref(), Ok("5"), "case ignored");
    let me = crate::nbtio::uuid_text(s.ent_uuid(crate::commands::ents::E::Player));
    check_eq!(bad, text(s, &format!("{{score:{{name:\"{me}\",objective:\"o\"}}}}")).as_deref(), Ok("5"), "UUID");
    check_eq!(bad, text(s, "{score:{name:\"absent\",objective:\"o\"}}").as_deref(), Ok(""), "unknown name");
    check!(bad, run(s, "scoreboard objectives modify o numberformat fixed \"X\"").0);
    check_eq!(bad, text(s, "{score:{name:\"G3F11\",objective:\"o\"}}").as_deref(), Ok("X"), "fixed format");
    check!(bad, run(s, "scoreboard players display numberformat G3F11 o blank").0);
    check_eq!(bad, text(s, "{score:{name:\"G3F11\",objective:\"o\"}}").as_deref(), Ok(""), "score format");
    check!(bad, text(s, "{score:{name:\"@e[type=minecraft:cow]\",objective:\"o\"}}").is_err(), "several holders");

    // Selector: separator given.
    check_eq!(bad, text(s, "{selector:\"@e[type=minecraft:cow]\",separator:\"|\"}").as_deref(), Ok("Cow|Cow"));
    check_eq!(bad, text(s, "{selector:\"@e[type=minecraft:cow]\"}").as_deref(), Ok("Cow, Cow"));
    // Children kept after the content.
    check_eq!(bad, text(s, "{selector:\"@s\",extra:[\"!\"]}").as_deref(), Ok("G3F11!"));

    // NBT: storage, raw, interpreted, separator, entity.
    check!(bad, run(s, "data modify storage t:a v set value [1,2]").0);
    check!(bad, run(s, "data modify storage t:a c set value [{text:\"x\",color:\"red\"},\"y\"]").0);
    check_eq!(bad, text(s, "{nbt:\"v\",storage:\"t:a\"}").as_deref(), Ok("[1, 2]"));
    check_eq!(bad, text(s, "{nbt:\"v[]\",storage:\"t:a\",separator:\"/\"}").as_deref(), Ok("1/2"));
    check_eq!(bad, text(s, "{nbt:\"c[]\",storage:\"t:a\",interpret:true}").as_deref(), Ok("x, y"));
    check!(bad, text(s, "{nbt:\"v\",storage:\"t:a\",interpret:true,plain:true}").is_err());
    check_eq!(bad, text(s, "{nbt:\"missing\",storage:\"t:a\"}").as_deref(), Ok(""));
    let pos = text(s, "{nbt:\"Pos\",entity:\"@s\",plain:true}").unwrap();
    check!(bad, pos.starts_with('[') && pos.ends_with("d]"), "{pos}");
    // Interpreted: the separator and the following elements inherit the style of the first.
    let mut src = s.player_source();
    src.level = 4;
    let c = s.arg_component(&src, "{nbt:\"c[]\",storage:\"t:a\",interpret:true}", None).map_err(|e| e.0).unwrap();
    check_eq!(bad, c.style.color, Some(0xFF5555), "first element red, carrier of the following ones");
    assert!(bad.is_empty(), "differences:\n{}", bad.join("\n"));
}

/// Bench: writing a component (before: insertion only; after: comparison with the default) and
/// item predicate read per stack or once for 36 stacks.
#[test]
#[ignore = "bench: cargo test --release -p rc-sim bench_g3f11 -- --ignored --nocapture"]
fn bench_g3f11() {
    use std::hint::black_box;
    use std::time::Instant;
    let t = open("bench");
    let s = &t.s;
    let n = 200_000;
    let mut st = Stack { id: "minecraft:diamond_sword".into(), count: 1, components: None };
    let t0 = Instant::now();
    for i in 0..n {
        let c = st.components.get_or_insert_with(|| Nbt::Compound(Vec::new()));
        crate::nbtio::set(c, "minecraft:damage", Nbt::Int(black_box(i % 7 + 1)));
    }
    let old = t0.elapsed();
    let t0 = Instant::now();
    for i in 0..n {
        crate::items::set_component(&mut st, "minecraft:damage", Nbt::Int(black_box(i % 7 + 1)));
    }
    let new = t0.elapsed();
    let stacks: Vec<Stack> = (0..36)
        .map(|i| Stack { id: if i % 2 == 0 { "minecraft:stick" } else { "minecraft:apple" }.into(), count: 1, components: None })
        .collect();
    let text = "*[custom_data~{a:1}|max_stack_size=64,!food]";
    let reps = 2_000;
    let t0 = Instant::now();
    let mut hits = 0;
    for _ in 0..reps {
        hits += stacks.iter().filter(|x| s.item_matches(black_box(text), x)).count();
    }
    let per_stack = t0.elapsed();
    let t0 = Instant::now();
    let mut hits2 = 0;
    for _ in 0..reps {
        let p = crate::commands::itempred::ItemPred::parse(black_box(text));
        hits2 += stacks.iter().filter(|x| s.item_pred_test(&p, x)).count();
    }
    let once = t0.elapsed();
    assert_eq!(hits, hits2);
    eprintln!(
        "set_component: insert only {:.1} ns, with default {:.1} ns; 36-stack predicate: read per stack {:.2} µs, read once {:.2} µs",
        old.as_nanos() as f64 / f64::from(n),
        new.as_nanos() as f64 / f64::from(n),
        per_stack.as_nanos() as f64 / f64::from(reps) / 1000.0,
        once.as_nanos() as f64 / f64::from(reps) / 1000.0
    );
}
