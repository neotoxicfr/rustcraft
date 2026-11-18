//! Swirls of the active mob effects around living entities. The server only sends, as living
//! entity data, the particle of every visible effect (10: a list, usually `entity_effect` tinted
//! with the effect colour) and whether every visible effect is ambient (11: beacon, conduit); the
//! client tick of each living entity, local player included, may add one of them inside its box.
//! Client-side randomness (`WorldRandom`): the game's chances, bounds and draw order.

use rc_proto_v777::entity::{Meta, Particle};

use crate::entities::{Entities, Entity};
use crate::level_events::WorldRandom;
use crate::particles::{Options, Particles};

/// Living entity data: particle of every visible effect.
pub(crate) const EFFECT_PARTICLE_LIST: u8 = 10;
/// Living entity data: every visible effect is ambient.
const ALL_EFFECTS_AMBIENT: u8 = 11;
/// Shared flag (data 0) of an invisible entity.
const INVISIBLE_FLAG: i8 = 0x20;

/// Effect particles carried by an entity; empty without a visible effect and for every non-living
/// entity (its data 10 is something else, never a list).
pub(crate) fn effect_particle_list(e: &Entity) -> &[Particle] {
    match e.meta.get(&EFFECT_PARTICLE_LIST) {
        Some(Meta::Particles(list)) => list,
        _ => &[],
    }
}

/// What the effect tick reads of a living body.
#[derive(Clone, Copy)]
pub(crate) struct EffectBody<'a> {
    pub particles: &'a [Particle],
    pub ambient: bool,
    pub invisible: bool,
    /// Centre of the bottom of the box.
    pub feet: [f64; 3],
    pub width: f64,
    pub height: f64,
}

impl<'a> EffectBody<'a> {
    /// The effect data of `e`, with a box of `(width, height)` standing at `feet`.
    pub fn new(e: &'a Entity, feet: [f64; 3], (width, height): (f64, f64)) -> Self {
        EffectBody {
            particles: effect_particle_list(e),
            ambient: e.meta_bool(ALL_EFFECTS_AMBIENT),
            invisible: e.flags() & INVISIBLE_FLAG != 0,
            feet,
            width,
            height,
        }
    }

    /// An entity of the table at its position of the tick (already interpolated, as in the game),
    /// with its current box.
    fn of_table_entity(e: &'a Entity, dims: &[[f32; 4]]) -> Self {
        let size = e.size(dims);
        Self::new(e, e.pos, (f64::from(size.w), f64::from(size.h)))
    }
}

/// One chance in this many per tick of a particle: 4, or 15 for an invisible body; five times
/// rarer when every visible effect is ambient.
fn one_chance_in(invisible: bool, ambient: bool) -> usize {
    let base = if invisible { 15 } else { 4 };
    if ambient { base * 5 } else { base }
}

/// Uniform integer in `0..n` (`n` ≥ 1) from one draw.
fn below(r: &mut WorldRandom, n: usize) -> usize {
    // The f32 product rounds up to `n` for the draws closest to 1.
    ((r.f() * n as f32) as usize).min(n - 1)
}

/// Uniform in [-1, 1) from one draw.
fn centred(r: &mut WorldRandom) -> f64 {
    2.0 * f64::from(r.f()) - 1.0
}

/// The tick of one body: maybe one entry of its list, at a random point of its box (x and z within
/// the half-width around the feet, y over the height). Draws in the game's order: chance, entry,
/// x, y, z; none at all without particles.
pub(crate) fn roll_effect_particle(body: &EffectBody<'_>, r: &mut WorldRandom) -> Option<(usize, [f64; 3])> {
    if body.particles.is_empty() || below(r, one_chance_in(body.invisible, body.ambient)) != 0 {
        return None;
    }
    let entry = below(r, body.particles.len());
    let x = body.feet[0] + centred(r) * 0.5 * body.width;
    let y = body.feet[1] + f64::from(r.f()) * body.height;
    let z = body.feet[2] + centred(r) * 0.5 * body.width;
    Some((entry, [x, y, z]))
}

