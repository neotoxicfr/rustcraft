//! Site of the exit portal, the search that `Server::create_portal` runs before placing one:
//! every column of a 33×33 spiral around the arrival point is walked from its motion-blocking
//! height down to the bottom of the world, looking for runs of replaceable cells whose bottom can
//! host the frame. A spot where the frame also fits one block to each side ("full") beats any
//! other; among full spots, or among all spots when none is full, the nearest wins, and at equal
//! distance the first one in spiral order (then from the top down).
//!
//! The reference walk (`Server::portal_site_cell_by_cell`) reads every cell through the server's
//! block lookup. The paths here read the same cells from a read-only view of the chunks that
//! threads can share; each lead is behind its switch (`SitePaths::from_switches`).

use std::sync::atomic::{AtomicI64, Ordering};

use hashbrown::HashMap;
use rc_data::{StateInfo, VersionData, flags};
use rc_world::{Section, block_index};

use crate::blocks::{Dir, P};
use crate::chunk::Chunk;
use crate::portal_shape as ps;
use crate::server::Server;

/// Implementation of the site search; all false = the reference walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SitePaths {
    /// Column height read from the chunk's motion-blocking heightmap, the source the game reads,
    /// instead of a scan down from the top of the world with the former local rule.
    pub heightmap: bool,
    /// The cells of a column read from its chunk fetched once, instead of a chunk lookup per cell.
    pub column_reads: bool,
    /// Columns skipped, and a column's walk stopped, once they can only give spots farther than
    /// the nearest full spot already found.
    pub distance_cutoff: bool,
    /// Columns shared between the helper threads (the server thread taking shares too),
    /// one share per `COLUMNS_PER_SHARE` columns at most.
    pub parallel: bool,
}

/// Fewest columns of the spiral a share of the parallel search walks: the 1,089 columns of a
/// search make at most 8 shares, so a search never wakes every core for a few columns each.
const COLUMNS_PER_SHARE: usize = 128;

impl SitePaths {
    pub(crate) const REFERENCE: SitePaths =
        SitePaths { heightmap: false, column_reads: false, distance_cutoff: false, parallel: false };

    /// The paths chosen by the switches, each read once.
    pub(crate) fn from_switches() -> SitePaths {
        SitePaths {
            heightmap: rc_data::opt_switch!("RUSTCRAFT_OPT_PORTAL_SITE_HEIGHTMAP"),
            column_reads: rc_data::opt_switch!("RUSTCRAFT_OPT_PORTAL_SITE_COLUMN_READS"),
            distance_cutoff: rc_data::opt_switch!("RUSTCRAFT_OPT_PORTAL_SITE_DISTANCE_CUTOFF"),
            parallel: rc_data::opt_switch!("RUSTCRAFT_OPT_PORTAL_SITE_PARALLEL"),
        }
    }
}

/// Squared distance between two cells.
pub(crate) fn dist_sqr(a: P, b: P) -> i64 {
    let d = |i: usize| i64::from(a[i] - b[i]);
    d(0) * d(0) + d(1) * d(1) + d(2) * d(2)
}

/// The spots kept so far: the nearest full spot and the nearest spot of any kind, each the first
/// one met at its distance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SiteChoice {
    full: Option<(i64, P)>,
    any: Option<(i64, P)>,
}

/// `candidate` replaces `held` only when strictly nearer: at equal distance the first one stays.
fn nearer(candidate: i64, held: Option<(i64, P)>) -> bool {
    held.is_none_or(|(d, _)| candidate < d)
}

impl SiteChoice {
    /// A spot at squared distance `dist` from the arrival point, `full` if the frame fits on both
    /// sides too. Once a full spot is held, the "any" spot no longer matters (`best`).
    pub(crate) fn offer(&mut self, dist: i64, at: P, full: bool) {
        if full && nearer(dist, self.full) {
            self.full = Some((dist, at));
        }
        if nearer(dist, self.any) {
            self.any = Some((dist, at));
        }
    }

