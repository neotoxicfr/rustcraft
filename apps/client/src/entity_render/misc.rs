//! Transient blocks (`docs/specs/miscellaneous.md` § 5.5): drawn like a falling block,
//! at their position, until the meshing of their section takes them over.

use glam::{Mat4, Vec3};

use super::EntityRenderer;
use super::objects::BlockTint;

impl EntityRenderer {
    /// After `prepare` and the block entities: `(position, state)` blocks as world instances,
    /// tinted with the biome colours of their position (`biome_tints`, like a falling block).
    pub fn push_transient(
        &mut self,
        blocks: impl Iterator<Item = ([i32; 3], u32)>,
        origin: [i32; 2],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
        biome_tints: &dyn Fn(i32, i32, i32) -> [u32; 4],
    ) {
        let mut any = false;
        for (p, state) in blocks {
            let corner = Vec3::new((p[0] - origin[0]) as f32, p[1] as f32, (p[2] - origin[1]) as f32);
            let (b, s) = light(p[0], p[1], p[2]);
            let packed = (u32::from(b) * 16) | ((u32::from(s) * 16) << 8);
            let tint = BlockTint::World(biome_tints(p[0], p[1], p[2]));
            any |= self.push_block_tinted(state, Mat4::from_translation(corner), packed, 10 << 16, tint);
        }
        if any {
            self.hand_from = std::array::from_fn(|i| self.instances[i].len());
        }
    }
}
