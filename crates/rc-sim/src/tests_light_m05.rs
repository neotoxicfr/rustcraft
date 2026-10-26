//! Proofs of mission C2-M05 (singleplayer client: local relight cut when the
//! integrated server has just sent the light of the section, `RUSTCRAFT_OPT_SOLONORELIGHT`).
//!
//! A real integrated server plays a setup (slabs that cover the sky, glowstone, redstone lamp
//! lit by a block then by a torch, water and lava flows, removals,
//! emitter placed then removed in the same tick). Its packets feed several mirror clients
//! that follow the client's path (`apps/client/src/main.rs`: chunk, light update,
//! block changes, then one local relight per frame with `rc_light::relight_sections` or
//! `rc_light::relight_masked`):
//! - old: local relight of every change that touches light;
//! - cut: relight skipped when the section received its light in the frame ([`rc_light::Received`]);
//! - cut + sky bit (`relight_masked`);
//! - the same with the packets of a tick split between two frames;
//! - sabotage: relight cut AND light updates ignored.
//!
//! At each tick, the layers of the non-sabotaged clients are those of the server (bit for bit); the
//! sabotage must deviate from them; the cut must spare relights.

use std::sync::Arc;

use rc_client::Event;
use rc_testdata::need;
use rc_world::{Column, LightLayer};

use crate::server::Server;
use crate::tests_opt::{SEED, Scratch, cmd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Old,
    Cut,
    CutMasked,
    /// Sabotage: nothing relit, received light ignored.
    Blind,
    /// Client engine alone (received light ignored, everything relit): `relight_sections`, then
    /// `relight_masked` (proof of the sky bit on the client side: same layers as `Local`).
    Local,
    LocalMasked,
}

impl Mode {
    fn takes_light(self) -> bool {
        matches!(self, Mode::Old | Mode::Cut | Mode::CutMasked)
    }
    fn masked(self) -> bool {
        matches!(self, Mode::CutMasked | Mode::LocalMasked)
    }
}

std::thread_local! {
    /// Sky bit sabotage: list of sky changes emptied.
    static SKY_SABOTAGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn sky_list(sky: &[[i32; 3]]) -> &[[i32; 3]] {
    if SKY_SABOTAGE.with(std::cell::Cell::get) { &[] } else { sky }
}

struct Mirror {
    mode: Mode,
    split: bool,
    grid: rc_world::ChunkGrid,
    relight: Vec<[i32; 3]>,
    sky: Vec<[i32; 3]>,
    recv: rc_light::Received,
    /// Positions passed to the local relight, relights launched.
    relit: usize,
    runs: usize,
    /// Time spent in the local relight (micro-bench, loaded machine).
    relight_ns: u128,
    /// First deviation from the server, light cells that differ over the whole scenario.
    fault: Option<String>,
    diff: usize,
    /// Fingerprint of the layers of the columns received after each tick.
    digests: Vec<u64>,
}

struct Ctx<'a> {
    light: &'a rc_light::LightTables,
    min_y: i32,
    height: i32,
    has_sky: bool,
    bits: u8,
    air: &'a dyn Fn(u32) -> bool,
}

impl Mirror {
    fn new(mode: Mode, split: bool) -> Self {
        Mirror {
            mode,
            split,
            grid: rc_world::ChunkGrid::new(8),
            relight: Vec::new(),
            sky: Vec::new(),
            recv: rc_light::Received::default(),
            relit: 0,
            runs: 0,
            relight_ns: 0,
            fault: None,
            diff: 0,
            digests: Vec::new(),
        }
    }

    fn name(&self) -> String {
        format!("{:?}{}", self.mode, if self.split { " (tick over two frames)" } else { "" })
    }

