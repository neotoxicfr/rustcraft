//! Equivalence proof of `RUSTCRAFT_OPT_BLOCKTINTLAZY`: a moving block (pushed by a piston, piston
//! head and retracting base included, falling, transient) of each biome-tinted state and of
//! untinted states places the same instances and matrices, colours included, whether its biome
//! colours are always read or only for a biome-tinted state; the tinted states carry the expected
//! biome colour, the others never read it. 26.3 data and game, windowless GPU; without them, test
//! skipped (failure under `RUSTCRAFT_OPT_REQUIRE=1`).

use std::cell::Cell;

use super::tests_g2f03::{Kit, kit};
use super::*;
use crate::block_entities::BlockEntities;

/// Biome colours handed to the moving blocks (grass, foliage, dry foliage, water).
const BIOME: [u32; 4] = [0x11_2233, 0x44_5566, 0x77_8899, 0xaa_bbcc];

/// Instances (source, first vertex, count, bytes) and matrices placed by a frame.
type Placed = (Vec<(u8, u32, u32, Vec<u8>)>, Vec<Vec<u8>>);

fn placed(r: &EntityRenderer) -> Placed {
    let inst = r.instances.iter().flatten().map(|(s, f, c, i)| (*s, *f, *c, bytemuck::bytes_of(i).to_vec())).collect();
    let mats = r.mats.iter().map(|m| bytemuck::bytes_of(m).to_vec()).collect();
    (inst, mats)
}

fn clear(r: &mut EntityRenderer) {
    r.instances.iter_mut().for_each(Vec::clear);
    r.mats.clear();
}

fn state_with(k: &Kit, block: &str, props: &[(&str, &str)]) -> u32 {
    let b = k.r.source.blocks.iter().find(|b| b.name == format!("minecraft:{block}")).expect(block);
    (b.first_state..b.first_state + b.state_count)
        .find(|&s| props.iter().all(|(n, v)| k.r.source.state_properties(s).any(|(a, b)| a == *n && b == *v)))
        .unwrap_or_else(|| panic!("{block} {props:?}"))
}

/// Biome colour slots (0 grass … 3 water) read by the layers of `state`, from the raw kinds.
fn biome_slots(k: &Kit, state: u32) -> Vec<usize> {
    use rc_data::tint::{DRY_FOLIAGE, FOLIAGE, GRASS, WATER};
    let kinds = k.r.source.client_states[state as usize].tint_kind;
    [GRASS, FOLIAGE, DRY_FOLIAGE, WATER].iter().enumerate().filter(|(_, k)| kinds.contains(k)).map(|(i, _)| i).collect()
}

/// Biome colours that count their reads.
fn counting(reads: &Cell<u32>) -> impl Fn(i32, i32, i32) -> [u32; 4] + '_ {
    move |_, _, _| {
        reads.set(reads.get() + 1);
        BIOME
    }
}

fn draw_transient(k: &mut Kit, state: u32) -> (Placed, u32) {
    let reads = Cell::new(0);
    clear(&mut k.r);
    k.r.push_transient(std::iter::once(([0, 64, 0], state)), [0, 0], &|_, _, _| (15, 15), &counting(&reads));
    (placed(&k.r), reads.get())
}

fn draw_falling(k: &mut Kit, state: u32) -> (Placed, u32) {
    let names = k.r.source.registry("minecraft:entity_type").map(|r| r.entries.clone()).unwrap_or_default();
    let kind = names.iter().position(|n| n == "minecraft:falling_block").expect("falling_block") as i32;
    let mut all = crate::entities::Entities { type_names: names, ..Default::default() };
    let add = rc_proto_v777::entity::AddEntity {
        id: 7,
        uuid: 1007,
        kind,
        pos: [0.5, 64.0, 0.5],
        velocity: [0.0; 3],
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        data: state as i32,
    };
    all.apply(rc_client::EntityEvent::Add(Box::new(add)));
    let e = all.map.remove(&7).expect("entity added");
    let reads = Cell::new(0);
    let biome = counting(&reads);
    let world = ShadowWorld { state: &|_, _, _| 0, brightness: &|_, _, _| 15, ambient: 0.0, biome_tints: &biome };
    k.r.prepare(
        std::iter::once(&e),
        0.0,
        [0, 0],
        [0.5, 65.0, 5.5],
        &|_, _, _| (15, 15),
        &crate::render::FrameVisibility::everything(),
        None,
        &world,
    );
    (placed(&k.r), reads.get())
}