    /// Can a spot at squared distance `dist` or more still change the outcome? Besides this
    /// share's own full spot, `nearest_full` (parallel search) holds the nearest full spot any
    /// share has found, this one included: a spot strictly farther cannot be chosen (a full spot
    /// beats any other), while one at that very distance may still win if its share comes first.
    fn may_improve(&self, dist: i64, nearest_full: NearestFullBound) -> bool {
        nearer(dist, self.full) && nearest_full.is_none_or(|d| dist <= d.load(Ordering::Relaxed))
    }

    /// Choice over this part of the spiral followed by `later`.
    pub(crate) fn then(self, later: SiteChoice) -> SiteChoice {
        let pick = |a: Option<(i64, P)>, b: Option<(i64, P)>| match b {
            Some((d, _)) if nearer(d, a) => b,
            _ => a,
        };
        SiteChoice { full: pick(self.full, later.full), any: pick(self.any, later.any) }
    }

    pub(crate) fn best(&self) -> Option<P> {
        self.full.or(self.any).map(|(_, p)| p)
    }
}

/// What the site search reads, shareable between threads: the chunks and the state flags.
pub(crate) struct SiteWorld<'a> {
    chunks: &'a HashMap<(i32, i32), Option<Box<Chunk>>>,
    data: &'a VersionData,
    /// Fluid kind of each fluid index of the block table (0: empty).
    fluid_kind: &'a [u8],
    min_y: i32,
    top_y: i32,
    max_placeable: i32,
    /// State read where no chunk is loaded or outside the world height, like the server's lookup.
    void_air: u32,
    border: [f64; 4],
    column_reads: bool,
}

/// The cells of one column, from its chunk fetched once (no chunk: void air everywhere).
struct ColumnCells<'a> {
    sections: &'a [Section],
    x: usize,
    z: usize,
    min_y: i32,
    void_air: u32,
}

impl ColumnCells<'_> {
    fn state(&self, y: i32) -> u32 {
        let ly = y - self.min_y;
        let section = usize::try_from(ly >> 4).ok().and_then(|i| self.sections.get(i));
        section.map_or(self.void_air, |s| s.blocks.get(block_index(self.x, (ly & 15) as usize, self.z)))
    }
}

impl<'a> SiteWorld<'a> {
    fn info(&self, s: u32) -> StateInfo {
        self.data.states.get(s as usize).copied().unwrap_or_default()
    }

    fn state(&self, p: P) -> u32 {
        crate::play::block_at(self.chunks, self.min_y, p).unwrap_or(self.void_air)
    }

    /// Same rule as `Server::can_portal_replace`: replaceable or air, without fluid.
    fn can_replace(&self, s: u32) -> bool {
        let info = self.info(s);
        let fluid = self.fluid_kind.get(info.fluid as usize).copied().unwrap_or(0);
        (info.has(flags::REPLACEABLE) || info.has(flags::AIR)) && fluid == 0
    }

    fn solid(&self, s: u32) -> bool {
        self.info(s).has(flags::LEGACY_SOLID)
    }

