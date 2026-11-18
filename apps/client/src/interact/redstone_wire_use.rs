//! Redstone wire clicked by the main hand: a dot becomes a cross and a cross goes back to the
//! shape its neighbours give it. The game's client runs that click itself (the server's answer
//! then settles it), so the wire's connection rule is read here on the client's world.

use rc_data::VersionData;
use rc_data::model::sim_bits;

/// Horizontal sides of a wire: property, offset, index of the opposite face in the `sturdy`
/// bits (down, up, north, south, west, east).
const SIDES: [(&str, [i32; 3], u8); 4] =
    [("north", [0, 0, -1], 3), ("east", [1, 0, 0], 4), ("south", [0, 0, 1], 2), ("west", [-1, 0, 0], 5)];

/// Connection of a wire on one side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Connection {
    None,
    Side,
    Up,
}

impl Connection {
    fn parse(v: Option<&str>) -> Connection {
        match v {
            Some("side") => Connection::Side,
            Some("up") => Connection::Up,
            _ => Connection::None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Connection::None => "none",
            Connection::Side => "side",
            Connection::Up => "up",
        }
    }

    fn connected(self) -> bool {
        self != Connection::None
    }
}

/// The client's world as the wire rule reads it.
struct Wire<'a, F: Fn([i32; 3]) -> u32> {
    data: &'a VersionData,
    state_at: F,
}

impl<F: Fn([i32; 3]) -> u32> Wire<'_, F> {
    fn name(&self, s: u32) -> &str {
        self.data.block_of(s).map_or("", |b| b.name.as_str())
    }

    fn prop(&self, s: u32, key: &str) -> Option<&str> {
        self.data.state_properties(s).find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    fn sim(&self, s: u32, bit: u16) -> bool {
        self.data.sim.get(s as usize).is_some_and(|b| b & bit != 0)
    }

    fn sturdy(&self, s: u32, face: u8) -> bool {
        self.data.state(s).is_some_and(|i| i.sturdy & (1 << face) != 0)
    }

    /// A wire beside `s` turns towards it (`side`: the property of the side it lies on, `None`
    /// for the block above or below a neighbour).
    fn attracts(&self, s: u32, side: Option<&str>) -> bool {
        match self.name(s) {
            "minecraft:redstone_wire" => true,
            "minecraft:repeater" => {
                let facing = self.prop(s, "facing");
                side.is_some_and(|d| facing == Some(d) || facing == Some(opposite(d)))
            }
            "minecraft:observer" => side.is_some() && self.prop(s, "facing") == side,
            _ => self.sim(s, sim_bits::SIGNAL_SOURCE) && side.is_some(),
        }
    }

    /// Connection of the wire at `pos` towards side `i` of [`SIDES`] (`can_climb`: nothing
    /// conducting above the wire).
    fn connection(&self, pos: [i32; 3], i: usize, can_climb: bool) -> Connection {
        let (name, d, opposite_face) = SIDES[i];
        let at = |dy: i32| (self.state_at)([pos[0] + d[0], pos[1] + d[1] + dy, pos[2] + d[2]]);
        let beside = at(0);
        if can_climb {
            let carries = self.name(beside).ends_with("_trapdoor")
                || self.sturdy(beside, 1)
                || self.name(beside) == "minecraft:hopper";
            if carries && self.attracts(at(1), None) {
                return if self.sturdy(beside, opposite_face) { Connection::Up } else { Connection::Side };
            }
        }
        let cut = !self.attracts(beside, Some(name))
            && (self.sim(beside, sim_bits::CONDUCTOR) || !self.attracts(at(-1), None));
        if cut { Connection::None } else { Connection::Side }
    }
}

fn opposite(side: &str) -> &str {
    match side {
        "north" => "south",
        "south" => "north",
        "east" => "west",
        _ => "east",
    }
}

/// Sides of a wire state, in [`SIDES`] order.
fn sides_of(d: &VersionData, s: u32) -> [Connection; 4] {
    std::array::from_fn(|i| Connection::parse(d.state_properties(s).find(|(k, _)| *k == SIDES[i].0).map(|(_, v)| v)))
}

