//! `/test` (G3-F06): runs of test instances (chosen by identifier pattern, around the source,
//! looked at, or failed last time), the check of a test under its four rotations, location,
//! reset and clearing of test structures, stop, position relative to a test, new empty test
//! structures. The export subcommands only exist in the game's development environment.

use rc_text::Component;
use rc_worldgen::template::Rot;

use super::text::{GREEN, brackets, copy_click, hover_text, lit, suggest_click};
use super::{Fail, Parsed, R, Seg, Src};
use crate::blocks::P;
use crate::commands::ents::E;
use crate::gametest::run::{Planned, Retry, RunSource, RunSpec};
use crate::gametest::{Data, TestDef};
use crate::poi::{Occupancy, PoiType};
use crate::server::Server;
use crate::tr;
use std::sync::Arc;

/// Radius of the searches around the source.
const NEAR: i32 = 15;
const FAR: i32 = 250;

/// How a subcommand chooses its tests.
enum Finder {
    /// Tests of the registry (identifier pattern).
    Tests(Vec<Arc<TestDef>>),
    /// Test instance blocks within this radius of the source.
    Near(i32),
    Nearest,
    LookedAt,
    /// Tests that failed in the last runs (required ones only, or all).
    Failed(bool),
}

impl Server {
    pub(crate) fn cmd_test(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let int = |n: &str| p.arg(seg, n).and_then(|v| v.parse::<i32>().ok());
        let flag = |n: &str| p.arg(seg, n).is_some_and(|v| v == "true");
        let retry = match int("numberOfTimes") {
            None => Retry::NONE,
            Some(n) => Retry { tries: n, halt: flag("untilFailed") },
        };
        let tests = || self.selected_tests(&p.arg(seg, "tests").unwrap_or_default());
        match w.get(1).copied().unwrap_or("") {
            "run" => {
                let f = Finder::Tests(tests());
                self.test_run(s, f, retry, int("rotationSteps").unwrap_or(0), int("testsPerRow").unwrap_or(8), 1)
            }
            "runmultiple" => {
                let f = Finder::Tests(tests());
                self.test_run(s, f, Retry::NONE, 0, 8, int("amount").unwrap_or(1))
            }
            "runthese" => self.test_run(s, Finder::Near(FAR), retry, 0, 8, 1),
            "runclosest" => self.test_run(s, Finder::Nearest, retry, 0, 8, 1),
            "runthat" => self.test_run(s, Finder::LookedAt, retry, 0, 8, 1),
            "runfailed" => {
                let f = Finder::Failed(flag("onlyRequiredTests"));
                self.test_run(s, f, retry, int("rotationSteps").unwrap_or(0), int("testsPerRow").unwrap_or(8), 1)
            }
            "verify" => self.test_verify(s, tests()),
            "locate" => self.test_locate(s, tests()),
            "resetclosest" => self.test_reset(s, Finder::Nearest),
            "resetthese" => self.test_reset(s, Finder::Near(FAR)),
            "resetthat" => self.test_reset(s, Finder::LookedAt),
            "clearthat" => self.test_clear(s, Finder::LookedAt),
            "clearthese" => self.test_clear(s, Finder::Near(FAR)),
            "clearall" => self.test_clear(s, Finder::Near(int("radius").map_or(FAR, |r| r.clamp(0, 1024)))),
            "stop" => {
                self.tests_stop();
                Ok(1)
            }
            "pos" => self.test_pos(s, &p.arg(seg, "var").unwrap_or_else(|| "pos".into())),
            "create" => {
                let id = crate::tech_blocks::template_id(&p.arg(seg, "id").unwrap_or_default())
                    .ok_or_else(|| Fail(tr!("argument.id.invalid")))?;
                let wd = int("width").unwrap_or(5);
                let size = if int("width").is_none() { [5, 5, 5] } else { [wd, int("height").unwrap_or(wd), int("depth").unwrap_or(wd)] };
                self.test_create(s, &id, size)
            }
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    /// Tests of the registry matching a pattern (`*` and `?`, default namespace), in registry
    /// order.
    fn selected_tests(&self, pattern: &str) -> Vec<Arc<TestDef>> {
        let pat = if pattern.contains(':') { pattern.to_owned() } else { format!("minecraft:{pattern}") };
        self.cmd.tests.tests.iter().filter(|t| rc_commands::args::glob(&pat, &t.id)).cloned().collect()
    }

    fn run_source(&self, s: &Src) -> RunSource {
        let seat = if s.origin == super::Origin::Player { self.lan.as_ref().and_then(|l| l.active) } else { None };
        RunSource { src: s.clone(), seat }
    }

    /// Test instance blocks of a finder (source's dimension), and its tests.
    fn find(&mut self, s: &Src, f: &Finder) -> Result<(Vec<P>, Vec<Arc<TestDef>>), Fail> {
        let at = s.pos.map(|v| v.floor() as i32);
        Ok(match f {
            Finder::Tests(t) => (Vec::new(), t.clone()),
            Finder::Failed(required) => {
                let list = self.cmd.tests.failed.clone();
                let t = list
                    .iter()
                    .filter_map(|id| self.cmd.tests.test(id).cloned())
                    .filter(|t| !required || t.required)
                    .collect();
                (Vec::new(), t)
            }
            Finder::Near(r) => {
                let r = *r;
                (self.in_dimension(&s.dim.clone(), |sv| sv.test_blocks_around(at, r)), Vec::new())
            }
            Finder::Nearest => {
                let list = self.in_dimension(&s.dim.clone(), |sv| sv.test_blocks_around(at, NEAR));
                let manhattan = |q: &P| (0..3).map(|i| (q[i] - at[i]).abs()).sum::<i32>();
                let best = list.iter().min_by_key(|q| manhattan(q)).copied();
                (best.into_iter().collect(), Vec::new())
            }
            Finder::LookedAt => {
                if s.entity != Some(E::Player) {
                    return Err(Fail(tr!("command.failed")));
                }
                let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(E::Player), self.pos[2]];
                let v = rc_physics::ray::view_vector(self.pitch, self.yaw);
                let end = [eye[0] + v[0] * 250.0, eye[1] + v[1] * 250.0, eye[2] + v[2] * 250.0];
                let hit = self.in_dimension(&s.dim.clone(), |sv| {
                    let mut seen: Vec<P> = sv
                        .test_blocks_around(at, FAR)
                        .into_iter()
                        .filter(|q| sv.ti_structure_box(*q).is_some_and(|b| aabb_clip(b, eye, end)))
                        .collect();
                    let d2 = |q: &P| (0..3).map(|i| i64::from(q[i] - at[i]).pow(2)).sum::<i64>();
                    seen.sort_by_key(|q| d2(q));
                    seen.into_iter().next()
                });
                (hit.into_iter().collect(), Vec::new())
            }
        })
    }

    /// Test instance blocks (their points of interest) within `r` of `at`, in the game's order.
    fn test_blocks_around(&mut self, at: P, r: i32) -> Vec<P> {
        self.poi_in_range(|t| t == PoiType::TestInstance, at, r, Occupancy::Any).into_iter().map(|rec| rec.pos).collect()
    }

    /// Does a test's structure exist (its template)? Otherwise the failure is reported.
    fn test_structure_exists(&mut self, s: &Src, id: &str) -> bool {
        if self.template_get(id).is_some() {
            return true;
        }
        self.fail(s, tr!("commands.test.error.structure_not_found", id.to_owned()));
        false
    }

    /// A test instance block to run in place: its test, its own rotation (added to the test's
    /// once more by the run, like the game).
    fn plan_at(&mut self, s: &Src, pos: P, retry: Retry) -> Option<Planned> {
        let dim = s.dim.clone();
        let data = self.in_dimension(&dim, |sv| sv.ti_data(pos));
        let Some(d) = data else {
            self.fail(s, tr!("commands.test.error.test_instance_not_found.position", pos[0], pos[1], pos[2]));
            return None;
        };
        let Some(test) = d.test.as_deref().and_then(|t| self.cmd.tests.test(t)).cloned() else {
            let name = self.ti_name(&d);
            self.fail(s, tr!("commands.test.error.non_existant_test", name));
            return None;
        };
        let rot = test.rotation.then(d.rot);
        if !self.test_structure_exists(s, &test.structure.clone()) {
            return None;
        }
        Some(Planned { test, rot, dim, retry, pos: Some(pos) })
    }

    /// Tests of the registry to lay out: structure checked, dimension known.
    fn plan_tests(&mut self, s: &Src, tests: &[Arc<TestDef>], retry: Retry, steps: i32) -> Result<Vec<Planned>, Fail> {
        let mut out = Vec::new();
        for t in tests {
            if !self.test_structure_exists(s, &t.structure.clone()) {
                continue;
            }
            if !(0..=3).contains(&steps) {
                // The game's error escapes the command: generic failure, its text on hover.
                let mut c = tr!("command.failed");
                hover_text(&mut c, lit(format!("rotationSteps must be a value from 0-3. Got value {steps}")));
                return Err(Fail(c));
            }
            if !self.world_dimensions().contains(&t.dimension) {
                self.fail(s, lit(format!("Could not resolve level for dimension: {}", t.dimension)));
                continue;
            }
            out.push(Planned { test: t.clone(), rot: Rot::from_index(steps), dim: t.dimension.clone(), retry, pos: None });
        }
        Ok(out)
    }

    fn test_run(&mut self, s: &Src, f: Finder, retry: Retry, steps: i32, per_row: i32, copies: i32) -> R {
        self.tests_stop();
        let (mut positions, mut tests) = self.find(s, &f)?;
        if copies != 1 {
            let n = usize::try_from(copies).unwrap_or(0);
            positions = positions.repeat(n);
            tests = (0..n).flat_map(|_| tests.iter().cloned()).collect();
        }
        let mut plans = Vec::new();
        for q in positions {
            plans.extend(self.plan_at(s, q, retry));
        }
        plans.extend(self.plan_tests(s, &tests, retry, steps)?);
        if plans.is_empty() {
            self.ok(s, tr!("commands.test.no_tests"));
            return Ok(0);
        }
        self.cmd.tests.forget_failed();
        self.ok(s, tr!("commands.test.run.running", plans.len()));
        let rs = self.run_source(s);
        self.tests_start(rs, plans, RunSpec { per_row, clear_on_batch: false, verify: None });
        Ok(1)
    }

    /// Each test 100 times under each rotation, a batch per rotation, 10 per row, cleared between
    /// batches, stopped at the first failure.
    fn test_verify(&mut self, s: &Src, tests: Vec<Arc<TestDef>>) -> R {
        self.tests_stop();
        let base = self.plan_tests(s, &tests, Retry::NONE, 0)?;
        self.cmd.tests.forget_failed();
        let mut plans = Vec::new();
        let mut groups = Vec::new();
        for b in &base {
            for (k, rot) in Rot::ALL.into_iter().enumerate() {
                let first = plans.len();
                for _ in 0..100 {
                    plans.push(Planned { test: b.test.clone(), rot, dim: b.dim.clone(), retry: Retry { tries: 1, halt: true }, pos: b.pos });
                }
                groups.push(((first..plans.len()).collect(), b.test.env.clone(), k as i32, b.test.dimension.clone()));
            }
        }
        let rs = self.run_source(s);
        self.tests_start(rs, plans, RunSpec { per_row: 10, clear_on_batch: true, verify: Some(groups) });
        Ok(1)
    }

    fn test_reset(&mut self, s: &Src, f: Finder) -> R {
        self.tests_stop();
        let (positions, _) = self.find(s, &f)?;
        let mut n = 0;
        for q in positions {
            if self.plan_at(s, q, Retry::NONE).is_none() {
                continue;
            }
            let src = s.clone();
            let dim = s.dim.clone();
            self.in_dimension(&dim, |sv| sv.ti_reset(q, &mut |sv: &mut Server, c| sv.tell_source(&src, c)));
            n += 1;
        }
        // The game answers an empty reset with the clearing's error.
        if n == 0 {
            return Err(Fail(tr!("commands.test.clear.error.no_tests")));
        }
        self.ok(s, tr!("commands.test.reset.success", n));
        Ok(n)
    }

    fn test_clear(&mut self, s: &Src, f: Finder) -> R {
        self.tests_stop();
        let (positions, _) = self.find(s, &f)?;
        let dim = s.dim.clone();
        let n = self.in_dimension(&dim, |sv| {
            let found: Vec<P> = positions.into_iter().filter(|q| sv.ti_data(*q).is_some()).collect();
            for &q in &found {
                if let Some(g) = sv.ti_geo(q) {
                    sv.clear_space(g.test_box());
                }
                sv.ti_remove_barriers(q);
                sv.destroy_block(q, false, 512);
            }
            found.len()
        });
        if n == 0 {
            return Err(Fail(tr!("commands.test.clear.error.no_tests")));
        }
        self.ok(s, tr!("commands.test.clear.success", n));
        Ok(n as i32)
    }

    fn test_locate(&mut self, s: &Src, tests: Vec<Arc<TestDef>>) -> R {
        self.tell_source(s, tr!("commands.test.locate.started"));
        let at = s.pos.map(|v| v.floor() as i32);
        let wanted: Vec<String> = tests.iter().map(|t| t.id.clone()).collect();
        // Every world the server holds, in its order (the others hold no loaded block).
        let mut levels: Vec<String> = Vec::new();
        for d in self.world_dimensions() {
            if d == self.dim.name || self.dims.parked.iter().any(|w| w.dim.name == d) {
                levels.push(d);
            }
        }
        let mut found: Vec<(String, P, Rot)> = Vec::new();
        if !wanted.is_empty() {
            for d in levels {
                let hits = self.in_dimension(&d.clone(), |sv| {
                    sv.test_blocks_around(at, FAR)
                        .into_iter()
                        .filter_map(|q| {
                            let g = sv.ti_geo(q)?;
                            g.data.test.as_ref().filter(|t| wanted.contains(t))?;
                            Some((q, g.rot))
                        })
                        .collect::<Vec<_>>()
                });
                for (q, r) in hits {
                    if !found.iter().any(|x| x.0 == d && x.1 == q) {
                        found.push((d.clone(), q, r));
                    }
                }
            }
        }
        let mut dims: Vec<String> = Vec::new();
        for (d, q, rot) in &found {
            if !dims.contains(d) {
                dims.push(d.clone());
            }
            // In front of the block's north face once turned, three blocks away, facing it.
            let (dx, dz, yaw) = match rot {
                Rot::None => (0, -3, 0),
                Rot::Cw90 => (3, 0, 90),
                Rot::Cw180 => (0, 3, 180),
                Rot::Ccw90 => (-3, 0, 270),
            };
            let tp = format!("/execute in {d} run tp @s {} {} {} {yaw} 0", q[0] + dx, q[1], q[2] + dz);
            let distance = if *d == s.dim {
                let (ex, ez) = (at[0] - q[0], at[2] - q[2]);
                let d2 = ex.wrapping_mul(ex).wrapping_add(ez.wrapping_mul(ez));
                (f64::from(d2 as f32).sqrt() as f32).floor().to_string()
            } else {
                "N/A".into()
            };
            let path = d.split_once(':').map_or(d.as_str(), |x| x.1);
            let mut inner = tr!("chat.coordinates", q[0], q[1], q[2]);
            inner.extra.push(lit(format!(" ->[{path}]")));
            let mut c = brackets(inner);
            c.style.color = Some(GREEN);
            suggest_click(&mut c, tp);
            hover_text(&mut c, tr!("chat.coordinates.tooltip"));
            self.ok(s, tr!("commands.test.locate.found", c, distance));
        }
        let n = found.len();
        if n == 0 {
            return Err(Fail(tr!("commands.test.error.no_test_instances")));
        }
        self.output_player_coordinates(s, &dims);
        self.ok(s, tr!("commands.test.locate.done", n));
        Ok(n as i32)
    }

    /// `test pos`: the block looked at (10 blocks), relative to the corner of the test
    /// structure holding it (searched within 15 blocks, then 250), highlighted for the player.
    fn test_pos(&mut self, s: &Src, var: &str) -> R {
        if s.entity != Some(E::Player) {
            return Err(Fail(tr!("permissions.requires.player")));
        }
        let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(E::Player), self.pos[2]];
        let ray = crate::fluid_use::SimRay(self, false);
        let target = rc_physics::ray::item_clip(&ray, eye, self.pitch, self.yaw, 10.0, rc_physics::ray::Fluids::None).pos;
        let dim = s.dim.clone();
        let holder = self.in_dimension(&dim, |sv| {
            [NEAR, FAR].into_iter().find_map(|r| {
                sv.test_blocks_around(target, r).into_iter().find(|q| {
                    sv.ti_structure_box(*q).is_some_and(|(lo, hi)| (0..3).all(|i| target[i] >= lo[i] && target[i] <= hi[i]))
                })
            })
        });
        let Some(q) = holder else {
            return Err(Fail(tr!("commands.test.error.no_test_containing_pos", target[0], target[1], target[2])));
        };
        let Some(g) = self.in_dimension(&dim, |sv| sv.ti_geo(q)) else {
            return Err(Fail(tr!("commands.test.error.test_instance_not_found")));
        };
        let corner = g.structure_pos();
        let rel = [target[0] - corner[0], target[1] - corner[1], target[2] - corner[2]];
        let desc = format!("{}, {}, {}", rel[0], rel[1], rel[2]);
        let mut coords = tr!("commands.test.coordinates", rel[0], rel[1], rel[2]);
        coords.style.bold = Some(true);
        coords.style.color = Some(GREEN);
        hover_text(&mut coords, tr!("commands.test.coordinates.copy"));
        copy_click(&mut coords, format!("final BlockPos {var} = new BlockPos({desc});"));
        let name = crate::commands::cmds_ents::plain(&self.ti_name(&g.data));
        self.ok(s, tr!("commands.test.relative_position", name, coords));
        let _ = self.send(rc_client::Event::GameTestHighlight { absolute: target, relative: rel });
        Ok(1)
    }