/// Hands one effect particle to the engine like the game: normal addition (distance and particle
/// setting filters), velocity 1 on every axis, which each particle type turns into its own motion.
/// False when the engine does not know the type or filtered the particle out.
pub(crate) fn spawn_effect_particle(ps: &mut Particles, p: &Particle, at: [f64; 3]) -> bool {
    match ps.kind_of(p.kind) {
        Some(kind) => ps.spawn(kind, &Options::from_net(p, false), at, [1.0; 3]),
        None => false,
    }
}

/// Effect particles of one client tick: every entity of the table carrying effects, then the local
/// player (`local`, when in the world). A frozen world (`runs` false) still ticks the players and
/// what carries them, as the entity tick does; their particles wait in the engine's queue.
pub(crate) fn tick_effect_particles(
    ents: &Entities,
    local: Option<EffectBody<'_>>,
    runs: bool,
    r: &mut WorldRandom,
    spawn: &mut impl FnMut(&Particle, [f64; 3]),
) {
    for e in ents.with_effect_particles() {
        if runs || crate::ticking::carries_player(ents, e, 0) {
            roll_and_spawn(&EffectBody::of_table_entity(e, &ents.dims), r, spawn);
        }
    }
    if let Some(body) = local {
        roll_and_spawn(&body, r, spawn);
    }
}

fn roll_and_spawn(body: &EffectBody<'_>, r: &mut WorldRandom, spawn: &mut impl FnMut(&Particle, [f64; 3])) {
    if let Some((entry, at)) = roll_effect_particle(body, r) {
        spawn(&body.particles[entry], at);
    }
}

impl crate::App {
    /// Mob effect swirls for one client tick (`runs`: the world is not frozen). The local player
    /// reads its own data, its position before this tick's move (the game's effect tick comes
    /// first) and the box of its posture (0.2 × 0.2 in bed).
    pub(crate) fn tick_mob_effect_particles(&mut self, runs: bool) {
        let size = if self.sleeping_pos().is_some() { (0.2, 0.2) } else { self.player.dimensions() };
        let me = self.entities.me.as_ref().filter(|_| self.positioned);
        let local = me.map(|me| EffectBody::new(me, self.player.pos, size));
        let ps = &mut self.particles;
        tick_effect_particles(&self.entities, local, runs, &mut self.world_random, &mut |p, at| {
            spawn_effect_particle(ps, p, at);
        });
    }
}

#[cfg(test)]
mod tests {
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::AddEntity;

    use super::*;

    const KINDS: [&str; 4] = ["zombie", "player", "area_effect_cloud", "horse"];
    const ZOMBIE: usize = 0;
    const PLAYER: usize = 1;
    const CLOUD: usize = 2;
    const HORSE: usize = 3;
    const LOCAL: i32 = 9_000;

    fn world() -> Entities {
        Entities {
            type_names: KINDS.iter().map(|k| format!("minecraft:{k}")).collect(),
            local: LOCAL,
            dims: vec![[0.6, 1.95, 1.74, 2.0], [0.6, 1.8, 1.62, 2.0], [6.0, 0.5, 0.25, 0.5], [1.4, 1.6, 1.52, 1.6]],
            ..Default::default()
        }
    }

    fn spawn(ents: &mut Entities, id: i32, kind: usize, pos: [f64; 3], values: Vec<(u8, Meta)>) {
        let add = AddEntity {
            id,
            uuid: id as u128,
            kind: kind as i32,
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        ents.apply(EntityEvent::Add(Box::new(add)));
        ents.apply(EntityEvent::Data { id, values });
    }

    fn swirl(kind: i32, color: u32) -> Particle {
        Particle { kind, color, state: 0, item: -1, scale: 1.0, color2: 0, vib: None, trail: None }
    }

    fn effects(list: Vec<Particle>) -> (u8, Meta) {
        (EFFECT_PARTICLE_LIST, Meta::Particles(list))
    }

    fn body(particles: &[Particle], invisible: bool, ambient: bool) -> EffectBody<'_> {
        EffectBody { particles, ambient, invisible, feet: [10.0, 64.0, -3.0], width: 0.6, height: 1.8 }
    }

