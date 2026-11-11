//! Sections the fog hides whole are left out of the terrain draw lists (an idea read in the
//! optimisation mods): every fragment of such a section is the plain fog colour. The Nether (fog
//! ending at 96 blocks whatever the render distance), water, lava, powder snow, blindness,
//! darkness and boss fog gain the most.
//!
//! The frame clears to the fog colour, so leaving such a section out changes no pixel when nothing
//! but that background can show behind it: no sky dome, no End sky, no cloud still visible that far,
//! no world border wall (the one world shader without fog). Everything else drawn behind it lies
//! farther along the same sight line, so it is plain fog colour too. Under a drawn sky the section
//! stands as a fog-coloured patch in front of sky that is not (dome, sun, moon, stars): the two
//! paths that cull there are off until a screenshot comparison of the scene bench settles them.
//!
//! The visibility walk, the entities and the meshing still see every reached section: only the
//! terrain draw lists read the trimmed list ([`DrawnSections`]), remade when the walk, the fog
//! reach or the 8-block cell of the eye changes. A section goes only if it is in plain fog for
//! every eye of that cell.
//!
//! Switches, read once ([`FogCullPaths`]):
//! - `RUSTCRAFT_OPT_FOG_CULL` (`=0`: every reached section drawn, as before): the trim above,
//!   where only the fog-coloured background can show behind;
//! - `RUSTCRAFT_OPT_FOG_CULL_BELOW_SKY_RIM` (off, `=1` turns it on): under a drawn sky, the hidden
//!   sections that stay under the slope where the sky dome is plain fog colour itself (sun, moon,
//!   stars, sunrise glow and clouds low on the horizon can still show through);
//! - `RUSTCRAFT_OPT_FOG_CULL_AGAINST_SKY` (off, `=1` turns it on): under a drawn sky, every hidden
//!   section (the fog-coloured outline of far relief against the sky goes away);
//! - `RUSTCRAFT_OPT_FOG_CULL_CUTS_WALK` (off, `=1` turns it on): where one of the paths above
//!   culls, the walk itself stops at the sphere that holds everything the fog shows. The terrain
//!   drawn is the same, the walk is shorter, but the entities of the sections it no longer enters
//!   are not drawn (a glowing outline or a see-through label would show through the fog) and
//!   their dirty sections wait to be remeshed.

use super::FrameEnv;
use super::frustum::ReachedSection;

/// Blocks a section must lie beyond the fog's end to go: model parts that overflow their section
/// (two blocks on each axis, as the back-face test assumes: 3.5 along the diagonal) and the float
/// rounding of the distances the shader interpolates.
const SAFETY_BLOCKS: f64 = 4.0;
/// Farthest the eye gets from where a walk saw it before the walk is redone (the diagonal of
/// the 8-block cell that keys the walk), plus its travel while the next walk runs.
const WALK_EYE_TRAVEL: f64 = 13.856_406_460_551_018 + 4.0;
/// Smallest reach, and the first step of the ladder ([`ladder_step`]).
const SHORTEST_REACH: f64 = 16.0;
/// A fog ending farther than this hides nothing the walk can reach (64 columns at most).
const LONGEST_REACH: f64 = 2048.0;
/// The sky dome: a flat fan this high above the eye, its rim this far, whose fog distance grows
/// from the height at the centre to the radius at the rim (`terrain.wgsl`, `vs_sky`).
const DOME_HEIGHT: f64 = 16.0;
const DOME_RADIUS: f64 = 512.0;

/// Which fog culls run; every field off is the frame of before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FogCullPaths {
    /// `RUSTCRAFT_OPT_FOG_CULL`: hidden sections not drawn where only the background shows behind.
    pub over_plain_background: bool,
    /// `RUSTCRAFT_OPT_FOG_CULL_BELOW_SKY_RIM`: under a drawn sky, below the dome's fog-coloured rim.
    pub below_sky_rim: bool,
    /// `RUSTCRAFT_OPT_FOG_CULL_AGAINST_SKY`: under a drawn sky, everywhere.
    pub against_sky: bool,
    /// `RUSTCRAFT_OPT_FOG_CULL_CUTS_WALK`: the walk itself stops at the fog's sphere.
    pub cuts_walk: bool,
}

