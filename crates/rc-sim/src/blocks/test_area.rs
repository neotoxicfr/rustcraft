//! Test area on a copy of the solo test world (`RUSTCRAFT_SOLO_WORLD`): a stone floor at
//! y = -60 under air over x, z = 100..131, game commands, ticks that keep the events sent.
//! Without the test world the tests are skipped (`RUSTCRAFT_OPT_REQUIRE=1`: they fail).

use rc_client::Event;

use super::P;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;

pub(super) struct Area {
    pub(super) p: Probe,
}

pub(super) fn open(tag: &str) -> Option<Area> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n");
    assert_eq!(state_string(&p.s, p.s.get([100, -60, 100])), "minecraft:stone", "test area missing");
    p.run("player 100.5 -59 131.5 0 0\n");
    let _ = p.take_events();
    Some(Area { p })
}

impl Area {
    pub(super) fn set(&mut self, at: P, state: &str) {
        self.p.run(&format!("setblock {} {} {} {state}\n", at[0], at[1], at[2]));
    }

    pub(super) fn name(&self, at: P) -> String {
        state_string(&self.p.s, self.p.s.get(at))
    }

    /// Real player command (full parsing and execution); fails the test if refused.
    pub(super) fn cmd(&mut self, text: &str) {
        let src = self.p.s.player_source();
        let (ok, _) = self.p.s.perform(src, text);
        assert!(ok, "command refused: {text}");
    }

    /// `n` block ticks (time, scheduled and block entity phases), with the events sent.
    pub(super) fn ticks(&mut self, n: i32) -> Vec<Event> {
        let mut out = Vec::new();
        for _ in 0..n {
            self.p.s.time += 1;
            for c in &mut self.p.s.clocks {
                c.1 += 1;
            }
            self.p.s.tick_world_blocks();
            self.p.s.flush_block_changes();
            out.extend(self.p.take_events());
        }
        out
    }

    /// `n` whole server ticks (entities included), with the events sent.
    pub(super) fn game_ticks(&mut self, n: i32) -> Vec<Event> {
        let mut out = Vec::new();
        for _ in 0..n {
            self.p.s.game_tick();
            self.p.s.tick += 1;
            self.p.s.flush_block_changes();
            out.extend(self.p.take_events());
        }
        out
    }

    /// Items `id` lying on the ground (count summed).
    pub(super) fn dropped(&self, id: &str) -> i32 {
        self.p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == id).map(|d| d.stack.count).sum()
    }

    /// Container slots at `at` rewritten (`(slot, item, count)`).
    pub(super) fn put(&mut self, at: P, items: &[(usize, &str, i32)]) {
        let mut slots = self.p.s.slots(at);
        for &(i, id, n) in items {
            slots[i] = Some(Stack { id: id.into(), count: n, components: None }.to_nbt(None));
        }
        self.p.s.store_slots(at, &slots);
    }

    /// Count of item `id` in the container at `at`.
    pub(super) fn held(&self, at: P, id: &str) -> i32 {
        self.p.s.be_item_counts(at).iter().filter(|e| e.0 == id).map(|e| e.1).sum()
    }

    /// Particles `name` among `evs`.
    pub(super) fn particles(&self, evs: &[Event], name: &str) -> usize {
        let kind = self.p.s.data.registry("minecraft:particle_type").and_then(|r| r.entries.iter().position(|e| e == name));
        let kind = kind.unwrap_or_else(|| panic!("particle {name}")) as i32;
        evs.iter().filter(|e| matches!(e, Event::Particles(p) if p.particle.kind == kind)).count()
    }
}

/// Level events among `evs` as `(kind, data)`.
pub(super) fn level_events(evs: &[Event]) -> Vec<(i32, i32)> {
    evs.iter()
        .filter_map(|e| match e {
            Event::LevelEvent { kind, data, .. } => Some((*kind, *data)),
            _ => None,
        })
        .collect()
}