    /// Particles per tick as the game draws them: 1 in 4, 1 in 15 invisible, five times fewer when
    /// every effect is ambient (seeded, 400,000 ticks each). Sabotage verified (failure): the
    /// invisible bound left at 4; the ambient factor dropped.
    #[test]
    fn chance_per_tick_follows_invisibility_and_ambience() {
        let list = [swirl(0, 0xff00_ff00)];
        let mut r = WorldRandom::default();
        for (invisible, ambient, one_in) in
            [(false, false, 4.0), (true, false, 15.0), (false, true, 20.0), (true, true, 75.0)]
        {
            let b = body(&list, invisible, ambient);
            let ticks = 400_000;
            let hits = (0..ticks).filter(|_| roll_effect_particle(&b, &mut r).is_some()).count();
            let want = f64::from(ticks) / one_in;
            assert!(
                (hits as f64 - want).abs() < want * 0.06,
                "invisible {invisible}, ambient {ambient}: {hits}, want {want}"
            );
        }
    }

    /// A particle lands inside the box (x and z within the half-width, y from the feet to the
    /// top) and spreads over all of it; the list's entries come out evenly. Sabotage verified
    /// (failure): the full width used as the half-width; y spread around the feet.
    #[test]
    fn particles_fill_the_box_and_pick_entries_evenly() {
        let list = [swirl(0, 1), swirl(0, 2), swirl(0, 3)];
        let b = body(&list, false, false);
        let mut r = WorldRandom::default();
        let mut reached = Bounds::EMPTY;
        let mut picked = [0usize; 3];
        for _ in 0..200_000 {
            let Some((entry, at)) = roll_effect_particle(&b, &mut r) else { continue };
            picked[entry] += 1;
            reached.widen(at);
        }
        let (lo, hi) = ([9.7, 64.0, -3.3], [10.3, 65.8, -2.7]);
        assert!(reached.within(lo, hi) && reached.covers(lo, hi), "{:?} to {:?}", reached.low, reached.high);
        let total: usize = picked.iter().sum();
        assert!(picked.iter().all(|&n| (n as f64 - total as f64 / 3.0).abs() < total as f64 * 0.03), "{picked:?}");
    }

    /// No list, an empty list or the single particle a cloud carries at data 10: nothing, and no
    /// draw spent.
    #[test]
    fn nothing_and_no_draw_without_effect_particles() {
        let mut ents = world();
        spawn(&mut ents, 1, ZOMBIE, [0.0; 3], vec![]);
        spawn(&mut ents, 2, ZOMBIE, [0.0; 3], vec![effects(vec![])]);
        spawn(&mut ents, 3, CLOUD, [0.0; 3], vec![(EFFECT_PARTICLE_LIST, Meta::Particle(swirl(0, 0xffff_ffff)))]);
        let (mut r, mut fresh) = (WorldRandom::default(), WorldRandom::default());
        for e in ents.map.values() {
            assert!(effect_particle_list(e).is_empty(), "{}", e.kind);
            assert!(roll_effect_particle(&EffectBody::of_table_entity(e, &ents.dims), &mut r).is_none());
        }
        assert_eq!(r.f(), fresh.f(), "a draw was spent");
    }