    fn event(&mut self, e: &Event, c: &Ctx) {
        match e {
            Event::Chunk(p) => {
                self.grid.insert(Column {
                    x: p.x,
                    z: p.z,
                    sections: p.sections.clone().into_boxed_slice(),
                    sky_light: p.light.sky.clone().into_boxed_slice(),
                    block_light: p.light.block.clone().into_boxed_slice(),
                    heightmaps: p.heightmaps.clone(),
                });
            }
            Event::LightUpdate { x, z, light } if self.mode.takes_light() => {
                if self.mode != Mode::Old {
                    self.recv.note(*x, *z, &light.sky, &light.block);
                }
                if let Some(col) = self.grid.get_mut(*x, *z) {
                    for (dst, src) in [(&mut col.sky_light, &light.sky), (&mut col.block_light, &light.block)] {
                        for (d, s) in dst.iter_mut().zip(src) {
                            if s.is_some() {
                                d.clone_from(s);
                            }
                        }
                    }
                }
            }
            Event::BlockUpdate { pos, state } => {
                let sy = (pos.y - c.min_y) >> 4;
                let idx = rc_world::block_index((pos.x & 15) as usize, (pos.y & 15) as usize, (pos.z & 15) as usize);
                self.blocks(pos.x >> 4, pos.z >> 4, sy, &[(idx as u16, *state)], c);
            }
            Event::SectionUpdate { section, changes } => {
                let sy = section[1] - (c.min_y >> 4);
                self.blocks(section[0], section[2], sy, changes, c);
            }
            _ => {}
        }
    }

    fn blocks(&mut self, cx: i32, cz: i32, sy: i32, changes: &[(u16, u32)], c: &Ctx) {
        let relight = match self.mode {
            Mode::Old | Mode::Local | Mode::LocalMasked => true,
            Mode::Cut | Mode::CutMasked => !self.recv.covers(cx, sy, cz),
            Mode::Blind => false,
        };
        let Some(col) = self.grid.get_mut(cx, cz) else { return };
        let Some(sec) = usize::try_from(sy).ok().and_then(|i| col.sections.get_mut(i)) else { return };
        for &(i, state) in changes.iter().filter(|&&(i, _)| relight && usize::from(i) < rc_world::SECTION_BLOCKS) {
            let old = sec.blocks.get(usize::from(i));
            if c.light.matters(old, state) {
                let i = i32::from(i);
                let p = [cx * 16 + (i & 15), sy * 16 + (i >> 8), cz * 16 + ((i >> 4) & 15)];
                self.relight.push(p);
                if c.light.matters_sky(old, state) {
                    self.sky.push(p);
                }
            }
        }
        sec.set_blocks(changes, c.bits, |s| (c.air)(s));
    }

    /// End of frame: local relight, received sections forgotten.
    fn frame(&mut self, c: &Ctx) {
        if !self.relight.is_empty() {
            let changes = std::mem::take(&mut self.relight);
            let sky = std::mem::take(&mut self.sky);
            self.relit += changes.len();
            self.runs += 1;
            let t = std::time::Instant::now();
            if self.mode.masked() {
                rc_light::relight_masked(&mut self.grid, c.height, c.light, c.has_sky, &changes, sky_list(&sky));
            } else {
                rc_light::relight_sections(&mut self.grid, c.height, c.light, c.has_sky, &changes);
            }
            self.relight_ns += t.elapsed().as_nanos();
        }
        self.recv.clear();
    }

    /// Layers of each received column compared with those of the server.
    fn compare(&mut self, s: &Server, tick: u64) {
        use std::hash::{Hash, Hasher};
        let mut keys: Vec<(i32, i32)> = s.sent.iter().copied().collect();
        keys.sort_unstable();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for &(x, z) in &keys {
            let Some(c) = self.grid.get(x, z) else { continue };
            (x, z).hash(&mut h);
            for l in c.sky_light.iter().chain(c.block_light.iter()) {
                l.as_deref().map(|l| &l[..]).hash(&mut h);
            }
        }
        self.digests.push(h.finish());
        for &(x, z) in &keys {
            let (Some(mine), Some(Some(theirs))) = (self.grid.get(x, z), s.chunks.get(&(x, z))) else { continue };
            for (kind, a, b) in [("sky", &mine.sky_light, &theirs.sky), ("block", &mine.block_light, &theirs.block)] {
                for (i, (p, q)) in a.iter().zip(b.iter()).enumerate() {
                    let n = layer_cells(p, q);
                    if n > 0 {
                        self.diff += n;
                        self.fault.get_or_insert_with(|| format!("tick {tick}: ({x}, {z}) {kind} {i}: {n} cells"));
                    }
                }
            }
        }
    }
}

