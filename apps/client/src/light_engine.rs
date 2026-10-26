//! Client light engine: that of `rc-light`, shared with the integrated server of singleplayer.

use hashbrown::HashMap;
use rc_world::{ChunkGrid, LightLayer};

pub use rc_light::{LightTables, Received, relight_masked, relight_sections};

/// Switches of mission C2-M05 (`docs/perf.md`), read once at the first call.
/// - `RUSTCRAFT_OPT_SOLONORELIGHT`: in singleplayer (LAN host included), a block change received from the
///   integrated server does not trigger the local relight when its section has just received its light
///   ([`Received`]). Predictions, acks and piston replay keep the relight; remote
///   servers unchanged. `=0`: local relight of every change (validation of the client engine).
/// - `RUSTCRAFT_OPT_SKYBIT` (shared with `rc-light` and the integrated server): local relight through
///   [`relight_masked`], sky resolved around the changes that affect it only.
/// - `RUSTCRAFT_OPT_LIGHTNOCOPY`: light or biome update identical to what the
///   column already holds: column left as is (no copy of the column shared with
///   the meshing tasks); identical shared sheet recognized without comparing its bytes.
#[derive(Clone, Copy, Debug)]
pub struct Opts {
    pub solo_cut: bool,
    pub sky_bit: bool,
    pub no_copy: bool,
}

pub fn opts() -> Opts {
    static O: std::sync::OnceLock<Opts> = std::sync::OnceLock::new();
    *O.get_or_init(|| Opts {
        solo_cut: rc_data::env_str!("RUSTCRAFT_OPT_SOLONORELIGHT").map(str::trim) != Some("0"),
        sky_bit: rc_data::env_str!("RUSTCRAFT_OPT_SKYBIT").map(str::trim) != Some("0"),
        no_copy: rc_data::env_str!("RUSTCRAFT_OPT_LIGHTNOCOPY").map(str::trim) != Some("0"),
    })
}

/// Check `RUSTCRAFT_SOLOLIGHTCHECK=1` (singleplayer): server light received per column (chunk then
/// updates), compared after the frame's local relight with what the client keeps, for the
/// columns touched in the frame. A difference that lasts beyond one frame (packets of one tick
/// shared between two frames) is a defect of the cut or of the client engine.
#[derive(Default)]
pub struct Shadow {
    cols: HashMap<(i32, i32), (Vec<LightLayer>, Vec<LightLayer>)>,
    touched: Vec<(i32, i32)>,
}

impl Shadow {
    pub fn on() -> bool {
        rc_data::env_flag!("RUSTCRAFT_SOLOLIGHTCHECK")
    }

    pub fn chunk(&mut self, x: i32, z: i32, sky: &[LightLayer], block: &[LightLayer]) {
        self.cols.insert((x, z), (sky.to_vec(), block.to_vec()));
    }

    pub fn update(&mut self, x: i32, z: i32, sky: &[LightLayer], block: &[LightLayer]) {
        if let Some(c) = self.cols.get_mut(&(x, z)) {
            for (dst, src) in [(&mut c.0, sky), (&mut c.1, block)] {
                for (d, s) in dst.iter_mut().zip(src) {
                    if s.is_some() {
                        d.clone_from(s);
                    }
                }
            }
        }
        self.touched.push((x, z));
    }

    pub fn forget(&mut self, x: i32, z: i32) {
        self.cols.remove(&(x, z));
    }

    pub fn touch(&mut self, x: i32, z: i32) {
        self.touched.push((x, z));
    }

    /// Light cells that differ, per column touched in the frame, then forgetting the
    /// touched columns.
    pub fn check(&mut self, grid: &ChunkGrid) -> Vec<((i32, i32), usize, usize)> {
        let mut t = std::mem::take(&mut self.touched);
        t.sort_unstable();
        t.dedup();
        let mut out = Vec::new();
        for k in t {
            let (Some(s), Some(c)) = (self.cols.get(&k), grid.get(k.0, k.1)) else { continue };
            let (sky, block) = (layer_diff(&c.sky_light, &s.0), layer_diff(&c.block_light, &s.1));
            if sky + block > 0 {
                out.push((k, sky, block));
            }
        }
        out
    }
}

/// Cells (nibbles) that differ between two sequences of sheets; sheet absent on one side only:
/// compared against a null sheet.
pub fn layer_diff(a: &[LightLayer], b: &[LightLayer]) -> usize {
    let zero = [0u8; rc_world::LIGHT_BYTES];
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let (x, y) = (x.as_deref().unwrap_or(&zero), y.as_deref().unwrap_or(&zero));
            if std::ptr::eq(x, y) || x == y {
                return 0;
            }
            x.iter().zip(y).map(|(p, q)| usize::from((p ^ q) & 15 != 0) + usize::from((p ^ q) >> 4 != 0)).sum()
        })
        .sum()
}