impl FogCullPaths {
    pub fn from_switches() -> Self {
        FogCullPaths {
            over_plain_background: rc_data::opt_switch!("RUSTCRAFT_OPT_FOG_CULL"),
            below_sky_rim: rc_data::opt_switch!("RUSTCRAFT_OPT_FOG_CULL_BELOW_SKY_RIM", default_off),
            against_sky: rc_data::opt_switch!("RUSTCRAFT_OPT_FOG_CULL_AGAINST_SKY", default_off),
            cuts_walk: rc_data::opt_switch!("RUSTCRAFT_OPT_FOG_CULL_CUTS_WALK", default_off),
        }
    }
}

/// How far the fog of a frame lets anything show, in whole steps, and which hidden sections may
/// go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FogReach {
    /// Environment fog: a point this many blocks from the eye, or farther, is plain fog colour.
    sphere: f64,
    /// Render-distance fog: the same for a point this far sideways, or this far above or below.
    cylinder: f64,
    /// Steepest slope from the eye (rise over run) under which a hidden section may go:
    /// infinite when only the background shows behind any of them, else the slope under which
    /// the sky dome is plain fog colour.
    steepest_slope: f64,
}

impl FogReach {
    /// Reach of the frame `env`; `None` when every reached section is drawn (cull off, or
    /// something other than the fog colour could show behind a hidden section).
    pub fn of_frame(env: &FrameEnv, border_wall_drawn: bool, paths: FogCullPaths) -> Option<FogReach> {
        let fog = &env.fog;
        // The shaders blend towards the fog colour by its alpha: under 1 the fog never hides whole.
        if fog.color[3] < 1.0 {
            return None;
        }
        // The shader's fog is 0 up to its start, even when its end comes first.
        let sphere_end = fog.env_start.max(fog.env_end);
        let cylinder_end = fog.render_start.max(fog.render_end);
        let steepest_slope = steepest_slope(env, border_wall_drawn, sphere_end.min(cylinder_end), paths)?;
        Some(FogReach { sphere: ladder_step(sphere_end), cylinder: whole_sections(cylinder_end), steepest_slope })
    }

    /// Section `pos` (column x, height in sections from `min_y`, column z) may go: for every eye
    /// in the 8-block cell `cell` (block coordinates divided by 8, rounded down), each of its
    /// points, model overflow included, is plain fog colour and under the steepest slope.
    pub fn hides(&self, cell: [i64; 3], min_y: i32, pos: ReachedSection) -> bool {
        let gaps = gaps_to_cell(cell, min_y, pos);
        let sideways_squared = gaps[0] * gaps[0] + gaps[2] * gaps[2];
        self.in_plain_fog(gaps) && self.under_steepest_slope(cell, min_y, pos, sideways_squared)
    }

    /// A box at the distances `[x, y, z]` from the cell of the eye, on each axis, is in plain
    /// fog, model overflow included.
    fn in_plain_fog(&self, [x, y, z]: [f64; 3]) -> bool {
        let sideways_squared = x * x + z * z;
        let (sphere, cylinder) = (self.sphere + SAFETY_BLOCKS, self.cylinder + SAFETY_BLOCKS);
        sideways_squared + y * y >= sphere * sphere || sideways_squared >= cylinder * cylinder || y >= cylinder
    }

    /// Every section of column `column` (x, z) may go, whatever its height: the column is out of
    /// the nearer fog's reach sideways alone, and no slope spares its high sections.
    fn hides_column(&self, cell: [i64; 3], column: (i32, i32)) -> bool {
        let [x, _, z] = gaps_to_cell(cell, 0, (column.0, 0, column.1));
        let reach = self.sphere.min(self.cylinder) + SAFETY_BLOCKS;
        self.steepest_slope.is_infinite() && x * x + z * z >= reach * reach
    }

    /// The highest point of the section, seen from the lowest eye of the cell at the shortest
    /// run, stays under the steepest slope.
    fn under_steepest_slope(&self, cell: [i64; 3], min_y: i32, pos: ReachedSection, sideways_squared: f64) -> bool {
        if self.steepest_slope.is_infinite() {
            return true;
        }
        let rise = f64::from(min_y + (pos.1 + 1) * 16) - (cell[1] * 8) as f64 + SAFETY_BLOCKS;
        let run = sideways_squared.sqrt() - SAFETY_BLOCKS;
        rise <= 0.0 || (run > 0.0 && rise <= self.steepest_slope * run)
    }

