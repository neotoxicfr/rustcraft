// Direct read allowed (test: settings read at launch, off the hot path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Parsing on the real tree of an operator of the test server (outside the repository, ignored if missing).

use rc_commands::parse::{Source, completions, signable};
use rc_commands::{Data, Tree, parse};

fn setup() -> Option<(Tree, Data)> {
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("rustcraft-re").join("captures");
    let body = std::fs::read(dir.join("commands-op.bin")).ok()?;
    let c = rc_proto_v777::commands::read_commands(&mut rc_proto::Reader::new(&body)).ok()?;
    let v = rc_testdata::version_data()?;
    let mut d = Data::from_version(&v);
    d.players = vec!["AgentCommands".into(), "Alex".into()];
    d.op_level = 4;
    Some((Tree::new(&c), d))
}

const SRC: Source = Source { restricted: true, selectors: true };

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

#[test]
fn parses_commands() {
    let Some((t, d)) = setup() else { return };
    for ok in [
        "/give AgentCommands diamond 64",
        "/give @a minecraft:diamond_sword[custom_name='x',!damage] 1",
        "/tp ~ ~ ~",
        "/tp @s ^ ^ ^1",
        "/execute as @e[type=cow,limit=3,sort=nearest] at @s run tp ~ ~1 ~",
        "/setblock ~ ~ ~ minecraft:oak_stairs[facing=east]",
        "/fill ~ ~ ~ ~1 ~1 ~1 stone replace #logs",
        "/say hello everyone",
        "/scoreboard objectives add x dummy",
        "/time set 1d",
        "/gamemode creative",
        "/effect give @s minecraft:speed 10 1",
        "/data get entity @s Inventory[0].id",
        "/clear @s *[damage]",
        "/summon minecraft:zombie ~ ~ ~ {PersistenceRequired:1b}",
    ] {
        let i = chars(ok);
        let p = parse(&t, &i, &d, SRC);
        assert!(p.valid(&i), "{ok}: stopped at {} {:?}", p.cursor, p.errors);
    }
    let i = chars("/give AgentCommands dirtt");
    let p = parse(&t, &i, &d, SRC);
    assert!(!p.valid(&i));
    assert!(p.errors.iter().any(|(_, e)| e.key == "argument.item.id.invalid"), "{:?}", p.errors);
}

#[test]
fn signable_arguments() {
    let Some((t, d)) = setup() else { return };
    let i = chars("/say hi");
    assert_eq!(signable(&t, &parse(&t, &i, &d, SRC), &i), vec![("message".to_owned(), "hi".to_owned())]);
    let i = chars("/execute as @a run say hi");
    assert!(signable(&t, &parse(&t, &i, &d, SRC), &i).is_empty());
    let i = chars("/tp ~ ~ ~");
    assert!(signable(&t, &parse(&t, &i, &d, SRC), &i).is_empty());
}

#[test]
fn suggestions() {
    let Some((t, d)) = setup() else { return };
    let texts = |s: &str| {
        let i = chars(s);
        let p = parse(&t, &i, &d, SRC);
        let c = completions(&t, &p, &i, i.len(), &d, SRC);
        let m = rc_commands::Suggestions::merge(&i, c.local);
        (m.list.into_iter().map(|s| s.text).collect::<Vec<_>>(), c.server)
    };
    let (g, _) = texts("/gi");
    assert_eq!(g, vec!["give"]);
    let (items, _) = texts("/give AgentCommands ");
    assert!(items.iter().any(|s| s == "minecraft:diamond"), "{items:?}");
    let (who, _) = texts("/give ");
    assert!(who.contains(&"AgentCommands".to_owned()) && who.contains(&"@a".to_owned()), "{who:?}");
    let (tp, _) = texts("/tp ~ ");
    assert!(tp.contains(&"~ ~".to_owned()), "{tp:?}");
    let (ex, _) = texts("/execute ");
    assert!(ex.contains(&"as".to_owned()) && ex.contains(&"run".to_owned()), "{ex:?}");
    let (_, server) = texts("/scoreboard players set @s ");
    assert!(server);
}

#[test]
fn performance() {
    let Some((t, d)) = setup() else { return };
    let s = "/execute as @e[type=minecraft:zombie,distance=..10,limit=5,sort=nearest] at @s positioned ~ ~1 ~ \
             if block ~ ~-1 ~ minecraft:grass_block run summon minecraft:zombie ~ ~ ~ {PersistenceRequired:1b}";
    let i = chars(s);
    assert!(parse(&t, &i, &d, SRC).valid(&i));
    let n = 2000;
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        std::hint::black_box(parse(&t, &i, &d, SRC));
    }
    let per = t0.elapsed().as_secs_f64() * 1e3 / f64::from(n);
    eprintln!("bench: parsing a command of {} characters: {per:.4} ms", i.len());
    assert!(per < 0.5);
}
