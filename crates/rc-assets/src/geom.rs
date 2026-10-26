//! Small 3D geometry of the models: directions, 3×3 matrices, rotations.

pub type V3 = [f32; 3];
/// 3×3 matrix by rows: `m[row][column]`.
pub type M3 = [[f32; 3]; 3];

pub const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Directions in the official order: down, up, north, south, west, east.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

crate::codec_enum!(Dir { Down, Up, North, South, West, East });

impl Dir {
    pub const ALL: [Dir; 6] = [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];

    pub fn normal(self) -> V3 {
        match self {
            Dir::Down => [0.0, -1.0, 0.0],
            Dir::Up => [0.0, 1.0, 0.0],
            Dir::North => [0.0, 0.0, -1.0],
            Dir::South => [0.0, 0.0, 1.0],
            Dir::West => [-1.0, 0.0, 0.0],
            Dir::East => [1.0, 0.0, 0.0],
        }
    }

    pub fn offset(self) -> [i32; 3] {
        let n = self.normal();
        [n[0] as i32, n[1] as i32, n[2] as i32]
    }

    pub fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "down" | "bottom" => Dir::Down,
            "up" | "top" => Dir::Up,
            "north" => Dir::North,
            "south" => Dir::South,
            "west" => Dir::West,
            "east" => Dir::East,
            _ => return None,
        })
    }

    /// Direction closest to a vector (first found in case of a tie, official order).
    pub fn nearest(v: V3) -> Option<Dir> {
        if !v.iter().all(|c| c.is_finite()) {
            return None;
        }
        let mut best = None;
        let mut best_dot = 0.0f32;
        for d in Dir::ALL {
            let dot = dot(v, d.normal());
            if dot > best_dot {
                best_dot = dot;
                best = Some(d);
            }
        }
        best
    }
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn mul_v(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

pub fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

pub fn transpose(m: &M3) -> M3 {
    [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]]
}

/// Rotation by angle `deg` (degrees, counter-clockwise) around a unit axis.
pub fn rotation(axis: V3, deg: f32) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = axis;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// Rotation by a multiple of 90°: coefficients rounded exactly to −1, 0 or 1.
pub fn rotation_90(axis: V3, deg: i32) -> M3 {
    let mut m = rotation(axis, deg as f32);
    for row in &mut m {
        for c in row {
            *c = c.round();
        }
    }
    m
}

pub const X: V3 = [1.0, 0.0, 0.0];
pub const Y: V3 = [0.0, 1.0, 0.0];
pub const Z: V3 = [0.0, 0.0, 1.0];

/// Variant rotation: `x`, `y`, `z` in degrees (0/90/180/270), applied X then Y then
/// Z; each quarter turn is −90° around the axis (official convention).
pub fn variant_rotation(x: i32, y: i32, z: i32) -> M3 {
    let rx = rotation_90(X, -x);
    let ry = rotation_90(Y, -y);
    let rz = rotation_90(Z, -z);
    mul(&rz, &mul(&ry, &rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_quarter_turns() {
        // y: 90 turns north towards east.
        let m = variant_rotation(0, 90, 0);
        assert_eq!(Dir::nearest(mul_v(&m, Dir::North.normal())), Some(Dir::East));
        // x: 90 sends up towards north.
        let m = variant_rotation(90, 0, 0);
        assert_eq!(Dir::nearest(mul_v(&m, Dir::Up.normal())), Some(Dir::North));
        // X first, then Y.
        let m = variant_rotation(90, 90, 0);
        assert_eq!(Dir::nearest(mul_v(&m, Dir::Up.normal())), Some(Dir::East));
    }
}
