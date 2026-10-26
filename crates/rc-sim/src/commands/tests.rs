//! Command tests on the real tree (extractor): parsing, the game's errors.

use super::*;

fn levels() -> Option<(Vec<Level>, rc_commands::Data)> {
    let data = rc_testdata::version_data()?;
    let lv = load_levels(&rc_data::commands_path(&data.version));
    if lv.len() != 5 {
        return None;
    }
    Some((lv, rc_commands::Data::from_version(&data)))
}

fn texts(c: &[Component]) -> Vec<String> {
    c.iter().map(|x| format!("{x:?}")).collect()
}

#[test]
fn tree_per_level() {
    let Some((lv, _)) = levels() else { return };
    let names = |l: &Level| -> Vec<String> {
        let t = &l.tree;
        t.node(t.root).children.iter().map(|&c| t.node(c).name.clone()).collect()
    };
    // Without rights: commands of a non-operator in singleplayer (seed and version included).
    let n0 = names(&lv[0]);
    assert!(n0.contains(&"seed".to_owned()) && n0.contains(&"me".to_owned()) && !n0.contains(&"gamemode".to_owned()));
    // Owner: singleplayer commands, not those of the dedicated server.
    let n4 = names(&lv[4]);
    assert!(n4.contains(&"publish".to_owned()) && n4.contains(&"execute".to_owned()));
    assert!(!n4.contains(&"op".to_owned()) && !n4.contains(&"stop".to_owned()));
}

#[test]
fn parse_errors() {
    let Some((lv, data)) = levels() else { return };
    let l = &lv[4];
    // Unknown command: message then context "…<--[HERE]".
    let e = analyse(l, &data, "foo bar", 4).err().unwrap();
    assert_eq!(e.len(), 2);
    assert!(texts(&e)[0].contains("command.unknown.command"));
    assert!(texts(&e)[1].contains("command.context.here"));
    // Incomplete.
    let e = analyse(l, &data, "gamemode", 4).err().unwrap();
    assert!(texts(&e)[0].contains("command.unknown.command"));
    // Invalid argument.
    let e = analyse(l, &data, "gamemode foo", 4).err().unwrap();
    assert!(texts(&e)[0].contains("argument.gamemode.invalid"), "{:?}", texts(&e));
    // Valid, with redirected alias.
    let p = analyse(l, &data, "tp @s ~ ~1 ~", 4).ok().unwrap();
    assert_eq!(p.segs.len(), 2);
    assert_eq!(p.segs[0].path_str(), "tp");
    assert_eq!(p.segs[1].path_str(), "teleport <targets> <location>");
    let p = analyse(l, &data, "execute as @a at @s run say hi", 4).ok().unwrap();
    let paths: Vec<String> = p.segs.iter().map(Seg::path_str).collect();
    assert_eq!(paths, ["execute as <targets>", "execute at <targets>", "execute run", "say <message>"]);
}
