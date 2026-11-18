//! Dynamic item properties (G2-F04): broken / damaged and unbreakable, cast rod, carried on the
//! cursor, key, viewed entity, component predicate, recovery compass, item chosen from the
//! bundle, Christmas chest texture, local time (time zone, locale, once per second).
//! Definitions written here (no game data), clock injected by the environment.

use super::*;
use crate::cache::{decode, encode};

#[derive(Default)]
struct T {
    max_damage: i32,
    damage: i32,
    unbreakable: bool,
    fishing: bool,
    carried: bool,
    components: Vec<&'static str>,
    /// Item chosen from the bundle: (item, item model).
    bundle: Option<(&'static str, &'static str)>,
    special: Vec<u32>,
}

impl StackView for T {
    fn has(&self, c: &str) -> bool {
        match c {
            "minecraft:unbreakable" => self.unbreakable,
            "minecraft:max_damage" | "minecraft:damage" => self.max_damage > 0,
            _ => self.components.contains(&c),
        }
    }
    fn damage(&self) -> i32 {
        self.damage
    }
    fn max_damage(&self) -> i32 {
        self.max_damage
    }
    fn fishing_cast(&self) -> bool {
        self.fishing
    }
    fn carried(&self) -> bool {
        self.carried
    }
    fn special_data(&self, _kind: &str) -> Vec<u32> {
        self.special.clone()
    }
    fn bundle_selected(&self, f: &mut dyn FnMut(&str, &str, &dyn StackView)) -> bool {
        let Some((item, model)) = self.bundle else { return false };
        f(item, model, &T::default());
        true
    }
}

struct V {
    holder: bool,
    last_death: Option<(String, [i32; 3])>,
    keys: Vec<&'static str>,
    view: bool,
    now: Option<i64>,
    offset: i64,
    /// Monotonic clock; `None`: follows the wall clock.
    up: Option<i64>,
}

impl Default for V {
    fn default() -> Self {
        V { holder: true, last_death: None, keys: Vec::new(), view: true, now: None, offset: 0, up: None }
    }
}

impl EvalEnv for V {
    fn has_holder(&self) -> bool {
        self.holder
    }
    fn dimension(&self) -> Option<String> {
        Some("minecraft:overworld".into())
    }
    fn tick(&self) -> i64 {
        40
    }
    fn random(&self) -> f32 {
        0.37
    }
    fn holder_pos(&self) -> [f64; 3] {
        [0.5, 64.0, 0.5]
    }
    fn body_yaw(&self) -> f32 {
        90.0
    }
    fn last_death(&self) -> Option<(String, [i32; 3])> {
        self.last_death.clone()
    }
    fn key_down(&self, k: &str) -> bool {
        self.keys.contains(&k)
    }
    fn view_entity(&self) -> bool {
        self.view
    }
    fn now_ms(&self) -> Option<i64> {
        self.now
    }
    fn uptime_ms(&self) -> i64 {
        self.up.or(self.now).unwrap_or(0)
    }
    fn utc_offset_secs(&self, _ms: i64) -> i64 {
        self.offset
    }
}

