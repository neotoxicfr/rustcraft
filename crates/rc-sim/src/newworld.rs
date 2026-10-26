//! Creation of a new world ("Create New World" screen): `level.dat`, generation settings
//! (`data/minecraft/world_gen_settings.dat`, from the jar's presets), modified game
//! rules, then spawn point searched like the game (noise origin chunk, spiral
//! of 11 × 11 chunks, first block with a full top face outside a fluid). The generated chunks
//! for the search are written: the first load reads them back.

use std::path::Path;
use std::sync::Arc;

use rc_data::VersionData;
use rc_proto::nbt::Nbt;
use rc_worldgen::chunkgen::GenSpec;
use rc_worldgen::decor_blocks::{Blocks, Dir};
use rc_worldgen::decor_region::{DecoChunk, Hm};
use rc_worldgen::json::Json;

use crossbeam_channel::Receiver;

use crate::Error;
use crate::chunk::DATA_VERSION;
use crate::generation::{Out, Setup, Worker, spec_from_nbt};
use crate::nbtio;
use crate::region::{RegionFile, local_index, region_name};

/// World type (`worldgen/world_preset` presets of the jar).
#[derive(Debug, Clone, PartialEq)]
pub enum Preset {
    Normal,
    Flat,
    LargeBiomes,
    Amplified,
    /// A single biome at the surface (biome identifier).
    SingleBiome(String),
    Debug,
}

impl Preset {
    fn id(&self) -> &'static str {
        match self {
            Preset::Normal => "minecraft:normal",
            Preset::Flat => "minecraft:flat",
            Preset::LargeBiomes => "minecraft:large_biomes",
            Preset::Amplified => "minecraft:amplified",
            Preset::SingleBiome(_) => "minecraft:single_biome_surface",
            Preset::Debug => "minecraft:debug_all_block_states",
        }
    }
}

/// Choices of the creation screen.
#[derive(Debug, Clone)]
pub struct NewWorld {
    /// Displayed name (`LevelName`).
    pub name: String,
    pub seed: i64,
    pub preset: Preset,
    /// Settings of the custom flat world (`settings` compound of the flat generator), otherwise those of the preset.
    pub flat_settings: Option<Nbt>,
    /// 0 survival, 1 creative, 2 adventure, 3 spectator.
    pub game_mode: i32,
    pub hardcore: bool,
    /// `peaceful`, `easy`, `normal`, `hard`.
    pub difficulty: String,
    pub allow_commands: bool,
    pub generate_structures: bool,
    pub bonus_chest: bool,
    /// Changed rules (`minecraft:<name>` → value: byte 0/1 or integer).
    pub game_rules: Vec<(String, Nbt)>,
}

impl NewWorld {
    /// Default settings of the screen (survival, normal, structures, random seed).
    pub fn new(name: &str) -> NewWorld {
        NewWorld {
            name: name.to_owned(),
            seed: seed_from_text(""),
            preset: Preset::Normal,
            flat_settings: None,
            game_mode: 0,
            hardcore: false,
            difficulty: "normal".into(),
            allow_commands: false,
            generate_structures: true,
            bonus_chest: false,
            game_rules: Vec::new(),
        }
    }
}

/// Seed from the screen field, like the game: empty → random; 64-bit integer →
/// itself; otherwise the Java string hash of the text (31·h + c, on 32 bits, extended).
pub fn seed_from_text(text: &str) -> i64 {
    let t = text.trim();
    if t.is_empty() {
        let n = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_nanos());
        let mut x = (n as u64) ^ 0x9e37_79b9_7f4a_7c15;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        return x as i64;
    }
    if let Ok(v) = t.parse::<i64>() {
        return v;
    }
    let mut h: i32 = 0;
    for c in t.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(i32::from(c));
    }
    i64::from(h)
}