    /// New empty test structure 3 blocks south of the player, at the surface: room cleared, test
    /// instance block with the given test and size, bedrock floor.
    fn test_create(&mut self, s: &Src, id: &str, size: [i32; 3]) -> R {
        if size.iter().any(|&v| v > 48) {
            return Err(Fail(tr!("commands.test.error.too_large", 48)));
        }
        let rs = self.run_source(s);
        let dim = s.dim.clone();
        let at = self.test_position_around(&rs, &dim);
        let made = self.in_dimension(&dim, |sv| {
            let corner = [at[0], at[1] + 1, at[2] + 1];
            let far = [corner[0] + size[0] - 1, corner[1] + size[1] - 1, corner[2] + size[2] - 1];
            let lo = [corner[0].min(far[0]), corner[1].min(far[1]), corner[2].min(far[2])];
            let hi = [corner[0].max(far[0]), corner[1].max(far[1]), corner[2].max(far[2])];
            sv.clear_space((lo, hi));
            let block = sv.default_state("minecraft:test_instance_block");
            sv.set_block(at, block, crate::blocks::flag::ALL);
            sv.ti_set(at, &Data::new(Some(id.to_owned()), size, Rot::None));
            let g = sv.ti_geo(at)?;
            let low = g.structure_pos();
            let high = [low[0] + size[0] - 1, low[1], low[2] + size[2] - 1];
            let bedrock = sv.default_state("minecraft:bedrock");
            for z in low[2].min(high[2])..=low[2].max(high[2]) {
                for x in low[0].min(high[0])..=low[0].max(high[0]) {
                    sv.set_block([x, low[1], z], bedrock, crate::blocks::flag::ALL);
                }
            }
            Some(sv.ti_name(&g.data))
        });
        let name: Component = made.unwrap_or_else(|| lit(id.to_owned()));
        self.ok(s, tr!("commands.test.create.success", name));
        Ok(1)
    }
}