    /// Distance from the eye of a walk from which the walk may skip every section
    /// (`RUSTCRAFT_OPT_FOG_CULL_CUTS_WALK`): they are hidden for as long as that walk serves.
    /// Infinite when a slope spares some far sections.
    pub fn walk_sight_limit(&self) -> f64 {
        if self.steepest_slope.is_finite() {
            return f64::INFINITY;
        }
        let (sphere, cylinder) = (self.sphere + SAFETY_BLOCKS, self.cylinder + SAFETY_BLOCKS);
        // √2 times the cylinder's reach away, a point is past it sideways or in height; 1.5 keeps
        // clear of the rounding of the squares.
        sphere.min(cylinder * 1.5) + WALK_EYE_TRAVEL
    }
}

/// Distance in blocks, on each axis (x, y, z), between the 8-block cell `cell` and the box of
/// section `pos`: no eye of the cell is closer to the section on that axis.
fn gaps_to_cell(cell: [i64; 3], min_y: i32, pos: ReachedSection) -> [f64; 3] {
    let section_low = [pos.0 * 16, min_y + pos.1 * 16, pos.2 * 16].map(i64::from);
    std::array::from_fn(|axis| {
        let cell_low = cell[axis] * 8;
        (section_low[axis] - (cell_low + 8)).max(cell_low - (section_low[axis] + 16)).max(0) as f64
    })
}

/// The 8-block cell of the eye, the one that keys the visibility walk.
pub(super) fn cell_of(eye: [f64; 3]) -> [i64; 3] {
    eye.map(|v| (v / 8.0).floor() as i64)
}

/// `blocks` rounded up to a step of the ladder 16, 24, 32, 48, 64, 96… (each power of two, then
/// its half more): the end of a fog that creeps or sweeps (vision under water, blindness wearing
/// off) crosses a handful of steps, so the list is remade a handful of times, while the usual
/// fixed ends (96 in the Nether) are steps themselves. Infinite beyond the longest useful reach,
/// and for a NaN.
fn ladder_step(blocks: f32) -> f64 {
    let blocks = f64::from(blocks);
    if blocks.is_nan() || blocks > LONGEST_REACH {
        return f64::INFINITY;
    }
    let mut step = SHORTEST_REACH;
    loop {
        if step >= blocks {
            return step;
        }
        if step * 1.5 >= blocks {
            return step * 1.5;
        }
        step *= 2.0;
    }
}

/// `blocks` rounded up to a whole section: the render distance only changes with the settings,
/// and is a number of sections already. Infinite like [`ladder_step`].
fn whole_sections(blocks: f32) -> f64 {
    let blocks = f64::from(blocks);
    if blocks.is_nan() || blocks > LONGEST_REACH {
        return f64::INFINITY;
    }
    ((blocks / 16.0).ceil() * 16.0).max(SHORTEST_REACH)
}

/// Steepest slope of the sections that may go this frame ([`FogReach::steepest_slope`]), `None`
/// when none may. `fog_end`: the nearer end of the two fogs, no hidden section is closer.
fn steepest_slope(env: &FrameEnv, border_wall_drawn: bool, fog_end: f32, paths: FogCullPaths) -> Option<f64> {
    let sky_drawn = env.dome || env.bodies.end.is_some();
    // Clouds fade out at the end of a fog of their own (a NaN end counts as still showing).
    let clouds_gone_by_then = env.bodies.clouds.is_none_or(|clouds| clouds.cloud_end <= fog_end);
    if !sky_drawn && clouds_gone_by_then && !border_wall_drawn {
        return paths.over_plain_background.then_some(f64::INFINITY);
    }
    if paths.against_sky {
        return Some(f64::INFINITY);
    }
    // The dark disc under the horizon and the End sky are not fog-coloured anywhere a section hides.
    let dome_alone = env.dome && !env.disc && env.bodies.end.is_none();
    (paths.below_sky_rim && dome_alone).then(|| sky_rim_slope(env.fog.sky_end))
}

