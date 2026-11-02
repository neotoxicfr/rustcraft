//! In-game tests of the integrated server (G3-F06): the test instances and test environments of
//! the chosen packs (read at opening and frozen, like the game's registries), the data of the test
//! instance blocks, and the runner behind `/test` and the test instance screen (`run.rs`):
//! batches by environment, structures laid out in a grid or rebuilt in place, ticked after the
//! world, retries, reports to the players.
//!
//! The only built-in test function of the game is `minecraft:always_pass`; a test naming another
//! function fails when it starts, like the game's (its function registry holds nothing else).

pub(crate) mod run;

use std::sync::Arc;

use hashbrown::{HashMap, HashSet};
use rc_proto::nbt::Nbt;
use rc_text::Component;
use rc_worldgen::template::Rot;
use serde_json::Value;

use crate::blocks::P;
use crate::nbtio::{int, set};

/// What a test runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// Built-in function of the game, by identifier.
    Function(String),
    /// Test blocks of the structure: a single start block, accept, fail and log blocks.
    TestBlocks,
}

/// Environment of a test: an entry of the environment registry or an inline definition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EnvRef {
    Named(String),
    Inline(Arc<EnvDef>),
}

impl EnvRef {
    /// Name shown by the batch messages (`[unregistered]` for an inline definition).
    pub(crate) fn name(&self) -> String {
        match self {
            EnvRef::Named(n) => n.clone(),
            EnvRef::Inline(_) => "[unregistered]".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Weather {
    Clear,
    Rain,
    Thunder,
}

/// Definition of a test environment (set up before a batch, torn down after it).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EnvDef {
    Group(Vec<EnvRef>),
    Clock { clock: String, time: i64 },
    Difficulty(i32),
    Function { setup: Option<String>, teardown: Option<String> },
    /// Rules (`minecraft:` key) and their values as the rules file stores them.
    Rules(Vec<(String, Nbt)>),
    /// Timelines added to the environment attributes: the integrated server keeps no attribute
    /// layers of its own, the setting is read and has no effect.
    TimelineLayers(Vec<String>),
    Weather(Weather),
}

/// A test instance of the registry.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TestDef {
    pub id: String,
    pub kind: Kind,
    pub env: EnvRef,
    pub dimension: String,
    pub structure: String,
    pub max_ticks: i32,
    pub setup_ticks: i32,
    pub required: bool,
    pub rotation: Rot,
    pub manual_only: bool,
    pub max_attempts: i32,
    pub required_successes: i32,
    pub sky_access: bool,
    pub padding: i32,
}

impl TestDef {
    pub(crate) fn flaky(&self) -> bool {
        self.max_attempts > 1
    }
}

/// Identifier with the default namespace, or `None` if it is not valid.
fn id(v: &Value) -> Option<String> {
    crate::tech_blocks::template_id(v.as_str()?)
}

const ROTATIONS: [&str; 4] = ["none", "clockwise_90", "180", "counterclockwise_90"];

pub(crate) fn rot_name(r: Rot) -> &'static str {
    ROTATIONS[r as usize]
}

fn rot_of(name: &str) -> Option<Rot> {
    ROTATIONS.iter().position(|r| *r == name).map(|i| Rot::from_index(i as i32))
}

/// Holder of an environment: identifier of the registry or inline definition.
fn env_ref(v: &Value) -> Option<EnvRef> {
    match v {
        Value::String(_) => id(v).map(EnvRef::Named),
        Value::Object(_) => env_def(v).map(|d| EnvRef::Inline(Arc::new(d))),
        _ => None,
    }
}

