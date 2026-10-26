//! Mobs (`entities-animals.md` §§ 9-14, 16): axolotl, fish, squids, guardians, cubes,
//! ghasts, nautiluses.
//!
//! Entry points (called by `entity_render.rs` before its own rules):
//! - `layers`: layers (layer, texture, pass, wool) of a kind depending on (baby, `skin`);
//! - `skin`: 32-bit resolution key drawn from the metadata (variant, colours, saddle…);
//!   the layers are resolved once per (kind, baby, key);
//! - `animate`: animation of the parts (true = kind handled here);
//! - `rotations`: render rotations in place of "Y(180 − body)" (true = replaced);
//! - `scale`: kind-specific scale (after the flip);
//! - `shadow`: shadow radius and intensity;
//! - `tick`: client state per tick (`Entity::extra`, `Entity::history`).
//!
//! "In water" is not known to the client here: it is deduced from the air supply (index 1),
//! reset to the maximum every tick in water by the server for aquatic mobs.

use super::keyframes::State;
use super::*;

const TENTACLES: [&str; 9] = [
    "tentacle0",
    "tentacle1",
    "tentacle2",
    "tentacle3",
    "tentacle4",
    "tentacle5",
    "tentacle6",
    "tentacle7",
    "tentacle8",
];
const SPIKES: [&str; 12] = [
    "spike0", "spike1", "spike2", "spike3", "spike4", "spike5", "spike6", "spike7", "spike8", "spike9", "spike10",
    "spike11",
];
const CUBES: [&str; 8] = ["cube0", "cube1", "cube2", "cube3", "cube4", "cube5", "cube6", "cube7"];
/// Pitch of the raised goggles of the happy ghast harness: the game's constant (−0.7854), not
/// exactly −π/4.
#[allow(clippy::approx_constant)]
const GOGGLES_UP: f32 = -0.7854;
const AXOLOTLS: [&str; 5] = ["lucy", "wild", "gold", "cyan", "blue"];

/// Full air supply (in water): 6000 for the axolotl, 300 for the others.
fn in_water(e: &Entity) -> bool {
    let max = if e.kind == "axolotl" { 6000 } else { 300 };
    e.meta_int(1).unwrap_or(max) >= max
}