/// Slope from the eye under which the sky dome is plain fog colour, or absent, when the sky fog
/// ends at `sky_end` (taken at its ladder step, so the slope moves in steps too). A point of the
/// fan at the share `s` of the way from the centre to the rim is at most `512 s` blocks sideways
/// and its fog distance is `16 + 496 s`: plain fog from `s = (sky_end − 16) / 496` on.
fn sky_rim_slope(sky_end: f32) -> f64 {
    let end = ladder_step(sky_end).min(DOME_RADIUS);
    if end <= DOME_HEIGHT {
        return f64::INFINITY;
    }
    let fogged_from = (end - DOME_HEIGHT) / (DOME_RADIUS - DOME_HEIGHT);
    DOME_HEIGHT / (DOME_RADIUS * fogged_from)
}

/// The reached sections the terrain draw lists read: all of them, or, under a fog that hides
/// some, those it leaves in sight.
#[derive(Default)]
pub(super) struct DrawnSections {
    /// Sections in sight when a fog hides some (same order as the reached sections); unused
    /// otherwise. Kept from one list to the next.
    in_sight: Vec<ReachedSection>,
    /// What the current list was made from.
    made_from: Option<ListSource>,
    /// Bumped each time the list changes.
    generation: u64,
}

/// What a list of drawn sections is made from: the walk's result (by its generation) and, under a
/// fog that hides some sections, the cell of the eye and the fog's reach.
type ListSource = (u64, Option<([i64; 3], FogReach)>);

impl DrawnSections {
    /// Remakes the list if the walk's result `reached` (sorted by column then height, generation
    /// `walk_generation`), the fog reach or the cell of `eye` changed since the last one.
    pub fn refresh(
        &mut self,
        reached: &[ReachedSection],
        walk_generation: u64,
        eye: [f64; 3],
        min_y: i32,
        fog_reach: Option<FogReach>,
    ) {
        let cell = cell_of(eye);
        let source = (walk_generation, fog_reach.map(|reach| (cell, reach)));
        if self.made_from == Some(source) {
            return;
        }
        self.made_from = Some(source);
        self.generation += 1;
        self.in_sight.clear();
        let Some(reach) = fog_reach else { return };
        // The walk's sections come column by column, from the lowest up: most columns go or stay
        // whole on one or two tests instead of one per section.
        for column in reached.chunk_by(|a, b| (a.0, a.2) == (b.0, b.2)) {
            let (lowest, highest) = (column[0], column[column.len() - 1]);
            if reach.hides_column(cell, (lowest.0, lowest.2)) {
                continue;
            }
            // In a column the fog thickens away from the eye's height: clear at both ends, it is
            // clear in between.
            let clear = |pos| !reach.in_plain_fog(gaps_to_cell(cell, min_y, pos));
            if clear(lowest) && clear(highest) {
                self.in_sight.extend_from_slice(column);
            } else {
                self.in_sight.extend(column.iter().copied().filter(|&pos| !reach.hides(cell, min_y, pos)));
            }
        }
    }

    /// The list: `reached` itself when the fog hides nothing.
    pub fn sections<'a>(&'a self, reached: &'a [ReachedSection]) -> &'a [ReachedSection] {
        match self.made_from {
            Some((_, Some(_))) => &self.in_sight,
            _ => reached,
        }
    }

    /// Changes whenever [`Self::sections`] does: the key of what the draw lists cache.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// Inputs of a visibility walk: eye, bottom of the world, sections per column, radius in columns,
/// occlusion rules on, distance from which nothing shows (infinite: no limit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WalkInputs {
    pub eye: [f64; 3],
    pub min_y: i32,
    pub sections: i32,
    pub radius: i32,
    pub occlusion: bool,
    pub sight_limit: f64,
}

/// Sight limit of the walk of a frame whose fog reaches `fog_reach`: infinite unless the walk is
/// cut at the fog.
pub(super) fn walk_sight_limit(fog_reach: Option<FogReach>, paths: FogCullPaths) -> f64 {
    match fog_reach {
        Some(reach) if paths.cuts_walk => reach.walk_sight_limit(),
        _ => f64::INFINITY,
    }
}

#[cfg(test)]
#[path = "fog_cull_test.rs"]
mod tests;
