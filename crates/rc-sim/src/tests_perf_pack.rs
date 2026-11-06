//! The scene bench's `perf` data pack (`tools/bench/perf_pack.py`) compiles with our command
//! parser, the way the game compiles functions at load time: one invalid line makes the game
//! refuse the whole function, and the scene calling it then measures nothing. Needs the pack
//! written for the flat world (`python -I tools/bench/scenes.py pack-files target/perf-pack`)
//! and `RUSTCRAFT_PERF_PACK=<repo>/target/perf-pack/datapacks/perf`.

use std::collections::BTreeSet;

use crate::bench_kit;

fn pack_function_names(pack: &std::path::Path) -> BTreeSet<String> {
    let dir = pack.join("data").join("perf").join("function");
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".mcfunction").map(|n| format!("perf:{n}")))
        .collect()
}

#[test]
#[ignore = "needs RUSTCRAFT_PERF_PACK (tools/bench/scenes.py pack-files)"]
fn perf_pack_functions_compile() {
    let pack = bench_kit::perf_pack_dir();
    let names = pack_function_names(&pack);
    assert!(names.len() >= 50, "only {} functions in {}", names.len(), pack.display());
    let (p, dir) = bench_kit::flat_world_with_pack("perf-pack", &pack);
    let mut refused = Vec::new();
    for name in names.iter().filter(|n| !p.s.cmd.functions.contains_key(*n)) {
        // Refused at load: compiled again for the game's message.
        let file = pack.join("data/perf/function").join(format!("{}.mcfunction", &name["perf:".len()..]));
        let text = std::fs::read_to_string(&file).expect("function file");
        let lines: Vec<&str> = text.lines().collect();
        let why = match crate::commands::exec::compile_function(&lines, p.s.cmd.levels.get(2), &p.s.cmd.data) {
            Err(e) => e,
            Ok(_) => "refused at load".into(),
        };
        refused.push(format!("{name}: {why}"));
    }
    assert!(p.s.cmd.levels.len() > 2, "command tree missing: nothing was checked");
    assert_eq!(p.s.cmd.function_tags.get("minecraft:tick").map(|m| &m[..]), Some(&["perf:tick".to_owned()][..]));
    crate::tests_g3f13::finish(p, dir);
    assert!(refused.is_empty(), "{} functions refused:\n{}", refused.len(), refused.join("\n"));
}

/// The staging commands the scenes type in the chat (`pack-files` writes them in
/// `<out>/chat-commands`, next to the pack): the singleplayer host types them at level 4
/// (cheats on). A refused one shows an error in the chat and its scene measures an empty
/// view while nothing fails.
#[test]
#[ignore = "needs RUSTCRAFT_PERF_PACK (tools/bench/scenes.py pack-files)"]
fn scene_chat_commands_compile() {
    let pack = bench_kit::perf_pack_dir();
    let folder = pack.parent().and_then(std::path::Path::parent).expect("<out>/datapacks/perf").join("chat-commands");
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap_or_else(|e| panic!("{}: {e} (run pack-files again)", folder.display()))
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    let (p, dir) = bench_kit::flat_world_with_pack("perf-chat-commands", &pack);
    assert!(p.s.cmd.levels.len() > 4, "command tree missing: nothing was checked");
    let (mut refused, mut checked) = (Vec::new(), 0);
    for file in &files {
        let text = std::fs::read_to_string(file).expect("commands file");
        for line in text.lines() {
            checked += 1;
            if let Err(e) = crate::commands::exec::compile_function(&[line], p.s.cmd.levels.get(4), &p.s.cmd.data) {
                refused.push(format!("{}: {line}: {e}", file.display()));
            }
        }
    }
    crate::tests_g3f13::finish(p, dir);
    assert!(checked >= 100, "only {checked} commands in {} files of {}", files.len(), folder.display());
    assert!(refused.is_empty(), "{} commands refused:\n{}", refused.len(), refused.join("\n"));
}