/// Stable random draw from the UUID (initial client state), in [0, 1).
fn rand01(e: &Entity, salt: u32) -> f32 {
    let mut x = (e.uuid as u64) ^ ((e.uuid >> 64) as u64) ^ u64::from(salt).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

/// Item of slot `slot`, encoded id + 1 on 12 bits (0 = empty).
fn item_key(e: &Entity, slot: usize) -> u32 {
    e.equipment[slot].as_ref().and_then(|s| u32::try_from(s.item + 1).ok()).filter(|&i| i < 4096).unwrap_or(0)
}

/// Equipment textures of an item (key `item_key`) for a layer type.
fn equip_textures(r: &EntityRenderer, key: u32, ty: &str) -> Vec<String> {
    let Some(info) = key.checked_sub(1).and_then(|i| r.source.items.get(i as usize)) else { return Vec::new() };
    r.equipment
        .get(&info.equip_asset)
        .and_then(|t| t.get(ty))
        .map(|v| v.iter().map(|(t, _)| format!("entity/equipment/{ty}/{t}")).collect())
        .unwrap_or_default()
}

fn layer(m: &str, t: &str, pass: Pass) -> (String, String, Pass, Wool) {
    (m.to_owned(), t.to_owned(), pass, Wool::None)
}

pub(super) fn layers(r: &EntityRenderer, k: &str, baby: bool, skin: u32) -> Option<Vec<(String, String, Pass, Wool)>> {
    let c = Pass::Cutout;
    let bb = if baby { "_baby" } else { "" };
    Some(match k {
        "axolotl" => {
            let name = AXOLOTLS[(skin as usize).min(4)];
            vec![layer(&format!("minecraft:axolotl{bb}#main"), &format!("entity/axolotl/axolotl_{name}{bb}"), c)]
        }
        "cod" => vec![layer("minecraft:cod#main", "entity/fish/cod", c)],
        "salmon" => {
            let m = ["minecraft:salmon_small#main", "minecraft:salmon#main", "minecraft:salmon_large#main"];
            vec![layer(m[(skin as usize).min(2)], "entity/fish/salmon", c)]
        }
        "pufferfish" => {
            let m = [
                "minecraft:pufferfish_small#main",
                "minecraft:pufferfish_medium#main",
                "minecraft:pufferfish_big#main",
            ];
            vec![layer(m[(skin as usize).min(2)], "entity/fish/pufferfish", c)]
        }
        // `skin` = shape | pattern index << 1. Base (channel 0) and pattern (channel 1) tints
        // read from the variant by `tint`.
        "tropical_fish" => {
            let (m, t) = if skin & 1 == 0 { ("small", "a") } else { ("large", "b") };
            let i = (skin >> 1).min(5) + 1;
            let mut base = layer(&format!("minecraft:tropical_fish_{m}#main"), &format!("entity/fish/tropical_{t}"), c);
            base.3 = Wool::Channel(0);
            let mut pattern = layer(
                &format!("minecraft:tropical_fish_{m}#pattern"),
                &format!("entity/fish/tropical_{t}_pattern_{i}"),
                c,
            );
            pattern.3 = Wool::Channel(1);
            vec![base, pattern]
        }
        "squid" | "glow_squid" => vec![layer(&format!("minecraft:{k}{bb}#main"), &format!("entity/squid/{k}{bb}"), c)],
        "guardian" => vec![layer("minecraft:guardian#main", "entity/guardian/guardian", c)],
        "elder_guardian" => vec![layer("minecraft:elder_guardian#main", "entity/guardian/guardian_elder", c)],
        "slime" => vec![
            layer("minecraft:slime#main", "entity/slime/slime", c),
            layer("minecraft:slime#outer", "entity/slime/slime", Pass::Translucent),
        ],
        "magma_cube" => vec![layer("minecraft:magma_cube#main", "entity/slime/magmacube", c)],
        // Inner layer first (order −1), then the shell; both translucent.
        // `skin` 1: swallowed content, drawn in place of the interior (`push_swallowed`).
        "sulfur_cube" => {
            let (m, s) = if baby { ("sulfur_cube_small", "_small") } else { ("sulfur_cube", "") };
            let outer = layer(
                &format!("minecraft:{m}#main"),
                &format!("entity/sulfur_cube/sulfur_cube_outer{s}"),
                Pass::Translucent,
            );
            if skin & 1 != 0 {
                vec![outer]
            } else {
                vec![
                    layer(
                        &format!("minecraft:{m}#inner"),
                        &format!("entity/sulfur_cube/sulfur_cube_inner{s}"),
                        Pass::Translucent,
                    ),
                    outer,
                ]
            }
        }
        // `skin` 1: charging.
        "ghast" => vec![layer(
            "minecraft:ghast#main",
            if skin & 1 != 0 { "entity/ghast/ghast_shooting" } else { "entity/ghast/ghast" },
            c,
        )],
        // `skin` = harness (item key, 12 bits) | ropes << 12; harness also drawn on the baby.
        "happy_ghast" => {
            let mut v =
                vec![layer(&format!("minecraft:happy_ghast{bb}#main"), &format!("entity/ghast/happy_ghast{bb}"), c)];
            for t in equip_textures(r, skin & 0xfff, "happy_ghast_body") {
                v.push(layer(&format!("minecraft:happy_ghast{bb}_harness#main"), &t, c));
            }
            if skin & 0x1000 != 0 {
                v.push(layer(&format!("minecraft:happy_ghast{bb}_ropes#main"), "entity/ghast/happy_ghast_ropes", c));
            }
            v
        }
        // `skin` = armor (12 bits) | saddle << 12 | variant << 24 (255 = none).
        "nautilus" | "zombie_nautilus" => {
            let mut v = if k == "nautilus" {
                vec![layer(&format!("minecraft:nautilus{bb}#main"), &format!("entity/nautilus/nautilus{bb}"), c)]
            } else {
                let i = (skin >> 24) as usize;
                let def = r.variant_def("minecraft:zombie_nautilus_variant", i);
                // Content omitted by the server (known pack): vanilla order temperate, warm.
                let warm = match def {
                    Some(d) if !d.asset.is_empty() => d.model == "warm",
                    _ if i <= 1 => i == 1,
                    _ => return Some(Vec::new()),
                };
                if warm {
                    vec![layer("minecraft:zombie_nautilus_coral#main", "entity/nautilus/zombie_nautilus_coral", c)]
                } else {
                    vec![layer("minecraft:zombie_nautilus#main", "entity/nautilus/zombie_nautilus", c)]
                }
            };
            if !baby {
                for t in equip_textures(r, skin & 0xfff, "nautilus_body") {
                    v.push(layer("minecraft:nautilus_armor#main", &t, c));
                }
                for t in equip_textures(r, (skin >> 12) & 0xfff, "nautilus_saddle") {
                    v.push(layer("minecraft:nautilus#saddle", &t, c));
                }
            }
            v
        }
        _ => return None,
    })
}

/// Kinds for which `skin` answers (all its branches except the last).
pub(super) fn ours(k: &str) -> bool {
    matches!(
        k,
        "axolotl"
            | "salmon"
            | "pufferfish"
            | "tropical_fish"
            | "ghast"
            | "happy_ghast"
            | "nautilus"
            | "zombie_nautilus"
            | "sulfur_cube"
    )
}

pub(super) fn skin(r: &EntityRenderer, e: &Entity) -> Option<u32> {
    Some(match e.kind.as_str() {
        "axolotl" => e.meta_int(18).filter(|v| (0..5).contains(v)).unwrap_or(0) as u32,
        "salmon" => e.meta_int(17).unwrap_or(1).clamp(0, 2) as u32,
        "pufferfish" => e.meta_int(17).unwrap_or(0).clamp(0, 2) as u32,
        "tropical_fish" => {
            let id = e.meta_int(17).unwrap_or(0) & 0xffff;
            let (shape, idx) = (id & 0xff, id >> 8);
            if shape <= 1 && idx <= 5 { (shape | idx << 1) as u32 } else { 0 }
        }
        "ghast" => u32::from(e.meta_bool(16)),
        "happy_ghast" => {
            let harness = item_key(e, 6);
            let is_harness = harness > 0
                && r.source
                    .items
                    .get(harness as usize - 1)
                    .and_then(|i| r.equipment.get(&i.equip_asset))
                    .is_some_and(|t| t.contains_key("happy_ghast_body"));
            harness | u32::from(is_harness && e.meta_bool(18)) << 12
        }
        "nautilus" | "zombie_nautilus" => {
            let variant =
                if e.kind == "zombie_nautilus" { e.meta_int(21).map_or(0, |v| v.clamp(0, 255) as u32) } else { 0 };
            item_key(e, 6) | item_key(e, 7) << 12 | variant << 24
        }
        // 1: swallowed content (block or skull), inner layer removed.
        "sulfur_cube" => u32::from(e.equipment[6].is_some()),
        _ => return None,
    })
}

pub(super) fn animate(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) -> bool {
    let age = s.age;
    match e.kind.as_str() {
        "axolotl" => {
            if s.baby {
                baby_axolotl(p, e, s);
            } else {
                axolotl(p, e, s, pt);
            }
        }
        "tadpole" => {
            let a = if in_water(e) { 1.0 } else { 1.5 };
            p.set("tail", |t| t[4] = -a * 0.25 * (0.3 * age).sin());
        }
        "dolphin" => {
            let v = e.velocity;
            p.set("body", |b| {
                b[3] = s.pitch;
                b[4] = s.yaw;
            });
            if v[0] * v[0] + v[2] * v[2] > 1.0e-7 {
                let c = (age * 0.3).cos();
                p.set("body", |b| b[3] += -0.05 - 0.05 * c);
                p.set("tail", |t| t[3] = -0.1 * c);
                p.set("tail_fin", |t| t[3] = -0.2 * c);
            }
        }
        "iron_golem" => iron_golem(p, e, s, pt),
        "snow_golem" => {
            p.set("head", |h| {
                h[4] = s.yaw;
                h[3] = s.pitch;
            });
            let y = s.yaw * 0.25;
            p.set("upper_body", |b| b[4] = y);
            let (sn, cs) = (y.sin(), y.cos());
            p.set("left_arm", |a| {
                a[4] = y;
                a[0] = cs * 5.0;
                a[2] = -sn * 5.0;
            });
            p.set("right_arm", |a| {
                a[4] = y + PI;
                a[0] = -cs * 5.0;
                a[2] = sn * 5.0;
            });
        }
        "copper_golem" => copper_golem(p, e, s),
        "villager" | "wandering_trader" | "witch" => {
            p.set("head", |h| {
                h[4] = s.yaw;
                h[3] = s.pitch;
            });
            if e.kind == "villager" && e.meta_int(18).unwrap_or(0) > 0 {
                p.set("head", |h| {
                    h[5] = 0.3 * (0.45 * age).sin();
                    h[3] = 0.4;
                });
            } else if e.kind != "witch" {
                p.set("head", |h| h[5] = 0.0);
            }
            let q = s.walk_pos * 0.6662;
            p.set("right_leg", |l| {
                l[3] = q.cos() * 1.4 * s.walk_speed * 0.5;
                l[4] = 0.0;
            });
            p.set("left_leg", |l| {
                l[3] = (q + PI).cos() * 1.4 * s.walk_speed * 0.5;
                l[4] = 0.0;
            });
            if e.kind == "witch" {
                let sp = 0.01 * (e.id.rem_euclid(10)) as f32;
                p.set("nose", |n| {
                    n[3] = (age * sp).sin() * 4.5 * PI / 180.0;
                    n[5] = (age * sp).cos() * 2.5 * PI / 180.0;
                });
                if e.equipment[0].is_some() {
                    p.set("nose", |n| {
                        n[0] = 0.0;
                        n[1] = 1.0;
                        n[2] = -1.5;
                        n[3] = -0.9;
                    });
                }
            }
        }
        "strider" => strider(p, e, s),
        "blaze" => {
            // Three rings of 4 rods: (start angle, height, phase step, frequency,
            // radius); each rod advances by a quarter turn.
            let rings = [
                (age * PI * -0.1, -2.0, 2.0, 0.25, 9.0),
                (PI / 4.0 + age * PI * 0.03, 2.0, 2.0, 0.25, 7.0),
                (0.471_238_94 + age * PI * -0.05, 11.0, 1.5, 0.5, 5.0),
            ];
            for (i, name) in BLAZE_RODS.iter().enumerate() {
                let (start, y, step, f, r) = rings[i / 4];
                let a = start + (i % 4) as f32 * (PI / 2.0);
                p.set(name, |x| {
                    x[1] = y + ((i as f32 * step + age) * f).cos();
                    x[0] = a.cos() * r;
                    x[2] = a.sin() * r;
                });
            }
            p.set("head", |h| {
                h[4] = s.yaw;
                h[3] = s.pitch;
            });
        }
        "phantom" => {
            let flap = (e.id * 3) as f32 + age;
            let an = flap * 7.448_451 * PI / 180.0;
            let w = an.cos() * 16.0 * PI / 180.0;
            p.set("left_wing_base", |x| x[5] = w);
            p.set("left_wing_tip", |x| x[5] = w);
            p.set("right_wing_base", |x| x[5] = -w);
            p.set("right_wing_tip", |x| x[5] = -w);
            let t = -(5.0 + (an * 2.0).cos() * 5.0) * PI / 180.0;
            p.set("tail_base", |x| x[3] = t);
            p.set("tail_tip", |x| x[3] = t);
        }
        "enderman" => {
            humanoid(p, s);
            let carried =
                matches!(e.meta.get(&16), Some(rc_proto_v777::entity::Meta::BlockState(Some(st))) if *st != 0);
            for n in ["right_arm", "left_arm", "right_leg", "left_leg"] {
                p.set(n, |x| x[3] = (x[3] * 0.5).clamp(-0.4, 0.4));
            }
            if carried {
                p.set("right_arm", |a| {
                    a[3] = -0.5;
                    a[5] = 0.05;
                });
                p.set("left_arm", |a| {
                    a[3] = -0.5;
                    a[5] = -0.05;
                });
            }
            if e.meta_bool(17) {
                p.set("head", |h| h[1] -= 5.0);
                p.set("hat", |h| h[1] += 5.0);
            }
        }
        "endermite" | "silverfish" => {
            let (n, ky, kx) = if e.kind == "endermite" { (4, 0.01, 0.1) } else { (7, 0.05, 0.2) };
            let mut seg = [[0.0f32; 2]; 7];
            for (i, sg) in seg.iter_mut().enumerate().take(n) {
                let ph = age * 0.9 + i as f32 * 0.15 * PI;
                let d = (i as i32 - 2).abs() as f32;
                sg[0] = ph.cos() * PI * ky * (1.0 + d);
                sg[1] = ph.sin() * PI * kx * d;
                p.set(SEGMENTS[i], |x| {
                    x[4] = sg[0];
                    x[0] = sg[1];
                });
            }
            if n == 7 {
                p.set("layer0", |x| x[4] = seg[2][0]);
                p.set("layer1", |x| {
                    x[4] = seg[4][0];
                    x[0] = seg[4][1];
                });
                p.set("layer2", |x| {
                    x[4] = seg[1][0];
                    x[0] = seg[1][1];
                });
            }
        }
        "warden" => warden(p, e, s, pt),
        "creaking" => {
            p.set("head", |h| {
                h[3] = s.pitch;
                h[4] = s.yaw;
            });
            if !matches!(e.meta.get(&16), Some(rc_proto_v777::entity::Meta::Bool(false)))
                && let Some(a) = keyframes::get("creaking.walk")
            {
                keyframes::apply_walk(p, a, s, 1.0, 1.0);
            }
            keyframes::play(p, "creaking.attack", e.extra[0], age);
            keyframes::play(p, "creaking.invulnerable", e.extra[1], age);
            keyframes::play(p, "creaking.death", e.extra[2], age);
        }
        "breeze" => {
            for (i, n) in ["idle", "shoot", "slide", "slide_back", "inhale", "jump"].iter().enumerate() {
                keyframes::play(p, &format!("breeze.{n}"), e.extra[i], age);
            }
        }
        "cod" | "tropical_fish" => {
            let a = if in_water(e) { 1.0 } else { 1.5 };
            let part = if e.kind == "cod" { "tail_fin" } else { "tail" };
            p.set(part, |t| t[4] = -a * 0.45 * (0.6 * age).sin());
        }
        "salmon" => {
            let (a, b) = if in_water(e) { (1.0, 1.0) } else { (1.3, 1.7) };
            p.set("body_back", |t| t[4] = -a * 0.25 * (b * 0.6 * age).sin());
        }
        "pufferfish" => {
            let f = 0.4 * (age * 0.2).sin();
            let (r, l) =
                if p.has("right_fin") { ("right_fin", "left_fin") } else { ("right_blue_fin", "left_blue_fin") };
            p.set(r, |x| x[5] = -0.2 + f);
            p.set(l, |x| x[5] = 0.2 - f);
        }
        "squid" | "glow_squid" => {
            let a = lerp(pt, e.extra[4], e.extra[3]);
            for t in &TENTACLES[..8] {
                p.set(t, |x| x[3] = a);
            }
        }
        "guardian" | "elder_guardian" => guardian(p, e, s, pt),
        "slime" | "sulfur_cube" => {}
        "magma_cube" => {
            let sq = lerp(pt, e.extra[1], e.extra[0]).max(0.0);
            for (i, c) in CUBES.iter().enumerate() {
                p.set(c, |x| x[1] = -(4.0 - i as f32) * sq * 1.7);
            }
        }
        "ghast" | "happy_ghast" => {
            for (i, t) in TENTACLES.iter().enumerate() {
                p.set(t, |x| x[3] = 0.2 * (age * 0.3 + i as f32).sin() + 0.4);
            }
            if e.kind == "happy_ghast" {
                if e.equipment[6].is_some() {
                    p.set("body", |b| {
                        b[6] = 0.9375;
                        b[7] = 0.9375;
                        b[8] = 0.9375;
                    });
                }
                // Goggles lowered if mounted (at least one passenger), raised otherwise.
                let ridden = !e.passengers.is_empty();
                p.set("goggles", |g| {
                    g[3] = if ridden { 0.0 } else { GOGGLES_UP };
                    g[1] = if ridden { 14.0 } else { 9.0 };
                });
            }
        }
        "nautilus" | "zombie_nautilus" => {
            // Head limited to ±10°, swim: walk position + age / 5, speed + 0.2.
            let lim = deg(10.0);
            p.set("body", |b| {
                b[4] = s.yaw.clamp(-lim, lim);
                b[3] = s.pitch.clamp(-lim, lim);
            });
            if let Some(a) = keyframes::get("nautilus.swimming") {
                keyframes::apply_walk_at(p, a, s.walk_pos + age / 5.0, s.walk_speed + 0.2, 2.0, 3.0);
            }
            if e.equipment[6].is_some() {
                p.hide("corals");
            }
        }
        _ => return false,
    }
    true
}

const BLAZE_RODS: [&str; 12] =
    ["part0", "part1", "part2", "part3", "part4", "part5", "part6", "part7", "part8", "part9", "part10", "part11"];
const SEGMENTS: [&str; 7] = ["segment0", "segment1", "segment2", "segment3", "segment4", "segment5", "segment6"];

/// Triangle wave of the game: −1..1 over `period`.
fn triangle_wave(index: f32, period: f32) -> f32 {
    ((index % period - period * 0.5).abs() - period * 0.25) / (period * 0.25)
}

/// Change of a synchronised value seen at the tick (key `v + 1`): true once per change.
fn changed(x: &mut f32, v: i32) -> bool {
    let key = v as f32 + 1.0;
    if *x != key {
        *x = key;
        true
    } else {
        false
    }
}

/// Baby axolotl: animation only (extra 10 = number + 1, 11 = start encoded by `State`);
/// 1 swim, 2 walk, 3 walk underwater, 4 idle in water, 5 idle on the bottom, 6 idle on land,
/// 7 play dead. The model's underwater walk reads the land-walk state (game, reproduced).
fn baby_axolotl(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    let (which, st) = (e.extra[10] as i32, e.extra[11]);
    let on = |k: i32| if which == k { st } else { 0.0 };
    if which == 2
        && let Some(a) = keyframes::get("baby_axolotl.axolotl_walk_floor")
    {
        keyframes::apply_walk(p, a, s, 15.0, 30.0);
    }
    keyframes::play(p, "baby_axolotl.swim", on(1), s.age);
    keyframes::play(p, "baby_axolotl.walk_floor_underwater", on(2), s.age);
    keyframes::play(p, "baby_axolotl.idle_floor", on(6), s.age);
    keyframes::play(p, "baby_axolotl.idle_underwater", on(4), s.age);
    keyframes::play(p, "baby_axolotl.idle_floor_underwater", on(5), s.age);
    keyframes::play(p, "baby_axolotl.play_dead", on(7), s.age);
}

/// Iron golem: arms raised on attack (event 4, 10 ticks, `timers[0]`), poppy offered
/// (events 11/34, 400 ticks in extra 0), walk as a triangle wave of period 13.
fn iron_golem(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let (v, pos) = (s.walk_speed, s.walk_pos);
    let attack = if e.timers[0] > 0 { e.timers[0] as f32 - pt } else { 0.0 };
    let offer = e.extra[0] as i32;
    if attack > 0.0 {
        let x = -2.0 + 1.5 * triangle_wave(attack, 10.0);
        p.set("right_arm", |a| a[3] = x);
        p.set("left_arm", |a| a[3] = x);
    } else if offer > 0 {
        p.set("right_arm", |a| a[3] = -0.8 + 0.025 * triangle_wave(offer as f32, 70.0));
        p.set("left_arm", |a| a[3] = 0.0);
    } else {
        let t = triangle_wave(pos, 13.0);
        p.set("right_arm", |a| a[3] = (-0.2 + 1.5 * t) * v);
        p.set("left_arm", |a| a[3] = (-0.2 - 1.5 * t) * v);
    }
    p.set("head", |h| {
        h[4] = s.yaw;
        h[3] = s.pitch;
    });
    let t = triangle_wave(pos, 13.0);
    p.set("right_leg", |l| {
        l[3] = -1.5 * t * v;
        l[4] = 0.0;
    });
    p.set("left_leg", |l| {
        l[3] = 1.5 * t * v;
        l[4] = 0.0;
    });
}

/// Copper golem: extra 0 inactive (start), 1 next start from rest (tick, 0 = to be drawn),
/// 2-5 interactions (takes, takes nothing, drops, drops nothing). State in 17.
fn copper_golem(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    let holding = e.equipment[0].is_some() || e.equipment[1].is_some();
    let walk = if holding { "copper_golem.walk_item" } else { "copper_golem.walk" };
    if let Some(a) = keyframes::get(walk) {
        keyframes::apply_walk(p, a, s, 2.0, 2.5);
    }
    if holding {
        p.set("right_arm", |a| {
            a[3] = a[3].min(-0.872_664_63);
            a[4] = a[4].min(-0.113_446_4);
            a[5] = a[5].min(-0.064_577_185);
        });
        p.set("left_arm", |a| {
            a[3] = a[3].min(-0.872_664_63);
            a[4] = a[4].max(0.113_446_4);
            a[5] = a[5].max(0.064_577_185);
        });
    }
    let x = &e.extra;
    keyframes::play(p, "copper_golem.idle", x[0], s.age);
    keyframes::play(p, "copper_golem.chest_interaction_noitem_get", x[2], s.age);
    keyframes::play(p, "copper_golem.chest_interaction_noitem_noget", x[3], s.age);
    keyframes::play(p, "copper_golem.chest_interaction_item_drop", x[4], s.age);
    keyframes::play(p, "copper_golem.chest_interaction_item_nodrop", x[5], s.age);
}

/// Strider: roll and legs (speed bounded to 0.25), body turned towards the head unless mounted,
/// bristles (adult: in z, baby: in x).
fn strider(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    let pos = s.walk_pos;
    let v = s.walk_speed.min(0.25);
    let ridden = !e.passengers.is_empty();
    p.set("body", |b| {
        if ridden {
            b[3] = 0.0;
            b[4] = 0.0;
        } else {
            b[3] = s.pitch;
            b[4] = s.yaw;
        }
        b[5] = 0.1 * (pos * 1.5).sin() * 4.0 * v;
    });
    let h = pos * 1.5 * 0.5;
    p.set("left_leg", |l| {
        l[3] = h.sin() * 2.0 * v;
        l[5] = PI / 18.0 * h.cos() * v;
    });
    p.set("right_leg", |l| {
        l[3] = (h + PI).sin() * 2.0 * v;
        l[5] = PI / 18.0 * (h + PI).cos() * v;
    });
    let flow = (pos * 1.5 + PI).cos() * v;
    let age = s.age;
    let bristle = |p: &mut Pose<'_>, names: [&str; 3], axis: usize| {
        let adds = [
            flow * 0.6 + 0.1 * (age * 0.4).sin(),
            flow * 1.2 + 0.1 * (age * 0.2).sin(),
            flow * 1.3 + 0.05 * (age * -0.4).sin(),
        ];
        for (n, d) in names.iter().zip(adds) {
            p.set(n, |x| x[axis] += d);
        }
    };
    if s.baby {
        p.set("body", |b| b[1] = 17.25 - (pos * 1.5).cos() * 2.0 * v);
        p.set("left_leg", |l| l[1] = 20.0 + 2.0 * (h + PI).sin() * 2.0 * v);
        p.set("right_leg", |l| l[1] = 20.0 + 2.0 * h.sin() * 2.0 * v);
        bristle(p, ["bristle2", "bristle1", "bristle0"], 3);
    } else {
        for (n, z) in [
            ("right_bottom_bristle", -1.221_730_5),
            ("right_middle_bristle", -1.134_464),
            ("right_top_bristle", -0.872_664_63),
            ("left_top_bristle", 0.872_664_63),
            ("left_middle_bristle", 1.134_464),
            ("left_bottom_bristle", 1.221_730_5),
        ] {
            p.set(n, |x| x[5] = z);
        }
        bristle(p, ["right_top_bristle", "right_middle_bristle", "right_bottom_bristle"], 5);
        bristle(p, ["left_top_bristle", "left_middle_bristle", "left_bottom_bristle"], 5);
        p.set("body", |b| b[1] = 2.0 - 2.0 * (pos * 1.5).cos() * 2.0 * v);
        p.set("left_leg", |l| l[1] = 8.0 + 2.0 * (h + PI).sin() * 2.0 * v);
        p.set("right_leg", |l| l[1] = 8.0 + 2.0 * h.sin() * 2.0 * v);
    }
}

