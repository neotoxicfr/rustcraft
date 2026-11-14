//! Orientation of a redstone update under the "redstone experiments" feature, which a world may
//! enable at its creation: a front, an up and a side bias, 48 in all. Every neighbour update
//! then carries one, and the wire's second power evaluator walks its neighbours in the order it
//! gives. They are numbered like the game's, so that a draw of the level's random picks the same
//! orientation; the facts of each one are worked out once, in a table.

use std::sync::LazyLock;

use super::Dir;

/// Side an orientation turns to after its front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SideBias {
    Left = 0,
    Right = 1,
}

/// One of the 48 orientations (its number in the game's order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Orientation(u8);

const COUNT: usize = 48;

/// What an orientation is made of, and the orientations next to it.
struct Facts {
    up: Dir,
    front: Dir,
    /// Neighbour order: behind the front, the front, the side, the other side, below, above.
    neighbours: [Dir; 6],
    /// The four of them across the up axis, and the two along it, in that same order.
    horizontal: [Dir; 4],
    vertical: [Dir; 2],
    with_front: [u8; 6],
    with_up: [u8; 6],
    with_bias: [u8; 2],
}

/// The game's number of (up, front, bias): up, then the front's axis and sign, then the bias.
fn number_of(up: Dir, front: Dir, bias: SideBias) -> u8 {
    assert_ne!(up.axis(), front.axis(), "up and front on the same axis");
    let front_axis_key = if up.axis() == 1 { u8::from(front.axis() == 0) } else { u8::from(front.axis() == 1) };
    let negative = front.step().iter().sum::<i32>() < 0;
    let front_key = (front_axis_key << 1) | u8::from(negative);
    ((((up as u8) << 2) + front_key) << 1) + bias as u8
}

/// The side: the cross product of the front and the up (to the right), or its opposite.
fn side_of(up: Dir, front: Dir, bias: SideBias) -> Dir {
    let (f, u) = (front.step(), up.step());
    let right = [f[1] * u[2] - f[2] * u[1], f[2] * u[0] - f[0] * u[2], f[0] * u[1] - f[1] * u[0]];
    let side = Dir::ALL.into_iter().find(|d| d.step() == right).expect("front and up are perpendicular");
    if bias == SideBias::Right { side } else { side.opposite() }
}

fn facts_of(up: Dir, front: Dir, bias: SideBias) -> Facts {
    let side = side_of(up, front, bias);
    let neighbours = [front.opposite(), front, side, side.opposite(), up.opposite(), up];
    let mut horizontal = [front; 4];
    let mut vertical = [up; 2];
    let (mut h, mut v) = (0, 0);
    for d in neighbours {
        if d.axis() == up.axis() {
            vertical[v] = d;
            v += 1;
        } else {
            horizontal[h] = d;
            h += 1;
        }
    }
    // Turned to face `facing`: an up along that axis moves to the old front's side.
    let with_front = Dir::ALL.map(|facing| {
        let new_up = if facing == up {
            front.opposite()
        } else if facing == up.opposite() {
            front
        } else {
            up
        };
        number_of(new_up, facing, bias)
    });
    let with_up = Dir::ALL.map(|facing| {
        let new_front = if facing == front {
            up.opposite()
        } else if facing == front.opposite() {
            up
        } else {
            front
        };
        number_of(facing, new_front, bias)
    });
    let with_bias = [SideBias::Left, SideBias::Right].map(|b| number_of(up, front, b));
    Facts { up, front, neighbours, horizontal, vertical, with_front, with_up, with_bias }
}

static TABLE: LazyLock<Vec<Facts>> = LazyLock::new(|| {
    let mut table: Vec<Option<Facts>> = (0..COUNT).map(|_| None).collect();
    for up in Dir::ALL {
        for front in Dir::ALL.into_iter().filter(|f| f.axis() != up.axis()) {
            for bias in [SideBias::Left, SideBias::Right] {
                table[usize::from(number_of(up, front, bias))] = Some(facts_of(up, front, bias));
            }
        }
    }
    table.into_iter().map(|f| f.expect("48 distinct numbers")).collect()
});

