//! Allocations of a movement: none once the thread's buffers have grown
//! (`RUSTCRAFT_OPT_RESOLVE_NO_COPY`, default), against a wall with step-up, the world border and a
//! solid entity in reach. The old path (`RUSTCRAFT_OPT_RESOLVE_NO_COPY=0`) allocates at every call:
//! run under it, this test fails. Per-thread counter: the other tests of the binary do not skew it.

// Counting allocator: delegates to the system (same precedent as `rc-env/tests/noalloc.rs`).
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use rc_physics::{Aabb, World, WorldBorder, collide, entity_boxes_in};

struct Counting;

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        COUNT.with(|c| c.set(c.get() + 1));
        // SAFETY: same contract as the caller's, passed on unchanged.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: `p` comes from this allocator with the same layout.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        COUNT.with(|c| c.set(c.get() + 1));
        // SAFETY: `p` comes from this allocator with layout `l`.
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> u64 {
    COUNT.with(Cell::get)
}

/// Full ground under y = 0, a row of bottom slabs along x = 2, a border of 12 blocks around the
/// origin and a boat-sized solid entity on the ground.
struct Yard {
    border: WorldBorder,
    entities: Vec<Aabb>,
}

impl World for Yard {
    fn state(&self, x: i32, y: i32, _z: i32) -> Option<u32> {
        Some(u32::from(y < 0 || (x == 2 && y == 0)))
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        if y < 0 {
            out.push(Aabb::UNIT);
        } else if self.state(x, y, z) == Some(1) {
            out.push(Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] });
        }
        false
    }
    fn border(&self) -> Option<&WorldBorder> {
        Some(&self.border)
    }
    fn entity_colliders(&self, area: &Aabb, out: &mut Vec<Aabb>) {
        entity_boxes_in(&self.entities, area, out);
    }
    fn shape_reach(&self, _: [i32; 3], _: [i32; 3]) -> Aabb {
        Aabb::UNIT
    }
}

fn yard() -> Yard {
    let mut border = WorldBorder::default();
    border.set_size(12.0);
    Yard { border, entities: vec![Aabb { min: [-2.0, 0.0, 0.5], max: [-0.6, 0.5625, 1.9] }] }
}

#[test]
fn resolve_allocates_nothing_once_warm() {
    let w = yard();
    // A player walking onto the slabs (step-up), along the boat, near the border.
    let starts = [
        Aabb { min: [1.3, 0.0, 0.2], max: [1.9, 1.8, 0.8] },
        Aabb { min: [-0.5, 0.0, 0.7], max: [0.1, 1.8, 1.3] },
        Aabb { min: [4.9, 0.0, 4.9], max: [5.5, 1.8, 5.5] },
    ];
    let deltas = [[0.3, -0.08, 0.0], [-0.3, -0.08, 0.0], [0.3, -0.08, 0.3]];
    let run = || {
        let mut moved = 0.0;
        for b in &starts {
            for d in deltas {
                moved += collide::resolve(&w, b, d, true, 0.6).iter().sum::<f64>();
            }
        }
        moved
    };
    run();
    let before = allocs();
    for _ in 0..100 {
        std::hint::black_box(run());
    }
    assert_eq!(allocs() - before, 0, "allocations of 900 movements once warm");
    // The step-up happened (onto the slabs), so its heights buffer was exercised.
    let up = collide::resolve(&w, &starts[0], [0.3, -0.08, 0.0], true, 0.6);
    assert!(up[1] > 0.4, "step-up onto the slabs: {up:?}");
    // The counter sees allocations: a collection into a new buffer allocates.
    let before = allocs();
    let mut fresh = Vec::new();
    collide::collect(&w, &starts[0].expand_towards([0.3, -0.08, 0.0]), &mut fresh);
    assert!(!fresh.is_empty() && allocs() > before);
}