/// Warden: extra 0 pose seen (key), 1-2 tentacles (10..0), starts 3 attack, 4 sonic boom,
/// 5 dig, 6 emerge, 7 roar, 8 sniff.
fn warden(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    // Walk.
    let sm = (3.0 * s.walk_speed).min(0.5);
    let ap = s.walk_pos * 0.8662;
    let (c, sn) = (ap.cos(), ap.sin());
    let smm = sm.min(0.35);
    p.set("head", |h| {
        h[5] += 0.3 * sn * sm;
        h[3] += 1.2 * (ap + PI / 2.0).cos() * smm;
    });
    p.set("body", |b| {
        b[5] = 0.1 * sn * sm;
        b[3] = c * smm;
    });
    p.set("left_leg", |l| l[3] = c * sm);
    p.set("right_leg", |l| l[3] = (ap + PI).cos() * sm);
    p.set("left_arm", |a| {
        a[3] = -(0.8 * c * sm);
        a[5] = 0.0;
        a[4] = 0.0;
        a[2] = 1.0;
        a[0] = 13.0;
        a[1] = -13.0;
    });
    p.set("right_arm", |a| {
        a[3] = -(0.8 * sn * sm);
        a[5] = 0.0;
        a[4] = 0.0;
        a[2] = 1.0;
        a[0] = -13.0;
        a[1] = -13.0;
    });
    // Rest.
    let sa = s.age * 0.1;
    p.set("head", |h| {
        h[5] += 0.06 * sa.cos();
        h[3] += 0.06 * sa.sin();
    });
    p.set("body", |b| {
        b[5] += 0.025 * sa.sin();
        b[3] += 0.025 * sa.cos();
    });
    // Tentacles (event 61).
    let x = &e.extra;
    let tendril = lerp(pt, x[2], x[1]) / 10.0;
    let tx = tendril * ((f64::from(s.age) * 2.25).cos() * std::f64::consts::PI * f64::from(0.1f32)) as f32;
    p.set("left_tendril", |t| t[3] = tx);
    p.set("right_tendril", |t| t[3] = -tx);
    for (i, n) in ["attack", "sonic_boom", "dig", "emerge", "roar", "sniff"].iter().enumerate() {
        keyframes::play(p, &format!("warden.{n}"), x[3 + i], s.age);
    }
}