fn json_to_nbt(j: &Json) -> Nbt {
    match j {
        Json::Null => Nbt::Compound(Vec::new()),
        Json::Bool(b) => Nbt::Byte(i8::from(*b)),
        Json::Num(s) => match s.parse::<i32>() {
            Ok(v) => Nbt::Int(v),
            Err(_) => s.parse::<i64>().map(Nbt::Long).unwrap_or_else(|_| Nbt::Double(s.parse().unwrap_or(0.0))),
        },
        Json::Str(s) => Nbt::String(s.clone()),
        Json::Arr(a) => Nbt::List(a.iter().map(json_to_nbt).collect()),
        Json::Obj(m) => Nbt::Compound(m.iter().map(|(k, v)| (k.clone(), json_to_nbt(v))).collect()),
    }
}

fn s(v: &str) -> Nbt {
    Nbt::String(v.to_owned())
}

/// Creates the world in `dir` (new or empty folder). The game jar and the reports of the data generator
/// must be present (as for playing singleplayer).
pub fn create_world(dir: &Path, data: &Arc<VersionData>, w: &NewWorld) -> Result<(), Error> {
    if dir.join("level.dat").exists() {
        return Err(Error::Refused(format!("{}: a world already exists", dir.display())));
    }
    let jar = crate::registries::client_jar(data).ok_or_else(|| Error::Refused("game jar not found".into()))?;
    let pack = rc_worldgen::data::DataPack::from_jar(&jar).map_err(|e| Error::Format(e.to_string()))?;
    let preset = pack.json("world_preset", w.preset.id()).map_err(|e| Error::Format(e.to_string()))?;
    let mut dims = match preset.get("dimensions").map(json_to_nbt) {
        Some(d @ Nbt::Compound(_)) => d,
        _ => return Err(Error::Format("preset without dimensions".into())),
    };
    if let Some(ow) = nbtio::get_mut(&mut dims, "minecraft:overworld").and_then(|o| nbtio::get_mut(o, "generator")) {
        if let Preset::SingleBiome(b) = &w.preset
            && let Some(src) = nbtio::get_mut(ow, "biome_source")
        {
            nbtio::set(src, "biome", s(b));
        }
        if let Some(f) = &w.flat_settings
            && ow.get("type").and_then(Nbt::as_str) == Some("minecraft:flat")
        {
            nbtio::set(ow, "settings", f.clone());
        }
    }
    let wgs = Nbt::Compound(vec![
        ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
        (
            "data".into(),
            Nbt::Compound(vec![
                ("seed".into(), Nbt::Long(w.seed)),
                ("generate_structures".into(), Nbt::Byte(i8::from(w.generate_structures))),
                ("bonus_chest".into(), Nbt::Byte(i8::from(w.bonus_chest))),
                ("dimensions".into(), dims.clone()),
            ]),
        ),
    ]);
    let data_dir = dir.join("data").join("minecraft");
    std::fs::create_dir_all(&data_dir).map_err(|e| Error::io(&data_dir, e))?;
    write_dat(&data_dir.join("world_gen_settings.dat"), &wgs)?;
    let overworld = "minecraft:overworld";
    let spec = dims.get(overworld).and_then(|d| d.get("generator")).and_then(spec_from_nbt);
    let debug = matches!(spec, Some(GenSpec::Debug));
    let mut game_rules = w.game_rules.clone();
    if debug {
        // Debug world: weather frozen (set after the chosen rules, like the game).
        game_rules.retain(|(k, _)| k != "minecraft:advance_weather");
        game_rules.push(("minecraft:advance_weather".into(), Nbt::Byte(0)));
        // Clock of the main world brought to noon.
        if let Some(noon) = time_marker(&jar, overworld, "minecraft:noon") {
            let clock = Nbt::Compound(vec![("total_ticks".into(), Nbt::Long(noon))]);
            let clocks = Nbt::Compound(vec![
                ("data".into(), Nbt::Compound(vec![(overworld.into(), clock)])),
                ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
            ]);
            write_dat(&data_dir.join("world_clocks.dat"), &clocks)?;
        }
    }
    if !game_rules.is_empty() {
        let rules = Nbt::Compound(vec![
            ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
            ("data".into(), Nbt::Compound(game_rules)),
        ]);
        write_dat(&data_dir.join("game_rules.dat"), &rules)?;
    }
    let spawn = match spec {
        Some(GenSpec::Debug) | None => [0, 80, 0],
        Some(spec) => find_spawn(dir, data, &jar, spec, w.seed, w.bonus_chest)?,
    };
    write_dat(&dir.join("level.dat"), &level_dat(w, spawn, debug))?;
    Ok(())
}