const DEFS: [(&str, &str); 12] = [
    ("rod", r#"{"type":"condition","property":"minecraft:fishing_rod/cast","on_true":{"type":"model","model":"item/cast"},"on_false":{"type":"model","model":"item/rod"}}"#),
    ("broken", r#"{"type":"condition","property":"broken","on_true":{"type":"model","model":"item/yes"},"on_false":{"type":"model","model":"item/no"}}"#),
    ("damaged", r#"{"type":"condition","property":"damaged","on_true":{"type":"model","model":"item/yes"},"on_false":{"type":"model","model":"item/no"}}"#),
    ("carried", r#"{"type":"condition","property":"carried","on_true":{"type":"model","model":"item/yes"},"on_false":{"type":"model","model":"item/no"}}"#),
    ("key", r#"{"type":"condition","property":"keybind_down","keybind":"key.use","on_true":{"type":"model","model":"item/yes"},"on_false":{"type":"model","model":"item/no"}}"#),
    ("view", r#"{"type":"condition","property":"view_entity","on_true":{"type":"model","model":"item/yes"},"on_false":{"type":"model","model":"item/no"}}"#),
    ("worn", r#"{"type":"condition","property":"component","predicate":"minecraft:damage","value":{"damage":{"min":5},"durability":{"max":100}},"on_true":{"type":"model","model":"item/yes"},"on_false":{"type":"model","model":"item/no"}}"#),
    ("dyed", r#"{"type":"condition","property":"component","predicate":"dyed_color","on_true":{"type":"model","model":"item/yes"},"on_false":{"type":"model","model":"item/no"}}"#),
    // Bundle definition of the game (GUI context).
    ("bundle", r#"{"type":"select","property":"minecraft:display_context","cases":[{"when":"gui","model":{"type":"minecraft:condition","property":"minecraft:bundle/has_selected_item","on_true":{"type":"minecraft:composite","models":[{"type":"minecraft:model","model":"minecraft:item/bundle_open_back"},{"type":"minecraft:bundle/selected_item"},{"type":"minecraft:model","model":"minecraft:item/bundle_open_front"}]},"on_false":{"type":"minecraft:model","model":"minecraft:item/bundle"}}}],"fallback":{"type":"minecraft:model","model":"minecraft:item/bundle"}}"#),
    ("apple", r#"{"type":"model","model":"item/apple"}"#),
    // Chest definition of the game.
    ("chest", r#"{"type":"minecraft:select","cases":[{"model":{"type":"minecraft:special","base":"minecraft:item/chest","model":{"type":"minecraft:chest","texture":"minecraft:christmas"}},"when":["12-24","12-25","12-26"]}],"fallback":{"type":"minecraft:special","base":"minecraft:item/chest","model":{"type":"minecraft:chest","texture":"minecraft:normal"}},"pattern":"MM-dd","property":"minecraft:local_time"}"#),
    ("clock", r#"{"type":"select","property":"local_time","pattern":"HH:mm","time_zone":"GMT+2","cases":[{"when":"01:30","model":{"type":"model","model":"item/yes"}}],"fallback":{"type":"model","model":"item/no"}}"#),
];

/// Test definitions in the repository's `target` (erased after use).
struct Dir(std::path::PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/g2f04-trials")
            .join(format!("{tag}-{}", std::process::id()));
        let dir = root.join("assets/minecraft/items");
        std::fs::create_dir_all(&dir).expect("trial directory");
        for (name, json) in DEFS {
            std::fs::write(dir.join(format!("{name}.json")), format!(r#"{{"model":{json}}}"#)).expect("write");
        }
        // Reference models: a single layer, to find the index of a model.
        for m in ["yes", "no", "cast", "rod", "apple", "bundle", "bundle_open_back", "bundle_open_front"] {
            std::fs::write(dir.join(format!("ref_{m}.json")), format!(r#"{{"model":{{"type":"model","model":"item/{m}"}}}}"#))
                .expect("write");
        }
        Dir(root)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn load(tag: &str) -> (Dir, ItemModels) {
    let d = Dir::new(tag);
    let a = Assets::new(vec![crate::source::Source::dir(&d.0)]);
    let mut ids: Vec<String> = DEFS.iter().map(|(n, _)| format!("minecraft:{n}")).collect();
    ids.extend(
        ["yes", "no", "cast", "rod", "apple", "bundle", "bundle_open_back", "bundle_open_front"]
            .iter()
            .map(|m| format!("minecraft:ref_{m}")),
    );
    let m = ItemModels::load(&a, ids.iter().map(String::as_str));
    (d, m)
}

/// Layers of a definition, through the two evaluation paths (identical).
fn layers(m: &ItemModels, id: &str, s: &T, e: &V) -> Vec<Layer> {
    let id = format!("minecraft:{id}");
    let (mut l0, mut k0, mut l1, mut k1) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    m.layers_with(false, &id, &id, s, e, 3, &mut l0, &mut k0);
    m.layers_with(true, &id, &id, s, e, 3, &mut l1, &mut k1);
    assert_eq!((&l0, &k0), (&l1, &k1), "{id}: different paths");
    l1
}

fn models(m: &ItemModels, id: &str, s: &T, e: &V) -> Vec<u32> {
    layers(m, id, s, e).iter().map(|l| l.model).collect()
}

/// Index of the model `item/<name>`.
fn model(m: &ItemModels, name: &str) -> u32 {
    models(m, &format!("ref_{name}"), &T::default(), &V::default())[0]
}

#[test]
fn broken_and_damaged_ignore_unbreakable_stacks() {
    let (_d, m) = load("damage");
    let (yes, no) = (model(&m, "yes"), model(&m, "no"));
    let worn = T { max_damage: 250, damage: 249, ..T::default() };
    assert_eq!(models(&m, "broken", &worn, &V::default()), [yes]);
    assert_eq!(models(&m, "damaged", &worn, &V::default()), [yes]);
    let solid = T { unbreakable: true, ..worn };
    assert_eq!(models(&m, "broken", &solid, &V::default()), [no], "unbreakable: never broken");
    assert_eq!(models(&m, "damaged", &solid, &V::default()), [no], "unbreakable: never damaged");
    // Damage beyond the maximum: bounded, always broken; negative damage: new.
    let over = T { max_damage: 10, damage: 50, ..T::default() };
    assert_eq!(models(&m, "broken", &over, &V::default()), [yes]);
    let neg = T { max_damage: 10, damage: -3, ..T::default() };
    assert_eq!(models(&m, "damaged", &neg, &V::default()), [no]);
}

#[test]
fn cast_rod_cursor_key_and_viewed_entity() {
    let (_d, m) = load("holder");
    let (yes, no) = (model(&m, "yes"), model(&m, "no"));
    let e = V::default();
    assert_eq!(models(&m, "rod", &T { fishing: true, ..T::default() }, &e), [model(&m, "cast")]);
    assert_eq!(models(&m, "rod", &T::default(), &e), [model(&m, "rod")]);
    // Without holder (head, frame): never cast nor carried.
    let headless = V { holder: false, ..V::default() };
    assert_eq!(models(&m, "rod", &T { fishing: true, ..T::default() }, &headless), [model(&m, "rod")]);
    assert_eq!(models(&m, "carried", &T { carried: true, ..T::default() }, &e), [yes]);
    assert_eq!(models(&m, "carried", &T { carried: true, ..T::default() }, &headless), [no]);
    assert_eq!(models(&m, "carried", &T::default(), &e), [no]);
    assert_eq!(models(&m, "key", &T::default(), &V { keys: vec!["key.use"], ..V::default() }), [yes]);
    assert_eq!(models(&m, "key", &T::default(), &V { keys: vec!["key.attack"], ..V::default() }), [no]);
    assert_eq!(models(&m, "view", &T::default(), &e), [yes]);
    assert_eq!(models(&m, "view", &T::default(), &V { view: false, ..V::default() }), [no], "camera on another entity");
}

#[test]
fn component_predicates() {
    let (_d, m) = load("component");
    let (yes, no) = (model(&m, "yes"), model(&m, "no"));
    let e = V::default();
    // Damage >= 5 and remaining durability <= 100.
    assert_eq!(models(&m, "worn", &T { max_damage: 100, damage: 5, ..T::default() }, &e), [yes]);
    assert_eq!(models(&m, "worn", &T { max_damage: 100, damage: 4, ..T::default() }, &e), [no]);
    assert_eq!(models(&m, "worn", &T { max_damage: 200, damage: 50, ..T::default() }, &e), [no], "durability 150");
    assert_eq!(models(&m, "worn", &T::default(), &e), [no], "no damage");
    // Component type without its own predicate: presence.
    assert_eq!(models(&m, "dyed", &T { components: vec!["minecraft:dyed_color"], ..T::default() }, &e), [yes]);
    assert_eq!(models(&m, "dyed", &T::default(), &e), [no]);
}

#[test]
fn recovery_compass_points_to_the_death_place() {
    let p = NumProp::Compass { wobble: false, target: CompassTarget::Recovery };
    let (w, i) = (Wobbler::default(), Wobbler::default());
    let s = T::default();
    // Death place due east (+x): angle 0 towards the target, body turned by 90° (0.25 turn).
    let dead = V { last_death: Some(("minecraft:overworld".into(), [100, 64, 0])), ..V::default() };
    let v = number(&p, &s, &dead, 3, &w, &i);
    let a = ((0.5f64 - 0.5).atan2(100.5 - 0.5) / std::f64::consts::TAU) as f32;
    let expect = positive_mod(a + (0.5 - (0.25 - 0.25)), 1.0);
    assert!((v - expect).abs() < 1e-6, "needle {v} instead of {expect}");
    // Without a death place, or in another dimension: random rotation.
    let spin = number(&p, &s, &V::default(), 3, &w, &i);
    let other = V { last_death: Some(("minecraft:the_nether".into(), [100, 64, 0])), ..V::default() };
    assert_eq!(number(&p, &s, &other, 3, &w, &i), spin);
    assert!((spin - expect).abs() > 1e-3);
}

#[test]
fn bundle_draws_the_selected_item() {
    let (_d, m) = load("bundle");
    let e = V::default();
    let chosen = T { bundle: Some(("minecraft:apple", "minecraft:apple")), ..T::default() };
    assert_eq!(
        models(&m, "bundle", &chosen, &e),
        [model(&m, "bundle_open_back"), model(&m, "apple"), model(&m, "bundle_open_front")]
    );
    assert_eq!(models(&m, "bundle", &T::default(), &e), [model(&m, "bundle")]);
    // Item chosen without a definition: missing model in its place.
    let unknown = T { bundle: Some(("minecraft:x", "minecraft:unknown")), ..T::default() };
    assert_eq!(models(&m, "bundle", &unknown, &e)[1], u32::MAX);
}

/// 2026-12-24 23:30 UTC.
const XMAS_EVE: i64 = 1_798_155_005_123;

#[test]
fn christmas_chest_by_local_date() {
    let (_d, m) = load("chest");
    let chest = |e: &V| layers(&m, "chest", &T { special: vec![99], ..T::default() }, e)[0].tints.to_vec();
    let christmas = CHEST_TEXTURES.iter().position(|t| *t == "christmas").expect("christmas") as u32;
    assert_eq!(chest(&V { now: Some(XMAS_EVE), ..V::default() }), [christmas]);
    // On 27 December: normal texture (and not the stack's data).
    assert_eq!(chest(&V { now: Some(XMAS_EVE + 3 * 86_400_000), ..V::default() }), [0]);
}

#[test]
fn local_time_zone_and_pacing() {
    let (_d, m) = load("time");
    let (yes, no) = (model(&m, "yes"), model(&m, "no"));
    let s = T::default();
    // 23:30 UTC = 01:30 in GMT+2, whatever the system time zone.
    let e = V { now: Some(XMAS_EVE), offset: -7 * 3600, ..V::default() };
    assert_eq!(models(&m, "clock", &s, &e), [yes]);
    // Half a second later: text kept; two minutes later: recomputed.
    assert_eq!(models(&m, "clock", &s, &V { now: Some(XMAS_EVE + 500), ..V::default() }), [yes]);
    assert_eq!(models(&m, "clock", &s, &V { now: Some(XMAS_EVE + 120_000), ..V::default() }), [no]);
    // The pacing follows the monotonic clock: a wall clock going back by an hour is re-read one
    // second later (01:30 again), and not frozen until it catches up.
    let (_d4, m4) = load("time-back");
    let at = |now: i64, up: i64| models(&m4, "clock", &s, &V { now: Some(now), up: Some(up), ..V::default() });
    assert_eq!(at(XMAS_EVE + 3_600_000, 10_000), [no]);
    assert_eq!(at(XMAS_EVE, 10_500), [no], "less than a second: text kept");
    assert_eq!(at(XMAS_EVE, 11_001), [yes], "one second later: wall clock re-read");
    // Wall clock jump without an elapsed second: text kept.
    assert_eq!(at(XMAS_EVE + 120_000, 11_500), [yes]);
    // System time zone (environment offset): 23:30 UTC + 2 h.
    let (_d2, m2) = load("time-system");
    let chest = |e: &V| layers(&m2, "chest", &T::default(), e)[0].tints.to_vec();
    assert_eq!(chest(&V { now: Some(XMAS_EVE + 2 * 86_400_000), offset: 0, ..V::default() }), [7], "26/12");
    let (_d3, m3) = load("time-shifted");
    let chest = |e: &V| layers(&m3, "chest", &T::default(), e)[0].tints.to_vec();
    assert_eq!(chest(&V { now: Some(XMAS_EVE + 2 * 86_400_000), offset: 3600, ..V::default() }), [0], "27/12 to the east");
}

#[test]
fn new_fields_read_back_from_the_cache() {
    let (_d, m) = load("cache");
    let back = decode::<ItemModels>(&encode(&m)).expect("re-read");
    let e = V { now: Some(XMAS_EVE), keys: vec!["key.use"], ..V::default() };
    let s = T { max_damage: 100, damage: 7, bundle: Some(("minecraft:apple", "minecraft:apple")), ..T::default() };
    for (id, _) in DEFS {
        assert_eq!(layers(&m, id, &s, &e), layers(&back, id, &s, &e), "{id}");
    }
}

/// Non-living holder (dropped item, frame): the needles see it (position, rotation), the
/// "hand" and "entity type" choices do not; a living one gives its type.
struct NonLiving(Option<&'static str>);

impl EvalEnv for NonLiving {
    fn living_holder(&self) -> bool {
        self.0.is_some()
    }
    fn holder_type(&self) -> Option<String> {
        self.0.map(str::to_owned)
    }
    fn dimension(&self) -> Option<String> {
        Some("minecraft:overworld".into())
    }
    fn holder_pos(&self) -> [f64; 3] {
        [0.5, 64.0, 0.5]
    }
    fn body_yaw(&self) -> f32 {
        150.0
    }
    fn spawn(&self) -> Option<(String, [i32; 3])> {
        Some(("minecraft:overworld".into(), [0, 64, 100]))
    }
    fn local_ticking(&self) -> bool {
        false
    }
}

#[test]
fn non_living_holder_orients_the_needles_without_hand() {
    let p = NumProp::Compass { wobble: false, target: CompassTarget::Spawn };
    let (w, i) = (Wobbler::default(), Wobbler::default());
    let s = T::default();
    let v = number(&p, &s, &NonLiving(None), 3, &w, &i);
    // Target due south (+z): angle 0.25 turn; rotation 150°; formula outside the local player.
    let r = positive_mod(150.0 / 360.0, 1.0);
    let expect = positive_mod(0.5 - (r - 0.25 - 0.25), 1.0);
    assert!((v - expect).abs() < 1e-6, "needle {v} instead of {expect}");
    for prop in [SelectProp::MainHand, SelectProp::ContextEntityType] {
        assert_eq!(select(&prop, &s, &NonLiving(None)), None);
        assert_eq!(select_cow(&prop, &s, &NonLiving(None)), None);
    }
    let zombie = NonLiving(Some("minecraft:zombie"));
    assert_eq!(select(&SelectProp::ContextEntityType, &s, &zombie).as_deref(), Some("minecraft:zombie"));
    assert_eq!(select(&SelectProp::MainHand, &s, &zombie).as_deref(), Some("right"));
}