fn add_rot(p: &mut Pose<'_>, name: &str, r: [f32; 3]) {
    p.set(name, |x| {
        x[3] += r[0];
        x[4] += r[1];
        x[5] += r[2];
    });
}

/// Adult axolotl (§ 9.1): smoothed factors of `tick`, additive formulas.
fn axolotl(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let f = |i: usize| {
        let v = lerp(pt, e.extra[4 + i], e.extra[i]) / 10.0;
        (1.0 - (PI * v).cos()) / 2.0
    };
    let (d, w, g, m) = (f(0), f(1), f(2), f(3));
    let n = 1.0 - m;
    let r = 1.0 - g.min(m);
    let age = s.age;
    p.set("body", |b| b[4] += s.yaw);
    // Swim.
    let k = m.min(w);
    if k > 1e-5 {
        let a = age * 0.33;
        let (sa, ca) = a.sin_cos();
        let b = 0.13 * sa;
        p.set("body", |x| {
            x[3] += (s.pitch + b) * k;
            x[1] -= 0.45 * ca * k;
        });
        p.set("head", |x| x[3] -= 1.8 * b * k);
        p.set("top_gills", |x| x[3] += (-0.5 * sa - 0.8) * k);
        let gl = (0.3 * sa + 0.9) * k;
        p.set("left_gills", |x| x[4] += gl);
        p.set("right_gills", |x| x[4] -= gl);
        p.set("tail", |x| x[4] += 0.3 * (0.9 * a).cos() * k);
        add_rot(p, "left_hind_leg", [1.884_955_8 * k, -0.4 * sa * k, PI / 2.0 * k]);
        add_rot(p, "left_front_leg", [1.884_955_8 * k, (-0.2 * ca - 0.1) * k, PI / 2.0 * k]);
    }
    // Motionless floating.
    let k = n.min(w);
    if k > 1e-5 {
        let a = age * 0.075;
        let c = a.cos();
        let sv = 0.15 * a.sin();
        let bx = (-0.15 + 0.075 * c) * k;
        p.set("body", |x| {
            x[3] += bx;
            x[1] -= sv * k;
        });
        p.set("head", |x| x[3] -= bx);
        p.set("top_gills", |x| x[3] += 0.2 * c * k);
        let gl = (-0.3 * c - 0.19) * k;
        p.set("left_gills", |x| x[4] += gl);
        p.set("right_gills", |x| x[4] -= gl);
        add_rot(p, "left_hind_leg", [(3.0 * PI / 4.0 - 0.11 * c) * k, 0.471_238_94 * k, 1.727_876_1 * k]);
        add_rot(p, "left_front_leg", [(PI / 4.0 - 0.2 * c) * k, 2.042_035 * k, 0.0]);
        p.set("tail", |x| x[4] += 0.5 * c * k);
    }
    // Crawling on the ground.
    let k = m.min(g);
    if k > 1e-5 {
        let a = age * 0.11;
        let c = a.cos();
        let h = (c * c - 2.0 * c) / 5.0;
        let fw = 0.7 * c;
        p.set("head", |x| x[4] += 0.09 * c * k);
        p.set("tail", |x| x[4] += 0.09 * c * k);
        let gl = (0.6 - 0.08 * (c * c + 2.0 * a.sin())) * k;
        p.set("top_gills", |x| x[3] += gl);
        p.set("left_gills", |x| x[4] -= gl);
        p.set("right_gills", |x| x[4] += gl);
        add_rot(p, "left_hind_leg", [0.942_477_9 * k, (1.5 - h) * k, -0.1 * k]);
        add_rot(p, "left_front_leg", [1.099_557_4 * k, (PI / 2.0 - fw) * k, 0.0]);
        add_rot(p, "right_hind_leg", [0.942_477_9 * k, (-1.0 - h) * k, 0.0]);
        add_rot(p, "right_front_leg", [1.099_557_4 * k, (-PI / 2.0 - fw) * k, 0.0]);
    }
    // Still on the ground.
    let k = n.min(g);
    if k > 1e-5 {
        let a = age * 0.09;
        let (sa, ca) = a.sin_cos();
        let mm = sa * sa - 2.0 * sa;
        let m2 = ca * ca - 3.0 * sa;
        p.set("head", |x| {
            x[3] += -0.09 * mm * k;
            x[5] += -0.2 * k;
        });
        p.set("tail", |x| x[4] += (-0.1 + 0.1 * mm) * k);
        let gl = (0.6 + 0.05 * m2) * k;
        p.set("top_gills", |x| x[3] += gl);
        p.set("left_gills", |x| x[4] -= gl);
        p.set("right_gills", |x| x[4] += gl);
        add_rot(p, "left_hind_leg", [1.1 * k, 1.0 * k, 0.0]);
        add_rot(p, "left_front_leg", [0.8 * k, 2.3 * k, -0.5 * k]);
    }
    // Playing dead.
    if d > 1e-5 {
        add_rot(p, "left_hind_leg", [1.413_716_7 * d, 1.099_557_4 * d, PI / 4.0 * d]);
        add_rot(p, "left_front_leg", [PI / 4.0 * d, 2.042_035 * d, 0.0]);
        p.set("body", |x| {
            x[3] += -0.15 * d;
            x[5] += 0.35 * d;
        });
    }
    // Mirror of the left legs onto the right ones.
    if r > 1e-5 {
        for (from, to) in [("left_hind_leg", "right_hind_leg"), ("left_front_leg", "right_front_leg")] {
            if let Some(l) = p.get(from).map(|l| [l[3], l[4], l[5]]) {
                add_rot(p, to, [l[0] * r, -l[1] * r, -l[2] * r]);
            }
        }
    }
}