/// Instant when a new clock (at 0) reaches the marker `marker` of a jar timeline, as
/// the `time set` command brings it: the marker if it is not already there, otherwise 0.
fn time_marker(jar: &Path, clock: &str, marker: &str) -> Option<i64> {
    use serde_json::Value;
    let index = rc_data::jar::index(jar).ok()?;
    for (name, blob) in index.with_prefix("data/") {
        let Some((_, path)) = name["data/".len()..].split_once('/') else { continue };
        if !path.starts_with("timeline/") {
            continue;
        }
        let Ok(t) = serde_json::from_slice::<Value>(blob.as_bytes()) else { continue };
        if t.get("clock").and_then(Value::as_str) != Some(clock) {
            continue;
        }
        let Some(m) = t.get("time_markers").and_then(|m| m.get(marker)) else { continue };
        let ticks = m.as_i64().or_else(|| m.get("ticks").and_then(Value::as_i64)).unwrap_or(0);
        let period = t.get("period_ticks").and_then(Value::as_i64).filter(|p| *p > 0);
        let occurs = period.map_or(ticks == 0, |p| ticks.rem_euclid(p) == 0);
        return Some(if occurs { 0 } else { ticks });
    }
    None
}

fn write_dat(path: &Path, v: &Nbt) -> Result<(), Error> {
    crate::save::replace(path, &nbtio::gzip(&nbtio::encode_root(v)), false).map_err(|e| Error::io(path, e))
}

/// New `level.dat`; debug world: peaceful locked and spectator, whatever the choices
/// (hardcore included), like the game at initialisation.
fn level_dat(w: &NewWorld, spawn: [i32; 3], debug: bool) -> Nbt {
    let now = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64);
    let difficulty = if debug {
        "peaceful"
    } else if w.hardcore {
        "hard"
    } else {
        w.difficulty.as_str()
    };
    let game_type = if debug {
        3
    } else if w.hardcore {
        0
    } else {
        w.game_mode
    };
    let data = vec![
        (
            "difficulty_settings".into(),
            Nbt::Compound(vec![
                ("difficulty".into(), s(difficulty)),
                ("hardcore".into(), Nbt::Byte(i8::from(w.hardcore))),
                ("locked".into(), Nbt::Byte(i8::from(debug))),
            ]),
        ),
        ("Time".into(), Nbt::Long(0)),
        ("version_history".into(), Nbt::List(vec![Nbt::Int(DATA_VERSION)])),
        ("GameType".into(), Nbt::Int(game_type)),
        ("ServerBrands".into(), Nbt::List(vec![s("vanilla")])),
        ("version".into(), Nbt::Int(19133)),
        ("LastPlayed".into(), Nbt::Long(now)),
        (
            "spawn".into(),
            Nbt::Compound(vec![
                ("pos".into(), Nbt::IntArray(spawn.to_vec())),
                ("pitch".into(), Nbt::Float(0.0)),
                ("dimension".into(), s("minecraft:overworld")),
                ("yaw".into(), Nbt::Float(0.0)),
            ]),
        ),
        (
            "Version".into(),
            Nbt::Compound(vec![
                ("Snapshot".into(), Nbt::Byte(0)),
                ("Series".into(), s("main")),
                ("Id".into(), Nbt::Int(DATA_VERSION)),
                ("Name".into(), s("26.3")),
            ]),
        ),
        ("LevelName".into(), s(&w.name)),
        ("initialized".into(), Nbt::Byte(1)),
        ("WasModded".into(), Nbt::Byte(0)),
        ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
        ("allowCommands".into(), Nbt::Byte(i8::from(w.allow_commands))),
        (
            "DataPacks".into(),
            Nbt::Compound(vec![
                ("Enabled".into(), Nbt::List(vec![s("vanilla")])),
                (
                    "Disabled".into(),
                    Nbt::List(vec![s("minecart_improvements"), s("redstone_experiments"), s("trade_rebalance")]),
                ),
            ]),
        ),
    ];
    Nbt::Compound(vec![("Data".into(), Nbt::Compound(data))])
}