/// Cells that differ between two layers; different presence: all.
fn layer_cells(a: &LightLayer, b: &LightLayer) -> usize {
    match (a, b) {
        (None, None) => 0,
        (Some(a), Some(b)) if Arc::ptr_eq(a, b) => 0,
        (Some(a), Some(b)) => {
            a.iter().zip(b.iter()).map(|(p, q)| usize::from((p ^ q) & 15 != 0) + usize::from((p ^ q) >> 4 != 0)).sum()
        }
        _ => rc_world::LIGHT_BYTES * 2,
    }
}

/// The scenario in a new thread (reproducible server randoms, see `tests_light_m04`).
fn run(sky_sabotage: bool) -> Option<Vec<Mirror>> {
    std::thread::spawn(move || {
        SKY_SABOTAGE.with(|c| c.set(sky_sabotage));
        run_here()
    })
    .join()
    .unwrap_or_else(|p| std::panic::resume_unwind(p))
}

fn run_here() -> Option<Vec<Mirror>> {
    let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
    need(crate::registries::client_jar(&data), "26.3 client jar")?;
    let dir = std::env::temp_dir().join(format!("rc-sim-m05-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _guard = Scratch(dir.clone());
    let mut w = crate::newworld::NewWorld::new("m05");
    w.seed = SEED;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let (mut s, session) = crate::portal_tests::open_world_session(&dir, data.clone());
    // The scenario tests the client against the server's current sending (C2-M04), whatever the
    // process switches (all-0 suite: LIGHTSECTIONS=0 sends the layers differently).
    s.light_opts = crate::play::LightOpts::NEW;
    for r in ["minecraft:spawn_mobs", "minecraft:advance_time", "minecraft:advance_weather"] {
        crate::nbtio::set(&mut s.rules, r, rc_proto::nbt::Nbt::Byte(0));
    }
    s.enter_world(crate::dims::OVERWORLD);
    s.pos = [16.5, 201.0, 16.5];
    let list: Vec<(i32, i32)> = (-3..=4).flat_map(|x| (-3..=4).map(move |z| (x, z))).collect();
    for p in &list {
        s.load_now(std::slice::from_ref(p));
    }
    s.loaded = true;
    s.teleport = None;
    s.queue = (-1..=3).flat_map(|x| (-1..=3).map(move |z| (x, z))).collect();
    s.send_chunks();
    let mut mirrors: Vec<Mirror> = [
        (Mode::Old, false),
        (Mode::Cut, false),
        (Mode::CutMasked, false),
        (Mode::Old, true),
        (Mode::Cut, true),
        (Mode::CutMasked, true),
        (Mode::Blind, false),
        (Mode::Local, false),
        (Mode::LocalMasked, false),
    ]
    .into_iter()
    .map(|(m, sp)| Mirror::new(m, sp))
    .collect();
    // Tables built like the client (`apps/client/src/main.rs`).
    let light = rc_light::LightTables::new(&data);
    let states = s.states.clone();
    let air = move |st: u32| states.is_air(st);
    let ctx = Ctx {
        light: &light,
        min_y: s.dim.height.min_y,
        height: s.dim.height.sections as i32 * 16,
        has_sky: s.dim_kind().sky,
        bits: data.state_bits() as u8,
        air: &air,
    };
    let steps: Vec<(Vec<&str>, usize)> = vec![
        (vec![], 10),
        (vec!["fill 0 199 0 31 199 31 minecraft:stone"], 5),
        (vec!["setblock 15 207 15 minecraft:glowstone"], 3),
        (vec!["setblock 16 200 16 minecraft:redstone_lamp", "setblock 17 200 16 minecraft:redstone_block"], 4),
        (vec!["setblock 17 200 16 minecraft:air"], 8),
        (vec!["fill 2 203 2 12 203 12 minecraft:stone"], 3),
        (vec!["setblock 5 204 5 minecraft:water"], 30),
        (vec!["setblock 24 200 24 minecraft:lava"], 70),
        (vec!["setblock 28 201 4 minecraft:redstone_lamp", "setblock 28 200 4 minecraft:redstone_torch"], 6),
        (vec!["setblock 28 200 4 minecraft:air"], 8),
        (vec!["setblock 15 207 15 minecraft:air"], 3),
        (vec!["fill 2 203 2 12 203 12 minecraft:air"], 30),
        (vec!["setblock 8 220 8 minecraft:sea_lantern", "setblock 8 220 8 minecraft:air"], 2),
        (vec!["setblock 24 200 24 minecraft:air"], 40),
        (vec!["setblock 31 215 0 minecraft:glowstone"], 3),
    ];
    // Chunks sent before the first tick.
    let first: Vec<Event> = session.events.try_iter().collect();
    for m in &mut mirrors {
        for e in &first {
            m.event(e, &ctx);
        }
        m.frame(&ctx);
    }
    for (cmds, n) in steps {
        for c in cmds {
            cmd(&mut s, c);
        }
        for _ in 0..n {
            s.game_tick();
            s.tick += 1;
            let events: Vec<Event> = session.events.try_iter().collect();
            for m in &mut mirrors {
                // Frame cut in the middle of the tick's packets (often between the light and
                // the blocks): pre-existing splitting, the rule must hold.
                let cut = if m.split { events.len() / 2 } else { events.len() };
                for e in &events[..cut] {
                    m.event(e, &ctx);
                }
                if m.split {
                    m.frame(&ctx);
                }
                for e in &events[cut..] {
                    m.event(e, &ctx);
                }
                m.frame(&ctx);
                m.compare(&s, s.tick);
            }
        }
    }
    s.stop_gen();
    s.stop_parked();
    drop(s);
    Some(mirrors)
}

#[test]
fn solo_cut_relight_equivalent() {
    let Some(mirrors) = run(false) else { return };
    for m in &mirrors {
        eprintln!(
            "C2-M05 {}: {} positions relit in {} relights ({:.2} ms), {} cells different from the server, {:?}",
            m.name(),
            m.relit,
            m.runs,
            m.relight_ns as f64 / 1e6,
            m.diff,
            m.fault
        );
    }
    let get = |mode: Mode, split: bool| mirrors.iter().find(|m| m.mode == mode && m.split == split).unwrap();
    let old = get(Mode::Old, false);
    assert!(old.relit > 100, "setup without local relight: {}", old.relit);
    for m in mirrors.iter().filter(|m| m.mode.takes_light()) {
        assert_eq!(m.fault, None, "{}: layers different from the server", m.name());
    }
    let cut = get(Mode::Cut, false);
    assert!(cut.relit * 2 < old.relit, "cut without effect: {} against {}", cut.relit, old.relit);
    assert!(get(Mode::Blind, false).fault.is_some(), "sabotage (received light ignored) not seen");
    let (local, masked) = (get(Mode::Local, false), get(Mode::LocalMasked, false));
    assert!(local.digests.len() > 100);
    assert_eq!(local.digests, masked.digests, "sky bit: layers of the client alone differ");
}

/// Sky bit sabotage (sky changes forgotten): the client alone must deviate. With
/// `RUSTCRAFT_OPT_SKYBIT=0` (read once by rc-light), `relight_masked` ignores the sky list:
/// the sabotage is then without effect, and the two layers must stay equal.
#[test]
fn solo_sky_bit_sabotage_seen() {
    let Some(mirrors) = run(true) else { return };
    let get = |mode: Mode| mirrors.iter().find(|m| m.mode == mode && !m.split).unwrap();
    if rc_data::env_str!("RUSTCRAFT_OPT_SKYBIT").map(str::trim) == Some("0") {
        assert_eq!(get(Mode::Local).digests, get(Mode::LocalMasked).digests, "SKYBIT=0: sky list read anyway");
    } else {
        assert_ne!(get(Mode::Local).digests, get(Mode::LocalMasked).digests, "sky bit sabotage not seen");
    }
}