/// Piston-moved block at (0, 64, 0) carrying `moved`, at the start of its course.
fn draw_pushed(k: &mut Kit, moved: u32, extending: bool, source: bool) -> (Placed, u32) {
    let mut bes = BlockEntities::new(k.r.source.clone());
    let moving = state_with(k, "moving_piston", &[("facing", "east")]);
    bes.on_block_change([0, 64, 0], 0, moving);
    bes.set_mover([0, 64, 0], rc_sim::MovingBlock { moved, dir: 5, extending, source });
    let reads = Cell::new(0);
    let biome = counting(&reads);
    clear(&mut k.r);
    let frame = BlockFrame {
        pt: 0.0,
        origin: [0, 0],
        cam: [0.5, 65.0, 5.5],
        game_time: 0,
        min_y: -64,
        light: &|_, _, _| (15, 15),
        visible: &crate::render::FrameVisibility::everything(),
        items: None,
        render_distance: 12,
        top: 320,
        state: &|_, _, _| 0,
        scoping: false,
        biome_tints: &biome,
    };
    k.r.push_block_entities(&bes, &frame);
    (placed(&k.r), reads.get())
}

/// Draws with both paths; returns the eager frame after checking that the lazy one is the same
/// and that the lazy one read the biome only for a tinted state.
fn same_both_ways(k: &mut Kit, what: &str, tinted: bool, draw: &dyn Fn(&mut Kit) -> (Placed, u32)) -> Placed {
    k.r.opt.block_tint_lazy = false;
    let (eager, eager_reads) = draw(k);
    k.r.opt.block_tint_lazy = true;
    let (lazy, lazy_reads) = draw(k);
    assert!(!eager.0.is_empty(), "{what}: nothing drawn");
    assert_eq!(eager_reads, 1, "{what}: eager path reads the biome once");
    assert_eq!(lazy_reads, u32::from(tinted), "{what}: lazy path reads the biome only when tinted");
    assert_eq!(lazy, eager, "{what}: instances differ between the lazy and eager paths");
    eager
}

/// RGBA instance colour of a biome colour.
fn rgba(c: u32) -> u32 {
    let c = 0xff00_0000 | c;
    (c >> 16 & 0xff) | (c & 0xff00) | (c & 0xff) << 16 | (c & 0xff00_0000)
}

fn colours(p: &Placed) -> Vec<u32> {
    p.0.iter().map(|(_, _, _, b)| bytemuck::pod_read_unaligned::<Instance>(b).color).collect()
}

#[test]
fn moving_blocks_same_with_lazy_biome_tint() {
    let Some(mut k) = kit() else { return };
    let tinted_cases: [(&str, &[(&str, &str)]); 6] = [
        ("grass_block", &[("snowy", "false")]),
        ("oak_leaves", &[]),
        ("vine", &[("north", "true")]),
        ("leaf_litter", &[("segment_amount", "4")]),
        ("water_cauldron", &[("level", "3")]),
        ("sugar_cane", &[]),
    ];
    let untinted_cases: [(&str, &[(&str, &str)]); 4] =
        [("stone", &[]), ("lily_pad", &[]), ("sand", &[]), ("piston", &[("facing", "east"), ("extended", "true")])];
    let mut slots_seen = [false; 4];
    let lazy_was = k.r.opt.block_tint_lazy;
    for (block, props) in tinted_cases.iter().chain(&untinted_cases) {
        let s = state_with(&k, block, props);
        let slots = biome_slots(&k, s);
        let tinted = !slots.is_empty();
        assert_eq!(tinted, tinted_cases.iter().any(|c| c.0 == *block), "{block}: biome tint kinds {slots:?}");
        for &i in &slots {
            slots_seen[i] = true;
        }
        let frames = [
            same_both_ways(&mut k, &format!("transient {block}"), tinted, &|k| draw_transient(k, s)),
            same_both_ways(&mut k, &format!("falling {block}"), tinted, &|k| draw_falling(k, s)),
            same_both_ways(&mut k, &format!("pushed {block}"), tinted, &|k| draw_pushed(k, s, true, false)),
        ];
        for f in &frames {
            let c = colours(f);
            if tinted {
                assert!(slots.iter().any(|&i| c.contains(&rgba(BIOME[i]))), "{block}: biome colour missing in {c:x?}");
            } else {
                assert!(BIOME.iter().all(|&b| !c.contains(&rgba(b))), "{block}: biome colour on an untinted state");
            }
        }
    }
    assert_eq!(slots_seen, [true; 4], "grass, foliage, dry foliage and water each covered");
    // Piston head (extending source) and retracting base with its stationary extended base.
    let head = state_with(&k, "piston_head", &[("facing", "east"), ("type", "sticky"), ("short", "false")]);
    let base = state_with(&k, "sticky_piston", &[("facing", "east"), ("extended", "false")]);
    assert!(k.r.piston_states.head_of(head).is_some() && k.r.piston_states.base(base).is_some(), "piston states");
    same_both_ways(&mut k, "piston head", false, &|k| draw_pushed(k, head, true, true));
    let retracting = same_both_ways(&mut k, "retracting base", false, &|k| draw_pushed(k, base, false, true));
    assert!(retracting.1.len() >= 2, "head and base both drawn");
    k.r.opt.block_tint_lazy = lazy_was;
}
