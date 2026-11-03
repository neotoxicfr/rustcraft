//! Column meshes drawn at random (or made by hand) for the tests of the render submodules.

use rc_mesh::blocks::BlockMesh;
use rc_mesh::fluid::FluidQuad;

/// Horizontal quad at y = h on cell (x, z), top (counter-clockwise seen from above) or bottom.
pub(super) fn flat(x: f32, z: f32, h: f32, up: bool) -> FluidQuad {
    let mut c =
        [[x, h, z, 0.0, 0.0], [x, h, z + 1.0, 0.0, 0.0], [x + 1.0, h, z + 1.0, 0.0, 0.0], [x + 1.0, h, z, 0.0, 0.0]];
    if !up {
        c.reverse();
    }
    FluidQuad { corners: c, texture: 0, colors: [0; 4], light: [0; 4] }
}

/// Randomly drawn mesh: faces of the three layers, fluids, free translucents (lake
/// surface) one time in two, otherwise arbitrary.
pub(super) fn random_mesh(seed: &mut u64, scale: u64) -> BlockMesh {
    let mut next = |n: u64| {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed % n.max(1)
    };
    let faces = |next: &mut dyn FnMut(u64) -> u64| -> Vec<rc_mesh::blocks::Face> {
        (0..next(300 * scale)).map(|_| std::array::from_fn(|_| next(1 << 20) as u32)).collect()
    };
    let quad = |next: &mut dyn FnMut(u64) -> u64| FluidQuad {
        corners: std::array::from_fn(|_| std::array::from_fn(|_| next(64) as f32 * 0.25)),
        texture: next(900) as u32,
        colors: std::array::from_fn(|_| next(u64::from(u32::MAX)) as u32),
        light: std::array::from_fn(|_| next(0xffff) as u16),
    };
    let layers = [faces(&mut next), faces(&mut next), faces(&mut next)];
    let fl0: Vec<FluidQuad> = (0..next(10)).map(|_| quad(&mut next)).collect();
    let fl1: Vec<FluidQuad> = (0..next(10)).map(|_| quad(&mut next)).collect();
    let trans: Vec<FluidQuad> = if next(2) == 0 {
        (0..next(40)).map(|i| flat((i % 16) as f32, (i / 16) as f32, 14.875, true)).collect()
    } else {
        (0..next(40)).map(|_| quad(&mut next)).collect()
    };
    let n = trans.len() as u32;
    BlockMesh {
        x: next(200) as i32 - 100,
        z: next(200) as i32 - 100,
        version: 1,
        layers,
        fluids: [fl0, fl1, trans],
        starts: [vec![0], vec![0], vec![0]],
        fluid_starts: [vec![0, 0], vec![0, 0], vec![0, n]],
        vis: vec![0; 1],
    }
}