/// New state of the wire `state` at `pos` clicked by the main hand (`state_at`: the client's
/// world): `None` when it is neither a dot nor a cross, or when the click changes nothing (the
/// item in hand then acts).
pub(super) fn switched_wire(
    d: &VersionData,
    state_at: impl Fn([i32; 3]) -> u32,
    pos: [i32; 3],
    state: u32,
) -> Option<u32> {
    if d.block_of(state)?.name != "minecraft:redstone_wire" {
        return None;
    }
    let sides = sides_of(d, state);
    let cross = sides.iter().all(|s| s.connected());
    if !cross && sides.iter().any(|s| s.connected()) {
        return None;
    }
    let wire = Wire { data: d, state_at };
    // From a cross the wire starts again from a dot; from a dot, from a cross.
    let was_dot = cross;
    let above = (wire.state_at)([pos[0], pos[1] + 1, pos[2]]);
    let can_climb = !wire.sim(above, sim_bits::CONDUCTOR);
    let mut new: [Connection; 4] = std::array::from_fn(|i| wire.connection(pos, i, can_climb));
    let dot = new.iter().all(|s| !s.connected());
    if !(was_dot && dot) {
        extend_lone_line(&mut new);
    }
    let power = wire.prop(state, "power").unwrap_or("0");
    let mut changes: Vec<(&str, &str)> = SIDES.iter().zip(new).map(|((k, ..), s)| (*k, s.name())).collect();
    changes.push(("power", power));
    let switched = super::state_with_values(d, state, &changes)?;
    (switched != state).then_some(switched)
}

/// A wire left with no connection on an axis runs straight along the other one; a wire with no
/// connection at all becomes a cross.
fn extend_lone_line(sides: &mut [Connection; 4]) {
    let [north, east, south, west] = sides.map(Connection::connected);
    let (north_south_empty, east_west_empty) = (!north && !south, !east && !west);
    if !west && north_south_empty {
        sides[3] = Connection::Side;
    }
    if !east && north_south_empty {
        sides[1] = Connection::Side;
    }
    if !north && east_west_empty {
        sides[0] = Connection::Side;
    }
    if !south && east_west_empty {
        sides[2] = Connection::Side;
    }
}

#[cfg(test)]
mod tests {
    use super::switched_wire;
    use crate::block_anim::tests::state_of;

    /// Dot and cross switched on real data: alone, a dot becomes a cross and back; a plain stone
    /// beside connects nothing, a wire on it makes the side climb; with a wire to the north a
    /// cross becomes a north-south line; a line is left alone; power is kept.
    #[test]
    fn dot_and_cross_switch_like_the_game() {
        let Some(d) = rc_testdata::version_data() else { return };
        let wire = |n: &str, e: &str, s: &str, w: &str, power: &str| {
            let props = [("north", n), ("east", e), ("south", s), ("west", w), ("power", power)];
            state_of(&d, "minecraft:redstone_wire", &props)
        };
        let (air, stone) = (state_of(&d, "minecraft:air", &[]), state_of(&d, "minecraft:stone", &[]));
        let pos = [0, 64, 0];
        let floor = |p: [i32; 3]| if p[1] < 64 { stone } else { air };
        let dot = wire("none", "none", "none", "none", "7");
        let cross = wire("side", "side", "side", "side", "7");
        assert_eq!(switched_wire(&d, floor, pos, dot), Some(cross), "lone dot");
        assert_eq!(switched_wire(&d, floor, pos, cross), Some(dot), "lone cross");
        // A line is neither a dot nor a cross: the click passes.
        assert_eq!(switched_wire(&d, floor, pos, wire("side", "none", "side", "none", "0")), None);
        // A wire to the north: the cross becomes a north-south line, which a second click leaves.
        let north_wire = wire("none", "none", "none", "none", "0");
        let with_north = |p: [i32; 3]| if p == [0, 64, -1] { north_wire } else { floor(p) };
        let line = wire("side", "none", "side", "none", "7");
        assert_eq!(switched_wire(&d, with_north, pos, cross), Some(line));
        // Stone to the east: no climbing without a wire on it, a plain block cuts nothing.
        let with_stone = |p: [i32; 3]| if p == [1, 64, 0] { stone } else { floor(p) };
        assert_eq!(switched_wire(&d, with_stone, pos, cross), Some(dot), "stone beside");
        // A wire on that stone: the side climbs ("up": the stone's west face is sturdy), and the
        // lone connection runs on to the west.
        let climbs = |p: [i32; 3]| match p {
            [1, 64, 0] => stone,
            [1, 65, 0] => north_wire,
            _ => floor(p),
        };
        let up_east = wire("none", "up", "none", "side", "7");
        assert_eq!(switched_wire(&d, climbs, pos, dot), Some(up_east), "dot beside a climbing wire");
        // Not a wire: nothing.
        assert_eq!(switched_wire(&d, floor, pos, stone), None);
    }
}