/// Does the segment `from`→`to` enter the box (the game's box clip: a start inside does not
/// count)?
fn aabb_clip((lo, hi): (P, P), from: [f64; 3], to: [f64; 3]) -> bool {
    let min = lo.map(f64::from);
    let max = [f64::from(hi[0]) + 1.0, f64::from(hi[1]) + 1.0, f64::from(hi[2]) + 1.0];
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut scale = 1.0f64;
    let mut hit = false;
    for a in 0..3 {
        let plane = if d[a] > 1.0e-7 {
            min[a]
        } else if d[a] < -1.0e-7 {
            max[a]
        } else {
            continue;
        };
        let t = (plane - from[a]) / d[a];
        let (b, c) = ((a + 1) % 3, (a + 2) % 3);
        let pb = from[b] + t * d[b];
        let pc = from[c] + t * d[c];
        if 0.0 < t && t < scale && min[b] - 1.0e-7 < pb && pb < max[b] + 1.0e-7 && min[c] - 1.0e-7 < pc && pc < max[c] + 1.0e-7 {
            scale = t;
            hit = true;
        }
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::aabb_clip;

    #[test]
    fn box_clip_counts_entries_only() {
        let b = ([0, 0, 0], [1, 1, 1]);
        assert!(aabb_clip(b, [-1.0, 0.5, 0.5], [5.0, 0.5, 0.5]), "crosses the west face");
        assert!(!aabb_clip(b, [-1.0, 5.0, 0.5], [5.0, 5.0, 0.5]), "passes above");
        assert!(!aabb_clip(b, [1.0, 1.0, 1.0], [1.5, 1.5, 1.5]), "starts inside: no entry face");
        assert!(!aabb_clip(b, [-3.0, 0.5, 0.5], [-1.0, 0.5, 0.5]), "stops before the box");
    }
}