const SX: [f32; 12] = [0.0, 0.0, 8.0, -8.0, -8.0, 8.0, 8.0, -8.0, 0.0, 0.0, 8.0, -8.0];
const SY: [f32; 12] = [-8.0, -8.0, -8.0, -8.0, 0.0, 0.0, 0.0, 0.0, 8.0, 8.0, 8.0, 8.0];
const SZ: [f32; 12] = [8.0, -8.0, 0.0, 0.0, -8.0, -8.0, 8.0, 8.0, 8.0, -8.0, 0.0, 0.0];

/// Guardian (§ 12.3): head, spines, tail; the eye keeps its pose (target and camera unknown here).
fn guardian(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    p.set("head", |h| {
        h[4] = s.yaw;
        h[3] = s.pitch;
    });
    let sp = lerp(pt, e.extra[4], e.extra[3]);
    let r = (1.0 - sp) * 0.55;
    for (i, name) in SPIKES.iter().enumerate() {
        let o = 1.0 + (s.age * 1.5 + i as f32).cos() * 0.01 - r;
        p.set(name, |x| {
            x[0] = SX[i] * o;
            x[1] = 16.0 + SY[i] * o;
            x[2] = SZ[i] * o;
        });
    }
    let q = lerp(pt, e.extra[1], e.extra[0]).sin() * PI;
    p.set("tail0", |x| x[4] = q * 0.05);
    p.set("tail1", |x| x[4] = q * 0.1);
    p.set("tail2", |x| x[4] = q * 0.15);
}

/// Remaining fuse of the sulfur cube (ticks, partial tick included); 0 not primed.
fn fuse_left(e: &Entity, pt: f32) -> f32 {
    if e.extra[4] >= 0.0 { e.extra[4] - pt + 1.0 } else { 0.0 }
}

/// Own scale of the cube mobs (§ 13). Sulfur cube: squash cancelled if it contains a
/// block (`block`; not for a skull), fuse swelling, then ½ for the adult.
fn cube_scale(e: &Entity, pt: f32, block: bool) -> Vec3 {
    let n = e.meta_int(18).unwrap_or(1).clamp(1, 127) as f32;
    let sulfur = e.kind == "sulfur_cube";
    let sq = if sulfur && block { 0.0 } else { lerp(pt, e.extra[1], e.extra[0]) };
    let w = 1.0 / (sq / (n * 0.5 + 1.0) + 1.0);
    let mut v = Vec3::new(w * n, n / w, w * n);
    if e.kind != "magma_cube" {
        v *= 0.999;
    }
    if sulfur {
        let m = fuse_left(e, pt);
        if m > 0.0 && m < 10.0 {
            v *= 1.0 + (1.0 - m / 10.0).clamp(0.0, 1.0).powi(4) * 0.3;
        }
        if !e.baby() {
            v *= 0.5;
        }
    }
    v
}

/// Overlay of the interior or contained block of the sulfur cube: full white (no red tint)
/// when the lit fuse blinks (5 ticks out of 10), otherwise the body overlay without white.
pub(super) fn fuse_overlay(e: &Entity, pt: f32, overlay: u32) -> u32 {
    let f = fuse_left(e, pt);
    if f > 0.0 && (f / 5.0) as i32 % 2 == 0 { 15 | 10 << 16 } else { overlay & 0xffff_0000 }
}

/// Translation of the flipped frame after the own scale, before the common −1.501: phantom
/// (0, 1.3125, 0.1875); sulfur cube (0, h − 1/16, 0), h = 0.98 (adult) or 1.24
/// (small), without the 1/16 if invisible.
pub(super) fn model_offset(e: &Entity) -> Vec3 {
    match e.kind.as_str() {
        "phantom" => Vec3::new(0.0, 1.3125, 0.1875),
        "sulfur_cube" => {
            let h = if e.baby() { 1.24 } else { 0.98 };
            let px = if e.flags() & 0x20 != 0 { 0.0 } else { 1.0 / 16.0 };
            Vec3::new(0.0, h - px, 0.0)
        }
        _ => Vec3::ZERO,
    }
}

/// Swallowed content of the sulfur cube (body slot): skull (index into `SKULLS`) or block
/// state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Swallowed {
    Skull(usize),
    Block(u32),
}

impl EntityRenderer {
    /// Recognised swallowed content; `None` without an item or without a matching block. Cached by
    /// (item, fingerprint of the `block_state` component): each frame, one component lookup
    /// and one hash table access, names are read only on the first pass.
    pub(super) fn swallowed(&mut self, e: &Entity) -> Option<Swallowed> {
        let stack = e.equipment[6].as_ref()?;
        let comp = self.ids.block_state.and_then(|id| stack.component(id));
        let key = (
            stack.item,
            comp.map_or(0, |b| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                b.hash(&mut h);
                h.finish() | 1
            }),
        );
        if let Some(&s) = self.swallowed.get(&key) {
            return s;
        }
        let name =
            usize::try_from(stack.item).ok().and_then(|i| self.item_names()?.get(i));
        let s = match name.and_then(|n| SKULLS.iter().position(|s| s.0 == n)) {
            Some(k) => Some(Swallowed::Skull(k)),
            None => item_block(&self.source, stack.item, comp).map(Swallowed::Block),
        };
        self.swallowed.insert(key, s);
        s
    }

    /// Content of the sulfur cube in the frame of model `m`: flipped block (½ for the small one),
    /// re-centred, fuse overlay; skull shrunk by 0.842 and raised by 0.625, as on a head.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_swallowed(
        &mut self,
        e: &Entity,
        content: Swallowed,
        m: Mat4,
        pt: f32,
        light: u32,
        overlay: u32,
        items: Option<&rc_assets::item::ItemModels>,
        walk: f32,
    ) {
        match content {
            Swallowed::Block(s) => {
                let half = if e.baby() { 0.5 } else { 1.0 };
                let b = m
                    * Mat4::from_rotation_x(PI)
                    * Mat4::from_scale(Vec3::splat(half))
                    * Mat4::from_translation(Vec3::new(-0.5, -0.518, -0.5));
                self.push_block(s, b, light, fuse_overlay(e, pt, overlay));
            }
            Swallowed::Skull(kind) => {
                let Some(stack) = &e.equipment[6] else { return };
                const K: f32 = 0.842_105_27;
                let mtx = m
                    * Mat4::from_scale(Vec3::splat(K))
                    * Mat4::from_translation(Vec3::new(0.0, 0.625, 0.0))
                    * Mat4::from_scale(Vec3::splat(1.1875));
                let source = self.source.clone();
                let info = usize::try_from(stack.item).ok().and_then(|i| source.items.get(i));
                let view = crate::stack::StackRef {
                    stack,
                    info,
                    ids: &self.ids,
                    data: &source,
                    hold: crate::stack::Hold::NONE,
                };
                let key = Self::worn_head_key(kind, items, &view);
                self.push_worn_skull(kind, key, mtx, light, walk, true);
            }
        }
    }
}

/// Block state of an item (block of the same name), properties of the `block_state` component applied
/// (string pairs; unknown property or value ignored, like the game).
fn item_block(data: &rc_data::VersionData, item: i32, comp: Option<&[u8]>) -> Option<u32> {
    let name = usize::try_from(item).ok().and_then(|i| data.registry("minecraft:item")?.entries.get(i))?;
    let block = data.blocks.iter().find(|b| &b.name == name)?;
    let Some(bytes) = comp else { return Some(block.default_state) };
    let mut r = rc_proto::Reader::new(bytes);
    let n = rc_proto::varint::read_varint(&mut r).ok()?;
    let mut wanted: Vec<(String, String)> = Vec::new();
    for _ in 0..n {
        let k = rc_proto::read_str(&mut r, 32767).ok()?;
        let v = rc_proto::read_str(&mut r, 32767).ok()?;
        wanted.push((k.to_owned(), v.to_owned()));
    }
    let base: Vec<(&str, &str)> = data.state_properties(block.default_state).collect();
    (block.first_state..block.first_state + block.state_count)
        .find(|&s| {
            data.state_properties(s).zip(&base).all(|((k, v), &(_, d))| {
                let want = wanted.iter().find(|w| w.0 == k && valid(data, block, k, &w.1)).map_or(d, |w| w.1.as_str());
                v == want
            })
        })
        .or(Some(block.default_state))
}