/// Environment definition (`type` and its fields); `None` if it does not decode.
pub(crate) fn env_def(v: &Value) -> Option<EnvDef> {
    let ty = id(v.get("type")?)?;
    Some(match ty.as_str() {
        "minecraft:all_of" => {
            EnvDef::Group(v.get("definitions")?.as_array()?.iter().map(env_ref).collect::<Option<Vec<_>>>()?)
        }
        "minecraft:clock_time" => {
            let time = v.get("time")?.as_i64().filter(|t| (0..=i64::from(i32::MAX)).contains(t))?;
            EnvDef::Clock { clock: id(v.get("clock")?)?, time }
        }
        "minecraft:difficulty" => {
            let names = ["peaceful", "easy", "normal", "hard"];
            let d = v.get("difficulty")?.as_str()?;
            EnvDef::Difficulty(names.iter().position(|n| *n == d)? as i32)
        }
        "minecraft:function" => {
            let f = |k: &str| match v.get(k) {
                None => Some(None),
                Some(x) => id(x).map(Some),
            };
            EnvDef::Function { setup: f("setup")?, teardown: f("teardown")? }
        }
        "minecraft:game_rules" => {
            let mut rules = Vec::new();
            for (k, val) in v.get("rules")?.as_object()? {
                let key = crate::tech_blocks::template_id(k)?;
                let n = match val {
                    Value::Bool(b) => Nbt::Byte(i8::from(*b)),
                    Value::Number(n) => Nbt::Int(i32::try_from(n.as_i64()?).ok()?),
                    _ => return None,
                };
                rules.push((key, n));
            }
            EnvDef::Rules(rules)
        }
        "minecraft:timeline_attributes" => {
            EnvDef::TimelineLayers(v.get("timelines")?.as_array()?.iter().map(id).collect::<Option<Vec<_>>>()?)
        }
        "minecraft:weather" => EnvDef::Weather(match v.get("weather")?.as_str()? {
            "clear" => Weather::Clear,
            "rain" => Weather::Rain,
            "thunder" => Weather::Thunder,
            _ => return None,
        }),
        _ => return None,
    })
}

/// Test instance (`type`, test data fields); `None` if it does not decode.
pub(crate) fn test_def(test_id: &str, v: &Value) -> Option<TestDef> {
    let kind = match id(v.get("type")?)?.as_str() {
        "minecraft:function" => Kind::Function(id(v.get("function")?)?),
        "minecraft:block_based" => Kind::TestBlocks,
        _ => return None,
    };
    let int_or = |k: &str, d: i64| match v.get(k) {
        None => Some(d),
        Some(x) => x.as_i64(),
    };
    let bool_or = |k: &str, d: bool| match v.get(k) {
        None => Some(d),
        Some(x) => x.as_bool(),
    };
    let positive = |x: i64| (1..=i64::from(i32::MAX)).contains(&x).then_some(x as i32);
    Some(TestDef {
        id: test_id.to_owned(),
        kind,
        env: env_ref(v.get("environment")?)?,
        dimension: match v.get("dimension") {
            None => "minecraft:overworld".into(),
            Some(d) => id(d)?,
        },
        structure: id(v.get("structure")?)?,
        max_ticks: positive(v.get("max_ticks")?.as_i64()?)?,
        setup_ticks: int_or("setup_ticks", 0).filter(|x| (0..=i64::from(i32::MAX)).contains(x))? as i32,
        required: bool_or("required", true)?,
        rotation: match v.get("rotation") {
            None => Rot::None,
            Some(r) => rot_of(r.as_str()?)?,
        },
        manual_only: bool_or("manual_only", false)?,
        max_attempts: positive(int_or("max_attempts", 1)?)?,
        required_successes: positive(int_or("required_successes", 1)?)?,
        sky_access: bool_or("sky_access", false)?,
        padding: int_or("padding", 0).filter(|x| (0..=128).contains(x))? as i32,
    })
}

/// State of a test instance block (its `data`): test, structure size, extra rotation, entities
/// ignored, status (0 cleared, 1 running, 2 finished), error of the last run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Data {
    pub test: Option<String>,
    pub size: [i32; 3],
    pub rot: Rot,
    pub ignore_entities: bool,
    pub status: u8,
    pub error: Option<Component>,
}

const STATUS: [&str; 3] = ["cleared", "running", "finished"];

impl Data {
    pub(crate) fn new(test: Option<String>, size: [i32; 3], rot: Rot) -> Data {
        Data { test, size, rot, ignore_entities: false, status: 0, error: None }
    }