    /// Every carrier ticks: mobs, other players, the local player with its own data and box
    /// (invisible here: its rate drops to 1 in 15). The swirl comes out with its options, inside the
    /// carrier's box. Before, nothing ever came out. Sabotage verified (failure): the local player
    /// skipped; its invisible flag ignored.
    #[test]
    fn mobs_players_and_the_local_player_swirl() {
        let mut ents = world();
        spawn(&mut ents, 1, ZOMBIE, [5.0, 64.0, 5.0], vec![effects(vec![swirl(7, 0xff11_2233)])]);
        spawn(
            &mut ents,
            2,
            PLAYER,
            [-5.0, 70.0, 0.0],
            vec![effects(vec![swirl(8, 0x2644_5566)]), (11, Meta::Bool(true))],
        );
        spawn(&mut ents, 3, HORSE, [0.0, 64.0, 9.0], vec![]);
        ents.spawn_me(1, PLAYER as i32, [0.0; 3], 0.0);
        let me_data = vec![effects(vec![swirl(9, 0xff77_8899)]), (0, Meta::Byte(INVISIBLE_FLAG))];
        ents.apply(EntityEvent::Data { id: LOCAL, values: me_data });
        let mut r = WorldRandom::default();
        // Per particle type: count and corners reached.
        let mut seen: hashbrown::HashMap<i32, (usize, Bounds)> = Default::default();
        let mut colors = std::collections::BTreeSet::new();
        for _ in 0..30_000 {
            let local = ents.me.as_ref().map(|me| EffectBody::new(me, [100.0, 50.0, 100.0], (0.6, 1.8)));
            tick_effect_particles(&ents, local, true, &mut r, &mut |p, at| {
                let s = seen.entry(p.kind).or_insert((0, Bounds::EMPTY));
                s.0 += 1;
                s.1.widen(at);
                colors.insert((p.kind, p.color));
            });
        }
        assert_eq!(colors.into_iter().collect::<Vec<_>>(), [(7, 0xff11_2233), (8, 0x2644_5566), (9, 0xff77_8899)]);
        let count = |k: i32| seen.get(&k).map_or(0, |s| s.0);
        // Zombie 1 in 4, ambient player 1 in 20, invisible local player 1 in 15.
        for (kind, want) in [(7, 7_500.0), (8, 1_500.0), (9, 2_000.0)] {
            assert!((count(kind) as f64 - want).abs() < want * 0.1, "kind {kind}: {}, want {want}", count(kind));
        }
        assert!(seen[&7].1.within([4.7, 64.0, 4.7], [5.3, 65.95, 5.3]), "zombie box");
        assert!(seen[&8].1.within([-5.3, 70.0, -0.3], [-4.7, 71.8, 0.3]), "player box");
        assert!(seen[&9].1.within([99.7, 50.0, 99.7], [100.3, 51.8, 100.3]), "local player box");
    }

    /// Lowest and highest corner reached by the particles.
    #[derive(Clone, Copy)]
    struct Bounds {
        low: [f64; 3],
        high: [f64; 3],
    }

    impl Bounds {
        const EMPTY: Bounds = Bounds { low: [f64::MAX; 3], high: [f64::MIN; 3] };

        fn widen(&mut self, at: [f64; 3]) {
            for (i, v) in at.into_iter().enumerate() {
                (self.low[i], self.high[i]) = (self.low[i].min(v), self.high[i].max(v));
            }
        }

        /// Inside the box from `lo` to `hi`.
        fn within(&self, lo: [f64; 3], hi: [f64; 3]) -> bool {
            (0..3).all(|i| self.low[i] >= lo[i] && self.high[i] <= hi[i])
        }

        /// Reaches every face of the box from `lo` to `hi` (within a thousandth).
        fn covers(&self, lo: [f64; 3], hi: [f64; 3]) -> bool {
            (0..3).all(|i| self.low[i] - lo[i] < 1e-3 && hi[i] - self.high[i] < 1e-3)
        }
    }

    /// Frozen world: only the players and the mounts carrying one keep swirling (the local player
    /// riding a horse counts). Each carrier has its own particle type to tell them apart. Sabotage
    /// verified (failure): the frozen filter dropped.
    #[test]
    fn frozen_world_keeps_only_players_and_their_mounts() {
        let mut ents = world();
        for (id, kind) in [(1, ZOMBIE), (2, PLAYER), (3, HORSE), (4, HORSE)] {
            spawn(&mut ents, id, kind, [0.0; 3], vec![effects(vec![swirl(id, 0xffff_ffff)])]);
        }
        ents.spawn_me(1, PLAYER as i32, [0.0; 3], 0.0);
        ents.apply(EntityEvent::Passengers { vehicle: 3, passengers: vec![LOCAL] });
        let mut r = WorldRandom::default();
        for (runs, want) in [(false, vec![2, 3]), (true, vec![1, 2, 3, 4])] {
            let mut from = std::collections::BTreeSet::new();
            for _ in 0..2_000 {
                tick_effect_particles(&ents, None, runs, &mut r, &mut |p, _| {
                    from.insert(p.kind);
                });
            }
            assert_eq!(from.into_iter().collect::<Vec<_>>(), want, "world running: {runs}");
        }
    }