/// Value `v` exists for property `k` of the block.
fn valid(data: &rc_data::VersionData, block: &rc_data::BlockInfo, k: &str, v: &str) -> bool {
    block
        .properties
        .iter()
        .map(|&p| &data.properties[p as usize])
        .any(|p| p.name == k && p.values.iter().any(|x| x == v))
}

pub(super) fn rotations(e: &Entity, pt: f32, m: &mut Mat4) -> bool {
    let age = e.age as f32 + pt;
    let body = rot_lerp(pt, e.body_o, e.body);
    match e.kind.as_str() {
        "cod" | "salmon" | "tropical_fish" | "pufferfish" => {
            let water = in_water(e);
            if e.kind == "pufferfish" {
                *m *= Mat4::from_translation(Vec3::new(0.0, (age * 0.05).cos() * 0.08, 0.0));
            }
            *m *= Mat4::from_rotation_y(deg(180.0 - body));
            if e.kind == "pufferfish" {
                return true;
            }
            let (a, b) = if e.kind == "salmon" && !water { (1.3, 1.7) } else { (1.0, 1.0) };
            *m *= Mat4::from_rotation_y(deg(a * 4.3 * (b * 0.6 * age).sin()));
            if !water {
                let t = if e.kind == "cod" { Vec3::new(0.1, 0.1, -0.1) } else { Vec3::new(0.2, 0.1, 0.0) };
                *m *= Mat4::from_translation(t) * Mat4::from_rotation_z(deg(90.0));
            }
            true
        }
        "squid" | "glow_squid" => {
            let (up, down) = if e.baby() { (0.25, -0.6) } else { (0.5, -1.2) };
            let yb = rot_lerp(pt, e.extra[11], e.extra[10]);
            let xb = lerp(pt, e.extra[7], e.extra[6]);
            let zb = lerp(pt, e.extra[9], e.extra[8]);
            *m *= Mat4::from_translation(Vec3::new(0.0, up, 0.0))
                * Mat4::from_rotation_y(deg(180.0 - yb))
                * Mat4::from_rotation_x(deg(xb))
                * Mat4::from_rotation_y(deg(zb))
                * Mat4::from_translation(Vec3::new(0.0, down, 0.0));
            true
        }
        // Iron golem: walking sway (triangle wave, ±6.5° in z).
        "iron_golem" => {
            *m *= Mat4::from_rotation_y(deg(180.0 - body));
            let (pos, v) = e.walk_at(pt);
            if v >= 0.01 {
                let w = ((pos + 6.0) % 13.0 - 6.5).abs();
                *m *= Mat4::from_rotation_z(deg(6.5 * (w - 3.25) / 3.25));
            }
            true
        }
        // Phantom: pitch of the entity (scale and offset: `scale`, `model_offset`).
        "phantom" => {
            let x_rot = e.pitch_o + pt * (e.pitch - e.pitch_o);
            *m *= Mat4::from_rotation_y(deg(180.0 - body)) * Mat4::from_rotation_x(deg(x_rot));
            true
        }
        _ => false,
    }
}

/// Own render scale: cubes (size, squash; `block`: the sulfur cube contains a
/// block), phantom (1 + 0.15 × size).
pub(super) fn scale(e: &Entity, pt: f32, block: bool, scale: &mut Vec3) {
    match e.kind.as_str() {
        "slime" | "magma_cube" | "sulfur_cube" => *scale *= cube_scale(e, pt, block),
        "phantom" => *scale *= e.size_factor().unwrap_or(1.0),
        _ => {}
    }
}

/// Entity event specific to the family; true if handled.
pub(super) fn event(e: &mut Entity, id: i8) -> bool {
    match (e.kind.as_str(), id) {
        // Squids: tentacle cycle reset to zero.
        ("squid" | "glow_squid", 19) => {
            e.extra[0] = 0.0;
            true
        }
        // The following record the state and let the common handling continue (sounds…).
        // Iron golem: poppy offered (11, 400 ticks) or taken back (34).
        ("iron_golem", 11 | 34) => {
            e.extra[0] = if id == 11 { 400.0 } else { 0.0 };
            false
        }
        // Warden: attack (4, cuts the roar), tentacles (61), sonic boom (62).
        ("warden", 4) => {
            State::stop(&mut e.extra[7]);
            State::start(&mut e.extra[3], e.age as f32);
            false
        }
        ("warden", 61) => {
            e.extra[1] = 10.0;
            false
        }
        ("warden", 62) => {
            State::start(&mut e.extra[4], e.age as f32);
            false
        }
        // Creaking: invulnerable (66, 8 ticks); the attack (4, 15 ticks) goes through `timers[0]`.
        ("creaking", 66) => {
            e.extra[4] = 8.0;
            false
        }
        _ => false,
    }
}

/// ARGB tint of a layer channel (`Wool::Channel`): tropical fish, variant (index 17)
/// = shape | pattern << 8 | base colour << 16 | pattern colour << 24.
pub(super) fn tint(e: &Entity, ch: u8) -> u32 {
    match e.kind.as_str() {
        "tropical_fish" => {
            let v = e.meta_int(17).unwrap_or(0) as u32;
            let dye = (v >> (16 + 8 * u32::from(ch.min(1)))) & 0xff;
            0xff00_0000 | DYE_COLORS[dye as usize % 16]
        }
        _ => 0xffff_ffff,
    }
}

pub(super) fn shadow(k: &str) -> Option<(f32, f32)> {
    let r = match k {
        "axolotl" => 0.5,
        "cod" => 0.3,
        "salmon" => 0.4,
        // Variable radius (0.1 + 0.1 × state; n × 0.25 for the cubes): the engine caches the shadow
        // per kind, value of the most common state.
        "pufferfish" => 0.1,
        "tropical_fish" => 0.15,
        "squid" | "glow_squid" | "nautilus" | "zombie_nautilus" => 0.7,
        "guardian" => 0.5,
        "elder_guardian" => 1.2,
        "tadpole" => 0.14,
        "cave_spider" => 0.56,
        "creaking" => 0.6,
        "dolphin" | "iron_golem" => 0.7,
        "phantom" => 0.75,
        "warden" => 0.9,
        "slime" | "magma_cube" | "sulfur_cube" => 0.25,
        "ghast" => 1.5,
        "happy_ghast" => 2.0,
        _ => return None,
    };
    Some((r, 1.0))
}

/// Gaussian draw (Box-Muller) on a shared xorshift generator: the shaking is
/// random each frame, with no seed to reproduce.
fn gaussian() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let next = || {
        let mut x = STATE.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        STATE.store(x, Ordering::Relaxed);
        // 53 useful bits, in ]0, 1].
        ((x >> 11) as f64 + 1.0) / (1u64 << 53) as f64
    };
    let (u, v) = (next(), next());
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

/// Render offset of the "creepy" enderman (value 17): each frame, a Gaussian draw
/// in x and z with standard deviation 0.02 × the scale, which makes it shake.
pub(super) fn render_offset(e: &Entity) -> Option<[f64; 3]> {
    if e.kind != "enderman" || !e.meta_bool(17) {
        return None;
    }
    let d = 0.02 * f64::from(living_scale(e));
    Some([gaussian() * d, 0.0, gaussian() * d])
}