/// Spawn point (spiral of chunks around the noise origin); the chunks generated
/// for the search are written into the regions.
fn find_spawn(
    dir: &Path,
    data: &Arc<VersionData>,
    jar: &Path,
    spec: GenSpec,
    seed: i64,
    bonus_chest: bool,
) -> Result<[i32; 3], Error> {
    let regs = crate::registries::load(jar, data)?;
    let dimension = "minecraft:overworld";
    let height = crate::dimension_height(data, dimension);
    let (tx, rx) = crossbeam_channel::unbounded();
    let setup = Setup {
        data: data.clone(),
        world: dir.to_owned(),
        dimension: dimension.into(),
        height,
        biome_registry: regs.entries("minecraft:worldgen/biome").to_vec(),
        spec,
        seed,
    };
    let mut w = Worker::new(setup, tx).map_err(Error::Refused)?;
    let origin = w.dim_gen().spawn_chunk();
    let fallback_y = w.dim_gen().spawn_height();
    let mut found = None;
    let (mut x, mut z, mut dx, mut dz) = (0i32, 0i32, 0i32, -1i32);
    for _ in 0..121 {
        if (-5..=5).contains(&x) && (-5..=5).contains(&z) {
            let p = (origin.0 + x, origin.1 + z);
            w.process(&[p]);
            if let Some(c) = w.chunk(p)
                && let Some(pos) = spawn_in_chunk(c, &w.dim_gen().blocks)
            {
                found = Some(pos);
                break;
            }
        }
        if x == z || (x < 0 && x == -z) || (x > 0 && x == 1 - z) {
            let old = dx;
            dx = -dz;
            dz = old;
        }
        x += dx;
        z += dz;
    }
    // Writing of the chunks produced (full and proto-chunks, in order of arrival).
    let region_dir = crate::world::dimension_dir(dir, dimension).join("region");
    std::fs::create_dir_all(&region_dir).map_err(|e| Error::io(&region_dir, e))?;
    let mut files: hashbrown::HashMap<String, RegionFile> = hashbrown::HashMap::new();
    // Full chunks kept until the end (light redone when a neighbour arrives), then written.
    let mut fulls: Vec<crate::chunk::Chunk> = Vec::new();
    let mut sink = Sink { rx, ents: Vec::new(), out: Vec::new() };
    sink.absorb(&mut fulls)?;
    let spawn = found.unwrap_or([origin.0 * 16 + 8, fallback_y, origin.1 * 16 + 8]);
    if bonus_chest {
        bonus_chest_at(&mut fulls, spawn, data, &regs, &mut w, &mut sink, height)?;
    }
    // Proto-chunks still in memory (lazy writing): returned before writing; never
    // a full one (those follow and would replace their proto anyway).
    w.flush();
    sink.absorb(&mut fulls)?;
    let states = w.states();
    for c in fulls {
        if let Some(n) = c.encode(data, states, height, 0) {
            sink.out.push(((c.x, c.z), nbtio::encode_root(&n)));
        }
    }
    write_entities(dir, dimension, &sink.ents)?;
    for (p, bytes) in sink.out {
        let name = region_name(p.0, p.1);
        let f = match files.entry(name.clone()) {
            hashbrown::hash_map::Entry::Occupied(e) => e.into_mut(),
            hashbrown::hash_map::Entry::Vacant(e) => {
                let path = region_dir.join(&name);
                e.insert(RegionFile::open_or_create(&path).map_err(|er| Error::io(&path, er))?)
            }
        };
        f.write(local_index(p.0, p.1), p.0, p.1, &bytes).map_err(|e| Error::io(&region_dir, e))?;
    }
    for (_, f) in files {
        f.close().map_err(|e| Error::io(&region_dir, e))?;
    }
    Ok(spawn)
}

