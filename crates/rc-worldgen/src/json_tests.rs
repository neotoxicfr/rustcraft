//! Equivalence proofs of the compact JSON reader against the old allocating one: crafted texts
//! (escapes, keys written twice, long numbers and keys, every error), and every JSON file of the
//! jar against a rendering digest frozen with the old reader. Game data required for the last
//! one (jar 26.3 and rc-data cache); absent: skipped, except `RUSTCRAFT_OPT_REQUIRE=1`.

use md5::{Digest, Md5};

use super::*;

/// md5 of the debug rendering of every JSON file of the jar (see [`jar_json_files`]), taken with
/// the old reader (`BTreeMap<String, Json>` objects, `String` numbers) before the compact tree.
const JAR_RENDERING_MD5: &str = "81dfa95ccf68989b9421042607560df4";

/// Texts that reach every branch of both readers, valid or not.
const CRAFTED: &[&str] = &[
    r#"{"a": -0.6666667, "b": [1, 2.5e3], "c": "x\"y", "d": true}"#,
    r#"{"type": "minecraft:add", "argument2": {"x": null}, "argument1": [false, -1E+5, 0.0]}"#,
    r#"{"k": 1, "k": 2}"#,
    r#"{"k": 1, "j": 0, "k": 2, "a": {}, "k": 3}"#,
    r#"{"Ab": "\n\t\r\b\f\"\\\/é\ud83d", "Ab": 2, "Ab": 3}"#,
    r#"{"a_key_much_longer_than_twenty_three_bytes": 1.2345678901234567890123456789e-10}"#,
    r#"{"é": "ünïcode ✓", "": ""}"#,
    "  [ ]  ",
    "{}",
    "\t[[], [[]], {}, [{}]]\r\n",
    "-",
    "1e",
    "\"\"",
    "",
    "   ",
    "{",
    "[1,",
    "[1 2]",
    r#"{"a" 1}"#,
    r#"{"a":1,}"#,
    r#"{"a":1 "b":2}"#,
    "{1: 2}",
    "tru",
    "nul",
    "fals",
    "x",
    "+1",
    "1 2",
    r#""abc"#,
    r#""a\"#,
    r#""\u12""#,
    r#""\uzzzz""#,
    r#"{"k\"#,
    r#"["a", "b\"c", "d"]"#,
];

#[test]
fn keeps_number_text() {
    for compact in [true, false] {
        let v = parse_mode(CRAFTED[0], compact).unwrap();
        assert_eq!(v.get("a"), Some(&Json::Num("-0.6666667".into())));
        assert_eq!(v.get("a").and_then(Json::as_f32), Some(-0.666_666_7));
        assert_eq!(v.get("b").and_then(Json::as_array).map(<[Json]>::len), Some(2));
        assert_eq!(v.get("c").and_then(Json::as_str), Some("x\"y"));
        assert_eq!(v.get("d").and_then(Json::as_bool), Some(true));
    }
}

/// Both readers give the same tree, or the same error at the same byte.
#[test]
fn compact_reader_matches_the_allocating_one_on_crafted_texts() {
    let mut errors = 0;
    for t in CRAFTED {
        let old = parse_mode(t, false);
        errors += usize::from(old.is_err());
        assert_eq!(parse_mode(t, true), old, "{t:?}");
    }
    assert!(errors >= 20, "the crafted texts must keep reaching the error paths: {errors}");
}

/// Value frozen by hand: the last of a key written twice wins, members come in key order, and the
/// rendering is the old tree map's one.
#[test]
fn a_key_written_twice_keeps_its_last_value_and_members_come_in_key_order() {
    for compact in [true, false] {
        let v = parse_mode(r#"{"k": 1, "b": 0, "k": 2, "a": {"y": 1, "x": 2}, "k": 3}"#, compact).unwrap();
        let Json::Obj(m) = &v else { panic!("object expected: {v:?}") };
        assert_eq!(m.iter().map(|(k, _)| k).collect::<Vec<_>>(), ["a", "b", "k"]);
        assert_eq!(m.len(), 3);
        assert_eq!(v.get("k"), Some(&Json::Num("3".into())));
        assert_eq!(v.get("missing"), None);
        assert_eq!(format!("{v:?}"), r#"Obj({"a": Obj({"x": Num("2"), "y": Num("1")}), "b": Num("0"), "k": Num("3")})"#);
    }
}

/// Value frozen by hand for the scanner both readers share (whitespace and number text), which
/// comparing the two readers cannot check.
#[test]
fn every_whitespace_and_number_form_is_read() {
    let text = " \t\r\n[ -1.5E+3 ,\t{ \"a\" :\r\n 2e-7 }, 0, 17 ]\n\t";
    for compact in [true, false] {
        let v = parse_mode(text, compact).unwrap();
        assert_eq!(format!("{v:?}"), r#"Arr([Num("-1.5E+3"), Obj({"a": Num("2e-7")}), Num("0"), Num("17")])"#);
    }
}

/// A value of the tree stays four words: numbers and keys stored in place do not grow it.
#[test]
fn json_value_size() {
    assert_eq!(std::mem::size_of::<Json>(), 32);
    assert_eq!(std::mem::size_of::<(SmolStr, Json)>(), 56);
}

/// Every JSON file of the jar, sorted by key: generation data and block tags of the pack (biome
/// reports included), fluid and biome tags of the decoration data.
pub(crate) fn jar_json_files(w: &crate::chunkgen::WorldData) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = w.pack.texts().map(|(k, t)| (format!("pack {k}"), t.to_owned())).collect();
    for kind in ["tags_fluid", "tags_worldgen_biome"] {
        for (k, b) in w.decor.files_of(kind) {
            files.push((format!("decor {kind}/{k}"), String::from_utf8(b.to_vec()).expect("UTF-8 tag file")));
        }
    }
    files.sort();
    files
}

fn rendering_digest(files: &[(String, String)], compact: bool) -> String {
    let mut h = Md5::new();
    for (k, t) in files {
        h.update(k.as_bytes());
        h.update(b"\n");
        h.update(format!("{:?}", parse_mode(t, compact)).as_bytes());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Every JSON file of the jar reads the same tree with both readers, and that tree renders as the
/// old reader's did before the compact tree existed.
#[test]
fn every_jar_file_reads_the_tree_of_the_old_reader() {
    let Some(w) = crate::structure::tests::world_data() else { return };
    let files = jar_json_files(w);
    assert!(files.len() > 1500, "JSON files of the jar: {}", files.len());
    assert!(files.iter().any(|(k, _)| k == "pack biome_parameters/minecraft:overworld"), "biome reports missing");
    for (k, t) in &files {
        assert_eq!(parse_mode(t, true), parse_mode(t, false), "{k}");
    }
    assert_eq!(rendering_digest(&files, true), JAR_RENDERING_MD5, "compact reader");
    assert_eq!(rendering_digest(&files, false), JAR_RENDERING_MD5, "allocating reader");
}