impl Orientation {
    fn facts(self) -> &'static Facts {
        &TABLE[usize::from(self.0)]
    }

    /// The orientation numbered `n` in the game's order (below 48).
    pub(crate) fn from_number(n: u8) -> Orientation {
        assert!(usize::from(n) < COUNT, "orientation number {n}");
        Orientation(n)
    }

    pub(crate) fn number(self) -> u8 {
        self.0
    }

    /// One of the 48 drawn from `rng` (one bounded draw), like the game's draw on the level's
    /// random.
    pub(crate) fn random(rng: &mut super::JavaRandom) -> Orientation {
        Orientation(rng.next_int_bound(COUNT as i32) as u8)
    }

    pub(crate) fn of(up: Dir, front: Dir, bias: SideBias) -> Orientation {
        Orientation(number_of(up, front, bias))
    }

    pub(crate) fn up(self) -> Dir {
        self.facts().up
    }

    pub(crate) fn front(self) -> Dir {
        self.facts().front
    }

    pub(crate) fn neighbours(self) -> [Dir; 6] {
        self.facts().neighbours
    }

    pub(crate) fn horizontal(self) -> [Dir; 4] {
        self.facts().horizontal
    }

    pub(crate) fn vertical(self) -> [Dir; 2] {
        self.facts().vertical
    }

    pub(crate) fn with_front(self, front: Dir) -> Orientation {
        Orientation(self.facts().with_front[front as usize])
    }

    pub(crate) fn with_up(self, up: Dir) -> Orientation {
        Orientation(self.facts().with_up[up as usize])
    }

    pub(crate) fn with_side_bias(self, bias: SideBias) -> Orientation {
        Orientation(self.facts().with_bias[bias as usize])
    }

    /// Turned to face `front`, unless `front` lies along its up axis.
    pub(crate) fn with_front_keeping_up(self, front: Dir) -> Orientation {
        if front.axis() == self.up().axis() { self } else { self.with_front(front) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Dir::*;

    /// Numbers of the game's order (worked out by hand from its rule): up then front then bias.
    #[test]
    fn numbers_follow_the_game_order() {
        assert_eq!(Orientation::of(Down, South, SideBias::Left).number(), 0);
        assert_eq!(Orientation::of(Down, North, SideBias::Right).number(), 3);
        assert_eq!(Orientation::of(Down, East, SideBias::Left).number(), 4);
        assert_eq!(Orientation::of(Up, North, SideBias::Left).number(), 10);
        assert_eq!(Orientation::of(North, Up, SideBias::Left).number(), 20);
        assert_eq!(Orientation::of(East, North, SideBias::Right).number(), 43);
        let all: std::collections::HashSet<u8> = (0..48).map(|n| Orientation::from_number(n).number()).collect();
        assert_eq!(all.len(), 48);
    }

    /// Up, facing north, bias left: right of north is east, so the side is west; neighbours
    /// behind, front, side, other side, below, above.
    #[test]
    fn neighbour_order_of_one_orientation() {
        let o = Orientation::of(Up, North, SideBias::Left);
        assert_eq!(o.neighbours(), [South, North, West, East, Down, Up]);
        assert_eq!(o.horizontal(), [South, North, West, East]);
        assert_eq!(o.vertical(), [Down, Up]);
        assert_eq!(o.with_side_bias(SideBias::Right).neighbours()[2], East);
    }

    /// Turning keeps a valid pair: facing up moves the up behind the old front, and back.
    #[test]
    fn turning_rules() {
        let o = Orientation::of(Up, North, SideBias::Left);
        let facing_up = o.with_front(Up);
        assert_eq!((facing_up.up(), facing_up.front()), (South, Up));
        assert_eq!(o.with_front(East), Orientation::of(Up, East, SideBias::Left));
        let up_north = o.with_up(North);
        assert_eq!((up_north.up(), up_north.front()), (North, Down));
        assert_eq!(o.with_front_keeping_up(Down), o);
        for n in 0..48 {
            let o = Orientation::from_number(n);
            for d in Dir::ALL {
                assert_ne!(o.with_front(d).up().axis(), d.axis(), "{n} facing {d:?}");
                assert_eq!(o.with_front(d).front(), d);
                assert_eq!(o.with_up(d).up(), d);
            }
        }
    }
}