    fn column(&self, x: i32, z: i32) -> ColumnCells<'a> {
        let sections = match self.chunks.get(&(x >> 4, z >> 4)) {
            Some(Some(c)) => c.sections.as_slice(),
            _ => &[],
        };
        ColumnCells { sections, x: (x & 15) as usize, z: (z & 15) as usize, min_y: self.min_y, void_air: self.void_air }
    }

    /// Same as `Server::can_host_frame`.
    fn can_host_frame(&self, origin: P, dir: Dir, offset: i32) -> bool {
        let cw = dir.clockwise().step();
        let d = dir.step();
        for width in -1..3 {
            for height in -1..4 {
                let p = [
                    origin[0] + d[0] * width + cw[0] * offset,
                    origin[1] + height,
                    origin[2] + d[2] * width + cw[2] * offset,
                ];
                let s = self.state(p);
                if (height < 0 && !self.solid(s)) || (height >= 0 && !self.can_replace(s)) {
                    return false;
                }
            }
        }
        true
    }

    /// Height of a column under the former local rule (legacy solid but not cobweb nor bamboo
    /// sapling, or a fluid), scanned down from the top of the world.
    fn former_rule_height(&self, cells: &ColumnCells<'_>, x: i32, z: i32, ids: FormerRuleIds) -> i32 {
        for y in (self.min_y..self.top_y).rev() {
            let s = if self.column_reads { cells.state(y) } else { self.state([x, y, z]) };
            let block = self.info(s).block;
            let blocks = self.solid(s) && block != ids.cobweb && block != ids.bamboo_sapling;
            let fluid = self.fluid_kind.get(self.info(s).fluid as usize).copied().unwrap_or(0);
            if blocks || fluid != 0 {
                return y + 1;
            }
        }
        self.min_y
    }

    /// Spots of one column offered to `choice`, in the game's walk order (from the top down).
    fn offer_column(
        &self,
        query: &SiteQuery,
        col: P,
        height: Option<i32>,
        choice: &mut SiteChoice,
        nearest_full: NearestFullBound,
    ) {
        let (origin, d) = (query.origin, query.dir.step());
        // No spot of this column can be nearer than its horizontal distance.
        let column_dist = dist_sqr(origin, [col[0], origin[1], col[2]]);
        if query.paths.distance_cutoff && !choice.may_improve(column_dist, nearest_full) {
            return;
        }
        let in_border = |p: P| Server::block_in_border(self.border, p);
        if !(in_border(col) && in_border(ps::add(col, d, 1))) {
            return;
        }
        let cells = self.column(col[0], col[2]);
        let height = height.unwrap_or_else(|| self.former_rule_height(&cells, col[0], col[2], query.former_ids));
        let state = |y: i32| if self.column_reads { cells.state(y) } else { self.state([col[0], y, col[2]]) };
        let mut y = self.max_placeable.min(height);
        while y >= self.min_y {
            // Below the arrival point, every lower spot is farther: the walk can stop.
            if query.paths.distance_cutoff
                && y <= origin[1]
                && !choice.may_improve(dist_sqr(origin, [col[0], y, col[2]]), nearest_full)
            {
                return;
            }
            if self.can_replace(state(y)) {
                let first_empty = y;
                while y > self.min_y && self.can_replace(state(y - 1)) {
                    y -= 1;
                }
                let dy = first_empty - y;
                if y + 4 <= self.max_placeable && (dy <= 0 || dy >= 3) {
                    self.offer_spot(query, [col[0], y, col[2]], choice, nearest_full);
                }
            }
            y -= 1;
        }
    }

    fn offer_spot(&self, query: &SiteQuery, at: P, choice: &mut SiteChoice, nearest_full: NearestFullBound) {
        let dist = dist_sqr(query.origin, at);
        if query.paths.distance_cutoff && !choice.may_improve(dist, nearest_full) {
            return;
        }
        if self.can_host_frame(at, query.dir, 0) {
            let full = self.can_host_frame(at, query.dir, -1) && self.can_host_frame(at, query.dir, 1);
            choice.offer(dist, at, full);
            if full && let Some(nearest) = nearest_full {
                nearest.fetch_min(dist, Ordering::Relaxed);
            }
        }
    }

    /// Choice over `columns` (spiral order) with their heights (`None`: former local rule);
    /// `nearest_full`: the nearest full spot found by any share of a parallel search.
    fn choose(&self, query: &SiteQuery, columns: &[(P, Option<i32>)], nearest_full: NearestFullBound) -> SiteChoice {
        let mut choice = SiteChoice::default();
        for &(col, height) in columns {
            self.offer_column(query, col, height, &mut choice, nearest_full);
        }
        choice
    }
}

/// Squared distance of the nearest full spot found by any share of a parallel search, a bound
/// past which no spot can win (`None`: one walk, no bound shared).
type NearestFullBound<'a> = Option<&'a AtomicI64>;