/// Bonus chest at the spawn point (`chunk::place_bonus_chest`, non-reproducible world random),
/// then light redone of the full chunks touched and their neighbours.
fn bonus_chest_at(
    fulls: &mut Vec<crate::chunk::Chunk>,
    spawn: [i32; 3],
    data: &VersionData,
    regs: &crate::registries::Registries,
    w: &mut Worker,
    sink: &mut Sink,
    height: crate::chunk::Height,
) -> Result<(), Error> {
    let tags: hashbrown::HashMap<String, Vec<u32>> = regs
        .tags
        .iter()
        .find(|(r, _)| r == "minecraft:block")
        .map(|(_, t)| {
            t.iter()
                .map(|(n, ids)| {
                    let mut v: Vec<u32> = ids.iter().map(|&i| i as u32).collect();
                    v.sort_unstable();
                    (n.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default();
    let rules = crate::chunk::heightmap_rules(data, &tags);
    let rng = crate::rng::Rng::from_time();
    // A torch on an edge column falls into the neighbouring chunk: the game generates it up to the
    // full status to place it, we do the same then start again with the same seed.
    let mut unavailable: Vec<(i32, i32)> = Vec::new();
    let touched = loop {
        let mut attempt = rng;
        match crate::chunk::place_bonus_chest(
            fulls,
            spawn,
            data,
            w.states(),
            (&rules, &unavailable),
            height,
            &mut attempt,
        ) {
            Ok(t) => break t,
            Err(q) => {
                w.process(&[q]);
                sink.absorb(fulls)?;
                if !fulls.iter().any(|c| (c.x, c.z) == q) {
                    unavailable.push(q);
                }
            }
        }
    };
    let mut relight: Vec<(i32, i32)> = Vec::new();
    for &(x, z) in &touched {
        for i in 0..9 {
            let q = (x + i % 3 - 1, z + i / 3 - 1);
            if !relight.contains(&q) && fulls.iter().any(|c| (c.x, c.z) == q) {
                relight.push(q);
            }
        }
    }
    let lt = rc_light::LightTables::new(data);
    type Layers = (Vec<rc_world::LightLayer>, Vec<rc_world::LightLayer>);
    let h = height.sections as i32 * 16;
    let lit: Vec<((i32, i32), Layers)> = if newworld_light() {
        // Columns of the full chunks unfolded once per position, those of the generator borrowed;
        // lighting in parallel (order of `relight` kept by the collection).
        use rayon::prelude::*;
        let mut need: Vec<(i32, i32)> = Vec::new();
        for &(x, z) in &relight {
            for i in 0..9 {
                let q = (x + i % 3 - 1, z + i / 3 - 1);
                if !need.contains(&q) {
                    need.push(q);
                }
            }
        }
        let unpacked: hashbrown::HashMap<(i32, i32), Vec<u32>> = need
            .par_iter()
            .filter_map(|&q| fulls.iter().find(|c| (c.x, c.z) == q).map(|c| (q, c.unpack_blocks())))
            .collect();
        let cols: hashbrown::HashMap<(i32, i32), &[u32]> = need
            .iter()
            .filter_map(|&q| match unpacked.get(&q) {
                Some(v) => Some((q, &v[..])),
                None => w.chunk(q).map(|d| (q, &d.blocks[..])),
            })
            .collect();
        relight
            .par_iter()
            .map(|&(x, z)| {
                let refs: rc_light::Columns =
                    std::array::from_fn(|i| cols.get(&(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1)).copied());
                ((x, z), rc_light::light_chunk(&lt, refs, h, true))
            })
            .collect()
    } else {
        let blocks_of = |q: (i32, i32)| -> Option<Vec<u32>> {
            match fulls.iter().find(|c| (c.x, c.z) == q) {
                Some(c) => Some(c.unpack_blocks()),
                None => w.chunk(q).map(|d| d.blocks.clone()),
            }
        };
        relight
            .iter()
            .map(|&(x, z)| {
                let cols: Vec<Option<Vec<u32>>> = (0..9).map(|i| blocks_of((x + i % 3 - 1, z + i / 3 - 1))).collect();
                let refs: rc_light::Columns = std::array::from_fn(|i| cols[i].as_deref());
                ((x, z), rc_light::light_chunk(&lt, refs, h, true))
            })
            .collect()
    };
    for (q, (sky, block)) in lit {
        if let Some(c) = fulls.iter_mut().find(|c| (c.x, c.z) == q) {
            c.sky = sky;
            c.block = block;
        }
    }
    Ok(())
}

/// `RUSTCRAFT_OPT_NEWWORLDLIGHT` (default: on; `0`: columns cloned per lighting, one thread):
/// relight of the bonus chest on borrowed columns, in parallel.
fn newworld_light() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::generation::opt_switch(&C, "RUSTCRAFT_OPT_NEWWORLDLIGHT", true)
}

/// Structure entities of the full chunks produced: (chunk, entities).
type Produced = ((i32, i32), Vec<rc_proto::nbt::Nbt>);

/// What the generator returned during the spawn search.
struct Sink {
    rx: Receiver<Out>,
    /// Structure entities of the full chunks.
    ents: Vec<Produced>,
    /// Proto-chunks (and full ones, at the end) encoded, to write into their region.
    out: Vec<((i32, i32), Vec<u8>)>,
}

impl Sink {
    /// Stores the chunks that arrived: full ones (with their entities), light redone, proto-chunks.
    fn absorb(&mut self, fulls: &mut Vec<crate::chunk::Chunk>) -> Result<(), Error> {
        for o in self.rx.try_iter() {
            match o {
                Out::Full(c, e) => {
                    if !e.is_empty() {
                        self.ents.push(((c.x, c.z), e));
                    }
                    fulls.push(*c);
                }
                Out::Relight(p, sky, block) => {
                    if let Some(c) = fulls.iter_mut().find(|c| (c.x, c.z) == p) {
                        c.sky = sky;
                        c.block = block;
                    }
                }
                Out::Proto(p, b) => self.out.push((p, b)),
                Out::Disabled(e) => return Err(Error::Refused(e)),
                Out::Batch(_) => {}
            }
        }
        Ok(())
    }
}

/// Structure entities of the full chunks: `entities/` files of the dimension.
fn write_entities(dir: &Path, dimension: &str, ents: &[((i32, i32), Vec<rc_proto::nbt::Nbt>)]) -> Result<(), Error> {
    use rc_proto::nbt::Nbt;
    if ents.is_empty() {
        return Ok(());
    }
    let edir = crate::world::dimension_dir(dir, dimension).join("entities");
    std::fs::create_dir_all(&edir).map_err(|e| Error::io(&edir, e))?;
    let mut files: hashbrown::HashMap<String, RegionFile> = hashbrown::HashMap::new();
    for ((x, z), list) in ents {
        let root = Nbt::Compound(vec![
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
            ("Position".into(), Nbt::IntArray(vec![*x, *z])),
            ("Entities".into(), Nbt::List(list.clone())),
        ]);
        let name = region_name(*x, *z);
        let f = match files.entry(name.clone()) {
            hashbrown::hash_map::Entry::Occupied(e) => e.into_mut(),
            hashbrown::hash_map::Entry::Vacant(e) => {
                let path = edir.join(&name);
                e.insert(RegionFile::open_or_create(&path).map_err(|er| Error::io(&path, er))?)
            }
        };
        f.write(local_index(*x, *z), *x, *z, &nbtio::encode_root(&root)).map_err(|e| Error::io(&edir, e))?;
    }
    for (_, f) in files {
        f.close().map_err(|e| Error::io(&edir, e))?;
    }
    Ok(())
}

/// First valid spawn block of a chunk (columns x then z), like the game.
fn spawn_in_chunk(c: &DecoChunk, b: &Blocks) -> Option<[i32; 3]> {
    let top_y = c.min_y + c.height;
    for x in 0..16 {
        for z in 0..16 {
            let col = (x + 16 * z) as usize;
            let top = i32::from(c.heights[Hm::MotionBlocking as usize][col]) - 1;
            if top < c.min_y {
                continue;
            }
            let surface = i32::from(c.heights[Hm::WorldSurface as usize][col]) - 1;
            if surface <= top && surface > i32::from(c.heights[Hm::OceanFloor as usize][col]) - 1 {
                continue;
            }
            for y in (c.min_y..=top + 1).rev() {
                let st = if y >= top_y { 0 } else { c.blocks[c.index(x, y, z)] };
                let f = b.facts(st);
                if f.fluid != 0 {
                    break;
                }
                if f.coll_faces & (1 << Dir::Up as u8) != 0 {
                    return Some([c.cx * 16 + x, y + 1, c.cz * 16 + z]);
                }
            }
        }
    }
    None
}
