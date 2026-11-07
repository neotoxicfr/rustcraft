//! Equivalence proof of the parser walk without copies (`PARSERNOALLOC`): for every command of a
//! corpus and every truncation of it (what the client parses at each key press), at the five
//! permission levels of the real tree, the parse (context, cursor, errors) is the one of the
//! former walk. Also the parsed form kept by the functions (text and byte ranges): the arguments
//! read back are those of the characters parsed.

use super::*;

const CORPUS: &[&str] = &[
    "give @a minecraft:diamond_sword[custom_name='x',!damage] 1",
    "tp @s ^ ^ ^1",
    "tp ~ ~1 ~",
    "execute as @e[type=cow,limit=3,sort=nearest] at @s run tp ~ ~1 ~",
    "execute as @e[type=minecraft:sheep,limit=50,sort=nearest] if data entity @s {Color:0b} run scoreboard players add #white perf_dp 1",
    "execute store result score #count perf_dp if entity @e[type=minecraft:sheep,distance=..16]",
    "execute if score #a fc matches 1.. run say plain é",
    "execute positioned ~ ~1 ~ rotated as @p facing entity @e[limit=1] eyes anchored eyes run say x",
    "execute in minecraft:the_nether run return run function fc:val",
    "setblock ~ ~ ~ minecraft:oak_stairs[facing=east]{}",
    "setblock 3 -58 8 minecraft:repeating_command_block[facing=east]{auto:1b,Command:'say hi'}",
    "fill ~ ~ ~ ~1 ~1 ~1 stone replace #logs",
    "say hello everyone ✓ Zoé",
    "tellraw @a {\"text\":\"Zoé ✓\",\"color\":\"red\"}",
    "scoreboard objectives add x dummy",
    "scoreboard players operation #b fc += #a fc",
    "scoreboard bogus",
    "time set 1d",
    "gamemode creative",
    "effect give @e[type=minecraft:chicken,distance=..64] minecraft:speed 2 0 true",
    "particle minecraft:happy_villager ~ ~ ~ 4 1 4 0 8 normal",
    "particle dust{color:[1.0,0.0,0.0],scale:5} ~ ~ ~ 0 0 0 0 1",
    "data modify storage fc:out list append value 1",
    "data merge entity @s {CustomName:\"Bob\"}",
    "data get entity @s Inventory[0].id",
    "clear @s *[damage]",
    "summon minecraft:zombie ~ ~ ~ {PersistenceRequired:1b}",
    "function fc:mac {v:3,t:\"a b\"}",
    "function fc:mac with storage fc:args",
    "function #minecraft:tick",
    "schedule function fc:later 2t append",
    "schedule clear fc:other",
    "kill @e[type=minecraft:item,distance=..64]",
    "foo bar",
    "tp @s ~ ~ ~ ~2 ~",
    "return 42",
    "return run function fc:val",
    "item replace entity @s armor.head with minecraft:diamond_helmet",
    "gamerule max_command_sequence_length 10",
    "  say leading spaces",
    "/say slash",
    "execute if block ~ ~-1 ~ #minecraft:logs unless entity @a[gamemode=!creative,scores={x=1..}] run kill @s",
];

fn levels() -> (Vec<Level>, rc_commands::Data) {
    let data = rc_testdata::version_data().expect("26.3 version data missing");
    let lv = load_levels(&rc_data::commands_path(&data.version));
    assert_eq!(lv.len(), 5, "command tree missing: redo the data preparation");
    (lv, rc_commands::Data::from_version(&data))
}

fn parse_trace(l: &Level, data: &rc_commands::Data, input: &[char], op: u8, no_alloc: bool) -> String {
    let src = rc_commands::parse::Source { restricted: true, selectors: op >= 2 };
    format!("{:?}", rc_commands::parse::parse_with(&l.tree, input, data, src, no_alloc))
}

#[test]
fn parser_without_copies_parses_like_before() {
    let (lv, data) = levels();
    let mut compared = 0;
    for (op, l) in lv.iter().enumerate() {
        let op = op as u8;
        for text in CORPUS {
            let chars: Vec<char> = text.chars().collect();
            for end in 0..=chars.len() {
                let input = &chars[..end];
                let before = parse_trace(l, &data, input, op, false);
                let now = parse_trace(l, &data, input, op, true);
                assert_eq!(now, before, "level {op}, input {:?}", input.iter().collect::<String>());
                compared += 1;
            }
        }
    }
    assert!(compared > 5_000, "only {compared} parses compared");
}

/// The kept parse reads its arguments from the UTF-8 text by byte ranges: the values are those
/// of the character ranges of the parser, accents and symbols included.
#[test]
fn kept_parse_reads_the_arguments_parsed() {
    let (lv, data) = levels();
    let l = &lv[4];
    for text in CORPUS {
        let Ok(p) = analyse(l, &data, text.strip_prefix('/').unwrap_or(text), 4) else { continue };
        let chars: Vec<char> = p.text.chars().collect();
        let raw = rc_commands::parse(&l.tree, &chars, &data, rc_commands::parse::Source { restricted: true, selectors: true });
        // Same segments, same argument values as the character ranges of the walk.
        let mut ctx = Some(&raw.ctx);
        let mut segs = p.segs.iter();
        while let Some(c) = ctx {
            let seg = segs.next().expect("segment");
            let wanted: Vec<String> = c
                .nodes
                .iter()
                .filter(|(id, _)| l.tree.node(*id).kind == Kind::Argument)
                .map(|(_, (a, b))| chars[*a..*b].iter().collect())
                .collect();
            let got: Vec<String> = seg.args.iter().map(|(n, _)| p.arg(seg, n).expect("argument")).collect();
            assert_eq!(got, wanted, "{text}");
            assert_eq!(seg.path_str(), seg.path.join(" "), "{text}");
            ctx = c.child.as_deref();
        }
        assert!(segs.next().is_none(), "{text}: extra segments");
    }
}

/// A parse does not read the teams nor the players (only the suggestions do): a team or player
/// change never has to empty the parse cache.
#[test]
fn parse_ignores_teams_and_players() {
    let (lv, data) = levels();
    let mut other = data.clone();
    other.teams = vec!["red".into(), "blue".into(), "Équipe".into()];
    other.players = vec!["Alex".into(), "Zoé".into()];
    for text in CORPUS.iter().copied().chain(["team join red @s", "team leave Alex", "tell Zoé hi", "msg Alex x"]) {
        let before = analyse(&lv[4], &data, text, 4).map(|p| format!("{p:?}")).map_err(|e| format!("{e:?}"));
        let after = analyse(&lv[4], &other, text, 4).map(|p| format!("{p:?}")).map_err(|e| format!("{e:?}"));
        assert_eq!(before, after, "{text}");
    }
}