pub(super) fn tick(e: &mut Entity) {
    match e.kind.as_str() {
        "axolotl" if !e.baby() => tick_axolotl(e),
        "squid" | "glow_squid" => tick_squid(e),
        "guardian" | "elder_guardian" => tick_guardian(e),
        "slime" | "magma_cube" | "sulfur_cube" => tick_cube(e),
        "iron_golem" => e.extra[0] = (e.extra[0] - 1.0).max(0.0),
        "copper_golem" => tick_copper_golem(e),
        "warden" => {
            let pose = e.pose();
            let start = e.age as f32 - 1.0;
            let x = &mut e.extra;
            x[2] = x[1];
            x[1] = (x[1] - 1.0).max(0.0);
            if changed(&mut x[0], pose) {
                let slot = match pose {
                    13 => 6,
                    14 => 5,
                    11 => 7,
                    12 => 8,
                    _ => 0,
                };
                if slot != 0 {
                    State::start(&mut x[slot], start);
                }
            }
        }
        "creaking" => {
            // Attack: `timers[0]` (15 at event 4) is only counted down after this tick, the game
            // counts down before testing: still active as long as more than one tick remains.
            let attack = e.timers[0] > 1;
            let tearing = e.meta_bool(18);
            let age = e.age as f32;
            let x = &mut e.extra;
            x[4] = (x[4] - 1.0).max(0.0);
            State::animate_when(&mut x[0], attack, age);
            let invulnerable = x[4] > 0.0;
            State::animate_when(&mut x[1], invulnerable, age);
            State::animate_when(&mut x[2], tearing, age);
        }
        "breeze" => {
            let pose = e.pose();
            let (age, start) = (e.age as f32, e.age as f32 - 1.0);
            let x = &mut e.extra;
            // 0 rest, 1 shoot, 2 slide, 3 recoil, 4 inhale, 5 jump.
            if changed(&mut x[6], pose) {
                State::stop(&mut x[1]);
                State::stop(&mut x[0]);
                State::stop(&mut x[4]);
                State::stop(&mut x[5]);
                match pose {
                    16 => State::start_if_stopped(&mut x[1], start),
                    17 => State::start_if_stopped(&mut x[4], start),
                    15 => State::start_if_stopped(&mut x[2], start),
                    _ => {}
                }
            }
            if pose == 6 {
                State::start_if_stopped(&mut x[5], age);
            }
            State::start_if_stopped(&mut x[0], age);
            if pose != 15 && State::started(x[2]) {
                State::start(&mut x[3], age);
                State::stop(&mut x[2]);
            }
        }
        "axolotl" if e.baby() => {
            let dead = e.meta_bool(19);
            let water = in_water(e);
            let ground = e.on_ground;
            let moving = e.walk_speed > 1e-5 || e.yaw != e.yaw_o || e.pitch != e.pitch_o;
            let k = if dead {
                7
            } else if moving {
                if water && !ground {
                    1
                } else if !water && ground {
                    2
                } else {
                    3
                }
            } else if water && !ground {
                4
            } else if water {
                5
            } else {
                6
            };
            if e.extra[10] as i32 != k {
                e.extra[10] = k as f32;
                State::start(&mut e.extra[11], e.age as f32);
            }
        }
        _ => {}
    }
}

/// Copper golem (state in 17: 0 inactive, 1-4 interactions); AI off (15 & 1): nothing.
fn tick_copper_golem(e: &mut Entity) {
    if e.meta_byte(15) & 1 != 0 {
        return;
    }
    let state = e.meta_int(17).unwrap_or(0);
    let age = e.age as f32;
    // Game draw (200..239) replaced by a reproducible hash.
    let wait = 200.0 + (rand01(e, e.age) * 40.0).floor();
    let x = &mut e.extra;
    if state == 0 {
        for v in &mut x[2..6] {
            State::stop(v);
        }
        if x[1] == age {
            State::start(&mut x[0], age);
        } else if x[1] == 0.0 {
            x[1] = age + wait;
        }
        if age == x[1] + 10.0 {
            x[1] = 0.0;
        }
    } else {
        State::stop(&mut x[0]);
        x[1] = 0.0;
        for (i, v) in x[2..6].iter_mut().enumerate() {
            if i as i32 + 1 == state {
                State::start_if_stopped(v, age);
            } else {
                State::stop(v);
            }
        }
    }
}

/// Counters 0..10 (death, water, ground, movement): current in 0-3, old in 4-7.
fn tick_axolotl(e: &mut Entity) {
    let dead = e.meta_bool(19);
    let water = !dead && in_water(e);
    let ground = !dead && !water && e.on_ground;
    let moving = e.walk_speed > 1e-5 || e.yaw != e.yaw_o || e.pitch != e.pitch_o;
    for (i, on) in [dead, water, ground, moving].into_iter().enumerate() {
        e.extra[4 + i] = e.extra[i];
        e.extra[i] = (e.extra[i] + if on { 1.0 } else { -1.0 }).clamp(0.0, 10.0);
    }
}

/// Squid (§ 11): 0 movement, 1 old, 2 speed, 3 tentacle angle, 4 old,
/// 5 rotation speed, 6-7 bodyX, 8-9 bodyZ, 10-11 body yaw, 12 initialised.
fn tick_squid(e: &mut Entity) {
    if e.extra[12] == 0.0 {
        e.extra[12] = 1.0;
        e.extra[2] = 0.2 / (rand01(e, 1) + 1.0);
        e.extra[10] = e.body;
        e.extra[11] = e.body;
    }
    let water = in_water(e);
    let v = [e.pos[0] - e.pos_o[0], e.pos[1] - e.pos_o[1], e.pos[2] - e.pos_o[2]];
    let x = &mut e.extra;
    x[1] = x[0];
    x[4] = x[3];
    x[7] = x[6];
    x[9] = x[8];
    x[11] = x[10];
    x[0] += x[2];
    // The client caps at 2π; the server resets to zero through event 19 (`event`).
    if x[0] > 2.0 * PI {
        x[0] = 2.0 * PI;
    }
    if water {
        if x[0] < PI {
            let u = x[0] / PI;
            x[3] = (u * u * PI).sin() * PI * 0.25;
            if u > 0.75 {
                x[5] = 1.0;
            } else {
                x[5] *= 0.8;
            }
        } else {
            x[3] = 0.0;
            x[5] *= 0.99;
        }
        let yaw = -(v[0].atan2(v[2]).to_degrees() as f32);
        x[10] += (yaw - x[10]) * 0.1;
        x[8] += PI * x[5] * 1.5;
        let hl = (v[0] * v[0] + v[2] * v[2]).sqrt();
        let pitch = -(hl.atan2(v[1]).to_degrees() as f32);
        x[6] += (pitch - x[6]) * 0.1;
    } else {
        x[3] = x[0].sin().abs() * PI * 0.25;
        x[6] += (-90.0 - x[6]) * 0.02;
    }
}

/// Guardian (§ 12.2): 0-1 tail, 2 tail speed, 3-4 spines, 5 attack time,
/// 6 last target (bits), 7 initialised. Always assumed in water (air supply has no effect).
fn tick_guardian(e: &mut Entity) {
    let moving = e.meta_bool(16);
    let target = e.meta_int(17).unwrap_or(0);
    let dur = if e.kind == "elder_guardian" { 60.0 } else { 80.0 };
    if e.extra[7] == 0.0 {
        e.extra[7] = 1.0;
        e.extra[0] = rand01(e, 2) * 10.0;
        e.extra[1] = e.extra[0];
    }
    let x = &mut e.extra;
    x[1] = x[0];
    if moving {
        x[2] = if x[2] < 0.5 { 4.0 } else { x[2] + (0.5 - x[2]) * 0.1 };
    } else {
        x[2] += (0.125 - x[2]) * 0.2;
    }
    x[0] += x[2];
    x[4] = x[3];
    x[3] += if moving { -x[3] * 0.25 } else { (1.0 - x[3]) * 0.06 };
    if x[6].to_bits() != target as u32 {
        x[6] = f32::from_bits(target as u32);
        x[5] = 0.0;
    }
    if target != 0 && x[5] < dur {
        x[5] += 1.0;
    }
}