    /// The kept set of carriers equals the walk of every entity through spawns, effects gained and
    /// lost, removals by `apply` and elsewhere, ids spawned again with another kind, and ticks.
    /// Sabotage verified (failure): carriers not noted on data 10.
    #[test]
    fn kept_carriers_match_the_walk_of_every_entity() {
        let mut ents = world();
        let mut r = WorldRandom::default();
        let mut carriers = 0;
        for step in 0..5_000 {
            let id = below(&mut r, 60) as i32 + 1;
            match below(&mut r, 8) {
                0 | 1 => spawn(&mut ents, id, below(&mut r, KINDS.len()), [0.0; 3], vec![]),
                2 | 3 => {
                    let n = below(&mut r, 3);
                    let list = (0..n).map(|i| swirl(i as i32, 0xff00_0000)).collect();
                    ents.apply(EntityEvent::Data { id, values: vec![(0, Meta::Byte(0)), effects(list)] });
                }
                4 => ents.apply(EntityEvent::Remove(vec![id])),
                5 => drop(ents.map.remove(&id)),
                6 => ents.apply(EntityEvent::Data { id, values: vec![(11, Meta::Bool(true))] }),
                _ => ents.tick(),
            }
            let mut walked: Vec<i32> =
                ents.map.values().filter(|e| !effect_particle_list(e).is_empty()).map(|e| e.id).collect();
            let mut kept: Vec<i32> = ents.with_effect_particles().map(|e| e.id).collect();
            walked.sort_unstable();
            kept.sort_unstable();
            assert_eq!(kept, walked, "step {step}");
            carriers += kept.len();
        }
        assert!(carriers > 10_000, "{carriers}");
    }

    /// Every particle a mob effect can carry (registry ids of the extracted game) reaches the
    /// engine: tinted swirl, omens, infestation, oozing, weaving, wind charge.
    #[test]
    fn every_effect_particle_type_reaches_the_engine() {
        let data = rc_testdata::version_data().expect("version data missing (extraction)");
        let ids = &data.registry("minecraft:particle_type").expect("particle registry").entries;
        let mut ps = Particles::new(crate::particles::tests::assets(), &data);
        let names = ["entity_effect", "trial_omen", "raid_omen", "infested", "item_slime", "item_cobweb", "small_gust"];
        for name in names {
            let id = ids.iter().position(|n| n == &format!("minecraft:{name}")).expect(name) as i32;
            assert!(spawn_effect_particle(&mut ps, &swirl(id, 0x26ff_0000), [1.0, 2.0, 3.0]), "{name}");
        }
    }

    /// Micro-bench (ignored; `cargo test --release -p rc-app bench_effect_particle_pass -- --ignored
    /// --nocapture`): one tick of the pass over 1,000 mobs, 10 carrying effects, by walking every
    /// entity against the kept set.
    #[test]
    #[ignore]
    fn bench_effect_particle_pass() {
        let mut ents = world();
        for id in 0..1_000 {
            let values = if id % 100 == 0 { vec![effects(vec![swirl(0, 0xff00_00ff)])] } else { vec![] };
            spawn(&mut ents, id, ZOMBIE, [f64::from(id % 40), 64.0, f64::from(id / 40)], values);
        }
        let mut r = WorldRandom::default();
        let mut sink = 0usize;
        let mut time = |label: &str, pass: &mut dyn FnMut(&mut WorldRandom, &mut usize)| {
            let best = (0..5)
                .map(|_| {
                    let t0 = std::time::Instant::now();
                    for _ in 0..20_000 {
                        pass(&mut r, &mut sink);
                    }
                    t0.elapsed().as_secs_f64() * 1e6 / 20_000.0
                })
                .fold(f64::MAX, f64::min);
            eprintln!("{label:28} {best:8.3} µs per tick (loaded machine)");
        };
        let dims = ents.dims.clone();
        time("walk of every entity", &mut |r, sink| {
            for e in ents.map.values().filter(|e| !effect_particle_list(e).is_empty()) {
                *sink += usize::from(roll_effect_particle(&EffectBody::of_table_entity(e, &dims), r).is_some());
            }
        });
        time("kept set", &mut |r, sink| {
            let kept = ents.subsets.effect_particles.iter().filter_map(|id| ents.map.get(id));
            for e in kept.filter(|e| !effect_particle_list(e).is_empty()) {
                *sink += usize::from(roll_effect_particle(&EffectBody::of_table_entity(e, &dims), r).is_some());
            }
        });
        std::hint::black_box(sink);
    }
}