/// Block ids of the former local height rule.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FormerRuleIds {
    pub cobweb: u16,
    pub bamboo_sapling: u16,
}

/// What a site search looks for: arrival point, frame direction, paths.
struct SiteQuery {
    origin: P,
    dir: Dir,
    paths: SitePaths,
    former_ids: FormerRuleIds,
}

/// A site search ready to run without the server: what it reads, what it looks for, the
/// columns of the spiral with their heights.
pub(crate) struct SiteSearch<'a> {
    world: SiteWorld<'a>,
    query: SiteQuery,
    columns: Vec<(P, Option<i32>)>,
}

impl SiteSearch<'_> {
    /// The chosen site, shared with the helper threads when the search is.
    pub(crate) fn run(&self) -> Option<P> {
        self.run_on(crate::helper_pool::logical_cores())
    }

    /// The chosen site, shared with a helper pool of `threads` threads when the search is.
    pub(crate) fn run_on(&self, threads: usize) -> Option<P> {
        let shares = threads.min(self.columns.len() / COLUMNS_PER_SHARE);
        if self.query.paths.parallel && shares > 1 {
            choose_in_parallel(&self.world, &self.query, &self.columns, threads, shares).best()
        } else {
            self.world.choose(&self.query, &self.columns, None).best()
        }
    }
}

impl Server {
    /// Site of the exit portal around `origin` (chunks already loaded) through `paths`.
    pub(crate) fn portal_site(&self, origin: P, dir: Dir, paths: SitePaths) -> Option<P> {
        if paths == SitePaths::REFERENCE {
            return self.portal_site_cell_by_cell(origin, dir);
        }
        self.portal_site_search(origin, dir, paths).run()
    }

    pub(crate) fn portal_site_search(&self, origin: P, dir: Dir, paths: SitePaths) -> SiteSearch<'_> {
        let former_ids = FormerRuleIds {
            cobweb: self.table().block_id("cobweb"),
            bamboo_sapling: self.table().block_id("bamboo_sapling"),
        };
        let query = SiteQuery { origin, dir, paths, former_ids };
        let world = SiteWorld {
            chunks: &self.chunks,
            data: &self.data,
            fluid_kind: &self.table().fluid_kind,
            min_y: self.dim.height.min_y,
            top_y: self.top_y(),
            max_placeable: self.max_placeable_y(),
            void_air: self.view().default_state(self.table().id.void_air),
            border: self.border_bounds(),
            column_reads: paths.column_reads,
        };
        // The heightmap belongs to the server (live map, or a tag scan when a chunk has none):
        // read here, once per column, before any thread starts.
        let columns: Vec<(P, Option<i32>)> = ps::spiral(origin, 16, Dir::East, Dir::South)
            .into_iter()
            .map(|c| (c, paths.heightmap.then(|| self.hm_motion_blocking(c[0], c[2]))))
            .collect();
        SiteSearch { world, query, columns }
    }
}

/// The spiral cut into `shares` contiguous shares on the helper pool of `threads` threads (the
/// calling thread taking shares too), each share chosen alone, the shares' choices chained in
/// spiral order (same result as one walk). With the distance cutoff, the shares share the
/// nearest full spot found so far: without it, a far share would walk every column the one walk
/// skips.
fn choose_in_parallel(
    world: &SiteWorld<'_>,
    query: &SiteQuery,
    columns: &[(P, Option<i32>)],
    threads: usize,
    shares: usize,
) -> SiteChoice {
    let per = columns.len().div_ceil(shares.max(1)).max(1);
    let nearest_full_dist = AtomicI64::new(i64::MAX);
    let nearest_full = query.paths.distance_cutoff.then_some(&nearest_full_dist);
    let jobs: Vec<_> = columns.chunks(per).map(|share| move || world.choose(query, share, nearest_full)).collect();
    let choices = crate::helper_pool::run_in_order(threads, jobs);
    choices.into_iter().fold(SiteChoice::default(), SiteChoice::then)
}