/// Cube mobs (§ 13.1): 0-1 squash, 2 target, 3 on ground at the previous tick; sulfur:
/// 4 fuse, 5 last fuse received (bits), 6 received.
fn tick_cube(e: &mut Entity) {
    let magma = e.kind == "magma_cube";
    let sulfur = e.kind == "sulfur_cube";
    // Maximum fuse: index 20 (19 = came out of a bucket, boolean).
    let fuse = e.meta_int(20).unwrap_or(-1);
    let ground = e.on_ground;
    let x = &mut e.extra;
    x[1] = x[0];
    x[0] += (x[2] - x[0]) * 0.5;
    let was = x[3] != 0.0;
    if ground && !was {
        x[2] = -0.5;
    } else if !ground && was {
        x[2] = 1.0;
    }
    x[2] *= if magma { 0.9 } else { 0.6 };
    x[3] = f32::from(u8::from(ground));
    if sulfur {
        if x[6] == 0.0 || x[5].to_bits() != fuse as u32 {
            x[6] = 1.0;
            x[5] = f32::from_bits(fuse as u32);
            x[4] = fuse as f32;
        } else if x[4] > 0.0 {
            x[4] -= 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::keyframes::testkit::*;
    use super::*;
    use rc_proto_v777::entity::Meta;

    const GOLEM: [(&str, Option<usize>); 5] =
        [("head", None), ("right_arm", None), ("left_arm", None), ("right_leg", None), ("left_leg", None)];

    /// Iron golem: arms raised during the attack (event 4: −2 + 1.5 × wave(t − pt, 10)),
    /// poppy (11): right arm at −0.8 + 0.025 × wave(400, 70), taken back (34). Before: generic
    /// human walk, dangling arms.
    #[test]
    fn iron_golem_attack_and_flower() {
        let l = test_layer(&GOLEM);
        let mut all = spawn("iron_golem");
        all.tick();
        fire(&mut all, 4);
        let p = posed(&l, &all.map[&7], 0.25);
        let want = -2.0 + 1.5 * triangle_wave(9.75, 10.0);
        assert!((part(&p, "right_arm")[3] - want).abs() < 1e-6);
        assert_eq!(part(&p, "left_arm")[3], part(&p, "right_arm")[3]);
        for _ in 0..10 {
            all.tick();
        }
        fire(&mut all, 11);
        let p = posed(&l, &all.map[&7], 0.0);
        let want = -0.8 + 0.025 * triangle_wave(400.0, 70.0);
        assert!((part(&p, "right_arm")[3] - want).abs() < 1e-6);
        assert_eq!(part(&p, "left_arm")[3], 0.0);
        all.tick();
        assert_eq!(all.map[&7].extra[0], 399.0);
        fire(&mut all, 34);
        let p = posed(&l, &all.map[&7], 0.0);
        assert!((part(&p, "right_arm")[3]).abs() < 1e-6);
    }

    /// Warden: sonic boom (62) and attack (4) started at the event tick, the attack cuts
    /// the roar; "emerge" pose (13) seen at the next tick, start at the reception tick;
    /// tentacles (61) over 10 ticks.
    #[test]
    fn warden_event_and_pose_states() {
        let mut all = spawn("warden");
        for _ in 0..5 {
            all.tick();
        }
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(13));
        all.tick();
        let e = &all.map[&7];
        assert_eq!(State::start_of(e.extra[6]), Some(5.0));
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(11));
        all.tick();
        assert_eq!(State::start_of(all.map[&7].extra[7]), Some(6.0));
        fire(&mut all, 4);
        let e = &all.map[&7];
        assert_eq!(State::start_of(e.extra[3]), Some(7.0));
        assert!(!State::started(e.extra[7]));
        fire(&mut all, 62);
        fire(&mut all, 61);
        assert_eq!(State::start_of(all.map[&7].extra[4]), Some(7.0));
        all.tick();
        let e = &all.map[&7];
        assert_eq!((e.extra[1], e.extra[2]), (9.0, 10.0));
    }

    /// Creaking: attack played 15 ticks (event 4), invulnerable 8 ticks (66), like the game
    /// which counts down before testing.
    #[test]
    fn creaking_attack_and_invulnerable_windows() {
        let mut all = spawn("creaking");
        all.tick();
        fire(&mut all, 4);
        fire(&mut all, 66);
        let mut attack = 0;
        let mut inv = 0;
        for _ in 0..30 {
            all.tick();
            let e = &all.map[&7];
            attack += i32::from(State::started(e.extra[0]));
            inv += i32::from(State::started(e.extra[1]));
        }
        assert_eq!((attack, inv), (14, 7));
    }

    /// Enderman: carried block (16) → arms at −0.5; "creepy" (17) → head 5 px higher, hat
    /// compensated; limbs otherwise halved and bounded to ±0.4.
    #[test]
    fn enderman_carry_and_creepy() {
        let l = test_layer(&[
            ("head", None),
            ("hat", Some(0)),
            ("body", None),
            ("right_arm", None),
            ("left_arm", None),
            ("right_leg", None),
            ("left_leg", None),
        ]);
        let mut all = spawn("enderman");
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(16, Meta::BlockState(Some(9)));
        e.meta.insert(17, Meta::Bool(true));
        let p = posed(&l, e, 0.0);
        assert_eq!(part(&p, "right_arm")[3], -0.5);
        assert_eq!(part(&p, "left_arm")[5], -0.05);
        assert_eq!(part(&p, "head")[1], -5.0);
        assert_eq!(part(&p, "hat")[1], 5.0);
    }

    /// Creepy enderman: Gaussian render offset (standard deviation 0.02) in x and z, never in y;
    /// no offset otherwise.
    #[test]
    fn enderman_creepy_shakes() {
        let mut all = spawn("enderman");
        let e = all.map.get_mut(&7).unwrap();
        assert!(render_offset(e).is_none());
        e.meta.insert(17, Meta::Bool(true));
        let n = 20000;
        let (mut sx, mut sxx, mut sz) = (0.0, 0.0, 0.0);
        for _ in 0..n {
            let o = render_offset(e).unwrap();
            assert_eq!(o[1], 0.0);
            sx += o[0];
            sxx += o[0] * o[0];
            sz += o[2];
        }
        let n = f64::from(n);
        assert!((sxx / n).sqrt() > 0.018 && (sxx / n).sqrt() < 0.022);
        assert!((sx / n).abs() < 0.002 && (sz / n).abs() < 0.002);
    }

    /// Idle axolotl whose head alone turns: stays at rest (the game compares the entity's yaw, not
    /// the head's).
    #[test]
    fn axolotl_head_turn_is_not_moving() {
        let mut all = spawn("axolotl");
        all.tick();
        all.tick();
        let e = all.map.get_mut(&7).unwrap();
        e.head += 30.0;
        all.tick();
        let e = &all.map[&7];
        assert_eq!(e.extra[3], 0.0, "movement");
    }

    /// Blaze rods: high ring of radius 9 at t = 0 on the x axis.
    #[test]
    fn blaze_rods_orbit() {
        let names: Vec<(&str, Option<usize>)> =
            std::iter::once(("head", None)).chain(BLAZE_RODS.iter().map(|n| (*n, None))).collect();
        let l = test_layer(&names);
        let all = spawn("blaze");
        let p = posed(&l, &all.map[&7], 0.0);
        let r = part(&p, "part0");
        assert!((r[0] - 9.0).abs() < 1e-5 && r[2].abs() < 1e-5 && (r[1] - (-2.0 + 1.0)).abs() < 1e-5);
        let r = part(&p, "part1");
        assert!(r[0].abs() < 1e-4 && (r[2] - 9.0).abs() < 1e-4);
    }

    /// Phantom of size 4: scale 1 + 0.15 × 4 = 1.6 then translation (0, 1.3125, 0.1875)
    /// of the flipped frame. Before: neither (drawn too high, size ignored).
    #[test]
    fn phantom_size_scale_and_offset() {
        let mut all = spawn("phantom");
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(16, Meta::Int(4));
        let mut s = Vec3::ONE;
        scale(e, 0.0, false, &mut s);
        assert!((s - Vec3::splat(1.6)).abs().max_element() < 1e-6, "{s}");
        assert_eq!(model_offset(e), Vec3::new(0.0, 1.3125, 0.1875));
    }

    /// Sulfur cube: squash cancelled only for a contained block (not for a skull),
    /// raised by h − 1/16 (without the 1/16 when invisible), lit fuse: full white 5 ticks out of 10.
    #[test]
    fn sulfur_cube_content_and_fuse() {
        let mut all = spawn("sulfur_cube");
        let e = all.map.get_mut(&7).unwrap();
        e.extra[0] = 1.0;
        e.extra[1] = 1.0;
        e.extra[4] = -1.0;
        let squished = cube_scale(e, 0.0, false);
        assert_ne!(squished.x, squished.y);
        let block = cube_scale(e, 0.0, true);
        assert!((block.x - block.y).abs() < 1e-6);
        assert!((model_offset(e).y - (0.98 - 1.0 / 16.0)).abs() < 1e-6);
        e.meta.insert(0, Meta::Byte(0x20));
        assert_eq!(model_offset(e).y, 0.98);
        let body = 3 << 16;
        assert_eq!(fuse_overlay(e, 0.0, body), body);
        // Fuse 12: remaining 12 − 0 + 1 = 13 → (13 / 5) = 2, even: lit.
        e.extra[4] = 12.0;
        assert_eq!(fuse_overlay(e, 0.0, body), 15 | 10 << 16);
        e.extra[4] = 7.0;
        assert_eq!(fuse_overlay(e, 0.0, body), body);
    }

    /// Swallowed block: default state of the block of the same name, `block_state` component properties
    /// applied, unknown value ignored.
    #[test]
    fn swallowed_block_state() {
        let data = rc_data::load_cached("26.3").expect("26.3 data in cache (rc_data::load_cached)");
        let items = data.registry("minecraft:item").expect("item registry");
        let item = items.entries.iter().position(|n| n == "minecraft:oak_log").expect("oak_log") as i32;
        let block = data.blocks.iter().find(|b| b.name == "minecraft:oak_log").expect("oak_log block");
        assert_eq!(item_block(&data, item, None), Some(block.default_state));
        let comp = |v: &str| {
            let mut b = vec![1u8, 4];
            b.extend_from_slice(b"axis");
            b.push(v.len() as u8);
            b.extend_from_slice(v.as_bytes());
            b
        };
        let s = item_block(&data, item, Some(&comp("x"))).unwrap();
        assert_eq!(data.state_properties(s).find(|p| p.0 == "axis").map(|p| p.1), Some("x"));
        assert_eq!(item_block(&data, item, Some(&comp("w"))), Some(block.default_state));
    }

    /// Harness goggles: raised (−0.7854; y 9) without a passenger, lowered (0; y 14) when mounted.
    #[test]
    fn happy_ghast_goggles_follow_riders() {
        let l = test_layer(&[("body", None), ("goggles", None)]);
        let mut all = spawn("happy_ghast");
        let g = *part(&posed(&l, &all.map[&7], 0.0), "goggles");
        assert_eq!((g[3], g[1]), (GOGGLES_UP, 9.0));
        all.map.get_mut(&7).unwrap().passengers.push(8);
        let g = *part(&posed(&l, &all.map[&7], 0.0), "goggles");
        assert_eq!((g[3], g[1]), (0.0, 14.0));
    }
}
