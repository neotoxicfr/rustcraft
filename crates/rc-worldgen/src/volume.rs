//! Sampling grid (spec § 3.1): buffer of index `y + (x + z × size_x) × size_y`.

use crate::math::{floor_div, floor_mod};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Volume {
    pub sx: i32,
    pub sy: i32,
    pub sz: i32,
    pub x0: i32,
    pub y0: i32,
    pub z0: i32,
    pub stx: i32,
    pub sty: i32,
    pub stz: i32,
}

impl Volume {
    /// Step 1.
    pub fn new(sx: i32, sy: i32, sz: i32, x0: i32, y0: i32, z0: i32) -> Self {
        Self::stepped(sx, sy, sz, x0, y0, z0, 1, 1, 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stepped(sx: i32, sy: i32, sz: i32, x0: i32, y0: i32, z0: i32, stx: i32, sty: i32, stz: i32) -> Self {
        assert!(sx > 0 && sy > 0 && sz > 0, "positive size required");
        assert!(stx > 0 && sty > 0 && stz > 0, "positive step required");
        Volume { sx, sy, sz, x0, y0, z0, stx, sty, stz }
    }

    #[inline]
    pub fn index(&self, ix: i32, iy: i32, iz: i32) -> usize {
        (iy + (ix + iz * self.sx) * self.sy) as usize
    }

    #[inline]
    pub fn bx(&self, i: i32) -> i32 {
        self.x0 + i * self.stx
    }

    #[inline]
    pub fn by(&self, i: i32) -> i32 {
        self.y0 + i * self.sty
    }

    #[inline]
    pub fn bz(&self, i: i32) -> i32 {
        self.z0 + i * self.stz
    }

    pub fn max_x(&self) -> i32 {
        self.x0 + self.sx * self.stx - 1
    }

    pub fn max_y(&self) -> i32 {
        self.y0 + self.sy * self.sty - 1
    }

    pub fn max_z(&self) -> i32 {
        self.z0 + self.sz * self.stz - 1
    }

    pub fn size(&self) -> usize {
        (self.sx * self.sy * self.sz) as usize
    }

    /// Index of the block in the volume, if it is part of it (`RUSTCRAFT_OPT_VOL_POW2=0`: divisions
    /// for all the steps).
    #[inline]
    pub fn index_of_block(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        self.index_of_block_with(crate::opt::vol_pow2(), x, y, z)
    }

    /// [`Volume::index_of_block`] through the chosen path: `pow2` = steps powers of 2 by shift
    /// and mask (the differences are then positive, remainder and quotient in y are exact).
    #[inline]
    pub fn index_of_block_with(&self, pow2: bool, x: i32, y: i32, z: i32) -> Option<usize> {
        let (rx, ry, rz) = (x.wrapping_sub(self.x0), y.wrapping_sub(self.y0), z.wrapping_sub(self.z0));
        if pow2
            && (self.stx | self.sty | self.stz) != 1
            && self.stx & (self.stx - 1) == 0
            && self.sty & (self.sty - 1) == 0
            && self.stz & (self.stz - 1) == 0
        {
            if rx >= 0
                && ry >= 0
                && rz >= 0
                && (rx & (self.stx - 1)) | (ry & (self.sty - 1)) | (rz & (self.stz - 1)) == 0
            {
                let (ix, iy, iz) =
                    (rx >> self.stx.trailing_zeros(), ry >> self.sty.trailing_zeros(), rz >> self.stz.trailing_zeros());
                if ix < self.sx && iy < self.sy && iz < self.sz {
                    return Some(self.index(ix, iy, iz));
                }
            }
            return None;
        }
        if self.stx == 1 && self.sty == 1 && self.stz == 1 {
            if rx >= 0 && ry >= 0 && rz >= 0 && rx < self.sx && ry < self.sy && rz < self.sz {
                return Some(self.index(rx, ry, rz));
            }
        } else if rx >= 0
            && ry >= 0
            && rz >= 0
            && rx < self.sx * self.stx
            && ry < self.sy * self.sty
            && rz < self.sz * self.stz
            && floor_mod(rx, self.stx) == 0
            && floor_mod(ry, self.sty) == 0
            && floor_mod(rz, self.stz) == 0
        {
            return Some(self.index(floor_div(rx, self.stx), floor_div(ry, self.sty), floor_div(rz, self.stz)));
        }
        None
    }
}