    /// `data` of the block entity (absent or not decodable: an empty instance).
    pub(crate) fn read(be: &Nbt) -> Data {
        let empty = Data::new(None, [0; 3], Rot::None);
        let Some(d) = be.get("data") else { return empty };
        let size = match d.get("size") {
            Some(Nbt::IntArray(v)) if v.len() == 3 => [v[0], v[1], v[2]],
            Some(Nbt::List(l)) if l.len() == 3 => {
                let n = |i: usize| l[i].as_i64().unwrap_or(0) as i32;
                [n(0), n(1), n(2)]
            }
            _ => return empty,
        };
        let (Some(rot), Some(status)) = (
            d.get("rotation").and_then(Nbt::as_str).and_then(rot_of),
            d.get("status").and_then(Nbt::as_str).and_then(|s| STATUS.iter().position(|x| *x == s)),
        ) else {
            return empty;
        };
        let Some(ignore) = int(d, "ignore_entities") else { return empty };
        Data {
            test: d.get("test").and_then(Nbt::as_str).and_then(crate::tech_blocks::template_id),
            size,
            rot,
            ignore_entities: ignore != 0,
            status: status as u8,
            error: d.get("error_message").and_then(rc_text::from_nbt),
        }
    }

    pub(crate) fn write(&self, be: &mut Nbt) {
        let mut d = Vec::with_capacity(6);
        if let Some(t) = &self.test {
            d.push(("test".to_owned(), Nbt::String(t.clone())));
        }
        d.push(("size".to_owned(), Nbt::IntArray(self.size.to_vec())));
        d.push(("rotation".to_owned(), Nbt::String(rot_name(self.rot).to_owned())));
        d.push(("ignore_entities".to_owned(), Nbt::Byte(i8::from(self.ignore_entities))));
        d.push(("status".to_owned(), Nbt::String(STATUS[usize::from(self.status.min(2))].to_owned())));
        if let Some(e) = &self.error {
            d.push(("error_message".to_owned(), crate::commands::text::to_nbt(e)));
        }
        set(be, "data", Nbt::Compound(d));
    }

    /// Same data with a new status (the error is dropped).
    pub(crate) fn with_status(&self, status: u8) -> Data {
        Data { status, error: None, ..self.clone() }
    }
}

/// Registries and state of the in-game tests.
#[derive(Default)]
pub struct GameTests {
    /// Test instances in registry order (`test_instance`).
    pub(crate) tests: Vec<Arc<TestDef>>,
    index: HashMap<String, usize>,
    /// Test environments (`test_environment`).
    pub(crate) envs: HashMap<String, Arc<EnvDef>>,
    /// Runner in progress (the game keeps a single one).
    pub(crate) runner: Option<Box<run::Runner>>,
    /// The runner is ticking: a stop asked meanwhile waits for the end of its tick.
    pub(crate) ticking: bool,
    pub(crate) halting: bool,
    /// Stops asked so far (a run stopped while it starts is dropped).
    pub(crate) stops: u32,
    /// Tests that failed in the last runs (`runfailed`), in order of failure.
    pub(crate) failed: Vec<String>,
    /// Test blocks triggered since their last reset (the game keeps it in the block entity,
    /// never saved): dimension and position.
    pub(crate) triggered: HashSet<(String, P)>,
}

