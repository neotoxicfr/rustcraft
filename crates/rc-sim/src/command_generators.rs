//! Generators of the dimensions used outside the generation thread (C1-M03): structure and biome
//! searches, `/place`, bone meal and saplings, the eye of Ender, exploration maps, the jigsaw and
//! structure blocks. One per dimension and structure setting for the whole session, taken from the
//! dimension's generation thread when it built the same one; and the generator of a world just
//! created, handed from its spawn search to the server that opens the world.
//!
//! A generator only answers `&self` calls whose caches are pure (same answer whoever fills
//! them), so the generation thread and the server can share one. The one state its users see is
//! the height a scattered structure piece keeps once placed: shared, a search reads the height
//! the generation thread already measured, like the game (one structure start object per chunk,
//! read by the searches and modified by the placement).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rc_worldgen::chunkgen::{DimGen, GenSpec, WorldData};

use crate::server::Server;

#[cfg(test)]
thread_local! {
    /// Tests: switches of this file forced on the current thread (name, value).
    static FORCED: std::cell::RefCell<Vec<(&'static str, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests: forces the named switches of this file on the current thread (`None`: environment).
#[cfg(test)]
pub(crate) fn force(names: &[&'static str], value: Option<bool>) {
    FORCED.with(|f| {
        let mut f = f.borrow_mut();
        f.retain(|(n, _)| !names.contains(n));
        if let Some(v) = value {
            f.extend(names.iter().map(|n| (*n, v)));
        }
    });
}

#[cfg_attr(not(test), allow(unused_variables))]
fn forced_or(name: &'static str, from_env: bool) -> bool {
    #[cfg(test)]
    if let Some(v) = FORCED.with(|f| f.borrow().iter().find(|(n, _)| *n == name).map(|(_, v)| *v)) {
        return v;
    }
    from_env
}

/// `RUSTCRAFT_OPT_COMMAND_WORLD_DATA_ONLY` (default 1; `0`: old path): the template and structure
/// blocks take the generation data alone instead of building a whole generator to get them.
pub(crate) fn world_data_only() -> bool {
    forced_or("COMMAND_WORLD_DATA_ONLY", rc_data::opt_switch!("RUSTCRAFT_OPT_COMMAND_WORLD_DATA_ONLY"))
}

/// `RUSTCRAFT_OPT_ONE_COMMAND_GENERATOR` (default 1; `0`: old path, one generator for the searches,
/// one for `/place` and one more with structures, for the current dimension only): one generator
/// per dimension and structure setting, kept for the session.
pub(crate) fn one_command_generator() -> bool {
    forced_or("ONE_COMMAND_GENERATOR", rc_data::opt_switch!("RUSTCRAFT_OPT_ONE_COMMAND_GENERATOR"))
}

/// `RUSTCRAFT_OPT_SHARE_WORKER_GENERATOR` (default 1; `0`: the server builds its own): the commands
/// use the generator of the dimension's generation thread when it has the same structure setting.
pub(crate) fn share_worker_generator() -> bool {
    forced_or("SHARE_WORKER_GENERATOR", rc_data::opt_switch!("RUSTCRAFT_OPT_SHARE_WORKER_GENERATOR"))
}

/// `RUSTCRAFT_OPT_KEEP_NEW_WORLD_DATA` (default 1; `0`: released, then loaded again by the server):
/// the generation data of a world just created stay in memory until its server takes them, and
/// the creation reads its preset from them instead of indexing the jar's files again.
pub(crate) fn keep_new_world_data() -> bool {
    forced_or("KEEP_NEW_WORLD_DATA", rc_data::opt_switch!("RUSTCRAFT_OPT_KEEP_NEW_WORLD_DATA"))
}

/// `RUSTCRAFT_OPT_REUSE_SPAWN_GENERATOR` (default 1; `0`: built again): the generator of the spawn
/// search of a new world becomes the main world generator of its server.
pub(crate) fn reuse_spawn_generator() -> bool {
    forced_or("REUSE_SPAWN_GENERATOR", rc_data::opt_switch!("RUSTCRAFT_OPT_REUSE_SPAWN_GENERATOR"))
}

/// `RUSTCRAFT_OPT_EARLY_GENERATOR_START` (default 1; `0`: after the registries, tables and survival
/// data): the generation thread of the player's dimension starts as soon as the world's files are
/// read, its generator building while the server loads the rest.
pub(crate) fn early_generator_start() -> bool {
    forced_or("EARLY_GENERATOR_START", rc_data::opt_switch!("RUSTCRAFT_OPT_EARLY_GENERATOR_START"))
}

/// A dimension generator and the structure setting it was built with.
#[derive(Clone)]
pub(crate) struct BuiltGenerator {
    pub dg: Arc<DimGen>,
    /// Built with the structures enabled (world setting, or forced for `/place structure`).
    pub structures: bool,
}

/// Generators built by the server for its commands, and the generation data they come from.
#[derive(Default)]
pub(crate) struct CommandGenerators {
    data: Option<Arc<WorldData>>,
    /// Dimension, generator.
    built: Vec<(String, BuiltGenerator)>,
}

impl CommandGenerators {
    fn find(&self, dimension: &str, structures: bool) -> Option<Arc<DimGen>> {
        self.built.iter().find(|(d, b)| d == dimension && b.structures == structures).map(|(_, b)| b.dg.clone())
    }
}

impl Server {
    /// Generation data of the game (jar and reports), without building any generator: what the
    /// template and structure blocks need.
    pub(crate) fn world_data(&mut self) -> Option<Arc<WorldData>> {
        if let Some(wd) = &self.command_gens.data {
            return Some(wd.clone());
        }
        let jar = crate::registries::client_jar(&self.data)?;
        let reports = crate::generation::reports_dir(&self.data);
        let wd = WorldData::shared(&jar, reports.is_dir().then_some(reports.as_path()), &self.data).ok()?;
        self.command_gens.data = Some(wd.clone());
        Some(wd)
    }

    /// Generation data for the template and structure blocks (`RUSTCRAFT_OPT_COMMAND_WORLD_DATA_ONLY`).
    pub(crate) fn template_data(&mut self) -> Option<Arc<WorldData>> {
        if world_data_only() { self.world_data() } else { self.place_data().map(|(wd, _)| wd) }
    }

    /// `generate_structures` setting of the world, as read at opening.
    pub(crate) fn world_has_structures(&self) -> bool {
        self.wgs.data().and_then(|d| d.get("generate_structures")?.as_i64()).is_some_and(|v| v != 0)
    }

    /// Generator of the open dimension for the commands, built with the structures or not: the
    /// generation thread's one when it matches, otherwise the one the server built, otherwise a
    /// new one kept for the session.
    pub(crate) fn command_generator(&mut self, structures: bool) -> Option<Arc<DimGen>> {
        let dimension = self.dim.name.clone();
        if share_worker_generator()
            && let Some(dg) = self.generator.as_ref().and_then(|g| g.thread_generator(&dimension, structures))
        {
            return Some(dg);
        }
        if let Some(dg) = self.command_gens.find(&dimension, structures) {
            return Some(dg);
        }
        let dg = Arc::new(self.build_command_generator(structures)?);
        self.command_gens.built.push((dimension, BuiltGenerator { dg: dg.clone(), structures }));
        Some(dg)
    }

    /// New generator of the open dimension, as the world settings describe it.
    pub(crate) fn build_command_generator(&mut self, structures: bool) -> Option<DimGen> {
        let (spec, seed) = crate::generation::read_spec(&self.world, &self.dim.name)?;
        let wd = self.world_data()?;
        let mut dg = DimGen::new(&wd, &spec, &self.dim.name, seed).ok()?;
        if structures {
            dg.enable_structures(&wd).ok()?;
        }
        Some(dg)
    }
}

/// What a world just created leaves for the server that opens it.
struct NewWorldHandOver {
    world: PathBuf,
    dimension: String,
    spec: GenSpec,
    seed: i64,
    data: Arc<WorldData>,
    /// `None`: generation data only (debug world, or `RUSTCRAFT_OPT_REUSE_SPAWN_GENERATOR=0`).
    generator: Option<BuiltGenerator>,
}

/// Worlds just created and not opened yet, the latest last. The client opens a world right after
/// creating it, so two are plenty there; the tests create dozens side by side.
static NEW_WORLDS: Mutex<Vec<NewWorldHandOver>> = Mutex::new(Vec::new());
const NEW_WORLDS_KEPT: usize = if cfg!(test) { 64 } else { 2 };

fn world_key(world: &Path) -> PathBuf {
    std::fs::canonicalize(world).unwrap_or_else(|_| world.to_path_buf())
}

fn new_worlds() -> std::sync::MutexGuard<'static, Vec<NewWorldHandOver>> {
    NEW_WORLDS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keeps the generation data (and the generator, if given) of a world just created until its
/// server opens it; the oldest world left unopened is released past a few.
pub(crate) fn keep_new_world(
    world: &Path,
    dimension: &str,
    spec: GenSpec,
    seed: i64,
    data: Arc<WorldData>,
    generator: Option<BuiltGenerator>,
) {
    let world = world_key(world);
    let mut kept = new_worlds();
    kept.retain(|k| k.world != world);
    if kept.len() >= NEW_WORLDS_KEPT {
        kept.remove(0);
    }
    kept.push(NewWorldHandOver { world, dimension: dimension.to_owned(), spec, seed, data, generator });
}

/// The generation data and generator left by the creation of this world for this dimension, if
/// any (released from the list either way once the world is opened).
pub(crate) fn take_new_world(
    world: &Path,
    dimension: &str,
    spec: &GenSpec,
    seed: i64,
) -> Option<(Arc<WorldData>, Option<BuiltGenerator>)> {
    let world = world_key(world);
    let mut kept = new_worlds();
    let i = kept.iter().position(|k| k.world == world)?;
    let k = kept.remove(i);
    (k.dimension == dimension && k.seed == seed && k.spec == *spec).then_some((k.data, k.generator))
}

/// Tests: what is kept for this world (`Some(true)`: with its generator).
#[cfg(test)]
pub(crate) fn new_world_kept(world: &Path) -> Option<bool> {
    let world = world_key(world);
    new_worlds().iter().find(|k| k.world == world).map(|k| k.generator.is_some())
}

/// Tests: generator kept for this world.
#[cfg(test)]
pub(crate) fn kept_new_world_generator(world: &Path) -> Option<Arc<DimGen>> {
    let world = world_key(world);
    new_worlds().iter().find(|k| k.world == world)?.generator.as_ref().map(|g| g.dg.clone())
}

/// Generator a generation thread hands to the server, once built (`None`: it failed or ended first).
pub(crate) type ThreadGenerator = Arc<OnceLock<Option<BuiltGenerator>>>;

/// Releases the servers waiting for a generation thread's generator, whatever ends the thread.
pub(crate) struct ReleaseOnExit(pub ThreadGenerator);

impl Drop for ReleaseOnExit {
    fn drop(&mut self) {
        let _ = self.0.set(None);
    }
}

#[cfg(test)]
#[path = "command_generators_tests.rs"]
mod tests;
