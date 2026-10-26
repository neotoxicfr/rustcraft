use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;

use super::*;

fn adv(parent: Option<&str>, display: bool, hidden: bool) -> Value {
    let mut v = json!({ "criteria": { "a": { "trigger": "minecraft:impossible" } } });
    if let Some(p) = parent {
        v["parent"] = json!(p);
    }
    if display {
        v["display"] =
            json!({ "icon": { "id": "minecraft:stone" }, "title": "t", "description": "d", "hidden": hidden });
        if parent.is_none() {
            v["display"]["background"] = json!("minecraft:block/stone");
        }
    }
    v
}

fn tree(items: &[(&str, Value)]) -> Tree {
    let raw: BTreeMap<String, Value> = items.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
    Tree::build(raw)
}

#[test]
fn jar_tree() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(jar) = crate::registries::client_jar(&data) else { return };
    let jar = crate::registries::Jar::read(&jar).unwrap();
    let t = Tree::load(&jar);
    // 26.3: 1,866 advancements of which 1,740 recipes; 5 tabs displayed.
    assert!(t.list.len() > 1800, "{}", t.list.len());
    let roots: Vec<&str> =
        t.list.iter().filter(|a| a.parent.is_none() && a.display.is_some()).map(|a| a.id.as_str()).collect();
    assert_eq!(roots.len(), 5, "{roots:?}");
    let story = t.get("minecraft:story/root").unwrap();
    assert_eq!((t.list[story].x, t.list[story].y >= 0.0), (0.0, true));
    assert!(t.by_trigger["minecraft:inventory_changed"].len() > 1500);
}

#[test]
fn dates_round_trip() {
    let ms = 1_760_000_000_000i64;
    let s = format_date(ms);
    assert_eq!(parse_date(&s), Some(ms));
    assert_eq!(parse_date("2025-10-09 08:53:20 +0000"), Some(ms));
}

#[test]
fn walker_positions() {
    // r → a, b ; a → a1, a2 ; b → b1.
    let t = tree(&[
        ("x:r", adv(None, true, false)),
        ("x:a", adv(Some("x:r"), true, false)),
        ("x:b", adv(Some("x:r"), true, false)),
        ("x:a1", adv(Some("x:a"), true, false)),
        ("x:a2", adv(Some("x:a"), true, false)),
        ("x:b1", adv(Some("x:b"), true, false)),
    ]);
    let p = |id: &str| {
        let a = &t.list[t.get(id).unwrap()];
        (a.x, a.y)
    };
    assert_eq!(p("x:a1"), (2.0, 0.0));
    assert_eq!(p("x:a2"), (2.0, 1.0));
    assert_eq!(p("x:a"), (1.0, 0.5));
    assert_eq!(p("x:b1"), (2.0, 2.0));
    assert_eq!(p("x:b"), (1.0, 2.0));
    assert_eq!(p("x:r"), (0.0, 1.25));
}

#[test]
fn visibility_and_completion() {
    let t = Arc::new(tree(&[
        ("x:r", adv(None, true, false)),
        ("x:a", adv(Some("x:r"), true, false)),
        ("x:h", adv(Some("x:a"), true, true)),
        ("x:c", adv(Some("x:a"), true, false)),
        ("x:d", adv(Some("x:c"), true, false)),
        ("x:e", adv(Some("x:d"), true, false)),
    ]));
    let dir = std::env::temp_dir().join(format!("rc-s9-{}", std::process::id()));
    let mut p = Progress::new(t.clone(), &dir, 1);
    // Nothing done: nothing visible, first send empty (not sent).
    assert!(p.flush().is_none());
    let r = t.get("x:r").unwrap();
    assert_eq!(p.grant(r, 0), (true, true));
    assert!(p.done(r));
    let (reset, added, _, progress) = p.flush().unwrap();
    assert!(!reset);
    let names: Vec<&str> = added.iter().map(|&i| t.list[i].id.as_str()).collect();
    // Root done: it, a and c (2 levels below it) visible; the hidden one, d and e not.
    assert!(names.contains(&"x:r") && names.contains(&"x:a") && names.contains(&"x:c"));
    assert!(!names.contains(&"x:h") && !names.contains(&"x:d"));
    assert!(progress.contains(&r));
    let json = p.to_json(4000);
    assert!(json.contains("\"x:r\"") && json.contains("\"done\": true"));
}
