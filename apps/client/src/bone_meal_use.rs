//! Bone meal as seen by the client (right click predicted, like the 26.3 game client): does the
//! clicked block accept the bone meal, otherwise is the clicked face solid in front of full
//! water. A success makes the arm swing and ends the click; a refusal lets the click fall
//! through to the item use, then to the other hand. Saplings, mushrooms and azaleas are judged
//! on the server only: the game client refuses them, and so does this one.

pub(crate) type P = [i32; 3];

/// Directions in the game's order: down, up, north, south, west, east (axis = index / 2,
/// opposite = index ^ 1).
const DIRS: [P; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
const FACE_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
const DOWN: usize = 0;
const UP: usize = 1;

/// World as seen by the prediction.
pub(crate) trait World {
    /// Block name, namespace included.
    fn name(&self, p: P) -> &str;
    fn prop(&self, p: P, key: &str) -> Option<&str>;
    fn is_air(&self, p: P) -> bool;
    /// Replaceable without placement context (air, short grass, water…).
    fn replaceable(&self, p: P) -> bool;
    /// Whether the block at `p` is in a synchronised block tag.
    fn tag(&self, p: P, tag: &str) -> bool;
    /// Whether face `face` of the block at `p` is full in its support shape or, if `collision`,
    /// in its collision shape.
    fn face_full(&self, p: P, face: usize, collision: bool) -> bool;
    /// Amount of fluid at `p` (0 to 8).
    fn fluid_amount(&self, p: P) -> u8;
    fn fluid_source(&self, p: P) -> bool;
    fn skylight_through(&self, p: P) -> bool;
    /// Raw light (sky or block, without darkening).
    fn raw_light(&self, p: P) -> i32;
    fn in_height(&self, y: i32) -> bool;
}

fn rel(p: P, d: usize) -> P {
    [p[0] + DIRS[d][0], p[1] + DIRS[d][1], p[2] + DIRS[d][2]]
}

/// Bone meal click on `pos` through face `face`: predicted success.
pub(crate) fn predicts_success(w: &impl World, pos: P, face: usize) -> bool {
    let front = rel(pos, face);
    valid_target(w, pos)
        || (w.face_full(pos, face, false) && w.name(front) == "minecraft:water" && w.fluid_amount(front) == 8)
}

fn valid_target(w: &impl World, p: P) -> bool {
    let int = |k: &str| w.prop(p, k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
    let flag = |k: &str| w.prop(p, k) == Some("true");
    let (up, down) = (rel(p, UP), rel(p, DOWN));
    let free_above = || w.is_air(up) && w.in_height(up[1]);
    match w.name(p).trim_start_matches("minecraft:") {
        "wheat" | "carrots" | "potatoes" => int("age") < 7,
        "beetroots" => int("age") < 3,
        "torchflower_crop" => int("age") < 2,
        "pumpkin_stem" | "melon_stem" => int("age") != 7,
        "sweet_berry_bush" => int("age") < 3,
        "cocoa" => int("age") < 2,
        "shelf_mushroom" => int("age") < 1,
        "cave_vines" | "cave_vines_plant" => !flag("berries"),
        "mangrove_propagule" => !flag("hanging") || int("age") != 4,
        "short_dry_grass" | "small_dripleaf" | "sunflower" | "lilac" | "rose_bush" | "peony" | "pink_petals"
        | "wildflowers" => true,
        "grass_block" | "crimson_nylium" | "warped_nylium" | "bamboo_sapling" => free_above(),
        "moss_block" | "pale_moss_block" => w.is_air(up),
        "short_grass" | "fern" => w.tag(down, "minecraft:supports_vegetation") && free_above(),
        "bush" | "red_shrub" | "firefly_bush" => spreads(w, p, "minecraft:supports_vegetation"),
        "tall_dry_grass" => spreads(w, p, "minecraft:supports_dry_vegetation"),
        "crimson_fungus" => w.name(down) == "minecraft:crimson_nylium" && w.in_height(up[1]),
        "warped_fungus" => w.name(down) == "minecraft:warped_nylium" && w.in_height(up[1]),
        "netherrack" => w.skylight_through(up) && near_nylium(w, p),
        "seagrass" => w.name(up) == "minecraft:water",
        "sea_pickle" => flag("waterlogged") && w.tag(down, "minecraft:coral_blocks"),
        "mangrove_leaves" => w.is_air(down),
        "rooted_dirt" => w.is_air(down) && w.in_height(down[1]),
        "pale_hanging_moss" => {
            let tip = run_end(w, p, DOWN, "minecraft:pale_hanging_moss");
            w.is_air(tip) && w.in_height(tip[1])
        }
        "bamboo" => bamboo(w, p),
        "kelp" => grows_into(w, rel(p, UP), true),
        "weeping_vines" => grows_into(w, rel(p, DOWN), false),
        "twisting_vines" => grows_into(w, rel(p, UP), false),
        "kelp_plant" => vine_body(w, p, UP, "minecraft:kelp", true),
        "weeping_vines_plant" => vine_body(w, p, DOWN, "minecraft:weeping_vines", false),
        "twisting_vines_plant" => vine_body(w, p, UP, "minecraft:twisting_vines", false),
        "big_dripleaf" => dripleaf_room(w, up),
        "big_dripleaf_stem" => {
            let head = run_end(w, p, UP, "minecraft:big_dripleaf_stem");
            w.name(head) == "minecraft:big_dripleaf" && dripleaf_room(w, rel(head, UP))
        }
        "pitcher_crop" => pitcher(w, p),
        "pale_moss_carpet" => flag("bottom") && moss_topper(w, p),
        "glow_lichen" => lichen(w, p),
        // Saplings, mushrooms, azaleas: validity judged by the server alone.
        _ => false,
    }
}

/// First cell after the run of `block` starting at `p` toward `d`.
fn run_end(w: &impl World, p: P, d: usize, block: &str) -> P {
    let mut q = rel(p, d);
    while w.name(q) == block {
        q = rel(q, d);
    }
    q
}

/// An empty horizontal neighbour where the plant would hold (block below in the support tag).
fn spreads(w: &impl World, p: P, support: &str) -> bool {
    (2..6).map(|d| rel(p, d)).any(|q| w.is_air(q) && w.tag(rel(q, DOWN), support))
}

/// A nylium in the block's 3 × 3 × 3 cube.
fn near_nylium(w: &impl World, p: P) -> bool {
    (-1..=1).any(|dx| (-1..=1).any(|dy| (-1..=1).any(|dz| w.tag([p[0] + dx, p[1] + dy, p[2] + dz], "minecraft:nylium"))))
}

/// Cell where the head of a climbing plant grows: water (kelp) or air (vines), inside the world.
fn grows_into(w: &impl World, q: P, kelp: bool) -> bool {
    let ok = if kelp { w.name(q) == "minecraft:water" } else { w.is_air(q) };
    ok && w.in_height(q[1])
}

/// Body of a climbing plant: the run of bodies ends on the head, which can grow.
fn vine_body(w: &impl World, p: P, d: usize, head: &str, kelp: bool) -> bool {
    let h = run_end(w, p, d, w.name(p));
    w.name(h) == head && grows_into(w, rel(h, d), kelp)
}

/// Bamboo column (16 at most on each side) not finished, cell above the top free.
fn bamboo(w: &impl World, p: P) -> bool {
    let count = |d: i32| (1..=16).take_while(|&h| w.name([p[0], p[1] + d * h, p[2]]) == "minecraft:bamboo").count() as i32;
    let (up, down) = (count(1), count(-1));
    let grow = [p[0], p[1] + up + 1, p[2]];
    up + down + 1 < 16 && w.prop([p[0], p[1] + up, p[2]], "stage") != Some("1") && w.in_height(grow[1]) && w.is_air(grow)
}

/// Cell where the big dripleaf grows: inside the world, air, water or small dripleaf.
fn dripleaf_room(w: &impl World, q: P) -> bool {
    w.in_height(q[1]) && (w.is_air(q) || matches!(w.name(q), "minecraft:water" | "minecraft:small_dripleaf"))
}

/// Pitcher plant: lower half (this cell or the one below) not ripe, enough light, high enough in
/// the world and, from age 3, the cell above free (air or the plant).
fn pitcher(w: &impl World, p: P) -> bool {
    let lower = |q: P| w.name(q) == "minecraft:pitcher_crop" && w.prop(q, "half") == Some("lower");
    let lp = if lower(p) {
        p
    } else if lower(rel(p, DOWN)) {
        rel(p, DOWN)
    } else {
        return false;
    };
    let age = w.prop(lp, "age").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
    let up = rel(lp, UP);
    age < 4
        && w.raw_light(lp) >= 8
        && w.in_height(up[1])
        && (age + 1 < 3 || w.is_air(up) || w.name(up) == "minecraft:pitcher_crop")
}

/// Pale moss carpet (bottom layer): the layer above would have at least one side, and it would
/// change the cell. A side holds against a neighbouring full face, if the carpet below has that
/// side; it is tall under an upper carpet that has it too.
fn moss_topper(w: &impl World, p: P) -> bool {
    const CARPET: &str = "minecraft:pale_moss_carpet";
    let up = rel(p, UP);
    let carpet_above = w.name(up) == CARPET;
    if (carpet_above && w.prop(up, "bottom") == Some("true")) || (!carpet_above && !w.replaceable(up)) {
        return false;
    }
    let top = rel(up, UP);
    let (mut any, mut same) = (false, carpet_above);
    for (d, &face) in FACE_NAMES.iter().enumerate().skip(2) {
        let mut side = "none";
        if w.face_full(rel(up, d), d ^ 1, true) {
            side = "low";
            if w.name(top) == CARPET && w.prop(top, face) != Some("none") && w.prop(top, "bottom") != Some("true") {
                side = "tall";
            }
            if w.name(p) == CARPET && w.prop(p, face) == Some("none") {
                side = "none";
            }
        }
        any |= side != "none";
        same &= w.prop(up, face) == Some(side);
    }
    any && !same
}

/// Glow lichen: a face of the lichen can spread toward a direction (same cell, same plane or
/// around the edge) onto a free cell where the new face attaches.
fn lichen(w: &impl World, p: P) -> bool {
    let has = |q: P, d: usize| w.prop(q, FACE_NAMES[d]) == Some("true");
    let room = |q: P, f: usize| {
        let lichen = w.name(q) == "minecraft:glow_lichen";
        let free = w.is_air(q) || lichen || (w.name(q) == "minecraft:water" && w.fluid_source(q));
        free && !(lichen && has(q, f)) && w.face_full(rel(q, f), f ^ 1, true)
    };
    (0..6).any(|from| {
        has(p, from)
            && (0..6).any(|to| {
                to / 2 != from / 2
                    && !has(p, to)
                    && [(p, to), (rel(p, to), from), (rel(rel(p, to), from), to ^ 1)].into_iter().any(|(q, f)| room(q, f))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashbrown::HashMap;

    /// Fake world: named blocks with properties, air elsewhere; full cubes listed.
    #[derive(Default)]
    struct Fake {
        blocks: HashMap<P, (String, Vec<(String, String)>)>,
    }

    const FULL: [&str; 7] = [
        "minecraft:stone",
        "minecraft:netherrack",
        "minecraft:crimson_nylium",
        "minecraft:grass_block",
        "minecraft:dirt",
        "minecraft:sand",
        "minecraft:tube_coral_block",
    ];

    impl Fake {
        fn set(&mut self, p: P, spec: &str) {
            let (name, props) = spec.split_once('[').unwrap_or((spec, ""));
            let props = props
                .trim_end_matches(']')
                .split(',')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            self.blocks.insert(p, (format!("minecraft:{name}"), props));
        }
    }

    impl World for Fake {
        fn name(&self, p: P) -> &str {
            self.blocks.get(&p).map_or("minecraft:air", |b| b.0.as_str())
        }
        fn prop(&self, p: P, key: &str) -> Option<&str> {
            self.blocks.get(&p)?.1.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
        }
        fn is_air(&self, p: P) -> bool {
            self.name(p) == "minecraft:air"
        }
        fn replaceable(&self, p: P) -> bool {
            matches!(self.name(p), "minecraft:air" | "minecraft:water" | "minecraft:short_grass")
        }
        fn tag(&self, p: P, tag: &str) -> bool {
            let n = self.name(p);
            match tag {
                "minecraft:supports_vegetation" => matches!(n, "minecraft:grass_block" | "minecraft:dirt"),
                "minecraft:supports_dry_vegetation" => n == "minecraft:sand",
                "minecraft:nylium" => n == "minecraft:crimson_nylium",
                "minecraft:coral_blocks" => n == "minecraft:tube_coral_block",
                _ => false,
            }
        }
        fn face_full(&self, p: P, _face: usize, _collision: bool) -> bool {
            FULL.contains(&self.name(p))
        }
        fn fluid_amount(&self, p: P) -> u8 {
            if self.name(p) != "minecraft:water" {
                return 0;
            }
            match self.prop(p, "level").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0) {
                0 | 8.. => 8,
                l => 8 - l,
            }
        }
        fn fluid_source(&self, p: P) -> bool {
            self.name(p) == "minecraft:water" && self.prop(p, "level").is_none_or(|l| l == "0")
        }
        fn skylight_through(&self, p: P) -> bool {
            !FULL.contains(&self.name(p))
        }
        fn raw_light(&self, _p: P) -> i32 {
            15
        }
        fn in_height(&self, y: i32) -> bool {
            (-64..320).contains(&y)
        }
    }

    fn ok(w: &Fake, p: P) -> bool {
        predicts_success(w, p, UP)
    }

    /// Targets added in 26.3 (nether, climbing plants, bushes, water): success predicted where the
    /// server grows, refusal otherwise.
    #[test]
    fn nether_and_vines() {
        let mut w = Fake::default();
        w.set([0, 0, 0], "netherrack");
        assert!(!ok(&w, [0, 0, 0]), "netherrack without nylium");
        w.set([1, 0, 0], "crimson_nylium");
        assert!(ok(&w, [0, 0, 0]), "netherrack against a nylium");
        assert!(ok(&w, [1, 0, 0]), "nylium under air");
        w.set([1, 1, 0], "crimson_fungus");
        assert!(ok(&w, [1, 1, 0]), "fungus on its nylium");
        w.set([5, 0, 0], "grass_block");
        w.set([5, 1, 0], "crimson_fungus");
        assert!(!ok(&w, [5, 1, 0]), "fungus off its nylium");
        // Weeping vine: body then head, air below; blocked by stone.
        w.set([3, 10, 3], "weeping_vines_plant");
        w.set([3, 9, 3], "weeping_vines[age=3]");
        assert!(ok(&w, [3, 10, 3]) && ok(&w, [3, 9, 3]));
        w.set([3, 8, 3], "stone");
        assert!(!ok(&w, [3, 10, 3]) && !ok(&w, [3, 9, 3]));
        // Kelp: water above the head.
        w.set([7, 0, 7], "kelp");
        assert!(!ok(&w, [7, 0, 7]), "kelp under air");
        w.set([7, 1, 7], "water");
        assert!(ok(&w, [7, 0, 7]));
        // Bush: a free neighbour on grass; surrounded, refusal.
        w.set([10, 0, 10], "bush");
        for d in 2..6 {
            w.set(rel(rel([10, 0, 10], d), DOWN), "grass_block");
        }
        assert!(ok(&w, [10, 0, 10]));
        for d in 2..6 {
            w.set(rel([10, 0, 10], d), "stone");
        }
        assert!(!ok(&w, [10, 0, 10]), "surrounded bush");
    }

    /// Lichen, pickles, bamboo, pitcher plant, moss carpet, dripleaf.
    #[test]
    fn faces_and_columns() {
        let mut w = Fake::default();
        w.set([0, 0, 1], "stone");
        w.set([0, 0, 0], "glow_lichen[south=true]");
        assert!(ok(&w, [0, 0, 0]), "lichen against a wall");
        let mut box_in = Fake::default();
        box_in.set([0, 0, 0], "glow_lichen[south=true]");
        assert!(!ok(&box_in, [0, 0, 0]), "lichen without support");

        w.set([4, -1, 4], "tube_coral_block");
        w.set([4, 0, 4], "sea_pickle[pickles=1,waterlogged=true]");
        assert!(ok(&w, [4, 0, 4]));
        w.set([4, 0, 4], "sea_pickle[pickles=1,waterlogged=false]");
        assert!(!ok(&w, [4, 0, 4]), "dry pickle");

        for y in 0..3 {
            w.set([8, y, 8], "bamboo[stage=0]");
        }
        assert!(ok(&w, [8, 0, 8]));
        w.set([8, 2, 8], "bamboo[stage=1]");
        assert!(!ok(&w, [8, 0, 8]), "finished bamboo");

        w.set([12, 0, 12], "pitcher_crop[age=2,half=lower]");
        assert!(ok(&w, [12, 0, 12]));
        w.set([12, 1, 12], "stone");
        assert!(!ok(&w, [12, 0, 12]), "pitcher plant under stone at age 3");

        w.set([20, 0, 20], "pale_moss_carpet[bottom=true,north=low,south=none,west=none,east=none]");
        assert!(!ok(&w, [20, 0, 20]), "no wall beside");
        w.set([20, 1, 19], "stone");
        assert!(ok(&w, [20, 0, 20]), "wall north of the upper cell");

        w.set([24, 0, 24], "big_dripleaf_stem");
        w.set([24, 1, 24], "big_dripleaf");
        assert!(ok(&w, [24, 0, 24]) && ok(&w, [24, 1, 24]));
        w.set([24, 2, 24], "stone");
        assert!(!ok(&w, [24, 0, 24]) && !ok(&w, [24, 1, 24]));
    }

    /// On the client side, the game refuses saplings, mushrooms and azaleas (judged by the
    /// server); a full face in front of full water is a success, flowing water is not.
    #[test]
    fn server_only_targets_and_water() {
        let mut w = Fake::default();
        for b in ["oak_sapling", "poplar_sapling", "red_mushroom", "azalea"] {
            w.set([0, 0, 0], b);
            assert!(!ok(&w, [0, 0, 0]), "{b}");
        }
        w.set([0, 0, 0], "mangrove_propagule[hanging=false,age=0]");
        assert!(ok(&w, [0, 0, 0]), "propagule");
        w.set([5, 0, 5], "stone");
        assert!(!predicts_success(&w, [5, 0, 5], UP), "stone under air");
        w.set([5, 1, 5], "water[level=0]");
        assert!(predicts_success(&w, [5, 0, 5], UP), "stone under full water");
        w.set([5, 1, 5], "water[level=3]");
        assert!(!predicts_success(&w, [5, 0, 5], UP), "flowing water");
        w.set([5, 1, 5], "water[level=9]");
        assert!(predicts_success(&w, [5, 0, 5], UP), "falling water (full)");
    }
}