impl GameTests {
    /// Entries of the stacked packs, in the order of the synchronised registry.
    pub(crate) fn read(jar: &crate::registries::Jar, regs: &crate::registries::Registries) -> GameTests {
        let file = |dir: &str, entry: &str| -> Option<Value> {
            let (ns, path) = entry.split_once(':')?;
            serde_json::from_slice(jar.files.get(&format!("data/{ns}/{dir}/{path}.json"))?).ok()
        };
        let mut g = GameTests::default();
        for e in regs.entries("minecraft:test_environment") {
            match file("test_environment", e).as_ref().and_then(env_def) {
                Some(d) => {
                    g.envs.insert(e.clone(), Arc::new(d));
                }
                None => eprintln!("singleplayer: test environment {e} not decoded, ignored"),
            }
        }
        for e in regs.entries("minecraft:test_instance") {
            let def = file("test_instance", e).and_then(|v| test_def(e, &v));
            // A named environment must exist (the game's registry would refuse the entry).
            match def.filter(|d| !matches!(&d.env, EnvRef::Named(n) if !g.envs.contains_key(n))) {
                Some(d) => {
                    g.index.insert(e.clone(), g.tests.len());
                    g.tests.push(Arc::new(d));
                }
                None => eprintln!("singleplayer: test instance {e} not decoded, ignored"),
            }
        }
        g
    }

    pub(crate) fn test(&self, id: &str) -> Option<&Arc<TestDef>> {
        self.index.get(id).map(|&i| &self.tests[i])
    }

    /// Definition behind an environment holder (`None`: unknown name).
    pub(crate) fn env(&self, r: &EnvRef) -> Option<Arc<EnvDef>> {
        match r {
            EnvRef::Named(n) => self.envs.get(n).cloned(),
            EnvRef::Inline(d) => Some(d.clone()),
        }
    }

    /// Forgets the failed tests of the previous runs.
    pub(crate) fn forget_failed(&mut self) {
        self.failed.clear();
    }

    pub(crate) fn remember_failed(&mut self, id: &str) {
        if !self.failed.iter().any(|f| f == id) {
            self.failed.push(id.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_definitions_decode_with_the_game_defaults() {
        let t = test_def(
            "minecraft:always_pass",
            &serde_json::json!({"type": "minecraft:function", "environment": "minecraft:default",
                "function": "minecraft:always_pass", "max_ticks": 1, "required": false, "setup_ticks": 1,
                "structure": "minecraft:empty"}),
        )
        .expect("decoded");
        assert_eq!(t.kind, Kind::Function("minecraft:always_pass".into()));
        assert_eq!((t.max_ticks, t.setup_ticks, t.required, t.max_attempts, t.padding), (1, 1, false, 1, 0));
        assert_eq!((t.dimension.as_str(), t.rotation, t.sky_access), ("minecraft:overworld", Rot::None, false));
        assert_eq!(env_def(&serde_json::json!({"type": "all_of", "definitions": []})), Some(EnvDef::Group(Vec::new())));
        // Out of range or missing required fields: refused like the game's codec.
        let bad = serde_json::json!({"type": "block_based", "environment": "x", "structure": "s", "max_ticks": 0});
        assert!(test_def("x:y", &bad).is_none());
        let pad = serde_json::json!({"type": "block_based", "environment": {"type": "weather", "weather": "rain"},
            "structure": "s", "max_ticks": 5, "padding": 129});
        assert!(test_def("x:y", &pad).is_none());
        let rules = env_def(&serde_json::json!({"type": "game_rules", "rules": {"advance_time": false, "minecraft:random_tick_speed": 0}}));
        assert_eq!(
            rules,
            Some(EnvDef::Rules(vec![
                ("minecraft:advance_time".into(), Nbt::Byte(0)),
                ("minecraft:random_tick_speed".into(), Nbt::Int(0)),
            ]))
        );
    }

    #[test]
    fn instance_data_round_trips_and_rejects_bad_fields() {
        let mut be = Nbt::Compound(Vec::new());
        let mut d = Data::new(Some("minecraft:always_pass".into()), [3, 4, 5], Rot::Cw180);
        d.status = 2;
        d.error = Some(Component::text("boom"));
        d.write(&mut be);
        assert_eq!(Data::read(&be), d);
        assert_eq!(be.get("data").and_then(|x| x.get("rotation")).and_then(Nbt::as_str), Some("180"));
        set(&mut be, "data", Nbt::Compound(vec![("size".into(), Nbt::IntArray(vec![1, 1, 1]))]));
        assert_eq!(Data::read(&be), Data::new(None, [0; 3], Rot::None), "missing fields: default instance");
    }
}
