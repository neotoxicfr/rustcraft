//! Land animals (`entities-animals.md`): equines, camels, llamas, mooshroom, ocelot, rabbit, parrot, polar bear.
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
//! `skin` key of equines, camels and llamas: bits 0‑11 item of the `body` slot (0 = none),
//! bit 12 saddle, bits 16‑23 kind-specific variant (horse: colour | marking << 3; llama: 0‑3).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use rc_proto_v777::entity::Meta;

use super::keyframes::State;
use super::*;

/// Kinds handled here.
pub(super) fn ours(k: &str) -> bool {
    matches!(
        k,
        "horse"
            | "donkey"
            | "mule"
            | "skeleton_horse"
            | "zombie_horse"
            | "camel"
            | "camel_husk"
            | "llama"
            | "trader_llama"
            | "mooshroom"
            | "ocelot"
            | "rabbit"
            | "parrot"
            | "polar_bear"
    )
}

fn equine(k: &str) -> bool {
    matches!(k, "horse" | "donkey" | "mule" | "skeleton_horse" | "zombie_horse")
}

const SADDLE: u32 = 1 << 12;
const HORSE_COLORS: [&str; 7] = ["white", "creamy", "chestnut", "brown", "black", "gray", "darkbrown"];
const HORSE_MARKINGS: [&str; 5] = ["", "white", "whitefield", "whitedots", "blackdots"];
const LLAMAS: [&str; 4] = ["creamy", "white", "brown", "gray"];
const PARROTS: [&str; 5] = ["red_blue", "blue", "green", "yellow_blue", "grey"];
/// Rabbits: texture index 0‑6 (variants), 7 "Toast".
const RABBITS: [&str; 8] = ["brown", "white", "black", "white_splotched", "gold", "salt", "caerbannog", "toast"];

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

/// Textures of an equipment layer (`asset`, layer type), paths `entity/equipment/<type>/<texture>`.
fn equipment_textures(r: &EntityRenderer, asset: &str, ty: &str) -> Vec<String> {
    r.equipment
        .get(asset)
        .and_then(|t| t.get(ty))
        .map(|l| l.iter().map(|(tex, _)| format!("entity/equipment/{ty}/{tex}")).collect())
        .unwrap_or_default()
}

/// Equipment asset of the `body` item encoded in the key.
fn body_asset(r: &EntityRenderer, skin: u32) -> Option<&str> {
    let id = (skin & 0xfff) as usize;
    if id == 0 {
        return None;
    }
    let info = r.source.items.get(id)?;
    (!info.equip_asset.is_empty()).then_some(info.equip_asset.as_str())
}

pub(super) fn layers(r: &EntityRenderer, k: &str, baby: bool, skin: u32) -> Option<Vec<(String, String, Pass, Wool)>> {
    if !ours(k) {
        return None;
    }
    let b = if baby { "_baby" } else { "" };
    let mut v: Vec<(String, String, Pass, Wool)> = Vec::new();
    let mut push = |m: String, t: String, pass: Pass| v.push((m, t, pass, Wool::None));
    let var = (skin >> 16) & 0xff;
    let saddled = skin & SADDLE != 0 && !baby;
    match k {
        "horse" | "donkey" | "mule" | "skeleton_horse" | "zombie_horse" => {
            let tex = match k {
                "horse" => format!("entity/horse/horse_{}{b}", HORSE_COLORS[(var & 7) as usize % 7]),
                "skeleton_horse" => format!("entity/horse/horse_skeleton{b}"),
                "zombie_horse" => format!("entity/horse/horse_zombie{b}"),
                _ => format!("entity/horse/{k}{b}"),
            };
            push(format!("minecraft:{k}{b}#main"), tex, Pass::Cutout);
            let marking = ((var >> 3) & 7) as usize;
            if k == "horse" && (1..5).contains(&marking) {
                push(
                    format!("minecraft:horse{b}#main"),
                    format!("entity/horse/horse_markings_{}{b}", HORSE_MARKINGS[marking]),
                    Pass::Translucent,
                );
            }
            if !baby && let Some(asset) = body_asset(r, skin) {
                let model = match k {
                    "horse" => Some("minecraft:horse_armor#main"),
                    "skeleton_horse" | "zombie_horse" => Some("minecraft:undead_horse_armor#main"),
                    _ => None,
                };
                if let Some(model) = model {
                    // Dyeable leather: layer marked at the end of the function.
                    for t in equipment_textures(r, asset, "horse_body") {
                        push(model.into(), t, Pass::Cutout);
                    }
                }
            }
            if saddled {
                let ty = format!("{k}_saddle");
                let mut tex = equipment_textures(r, "minecraft:saddle", &ty);
                if tex.is_empty() {
                    tex.push(format!("entity/equipment/{ty}/saddle"));
                }
                for t in tex {
                    push(format!("minecraft:{k}#saddle"), t, Pass::Cutout);
                }
            }
        }
        "camel" | "camel_husk" => {
            let husk = k == "camel_husk";
            let (m, t) = match (husk, baby) {
                (true, _) => ("minecraft:camel#main", "entity/camel/camel_husk"),
                (false, true) => ("minecraft:camel_baby#main", "entity/camel/camel_baby"),
                (false, false) => ("minecraft:camel#main", "entity/camel/camel"),
            };
            push(m.into(), t.into(), Pass::Cutout);
            if skin & SADDLE != 0 && (husk || !baby) {
                let ty = format!("{k}_saddle");
                let mut tex = equipment_textures(r, "minecraft:saddle", &ty);
                if tex.is_empty() {
                    tex.push(format!("entity/equipment/{ty}/saddle"));
                }
                for t in tex {
                    push(format!("minecraft:{k}#saddle"), t, Pass::Cutout);
                }
            }
        }
        "llama" | "trader_llama" => {
            push(
                format!("minecraft:{k}{b}#main"),
                format!("entity/llama/llama_{}{b}", LLAMAS[(var as usize).min(3)]),
                Pass::Cutout,
            );
            // Decor: carpet (adult), otherwise the trader's decor.
            let carpet = if baby { None } else { body_asset(r, skin) };
            let decor = match carpet {
                Some(asset) => equipment_textures(r, asset, "llama_body"),
                None if k == "trader_llama" => {
                    let asset = if baby { "trader_llama_baby" } else { "trader_llama" };
                    let mut t = equipment_textures(r, &format!("minecraft:{asset}"), "llama_body");
                    if t.is_empty() {
                        t.push(format!("entity/equipment/llama_body/{asset}"));
                    }
                    t
                }
                None => Vec::new(),
            };
            for t in decor {
                push(format!("minecraft:llama{b}#decor"), t, Pass::Cutout);
            }
        }
        "mooshroom" => push(
            format!("minecraft:mooshroom{b}#main"),
            format!("entity/cow/mooshroom_{}{b}", if var == 1 { "brown" } else { "red" }),
            Pass::Cutout,
        ),
        "ocelot" => push(format!("minecraft:ocelot{b}#main"), format!("entity/cat/ocelot{b}"), Pass::Cutout),
        "rabbit" => push(
            format!("minecraft:rabbit{b}#main"),
            format!("entity/rabbit/rabbit_{}{b}", RABBITS[(var as usize).min(7)]),
            Pass::Cutout,
        ),
        "parrot" => push(
            "minecraft:parrot#main".into(),
            format!("entity/parrot/parrot_{}", PARROTS[(var as usize).min(4)]),
            Pass::Cutout,
        ),
        "polar_bear" => {
            push(format!("minecraft:polar_bear{b}#main"), format!("entity/bear/polarbear{b}"), Pass::Cutout)
        }
        _ => return None,
    }
    // Dyeable body armor layers (leather): item tint, otherwise default colour.
    if let Some(asset) = body_asset(r, skin)
        && let Some(defs) = r.equipment.get(asset).and_then(|t| t.get("horse_body"))
    {
        for l in &mut v {
            let tex = l.1.strip_prefix("entity/equipment/horse_body/");
            if let Some(&(_, Some(d))) = tex.and_then(|t| defs.iter().find(|(n, _)| n == t)) {
                l.3 = Wool::BodyDye(d);
            }
        }
    }
    Some(v)
}

/// Item of the `body` slot if it is equipped on the body with an asset (0 otherwise).
fn body_item(r: &EntityRenderer, e: &Entity) -> u32 {
    let Some(stack) = &e.equipment[6] else { return 0 };
    let Ok(id) = usize::try_from(stack.item) else { return 0 };
    match r.source.items.get(id) {
        Some(info) if info.equip_slot == 7 && !info.equip_asset.is_empty() && id < 0x1000 => id as u32,
        _ => 0,
    }
}

fn saddle_bit(e: &Entity) -> u32 {
    if e.equipment[7].is_some() { SADDLE } else { 0 }
}

/// Custom name exactly `name` (index 2).
fn named(e: &Entity, name: &str) -> bool {
    match e.meta.get(&2) {
        Some(rc_proto_v777::entity::Meta::Text(Some(n))) => {
            n.as_str().or_else(|| n.get("text").and_then(rc_proto::nbt::Nbt::as_str)) == Some(name)
        }
        _ => false,
    }
}

pub(super) fn skin(r: &EntityRenderer, e: &Entity) -> Option<u32> {
    let k = e.kind.as_str();
    if !ours(k) {
        return None;
    }
    let int = |i: u8| e.meta_int(i).unwrap_or(0);
    Some(match k {
        "horse" => {
            let v = int(19);
            let color = (v & 0xff).rem_euclid(7) as u32;
            let marking = ((v >> 8) & 0xff) as u32;
            let marking = if marking < 5 { marking } else { 0 };
            body_item(r, e) | saddle_bit(e) | (color | marking << 3) << 16
        }
        "donkey" | "mule" | "camel" | "camel_husk" => saddle_bit(e),
        "skeleton_horse" | "zombie_horse" => body_item(r, e) | saddle_bit(e),
        "llama" | "trader_llama" => body_item(r, e) | (int(21).clamp(0, 3) as u32) << 16,
        "mooshroom" => u32::from(int(18) == 1) << 16,
        "rabbit" => {
            let t = if named(e, "Toast") {
                7
            } else {
                match int(18) {
                    v @ 0..=5 => v as u32,
                    99 => 6,
                    _ => 0,
                }
            };
            t << 16
        }
        "parrot" => (int(20).clamp(0, 4) as u32) << 16,
        _ => 0,
    })
}

pub(super) fn animate(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) -> bool {
    let k = e.kind.as_str();
    match k {
        _ if equine(k) => horse(p, e, s, pt),
        "camel" | "camel_husk" => {
            let jc = (e.extra[0] - pt).max(0.0);
            let y = s.yaw.clamp(deg(-30.0), deg(30.0));
            let mut x = s.pitch.clamp(deg(-25.0), deg(45.0));
            if jc > 0.0 {
                x = (x + deg(45.0) * jc / 55.0).clamp(deg(-25.0), deg(70.0));
            }
            p.set("head", |h| {
                h[3] = x;
                h[4] = y;
            });
            // Walk, sit, stand up, rest, dash through keyframes, in the model's order.
            let pre = if s.baby { "camel_baby" } else { "camel" };
            let anim =
                |n: &str| keyframes::get(&format!("{pre}.{n}")).or_else(|| keyframes::get(&format!("camel.{n}")));
            let sitting = matches!(e.meta.get(&20), Some(rc_proto_v777::entity::Meta::Long(t)) if *t < 0);
            let since = s.age - e.extra[7];
            // Walk always applied, including sitting or dashing (its speed drops by itself).
            if let Some(a) = anim("walk") {
                keyframes::apply_walk(p, a, s, 2.0, 2.5);
            }
            if e.extra[5] != 0.0 {
                if sitting && since < 40.0 {
                    if let Some(a) = anim("sit") {
                        keyframes::apply_since(p, a, e.extra[7], s.age, 1.0);
                    }
                } else if sitting {
                    if let Some(a) = anim("sit_pose") {
                        keyframes::apply_since(p, a, e.extra[7] + 40.0, s.age, 1.0);
                    }
                } else if since < 52.0
                    && let Some(a) = anim("standup")
                {
                    keyframes::apply_since(p, a, e.extra[7], s.age, 1.0);
                }
            }
            if let Some(a) = anim("idle")
                && e.extra[2] != 0.0
            {
                keyframes::apply_since(p, a, e.extra[4], s.age, 1.0);
            }
            if !sitting
                && e.extra[8] >= 0.0
                && let Some(a) = anim("dash")
            {
                keyframes::apply_since(p, a, e.extra[8], s.age, 1.0);
            }
            // Reins: only when mounted (at least one passenger).
            if e.passengers.is_empty() {
                p.hide("reins");
            }
        }
        "llama" | "trader_llama" => {
            quadruped(p, s);
            if !(e.meta_bool(19) && !s.baby) {
                p.hide("right_chest");
                p.hide("left_chest");
            }
        }
        "mooshroom" => quadruped(p, s),
        "polar_bear" => {
            quadruped(p, s);
            let st = lerp(pt, e.extra[1], e.extra[0]) / 6.0;
            let s2 = st * st;
            if s2 > 0.0 {
                let a = if s.baby { 0.5 } else { 1.0 };
                p.set("body", |b| {
                    b[3] -= s2 * PI * 0.35;
                    b[1] += s2 * a * 2.0;
                });
                let mut yz = (0.0, 0.0);
                p.set("right_front_leg", |l| {
                    l[1] -= s2 * a * 20.0;
                    l[2] += s2 * a * 4.0;
                    l[3] -= s2 * PI * 0.45;
                    yz = (l[1], l[2]);
                });
                p.set("left_front_leg", |l| {
                    l[1] = yz.0;
                    l[2] = yz.1;
                    l[3] -= s2 * PI * 0.45;
                });
                p.set("head", |h| {
                    h[1] -= s2 * 24.0;
                    h[2] += s2 * 13.0;
                    h[3] += s2 * PI * 0.15;
                });
            }
        }
        "ocelot" | "cat" => feline(p, e, s, pt),
        "wolf" => wolf(p, e, s, pt),
        "fox" => fox(p, e, s, pt),
        "goat" => {
            quadruped(p, s);
            // Horns (19, 20; present by default) and charging head.
            if matches!(e.meta.get(&19), Some(Meta::Bool(false))) {
                p.hide("left_horn");
            }
            if matches!(e.meta.get(&20), Some(Meta::Bool(false))) {
                p.hide("right_horn");
            }
            let ram = goat_ram(e, s.baby);
            if ram != 0.0 {
                p.set("head", |h| h[3] = ram);
            } else if s.baby {
                p.set("head", |h| h[3] = PI / 8.0);
            }
        }
        "panda" => panda(p, e, s, pt),
        "turtle" => turtle(p, e, s),
        "armadillo" => armadillo(p, e, s),
        "sniffer" => sniffer(p, e, s),
        "frog" => frog(p, e, s),
        "bee" => bee(p, e, s, pt),
        "bat" => {
            if e.meta_byte(16) & 1 != 0 {
                p.set("head", |h| h[4] = s.yaw);
            }
            keyframes::play(p, "bat.flying", e.extra[0], s.age);
            keyframes::play(p, "bat.resting", e.extra[1], s.age);
        }
        "allay" => allay(p, e, s, pt),
        "rabbit" => {
            // State (`tick`): jump start in extra[3], head tilt start in extra[4]
            // (−1 = stopped). Head free only outside the tilt.
            let (tilt, hop) = (e.extra[4], e.extra[3]);
            if tilt < 0.0 {
                p.set("head", |h| {
                    h[3] = s.pitch;
                    h[4] = s.yaw;
                });
            }
            let b = if s.baby { "baby_rabbit" } else { "rabbit" };
            if hop >= 0.0
                && let Some(a) = keyframes::get(&format!("{b}.hop"))
            {
                keyframes::apply_since(p, a, hop, s.age, 1.0);
            }
            if tilt >= 0.0
                && let Some(a) = keyframes::get(&format!("{b}.idle_head_tilt"))
            {
                keyframes::apply_since(p, a, tilt, s.age, 1.0);
            }
        }
        "parrot" => parrot(p, e, s, pt),
        _ => return false,
    }
    true
}

/// Equines (§ 1.4): body, saddle and armor (same parts).
fn horse(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    // Model: 0 adult, 1 foal, 2 donkey/mule foal.
    let class = match (s.baby, e.kind.as_str()) {
        (false, _) => 0,
        (true, "donkey" | "mule") => 2,
        (true, _) => 1,
    };
    let (ang, ys, zs, xo, qo) = match class {
        0 => (PI / 12.0, 12.0, 4.0, -PI / 3.0, 0.0),
        1 => (PI / 12.0, 4.0, 0.0, -PI / 3.0, -PI / 2.0),
        _ => (PI / 3.0, 1.0, 0.5, 0.0, -PI / 4.0),
    };
    let x = &e.extra;
    let (eat, stand, mouth) = (lerp(pt, x[1], x[0]), lerp(pt, x[3], x[2]), lerp(pt, x[5], x[4]));
    let age_scale = if s.baby { 0.5 } else { 1.0 };
    let (pp, v, age) = (s.walk_pos, s.walk_speed, s.age);
    let yr = s.yaw.clamp(deg(-20.0), deg(20.0));
    let mut xr = s.pitch;
    if v > 0.2 {
        xr += (pp * 0.8).cos() * 0.15 * v;
    }
    let m = 1.0 - stand.max(eat);
    let sa = age.sin();
    p.set("head_parts", |h| {
        h[3] = stand * (PI / 12.0 + xr) + eat * (2.181_661_6 + sa * 0.05) + m * (PI / 6.0 + xr + mouth * sa * 0.05);
        h[4] = (stand + m) * yr;
        match class {
            0 => {
                h[1] += lerp(eat, lerp(stand, 0.0, -8.0), 7.0);
                h[2] = lerp(stand, h[2], -4.0);
            }
            1 => {
                h[1] += lerp(eat, lerp(stand, 0.0, -2.0), 2.0);
                h[2] = lerp(stand, h[2], -4.0);
            }
            _ => {
                h[1] = lerp(eat, h[1], -1.2);
                h[2] = lerp(stand, h[2], -3.6);
            }
        }
    });
    p.set("body", |b| b[3] = stand * (-PI / 4.0) + (1.0 - stand) * b[3]);
    let mut yz = (0.0, 0.0);
    p.set("left_front_leg", |l| {
        l[1] -= ys * stand;
        l[2] += zs * stand;
        yz = (l[1], l[2]);
    });
    p.set("right_front_leg", |l| {
        l[1] = yz.0;
        l[2] = yz.1;
    });
    // Stride shortened in water (× 0.2).
    let w = if s.in_water { 0.2 } else { 1.0 };
    let l = (w * pp * 0.6662 + PI).cos();
    let a = l * 0.8 * v;
    let o = (age * 0.6 + PI).cos();
    let walk = 1.0 - stand;
    p.set("left_front_leg", |x| x[3] = (xo + o) * stand + a * walk);
    p.set("right_front_leg", |x| x[3] = (xo - o) * stand - a * walk);
    p.set("left_hind_leg", |x| x[3] = ang * stand - l * 0.5 * v * walk);
    p.set("right_hind_leg", |x| x[3] = ang * stand + l * 0.5 * v * walk);
    if class == 2 {
        let mut y = 0.0;
        p.set("left_hind_leg", |x| {
            x[1] = lerp(stand, x[1], -0.3);
            y = x[1];
        });
        p.set("right_hind_leg", |x| x[1] = lerp(stand, y, -0.3));
    }
    let tail_moving = x[6] > 0.0;
    p.set("tail", |t| {
        t[3] = qo + PI / 6.0 + v * 0.75;
        t[1] += v * age_scale;
        t[2] += 2.0 * v * age_scale;
        t[4] = if tail_moving { (age * 0.7).cos() } else { 0.0 };
    });
    if class == 2 {
        // Pitch forced to −30°: only the head rotation depends on it.
        p.set("head_parts", |h| {
            h[3] = stand * (PI / 12.0 - PI / 6.0) + eat * (PI / 2.0 + sa * 0.05) + m * (mouth * sa * 0.05);
        });
    }
    // Reins: only with a passenger.
    if e.passengers.is_empty() {
        p.hide("left_saddle_line");
        p.hide("right_saddle_line");
    }
    if matches!(e.kind.as_str(), "donkey" | "mule") && !(e.meta_bool(19) && !s.baby) {
        p.hide("left_chest");
        p.hide("right_chest");
    }
}

/// Felines: crouching (pose 5), sprint (bit 0x08); cat only: sitting (18 & 1), lying and
/// relaxed (`tick` ramps in extra 0-5).
fn feline(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let a = if s.baby { 0.5 } else { 1.0 };
    let crouch = e.pose() == 5;
    let sprint = e.flags() & 0x08 != 0;
    let cat = e.kind == "cat";
    let sitting = cat && e.meta_byte(18) & 1 != 0;
    let x = &e.extra;
    let (lie, lie_tail, relax) =
        if cat { (lerp(pt, x[1], x[0]), lerp(pt, x[3], x[2]), lerp(pt, x[5], x[4])) } else { (0.0, 0.0, 0.0) };
    if crouch {
        p.set("body", |b| b[1] += a);
        p.set("head", |h| h[1] += 2.0 * a);
        p.set("tail1", |t| {
            t[1] += a;
            t[3] = PI / 2.0;
        });
        p.set("tail2", |t| {
            t[1] -= 4.0 * a;
            t[2] += 2.0 * a;
            t[3] = PI / 2.0;
        });
    } else if sprint {
        let y1 = p.get("tail1").map(|t| t[1]);
        p.set("tail1", |t| t[3] = PI / 2.0);
        p.set("tail2", |t| {
            if let Some(y) = y1 {
                t[1] = y;
            }
            t[2] += 2.0 * a;
            t[3] = PI / 2.0;
        });
    }
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    if !sitting {
        if !s.baby {
            p.set("body", |b| b[3] = PI / 2.0);
        }
        let (pp, v) = (s.walk_pos, s.walk_speed);
        let q = pp * 0.6662;
        let (lh, rh, lf, rf, swing) = if sprint {
            (q, q + 0.3, q + PI + 0.3, q + PI, PI / 10.0)
        } else {
            (q, q + PI, q + PI, q, if crouch { 0.471_238_94 } else { PI / 4.0 })
        };
        p.set("left_hind_leg", |l| l[3] = lh.cos() * v);
        p.set("right_hind_leg", |l| l[3] = rh.cos() * v);
        p.set("left_front_leg", |l| l[3] = lf.cos() * v);
        p.set("right_front_leg", |l| l[3] = rf.cos() * v);
        p.set("tail2", |t| t[3] = 1.727_876_1 + swing * pp.cos() * v);
    } else if !s.baby {
        p.set("body", |b| {
            b[3] = PI / 4.0;
            b[1] += -4.0 * a;
            b[2] += 5.0 * a;
        });
        p.set("head", |h| {
            h[1] += -3.3 * a;
            h[2] += a;
        });
        p.set("tail1", |t| {
            t[1] += 8.0 * a;
            t[2] += -2.0 * a;
            t[3] = 1.727_876_1;
        });
        p.set("tail2", |t| {
            t[1] += 2.0 * a;
            t[2] += -0.8 * a;
            t[3] = 2.670_354;
        });
        for leg in ["left_front_leg", "right_front_leg"] {
            p.set(leg, |l| {
                l[3] = -PI / 20.0;
                l[1] += 2.0 * a;
                l[2] -= 2.0 * a;
            });
        }
        for leg in ["left_hind_leg", "right_hind_leg"] {
            p.set(leg, |l| {
                l[3] = -PI / 2.0;
                l[1] += 3.0 * a;
                l[2] -= 4.0 * a;
            });
        }
    } else {
        p.set("body", |b| {
            b[3] += -0.436_332_32;
            b[1] += 1.25;
        });
        p.set("head", |h| h[2] += 0.75);
        p.set("tail1", |t| {
            t[3] += 0.545_415_4;
            t[1] += 4.0;
            t[2] -= 0.9;
        });
        p.set("left_hind_leg", |l| l[2] -= 0.9);
        p.set("right_hind_leg", |l| l[2] -= 0.9);
    }
    if lie > 0.0 && !s.baby {
        p.set("head", |h| {
            h[5] = rot_lerp_deg(lie, h[5], -1.270_796_3);
            h[4] = rot_lerp_deg(lie, h[4], 1.270_796_3);
        });
        p.set("left_front_leg", |l| l[3] = -1.270_796_3);
        p.set("right_front_leg", |l| {
            l[3] = -0.470_796_35;
            l[5] = -0.2;
            l[0] += a;
        });
        p.set("left_hind_leg", |l| l[3] = -0.4);
        p.set("right_hind_leg", |l| {
            l[3] = 0.5;
            l[5] = -0.5;
            l[0] += 0.8 * a;
            l[1] += 2.0 * a;
        });
        p.set("tail1", |t| t[3] = rot_lerp_deg(lie_tail, t[3], 0.8));
        p.set("tail2", |t| t[3] = rot_lerp_deg(lie_tail, t[3], -0.4));
    } else if lie > 0.0 {
        p.set("body", |b| b[0] += 1.0);
        p.set("head", |h| {
            h[3] = rot_lerp_deg(lie, h[3], PI / 18.0);
            h[5] = rot_lerp_deg(lie, h[5], -PI * 5.0 / 12.0);
            h[0] += 1.5;
            h[1] += 0.75;
            h[2] -= 0.5;
        });
        p.set("right_front_leg", |l| {
            l[3] = -PI / 4.0;
            l[0] += 3.5;
            l[1] -= 0.5;
        });
        p.set("left_front_leg", |l| {
            l[3] = -PI / 2.0;
            l[0] += 1.5;
            l[1] -= 1.0;
            l[2] -= 2.0;
        });
        p.set("right_hind_leg", |l| {
            l[3] = PI * 2.0 / 9.0;
            l[4] = PI / 9.0;
            l[5] = -PI / 9.0;
            l[0] += 2.5;
            l[1] -= 0.25;
            l[2] += 0.5;
        });
        p.set("left_hind_leg", |l| {
            l[0] += 1.5;
            l[2] -= 1.0;
        });
        // The baby adds the interpolation to the current angle (double count in the game, reproduced).
        p.set("tail1", |t| {
            t[3] += rot_lerp_deg(lie_tail, t[3], -PI / 6.0);
            t[4] += rot_lerp_deg(lie_tail, t[4], 0.0);
            t[5] += rot_lerp_deg(lie_tail, t[5], -PI / 18.0);
            t[0] += 1.0;
            t[1] += 0.5;
            t[2] -= 0.25;
        });
    }
    if relax > 0.0 {
        p.set("head", |h| h[3] = rot_lerp_deg(relax, h[3], -0.581_776_44));
    }
}

/// Angle interpolation whose difference is brought into [−180, 180[ ("degrees" version of the game, applied
/// as is to radians by the felines: no wrap-around effect for these differences).
fn rot_lerp_deg(t: f32, a: f32, b: f32) -> f32 {
    let mut d = (b - a) % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    a + t * d
}

/// Angle interpolation in radians along the shortest path (of the game).
fn rot_lerp_rad(t: f32, a: f32, b: f32) -> f32 {
    let mut d = b - a;
    while d < -PI {
        d += 2.0 * PI;
    }
    while d >= PI {
        d -= 2.0 * PI;
    }
    a + t * d
}

/// Walk: target min(d × 4, 1) smoothed at 0.4; camel standing, not dashing
/// min(d × 6, 1) at 0.2; frog min(d × 25, 1), zero while jumping; creaking min(d × 25, 3).
pub(super) fn walk_target(e: &Entity, d: f32) -> (f32, f32) {
    match e.kind.as_str() {
        "camel" | "camel_husk" => {
            let walking = e.pose() == 0 && e.extra[8] < 0.0;
            (if walking { (d * 6.0).min(1.0) } else { 0.0 }, 0.2)
        }
        "frog" => (if State::started(e.extra[1]) { 0.0 } else { (d * 25.0).min(1.0) }, 0.4),
        "creaking" => ((d * 25.0).min(3.0), 0.4),
        _ => ((d * 4.0).min(1.0), 0.4),
    }
}

/// World time (ticks) received from the server, for anger with a deadline (wolf, bee).
static GAME_TIME: AtomicI64 = AtomicI64::new(0);

pub(super) fn set_game_time(t: i64) {
    GAME_TIME.store(t, Ordering::Relaxed);
}

/// Lying cats (value 21): a sleeping player (bed position, value 14) whose box
/// (0.2 × 0.2 lying) touches the cat's block widened by 2 → extra 6 = 1, otherwise 0. After the tick
/// of the entities; without a sleeping player, no cat walk.
pub(super) fn cat_track(ents: &mut crate::entities::Entities) {
    let sleeping = |e: &Entity| e.kind == "player" && matches!(e.meta.get(&14), Some(Meta::BlockPos(Some(_))));
    let beds: Vec<[f64; 3]> =
        ents.map.values().chain(ents.me.as_ref()).filter(|e| sleeping(e)).map(|e| e.pos).collect();
    for e in ents.map.values_mut() {
        if e.kind != "cat" {
            continue;
        }
        let on = e.meta_bool(21) && beds.iter().any(|p| cat_over_sleeper(e.pos, *p));
        e.extra[6] = f32::from(u8::from(on));
    }
}

/// Jukebox `p` that starts (1010) or stops (1011) playing: living entities whose box touches
/// the block widened by 3 (strict bounds) are notified; only the parrot reacts (jukebox and
/// party remembered).
pub(super) fn jukebox_notify(ents: &mut crate::entities::Entities, dims: &[[f32; 4]], p: [i32; 3], playing: bool) {
    for e in ents.map.values_mut() {
        if e.kind != "parrot" {
            continue;
        }
        let s = e.size(dims);
        let half = f64::from(s.w) / 2.0;
        let lo = [e.pos[0] - half, e.pos[1], e.pos[2] - half];
        let hi = [e.pos[0] + half, e.pos[1] + f64::from(s.h), e.pos[2] + half];
        if (0..3).all(|i| lo[i] < f64::from(p[i]) + 4.0 && hi[i] > f64::from(p[i]) - 3.0) {
            e.jukebox = Some((p, playing));
        }
    }
}

/// Start of the tick of a marked parrot: jukebox forgotten (party over) if its centre is 3.46
/// or more away, or if the block is no longer a jukebox. Other entities cost only one test.
pub(super) fn party_track(ents: &mut crate::entities::Entities, is_jukebox: &dyn Fn([i32; 3]) -> bool) {
    for e in ents.map.values_mut() {
        let Some((j, _)) = e.jukebox else { continue };
        let d: f64 = (0..3).map(|i| (f64::from(j[i]) + 0.5 - e.pos[i]).powi(2)).sum();
        if d >= 3.46 * 3.46 || !is_jukebox(j) {
            e.jukebox = None;
        }
    }
}

/// The cat's block widened by 2 (strict bounds) touches the player's 0.2 × 0.2 lying box.
fn cat_over_sleeper(cat: [f64; 3], player: [f64; 3]) -> bool {
    (0..3).all(|i| {
        let lo = cat[i].floor() - 2.0;
        let hi = cat[i].floor() + 3.0;
        let (plo, phi) = if i == 1 { (player[1], player[1] + 0.2) } else { (player[i] - 0.1, player[i] + 0.1) };
        lo < phi && hi > plo
    })
}

/// Client-side thunderstorm, set on every client tick.
static THUNDERING: AtomicBool = AtomicBool::new(false);

pub(super) fn set_thundering(on: bool) {
    THUNDERING.store(on, Ordering::Relaxed);
}

/// Scared panda: "worried" expressed gene (main 2, dominant: the hidden gene changes
/// nothing; value 21) during a thunderstorm.
pub(super) fn panda_scared(e: &Entity) -> bool {
    e.kind == "panda" && e.meta_byte(21) == 2 && THUNDERING.load(Ordering::Relaxed)
}

/// Anger of a neutral mob: deadline (value `i`, world tick) still to come.
pub(super) fn angry(e: &Entity, i: u8) -> bool {
    match e.meta.get(&i) {
        Some(Meta::Long(end)) => *end > 0 && *end - GAME_TIME.load(Ordering::Relaxed) > 0,
        _ => false,
    }
}

fn meta_float(e: &Entity, i: u8) -> Option<f32> {
    match e.meta.get(&i) {
        Some(Meta::Float(v)) => Some(*v),
        _ => None,
    }
}

/// Change of a synchronised value seen at the tick (key `v + 1` stored in `slot`): true once
/// per change, including on the first value received.
fn changed(x: &mut f32, v: i32) -> bool {
    let key = v as f32 + 1.0;
    if *x != key {
        *x = key;
        true
    } else {
        false
    }
}

/// 1 in `n` draw of the client (game random replaced by a reproducible hash).
fn one_in(e: &Entity, salt: u64, n: u64) -> bool {
    chance(e, salt, n)
}

/// Wolf (§ tail, sitting, shaking): `tick` state in extra 0-1 (interest), 2-3 (shaking),
/// 4 (currently shaking).
fn wolf(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let x = &e.extra;
    let a = if s.baby { 0.5 } else { 1.0 };
    let is_angry = angry(e, 22);
    let flags = e.meta_byte(18);
    let (pp, v) = (s.walk_pos, s.walk_speed);
    p.set("tail", |t| t[4] = if is_angry { 0.0 } else { (pp * 0.6662).cos() * 1.4 * v });
    if flags & 1 != 0 {
        p.set("body", |b| {
            b[1] += 4.0 * a;
            b[2] -= 2.0 * a;
            b[3] = PI / 4.0;
        });
        p.set("tail", |t| {
            t[1] += 9.0 * a;
            t[2] -= 2.0 * a;
        });
        for leg in ["right_hind_leg", "left_hind_leg"] {
            p.set(leg, |l| {
                l[1] += 6.7 * a;
                l[2] -= 5.0 * a;
                l[3] = PI * 3.0 / 2.0;
            });
        }
        for (leg, dx) in [("right_front_leg", 0.01), ("left_front_leg", -0.01)] {
            p.set(leg, |l| {
                l[3] = 5.811_947;
                l[0] += dx * a;
                l[1] += a;
            });
        }
        if s.baby {
            p.set("body", |b| b[3] -= PI / 2.0);
        } else {
            p.set("upper_body", |b| {
                b[1] += 2.0;
                b[3] = PI * 2.0 / 5.0;
                b[4] = 0.0;
            });
        }
    } else {
        let q = pp * 0.6662;
        p.set("right_hind_leg", |l| l[3] = q.cos() * 1.4 * v);
        p.set("left_hind_leg", |l| l[3] = (q + PI).cos() * 1.4 * v);
        p.set("right_front_leg", |l| l[3] = (q + PI).cos() * 1.4 * v);
        p.set("left_front_leg", |l| l[3] = q.cos() * 1.4 * v);
    }
    let shake = lerp(pt, x[3], x[2]);
    let roll = |off: f32| {
        let t = ((shake + off) / 1.8).clamp(0.0, 1.0);
        (t * PI).sin() * (t * PI * 11.0).sin() * 0.15 * PI
    };
    let head_roll = lerp(pt, x[1], x[0]) * 0.15 * PI;
    p.set("body", |b| b[5] = roll(-0.16));
    if s.baby {
        p.set("head", |h| h[5] = head_roll + roll(0.0));
        p.set("tail", |t| t[5] = roll(-0.2));
    } else {
        p.set("real_head", |h| h[5] = head_roll + roll(0.0));
        p.set("upper_body", |b| b[5] = roll(-0.08));
        p.set("real_tail", |t| t[5] = roll(-0.2));
    }
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    // Tail angle: anger, otherwise remaining health (tamed wolf, 40 points max), otherwise π/5.
    let tail = if is_angry {
        1.539_380_4
    } else if flags & 4 != 0 {
        let hp = meta_float(e, 9).unwrap_or(40.0);
        (0.55 - (40.0 - hp) / 40.0 * 0.4) * PI
    } else {
        PI / 5.0
    };
    p.set("tail", |t| t[3] = tail);
}

/// Charging head of the goat (extra 0 = lowering ticks 0..20).
fn goat_ram(e: &Entity, baby: bool) -> f32 {
    e.extra[0] / 20.0 * deg(if baby { 52.5 } else { 30.0 })
}

/// Leg spring of the fox facing the ground: counter specific to each model (adult, baby),
/// advanced at each draw like the game (so it depends on the number of frames).
static FOX_LEG_MOTION: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Fox: `tick` state in extra 0-1 (interest), 2-3 (crouching); flags in 19.
fn fox(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let f = e.meta_byte(19) as u8;
    let (sitting, crouching, pouncing, sleeping, faceplanted) =
        (f & 1 != 0, f & 4 != 0, f & 16 != 0, f & 32 != 0, f & 64 != 0);
    let x = &e.extra;
    let a = if s.baby { 0.5 } else { 1.0 };
    let crouch = lerp(pt, x[3], x[2]);
    let (pp, v) = (s.walk_pos, s.walk_speed);
    // Walk.
    p.set("head", |h| h[5] = lerp(pt, x[1], x[0]) * 0.11 * PI);
    if s.baby {
        if let Some(an) = keyframes::get("fox_baby.walk") {
            keyframes::apply_walk(p, an, s, 1.0, 2.5);
        }
    } else {
        let q = pp * 0.6662;
        p.set("right_hind_leg", |l| l[3] = q.cos() * 1.4 * v);
        p.set("left_hind_leg", |l| l[3] = (q + PI).cos() * 1.4 * v);
        p.set("right_front_leg", |l| l[3] = (q + PI).cos() * 1.4 * v);
        p.set("left_front_leg", |l| l[3] = q.cos() * 1.4 * v);
    }
    const LEGS: [&str; 4] = ["right_hind_leg", "left_hind_leg", "right_front_leg", "left_front_leg"];
    if crouching {
        p.set("body", |b| b[3] += 0.104_719_76);
        p.set("head", |h| h[1] += crouch * a);
        let w = s.age.cos() * 0.05;
        p.set("body", |b| b[4] = w);
        p.set("right_hind_leg", |l| l[5] = w);
        p.set("left_hind_leg", |l| l[5] = w);
        p.set("right_front_leg", |l| l[5] = w / 2.0);
        p.set("left_front_leg", |l| l[5] = w / 2.0);
        let dy = if s.baby { crouch / 6.0 } else { crouch };
        p.set("body", |b| b[1] += dy);
    } else if sleeping {
        for l in LEGS {
            p.hide(l);
        }
        if s.baby {
            p.set("body", |b| {
                b[5] = -PI / 2.0;
                b[3] = -PI / 18.0;
                b[1] += 1.5;
                b[2] -= 1.5;
                b[0] -= 1.5;
            });
            p.set("tail", |t| {
                t[3] = -2.181_661_6;
                t[0] -= 0.7;
                t[2] += 0.6;
                t[1] += 0.9;
            });
            p.set("head", |h| {
                h[0] -= 2.0;
                h[1] += 2.8;
                h[2] -= 4.0;
            });
        } else {
            p.set("body", |b| {
                b[5] = -PI / 2.0;
                b[1] += 5.0;
            });
            p.set("tail", |t| t[3] = -PI * 5.0 / 6.0);
            p.set("head", |h| {
                h[0] += 2.0;
                h[1] += 2.99;
            });
        }
        p.set("head", |h| {
            h[4] = -PI * 2.0 / 3.0;
            h[5] = 0.0;
        });
    } else if sitting {
        p.set("head", |h| {
            h[3] = 0.0;
            h[4] = 0.0;
        });
        if s.baby {
            p.set("body", |b| {
                b[3] = -0.959_931;
                b[2] -= 4.5 * a;
                b[1] += 3.0 * a;
            });
            p.set("tail", |t| {
                t[1] -= 0.6;
                t[2] -= 2.0 * a;
                t[3] = 0.959_931_14;
            });
            p.set("head", |h| h[1] -= 0.75);
            for (leg, dx) in [("right_front_leg", 0.01), ("left_front_leg", -0.01)] {
                p.set(leg, |l| {
                    l[3] = -PI / 12.0;
                    l[2] -= 1.5;
                    l[0] += dx;
                });
            }
            for (leg, dx) in [("right_hind_leg", 0.01), ("left_hind_leg", -0.01)] {
                p.set(leg, |l| {
                    l[2] -= 3.75;
                    l[0] += dx;
                });
            }
        } else {
            p.set("body", |b| {
                b[3] = PI / 6.0;
                b[1] -= 7.0;
                b[2] += 3.0;
            });
            p.set("tail", |t| {
                t[3] = PI / 4.0;
                t[2] -= 1.0;
            });
            p.set("head", |h| {
                h[1] -= 6.5;
                h[2] += 2.75;
            });
            p.set("right_front_leg", |l| l[3] = -PI / 12.0);
            p.set("left_front_leg", |l| l[3] = -PI / 12.0);
            for leg in ["right_hind_leg", "left_hind_leg"] {
                p.set(leg, |l| {
                    l[3] = -PI * 5.0 / 12.0;
                    l[1] += 4.0;
                    l[2] -= 0.25;
                });
            }
        }
    }
    if pouncing && !s.baby {
        p.set("body", |b| b[1] -= crouch / 2.0);
        p.set("head", |h| h[1] -= crouch / 2.0);
    }
    if !sleeping && !faceplanted && !crouching {
        p.set("head", |h| {
            h[3] = s.pitch;
            h[4] = s.yaw;
        });
    }
    if sleeping {
        p.set("head", |h| {
            h[3] = 0.0;
            h[4] = -PI * 2.0 / 3.0;
            h[5] = (s.age * 0.027).cos() / 22.0;
        });
    }
    if faceplanted {
        let cell = &FOX_LEG_MOTION[usize::from(s.baby)];
        let m = f32::from_bits(cell.load(Ordering::Relaxed)) + 0.67;
        cell.store(m.to_bits(), Ordering::Relaxed);
        let q = m * 0.4662;
        p.set("right_hind_leg", |l| l[3] = q.cos() * 0.1);
        p.set("left_hind_leg", |l| l[3] = (q + PI).cos() * 0.1);
        p.set("right_front_leg", |l| l[3] = (q + PI).cos() * 0.1);
        p.set("left_front_leg", |l| l[3] = q.cos() * 0.1);
    }
}

/// Panda: extra 0-1 sitting, 2-3 on the back, 4-5 roll (ramps), 6 roll counter.
/// Synchronised counters kept locally like the client: unhappy (18), sneeze (19),
/// meal (20); flags in 23 (2 sneezes, 4 rolls, 8 sitting, 16 on the back).
fn panda(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    quadruped(p, s);
    let x = &e.extra;
    let flags = e.meta_byte(23);
    let age = s.age;
    let sit = lerp(pt, x[1], x[0]);
    let back = lerp(pt, x[3], x[2]);
    let roll = if s.baby { 0.0 } else { lerp(pt, x[5], x[4]) };
    if e.meta_int(18).unwrap_or(0) > 0 {
        p.set("head", |h| {
            h[4] = 0.35 * (0.6 * age).sin();
            h[5] = 0.35 * (0.6 * age).sin();
        });
        p.set("right_front_leg", |l| l[3] = -0.75 * (0.3 * age).sin());
        p.set("left_front_leg", |l| l[3] = 0.75 * (0.3 * age).sin());
    } else {
        p.set("head", |h| h[5] = 0.0);
    }
    if flags & 2 != 0 {
        let t = e.meta_int(19).unwrap_or(0);
        if t < 15 {
            p.set("head", |h| h[3] = -PI / 4.0 * t as f32 / 14.0);
        } else if t < 20 {
            // Integer division of the game: (t − 15) / 5 is always 0 here.
            let k = ((t - 15) / 5) as f32;
            p.set("head", |h| h[3] = -PI / 4.0 + PI / 4.0 * k);
        }
    }
    let legs = ["right_hind_leg", "left_hind_leg", "right_front_leg", "left_front_leg"];
    if sit > 0.0 {
        if s.baby {
            p.set("body", |b| {
                b[3] = rot_lerp_rad(sit, b[3], PI / 18.0);
                b[2] = lerp(sit, b[2], -1.5);
            });
            p.set("head", |h| {
                h[2] = lerp(sit, h[2], -11.5);
                h[1] = lerp(sit, h[1], 17.5);
            });
            for (leg, z) in
                [("right_front_leg", -5.0), ("left_front_leg", -5.0), ("right_hind_leg", 3.0), ("left_hind_leg", 3.0)]
            {
                p.set(leg, |l| l[2] = lerp(sit, l[2], z));
            }
        } else {
            p.set("body", |b| b[3] = rot_lerp_rad(sit, b[3], 1.740_796_3));
            p.set("head", |h| h[3] = rot_lerp_rad(sit, h[3], PI / 2.0));
        }
        p.set("right_front_leg", |l| l[5] = -0.270_796_42);
        p.set("left_front_leg", |l| l[5] = 0.270_796_42);
        p.set("right_hind_leg", |l| l[5] = 0.570_796_4);
        p.set("left_hind_leg", |l| l[5] = -0.570_796_4);
        if e.meta_int(20).unwrap_or(0) > 0 {
            p.set("head", |h| h[3] = PI / 2.0 + 0.2 * (age * 0.6).sin());
            p.set("right_front_leg", |l| l[3] = -0.4 - 0.2 * (age * 0.6).sin());
            p.set("left_front_leg", |l| l[3] = -0.4 - 0.2 * (age * 0.6).sin());
        }
        // Scared by the thunderstorm: head tucked in, front legs over the eyes.
        if panda_scared(e) {
            p.set("head", |h| h[3] = 2.170_796_4);
            p.set("right_front_leg", |l| l[3] = -0.9);
            p.set("left_front_leg", |l| l[3] = -0.9);
        }
    } else {
        for l in legs {
            p.set(l, |q| q[5] = 0.0);
        }
    }
    if back > 0.0 {
        p.set("right_hind_leg", |l| l[3] = -0.6 * (age * 0.15).sin());
        p.set("left_hind_leg", |l| l[3] = 0.6 * (age * 0.15).sin());
        p.set("right_front_leg", |l| l[3] = 0.3 * (age * 0.25).sin());
        p.set("left_front_leg", |l| l[3] = -0.3 * (age * 0.25).sin());
        p.set("head", |h| h[3] = rot_lerp_rad(back, h[3], PI / 2.0));
    }
    if roll > 0.0 {
        p.set("head", |h| h[3] = rot_lerp_rad(roll, h[3], 2.056_194_5));
        p.set("right_hind_leg", |l| l[3] = -0.5 * (age * 0.5).sin());
        p.set("left_hind_leg", |l| l[3] = 0.5 * (age * 0.5).sin());
        p.set("right_front_leg", |l| l[3] = 0.5 * (age * 0.5).sin());
        p.set("left_front_leg", |l| l[3] = -0.5 * (age * 0.5).sin());
    }
}

/// Shifts the root parts (the game moves the model root).
fn shift_root(p: &mut Pose<'_>, dy: f32) {
    for i in 0..p.parts.len() {
        if p.layer.parts[i].parent == u32::MAX {
            p.parts[i][1] += dy;
        }
    }
}

/// Turtle: legs in yaw on land (laying: ×4 in front, amplitude ×2), swimming otherwise; carried egg
/// (18); laying (19). "On land": out of the water and on the ground.
fn turtle(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    quadruped(p, s);
    let (pp, v) = (s.walk_pos, s.walk_speed);
    if !s.in_water && e.on_ground {
        let laying = e.meta_bool(19);
        let (f, amp) = if laying { (4.0, 2.0) } else { (1.0, 1.0) };
        let sw = pp * 5.0;
        let front = (f * sw).cos();
        let hind = sw.cos();
        p.set("right_front_leg", |l| l[4] = -front * 8.0 * v * amp);
        p.set("left_front_leg", |l| l[4] = front * 8.0 * v * amp);
        p.set("right_hind_leg", |l| l[4] = -hind * 3.0 * v);
        p.set("left_hind_leg", |l| l[4] = hind * 3.0 * v);
    } else {
        let sw = (pp * 0.6662 * 0.6).cos() * 0.5 * v;
        p.set("right_hind_leg", |l| l[3] = sw);
        p.set("left_hind_leg", |l| l[3] = -sw);
        p.set("right_front_leg", |l| l[5] = -sw);
        p.set("left_front_leg", |l| l[5] = sw);
    }
    if !s.baby {
        if e.meta_bool(18) {
            shift_root(p, -1.0);
        } else {
            p.hide("egg_belly");
        }
    }
}

/// Armadillo: extra 0 state seen (key), 1 ticks in the state, 2 exit, 3 rolled, 4 peek, 5 peek received.
fn armadillo(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    let x = &e.extra;
    let state = e.meta_int(18).unwrap_or(0);
    // Change received but not yet seen at the tick: 0 ticks in the state.
    let ticks = if x[0] == state as f32 + 1.0 { x[1] } else { 0.0 };
    let hiding = match state {
        1 => ticks > 5.0,
        2 => true,
        3 => ticks < 26.0,
        _ => false,
    };
    let pre = if s.baby { "baby_armadillo.armadillo_baby_" } else { "armadillo." };
    if hiding {
        p.hide("left_hind_leg");
        p.hide("right_hind_leg");
        p.hide("tail");
    } else {
        p.hide("cube");
        p.set("head", |h| {
            h[3] = s.pitch.clamp(deg(-22.5), deg(25.0));
            h[4] = s.yaw.clamp(deg(-32.5), deg(32.5));
        });
        if let Some(a) = keyframes::get(&format!("{pre}walk")) {
            keyframes::apply_walk(p, a, s, 16.5, 2.5);
        }
    }
    keyframes::play(p, &format!("{pre}roll_out"), x[2], s.age);
    keyframes::play(p, &format!("{pre}roll_up"), x[3], s.age);
    keyframes::play(p, &format!("{pre}peek"), x[4], s.age);
    if hiding {
        skip_draw(p, "body");
    }
}

/// Part not drawn whose children still are: 1/1024 scale compensated on the direct children
/// (uniform rotation and scale: the children's composition is unchanged).
fn skip_draw(p: &mut Pose<'_>, name: &str) {
    const K: f32 = 1.0 / 1024.0;
    let Some(i) = p.index(name) else { return };
    for k in 6..9 {
        p.parts[i][k] *= K;
    }
    for c in 0..p.parts.len() {
        if p.layer.parts[c].parent == i as u32 {
            for k in 0..3 {
                p.parts[c][k] /= K;
            }
            for k in 6..9 {
                p.parts[c][k] /= K;
            }
        }
    }
}

/// Sniffer: extra 0 state seen (key); animation starts 1 dig, 2 long sniff,
/// 3 stand up, 4 happy, 5 scent. State in 18 (4 = searching).
fn sniffer(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    let walk = if e.meta_int(18) == Some(4) { "sniffer.sniff_search" } else { "sniffer.walk" };
    if let Some(a) = keyframes::get(walk) {
        keyframes::apply_walk(p, a, s, 9.0, 100.0);
    }
    let x = &e.extra;
    keyframes::play(p, "sniffer.dig", x[1], s.age);
    keyframes::play(p, "sniffer.longsniff", x[2], s.age);
    keyframes::play(p, "sniffer.stand_up", x[3], s.age);
    keyframes::play(p, "sniffer.happy", x[4], s.age);
    keyframes::play(p, "sniffer.sniffsniff", x[5], s.age);
}

/// Frog: extra 0 pose seen (key); starts 1 jump, 2 croak, 3 tongue, 4 rest in
/// water. Swim: box in contact with water (noted at the tick).
fn frog(p: &mut Pose<'_>, e: &Entity, s: &AnimState) {
    let x = &e.extra;
    keyframes::play(p, "frog.jump", x[1], s.age);
    keyframes::play(p, "frog.croak", x[2], s.age);
    keyframes::play(p, "frog.tongue", x[3], s.age);
    let (name, sf) = if s.in_water { ("frog.swim", 1.0) } else { ("frog.walk", 1.5) };
    if let Some(a) = keyframes::get(name) {
        keyframes::apply_walk(p, a, s, sf, 2.5);
    }
    keyframes::play(p, "frog.idle_water", x[4], s.age);
    if !State::started(x[2]) {
        p.hide("croaking_body");
    }
}

/// Kinds whose pose depends on contact with water (`water_track`): frog, turtle, equines.
pub(super) fn water_posed(k: &str) -> bool {
    k == "frog" || k == "turtle" || equine(k)
}

/// Bee: extra 0-1 roll (ramp). Flags in 18 (2 rolls, 4 has stung), anger in 19.
fn bee(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let flags = e.meta_byte(18);
    if flags & 4 != 0 {
        p.hide("stinger");
    }
    let v = e.velocity;
    let grounded = e.on_ground && v[0] * v[0] + v[1] * v[1] + v[2] * v[2] < 1.0e-7;
    let age = s.age;
    if !grounded {
        let sp = age * 120.321_13 * PI / 180.0;
        let rz = sp.cos() * PI * 0.15;
        let rx = p.get("right_wing").map_or(0.0, |w| w[3]);
        p.set("right_wing", |w| {
            w[4] = 0.0;
            w[5] = rz;
        });
        p.set("left_wing", |w| {
            w[3] = rx;
            w[4] = 0.0;
            w[5] = -rz;
        });
        for l in ["front_legs", "middle_legs", "back_legs"] {
            p.set(l, |q| q[3] = PI / 4.0);
        }
    }
    if !angry(e, 19) && !grounded {
        let sp = (age * 0.18).cos();
        p.set("bone", |b| {
            b[3] = 0.1 + sp * PI * 0.025;
            b[1] -= (age * 0.18).cos() * 0.9;
        });
        p.set("front_legs", |l| l[3] = -sp * PI * 0.1 + PI / 8.0);
        p.set("back_legs", |l| l[3] = -sp * PI * 0.05 + PI / 4.0);
        if !s.baby {
            p.set("left_antenna", |a| a[3] = sp * PI * 0.03);
            p.set("right_antenna", |a| a[3] = sp * PI * 0.03);
        }
    }
    let roll = lerp(pt, e.extra[1], e.extra[0]);
    if roll > 0.0 {
        p.set("bone", |b| b[3] = rot_lerp_rad(roll, b[3], 3.091_592_8));
    }
}

/// Allay: extra 0-1 held item (0..5), 2 dance ticks, 3-4 spinning (0..15); dance
/// in 16.
fn allay(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let x = &e.extra;
    let (v, pos, age) = (s.walk_speed, s.walk_pos, s.age);
    let flap_speed = age * 20.0 * PI / 180.0 + pos;
    let flap = flap_speed.cos() * PI * 0.15 + v;
    let bob_speed = age * 9.0 * PI / 180.0;
    let flying = (v / 0.3).min(1.0);
    let idle = 1.0 - flying;
    let hold = lerp(pt, x[1], x[0]) / 5.0;
    if e.meta_bool(16) {
        let ds = age * 8.0 * PI / 180.0 + v;
        let freq = ds.cos() * 16.0 * PI / 180.0;
        let spin = lerp(pt, x[4], x[3]) / 15.0;
        let spinning = x[2] % 55.0 < 15.0;
        p.set("root", |r| {
            if spinning {
                r[4] = PI * 4.0 * spin;
            }
            r[5] = freq * (1.0 - spin);
        });
        p.set("head", |h| {
            h[4] = ds.cos() * 30.0 * PI / 180.0 * (1.0 - spin);
            h[5] = ds.cos() * 14.0 * PI / 180.0 * (1.0 - spin);
        });
    } else {
        p.set("head", |h| {
            h[3] = s.pitch;
            h[4] = s.yaw;
        });
    }
    p.set("right_wing", |w| {
        w[3] = 0.436_332_32 * (1.0 - flying);
        w[4] = -PI / 4.0 + flap;
    });
    p.set("left_wing", |w| {
        w[3] = 0.436_332_32 * (1.0 - flying);
        w[4] = PI / 4.0 - flap;
    });
    p.set("body", |b| b[3] = flying * PI / 4.0);
    let arm_x = hold * lerp(flying, -PI / 3.0, -1.134_464);
    p.set("root", |r| r[1] += (bob_speed as f64).cos() as f32 * 0.25 * idle);
    let bob_f = idle * (1.0 - hold);
    let bob = 0.436_332_32 - (bob_speed + PI * 3.0 / 2.0).cos() * PI * 0.075 * bob_f;
    p.set("right_arm", |a| {
        a[3] = arm_x;
        a[5] = bob;
        a[4] = 0.279_252_68 * hold;
    });
    p.set("left_arm", |a| {
        a[3] = arm_x;
        a[5] = -bob;
        a[4] = -0.279_252_68 * hold;
    });
}

/// Parrot (§ 7): poses party (jukebox nearby, priority), sitting, flying, standing.
fn parrot(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) {
    let x = &e.extra;
    let f = (lerp(pt, x[1], x[0]).sin() + 1.0) * lerp(pt, x[3], x[2]);
    if matches!(e.jukebox, Some((_, true))) {
        // Party: legs spread, the rest describes a circle (cos, sin of the age), head straight
        // that rolls; no walk or swaying.
        let leg = (std::f64::consts::PI / 9.0) as f32;
        p.set("left_leg", |l| l[5] = -leg);
        p.set("right_leg", |l| l[5] = leg);
        let (dx, dy) = (s.age.cos(), s.age.sin());
        for n in ["head", "body", "left_wing", "right_wing", "tail"] {
            p.set(n, |q| {
                q[0] += dx;
                q[1] += dy;
            });
        }
        p.set("head", |h| {
            h[3] = 0.0;
            h[4] = 0.0;
            h[5] = s.age.sin() * 0.4;
        });
        p.set("left_wing", |w| w[5] = -0.0873 - f);
        p.set("right_wing", |w| w[5] = 0.0873 + f);
        return;
    }
    let sitting = e.meta_byte(18) & 1 != 0;
    let flying = !sitting && !e.on_ground;
    const ALL: [&str; 7] = ["head", "tail", "body", "left_wing", "right_wing", "left_leg", "right_leg"];
    if flying {
        p.set("left_leg", |l| l[3] += 2.0 * PI / 9.0);
        p.set("right_leg", |l| l[3] += 2.0 * PI / 9.0);
    } else if sitting {
        for n in ALL {
            p.set(n, |q| q[1] += 1.9);
        }
        p.set("tail", |t| t[3] += PI / 6.0);
        p.set("left_wing", |w| w[5] = -0.0873);
        p.set("right_wing", |w| w[5] = 0.0873);
        p.set("left_leg", |l| l[3] += PI / 2.0);
        p.set("right_leg", |l| l[3] += PI / 2.0);
    }
    p.set("head", |h| {
        h[3] = s.pitch;
        h[4] = s.yaw;
    });
    if sitting {
        return;
    }
    let (pp, v) = (s.walk_pos, s.walk_speed);
    if !flying {
        p.set("left_leg", |l| l[3] += (pp * 0.6662).cos() * 1.4 * v);
        p.set("right_leg", |l| l[3] += (pp * 0.6662 + PI).cos() * 1.4 * v);
    }
    let b = f * 0.3;
    for n in ALL {
        p.set(n, |q| q[1] += b);
    }
    p.set("tail", |t| t[3] += (pp * 0.6662).cos() * 0.3 * v);
    p.set("left_wing", |w| w[5] = -0.0873 - f);
    p.set("right_wing", |w| w[5] = 0.0873 + f);
}

/// Own render rotations, after "Y(180 − body)" (the death tilt, placed afterwards by the
/// engine, precedes these additions in the game: difference limited to dying mobs in these poses).
pub(super) fn rotations(e: &Entity, pt: f32, m: &mut Mat4) -> bool {
    let body = rot_lerp(pt, e.body_o, e.body);
    let x_rot = e.pitch_o + pt * (e.pitch - e.pitch_o);
    let base = Mat4::from_rotation_y(deg(180.0 - body));
    match e.kind.as_str() {
        "cat" => {
            let lie = lerp(pt, e.extra[1], e.extra[0]);
            *m *= base;
            if lie > 0.0 {
                *m *= Mat4::from_translation(Vec3::new(0.4 * lie, 0.15 * lie, 0.1 * lie))
                    * Mat4::from_rotation_z(deg(rot_lerp_deg(lie, 0.0, 90.0)));
                // Lying on a sleeping player (`cat_track`, extra 6): offset by 0.15.
                if e.extra[6] != 0.0 {
                    *m *= Mat4::from_translation(Vec3::new(0.15 * lie, 0.0, 0.0));
                }
            }
            true
        }
        "fox" => {
            *m *= base;
            if e.meta_byte(19) & (16 | 64) != 0 {
                *m *= Mat4::from_rotation_x(deg(-x_rot));
            }
            true
        }
        "panda" => {
            *m *= base;
            panda_rotations(e, pt, x_rot, m);
            true
        }
        _ => false,
    }
}

/// Roll, sitting and on-the-back position of the panda (the sitting panda has zero pitch).
fn panda_rotations(e: &Entity, pt: f32, x_rot: f32, m: &mut Mat4) {
    let baby = e.baby();
    let x = &e.extra;
    let x_rot = if e.meta_byte(23) & 8 != 0 { 0.0 } else { x_rot };
    let roll_time = if x[6] > 0.0 { x[6] + pt } else { 0.0 };
    if roll_time > 0.0 {
        let tr = roll_time.fract();
        let pos = roll_time.floor() as i32;
        let next = pos + 1;
        let y = if baby { 0.3 } else { 0.8 };
        let ang = |this: f32, nxt: f32, thr: f32| if (next as f32) < thr { lerp(tr, this, nxt) } else { this };
        let (dy, a) = if pos < 8 {
            let a = ang(90.0 * pos as f32 / 7.0, 90.0 * next as f32 / 7.0, 8.0);
            ((y + 0.2) * (a / 90.0), a)
        } else if pos < 16 {
            let a = ang(90.0 + 90.0 * (pos as f32 - 8.0) / 7.0, 90.0 + 90.0 * (next as f32 - 8.0) / 7.0, 16.0);
            (y + 0.2 + (y - 0.2) * (a - 90.0) / 90.0, a)
        } else if pos < 24 {
            let a = ang(180.0 + 90.0 * (pos as f32 - 16.0) / 7.0, 180.0 + 90.0 * (next as f32 - 16.0) / 7.0, 24.0);
            (y + y * (270.0 - a) / 90.0, a)
        } else if pos < 32 {
            let a = ang(270.0 + 90.0 * (pos as f32 - 24.0) / 7.0, 270.0 + 90.0 * (next as f32 - 24.0) / 7.0, 32.0);
            (y * ((360.0 - a) / 90.0), a)
        } else {
            (0.0, 0.0)
        };
        if pos < 32 {
            *m *= Mat4::from_translation(Vec3::new(0.0, dy, 0.0)) * Mat4::from_rotation_x(deg(-a));
        }
    }
    let sit = lerp(pt, x[1], x[0]);
    if sit > 0.0 {
        *m *= Mat4::from_translation(Vec3::new(0.0, 0.8 * sit, 0.0))
            * Mat4::from_rotation_x(deg(lerp(sit, x_rot, x_rot + 90.0)))
            * Mat4::from_translation(Vec3::new(0.0, -sit, 0.0));
        // Scared (worried gene, thunderstorm): yaw shaking; the baby is realigned.
        if panda_scared(e) {
            let age = e.age as f32 + pt;
            let shake = (f64::from(age * 1.25).cos() * std::f64::consts::PI * 0.05) as f32;
            *m *= Mat4::from_rotation_y(deg(shake));
            if baby {
                *m *= Mat4::from_translation(Vec3::new(0.0, 0.8, 0.55));
            }
        }
    }
    let back = lerp(pt, x[3], x[2]);
    if back > 0.0 {
        let y = if baby { 0.5 } else { 1.3 };
        *m *= Mat4::from_translation(Vec3::new(0.0, y * back, 0.0))
            * Mat4::from_rotation_x(deg(lerp(back, x_rot, x_rot + 180.0)));
    }
}

pub(super) fn scale(_e: &Entity, _pt: f32, _scale: &mut Vec3) {}

pub(super) fn shadow(k: &str) -> Option<(f32, f32)> {
    let r = match k {
        "horse" | "donkey" | "mule" | "skeleton_horse" | "zombie_horse" => 0.75,
        "camel" | "camel_husk" | "llama" | "trader_llama" | "mooshroom" => 0.7,
        "ocelot" | "allay" | "armadillo" | "bee" => 0.4,
        "rabbit" | "parrot" => 0.3,
        "bat" => 0.25,
        "polar_bear" | "panda" => 0.9,
        "sniffer" => 1.1,
        _ => return None,
    };
    Some((r, 1.0))
}

/// 1 in `n` draw reproducible by (entity, tick).
fn chance(e: &Entity, salt: u64, n: u64) -> bool {
    let mut h = (e.uuid as u64) ^ ((e.uuid >> 64) as u64) ^ salt.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    h = h.wrapping_add(u64::from(e.age).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (h ^ (h >> 31)).is_multiple_of(n)
}

pub(super) fn tick(e: &mut Entity) {
    let k = e.kind.as_str();
    if equine(k) {
        // extra: eating (0, 1 old), rearing (2, 3), mouth (4, 5), tail counter (6).
        let flags = e.meta_byte(18) as u8;
        let tail = chance(e, 1, 200);
        let x = &mut e.extra;
        if tail {
            x[6] = 1.0;
        }
        if x[6] > 0.0 {
            x[6] += 1.0;
            if x[6] > 8.0 {
                x[6] = 0.0;
            }
        }
        x[1] = x[0];
        x[0] = if flags & 0x10 != 0 {
            (x[0] + (1.0 - x[0]) * 0.4 + 0.05).min(1.0)
        } else {
            (x[0] - x[0] * 0.4 - 0.05).max(0.0)
        };
        x[3] = x[2];
        if flags & 0x20 != 0 {
            x[0] = 0.0;
            x[1] = 0.0;
            x[2] = (x[2] + (1.0 - x[2]) * 0.4 + 0.05).min(1.0);
        } else {
            let s = x[2];
            x[2] = (s + (0.8 * s * s * s - s) * 0.6 - 0.05).max(0.0);
        }
        x[5] = x[4];
        x[4] = if flags & 0x40 != 0 {
            (x[4] + (1.0 - x[4]) * 0.7 + 0.05).min(1.0)
        } else {
            (x[4] - x[4] * 0.7 - 0.05).max(0.0)
        };
        return;
    }
    match k {
        "polar_bear" => {
            let up = e.meta_bool(18);
            let x = &mut e.extra;
            x[1] = x[0];
            x[0] = (x[0] + if up { 1.0 } else { -1.0 }).clamp(0.0, 6.0);
        }
        "parrot" => {
            // extra: flap (0, 1 old), speed (2, 3), momentum (4).
            let air = !e.on_ground;
            let x = &mut e.extra;
            x[1] = x[0];
            x[3] = x[2];
            x[2] = (x[2] + if air { 4.0 } else { -1.0 } * 0.3).clamp(0.0, 1.0);
            if air && x[4] < 1.0 {
                x[4] = 1.0;
            }
            x[4] *= 0.9;
            x[0] += x[4] * 2.0;
        }
        "camel" | "camel_husk" => {
            // extra: dash cooldown (0), last value of index 19 (1), initialised (2),
            // wait before the next rest (3), start tick of the rest (4).
            let dash = if e.meta_bool(19) { 1.0 } else { 0.0 };
            let (age, uuid) = (e.age, e.uuid);
            let x = &mut e.extra;
            // Rest restarted every 80 to 119 ticks (game draw replaced by a hash).
            if x[3] <= 0.0 {
                let h = (uuid as u64 ^ (uuid >> 64) as u64 ^ u64::from(age)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                x[3] = 80.0 + ((h >> 32) % 40) as f32;
                x[4] = age as f32;
            } else {
                x[3] -= 1.0;
            }
            x[0] = (x[0] - 1.0).max(0.0);
            if x[2] != 0.0 && dash != x[1] && x[0] == 0.0 {
                x[0] = 55.0;
            }
            x[1] = dash;
            x[2] = 1.0;
            // F04: posture (index 20, tick of the last change, negative = sitting): age of the
            // change seen (5 initialised, 6 key of the value, 7 age of the change); dash (8).
            let t = match e.meta.get(&20) {
                Some(rc_proto_v777::entity::Meta::Long(t)) => *t,
                _ => 0,
            };
            let key = (t.rem_euclid(1 << 22) as f32 + 1.0) * if t < 0 { -1.0 } else { 1.0 };
            if x[5] == 0.0 {
                x[5] = 1.0;
                x[6] = key;
                x[7] = age as f32 - 1000.0;
                x[8] = -1.0;
            } else if x[6] != key {
                x[6] = key;
                x[7] = age as f32;
            }
            if dash > 0.0 {
                if x[8] < 0.0 {
                    x[8] = age as f32;
                }
            } else {
                x[8] = -1.0;
            }
        }
        "rabbit" => {
            // extra: jump ticks (0), jump duration (1, set by event 1), wait before
            // the head tilt (2), jump start (3) and tilt start (4), −1 = stopped;
            // initialised (5). Game draws (40 + 180) replaced by a hash.
            let no_ai = e.meta_byte(15) & 1 != 0;
            let (age, uuid) = (e.age, e.uuid);
            let wait = || {
                let h = (uuid as u64 ^ (uuid >> 64) as u64 ^ u64::from(age)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                180.0 + ((h >> 32) % 40) as f32
            };
            let x = &mut e.extra;
            if x[5] == 0.0 {
                x[5] = 1.0;
                x[2] = wait();
                x[3] = -1.0;
                x[4] = -1.0;
            }
            if x[0] != x[1] {
                x[0] += 1.0;
            } else if x[1] != 0.0 {
                x[0] = 0.0;
                x[1] = 0.0;
            }
            if x[2] <= 0.0 && !no_ai {
                x[2] = wait();
                x[4] = age as f32;
            } else if x[0] > 0.0 {
                if x[3] < 0.0 {
                    x[3] = age as f32;
                }
                x[4] = -1.0;
            } else {
                x[2] -= 1.0;
                x[3] = -1.0;
            }
        }
        "cat" => {
            // Lying (21), relaxed (22): ramps of 0.15/0.22, 0.08/0.13 (tail), 0.1/0.13.
            let (lying, relax) = (e.meta_bool(21), e.meta_bool(22));
            let x = &mut e.extra;
            x[1] = x[0];
            x[3] = x[2];
            x[5] = x[4];
            if lying {
                x[0] = (x[0] + 0.15).min(1.0);
                x[2] = (x[2] + 0.08).min(1.0);
            } else {
                x[0] = (x[0] - 0.22).max(0.0);
                x[2] = (x[2] - 0.13).max(0.0);
            }
            x[4] = if relax { (x[4] + 0.1).min(1.0) } else { (x[4] - 0.13).max(0.0) };
        }
        "wolf" => {
            if e.death_time > 0 {
                // Death: shaking cancelled.
                e.extra[2..5].fill(0.0);
                return;
            }
            let interested = e.meta_bool(20);
            let x = &mut e.extra;
            x[1] = x[0];
            x[0] += ((if interested { 1.0 } else { 0.0 }) - x[0]) * 0.4;
            if x[4] != 0.0 {
                x[3] = x[2];
                x[2] += 0.05;
                if x[3] >= 2.0 {
                    x[2..5].fill(0.0);
                }
            }
        }
        "goat" => {
            // Head lowered (events 58/59, extra 1): +1 or −2 per tick, bounded to 0..20.
            let x = &mut e.extra;
            x[0] = (x[0] + if x[1] != 0.0 { 1.0 } else { -2.0 }).clamp(0.0, 20.0);
        }
        "fox" => {
            let f = e.meta_byte(19);
            let x = &mut e.extra;
            x[1] = x[0];
            x[0] += ((if f & 8 != 0 { 1.0 } else { 0.0 }) - x[0]) * 0.4;
            x[3] = x[2];
            x[2] = if f & 4 != 0 { (x[2] + 0.2).min(5.0) } else { 0.0 };
        }
        "panda" => tick_panda(e),
        "armadillo" => {
            let state = e.meta_int(18).unwrap_or(0);
            let age = e.age as f32;
            let x = &mut e.extra;
            if changed(&mut x[0], state) {
                x[1] = 0.0;
            }
            match state {
                0 => {
                    State::stop(&mut x[2]);
                    State::stop(&mut x[3]);
                    State::stop(&mut x[4]);
                }
                1 => {
                    State::stop(&mut x[2]);
                    State::start_if_stopped(&mut x[3], age);
                    State::stop(&mut x[4]);
                }
                2 => {
                    State::stop(&mut x[2]);
                    State::stop(&mut x[3]);
                    if x[5] != 0.0 {
                        State::stop(&mut x[4]);
                        x[5] = 0.0;
                    }
                    if x[1] == 0.0 {
                        // Spawned (or passed) scared: peek advanced by its duration (50 ticks).
                        State::start(&mut x[4], age);
                        State::fast_forward(&mut x[4], 50.0);
                    } else {
                        State::start_if_stopped(&mut x[4], age);
                    }
                }
                _ => {
                    State::start_if_stopped(&mut x[2], age);
                    State::stop(&mut x[3]);
                    State::stop(&mut x[4]);
                }
            }
            x[1] += 1.0;
        }
        "sniffer" => {
            let state = e.meta_int(18).unwrap_or(0);
            let start = e.age as f32 - 1.0;
            let x = &mut e.extra;
            if changed(&mut x[0], state) {
                for v in &mut x[1..6] {
                    State::stop(v);
                }
                let slot = match state {
                    1 => 4,
                    2 => 5,
                    3 => 2,
                    5 => 1,
                    6 => 3,
                    _ => 0,
                };
                if slot != 0 {
                    State::start_if_stopped(&mut x[slot], start);
                }
            }
        }
        "frog" => {
            let pose = e.pose();
            let water = e.in_water;
            let moving = e.walk_speed > 1e-5;
            let (age, start) = (e.age as f32, e.age as f32 - 1.0);
            let x = &mut e.extra;
            State::animate_when(&mut x[4], water && !moving, age);
            if changed(&mut x[0], pose) {
                for (slot, want) in [(1, 6), (2, 8), (3, 9)] {
                    if pose == want {
                        State::start(&mut x[slot], start);
                    } else {
                        State::stop(&mut x[slot]);
                    }
                }
            }
        }
        "bee" => {
            let rolling = e.meta_byte(18) & 2 != 0;
            let x = &mut e.extra;
            x[1] = x[0];
            x[0] = if rolling { (x[0] + 0.2).min(1.0) } else { (x[0] - 0.24).max(0.0) };
        }
        "bat" => {
            let resting = e.meta_byte(16) & 1 != 0;
            let age = e.age as f32;
            let x = &mut e.extra;
            if resting {
                State::stop(&mut x[0]);
                State::start_if_stopped(&mut x[1], age);
            } else {
                State::stop(&mut x[1]);
                State::start_if_stopped(&mut x[0], age);
            }
        }
        "allay" => {
            let holding = e.equipment[0].is_some();
            let dancing = e.meta_bool(16);
            let x = &mut e.extra;
            x[1] = x[0];
            x[0] = (x[0] + if holding { 1.0 } else { -1.0 }).clamp(0.0, 5.0);
            if dancing {
                x[2] += 1.0;
                x[4] = x[3];
                let spinning = x[2] % 55.0 < 15.0;
                x[3] = (x[3] + if spinning { 1.0 } else { -1.0 }).clamp(0.0, 15.0);
            } else {
                x[2] = 0.0;
                x[3] = 0.0;
                x[4] = 0.0;
            }
        }
        _ => {}
    }
}

/// Client tick of the panda: synchronised counters modified locally like the game (the server
/// overwrites them at its next update), ramps sitting / back / roll.
fn tick_panda(e: &mut Entity) {
    let flags = e.meta_byte(23);
    let set_flag = |e: &mut Entity, bit: i8, on: bool| {
        let f = e.meta_byte(23);
        e.meta.insert(23, Meta::Byte(if on { f | bit } else { f & !bit }));
    };
    let unhappy = e.meta_int(18).unwrap_or(0);
    if unhappy > 0 {
        e.meta.insert(18, Meta::Int(unhappy - 1));
    }
    if flags & 2 != 0 {
        let n = e.meta_int(19).unwrap_or(0) + 1;
        if n > 20 {
            set_flag(e, 2, false);
            e.meta.insert(19, Meta::Int(0));
        } else {
            e.meta.insert(19, Meta::Int(n));
        }
    }
    if flags & 4 != 0 {
        e.extra[6] += 1.0;
        if e.extra[6] > 32.0 {
            set_flag(e, 4, false);
        }
    } else {
        e.extra[6] = 0.0;
    }
    let flags = e.meta_byte(23);
    let sitting = flags & 8 != 0;
    // Meal: starts 1 time in 80 when sitting with an item, stops without an item or standing.
    let eating = e.meta_int(20).unwrap_or(0) > 0;
    let held = e.equipment[0].is_some();
    if !eating && sitting && held && one_in(e, 23, 80) {
        e.meta.insert(20, Meta::Int(1));
    } else if !held || !sitting {
        e.meta.insert(20, Meta::Int(0));
    }
    if let Some(n) = e.meta_int(20).filter(|&n| n > 0) {
        e.meta.insert(20, Meta::Int(n + 1));
    }
    let x = &mut e.extra;
    for (i, on) in [(0, sitting), (2, flags & 16 != 0), (4, flags & 4 != 0)] {
        x[i + 1] = x[i];
        x[i] = if on { (x[i] + 0.15).min(1.0) } else { (x[i] - 0.19).max(0.0) };
    }
}

/// Entity event specific to the family; true if fully handled here (false: the state
/// is noted and the common handling, sounds included, continues).
pub(super) fn event(e: &mut Entity, id: i8) -> bool {
    match (e.kind.as_str(), id) {
        // Rabbit jump: 15 ticks.
        ("rabbit", 1) => {
            e.extra[1] = 15.0;
            e.extra[0] = 0.0;
            true
        }
        // Wolf: start (8) and stop (56, back in the water) of the shaking.
        ("wolf", 8) => {
            e.extra[2] = 0.0;
            e.extra[3] = 0.0;
            e.extra[4] = 1.0;
            false
        }
        ("wolf", 56) => {
            e.extra[2..5].fill(0.0);
            false
        }
        // Goat: head lowered to charge (58), raised (59).
        ("goat", 58 | 59) => {
            e.extra[1] = if id == 58 { 1.0 } else { 0.0 };
            false
        }
        // Armadillo: peek received (64), the peek in progress restarts at the next tick.
        ("armadillo", 64) => {
            e.extra[5] = 1.0;
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::keyframes::testkit::*;
    use super::*;

    const WOLF: [(&str, Option<usize>); 10] = [
        ("head", None),
        ("real_head", Some(0)),
        ("body", None),
        ("upper_body", None),
        ("right_hind_leg", None),
        ("left_hind_leg", None),
        ("right_front_leg", None),
        ("left_front_leg", None),
        ("tail", None),
        ("real_tail", Some(8)),
    ];

    /// Shaking (event 8): body and head roll for 2 / 0.05 = 40 ticks,
    /// stopped by event 56; before, the wolf fell back on the generic walk without roll.
    #[test]
    fn wolf_shake_rolls_then_stops() {
        let l = test_layer(&WOLF);
        let mut all = spawn("wolf");
        all.tick();
        fire(&mut all, 8);
        for _ in 0..10 {
            all.tick();
        }
        let e = &all.map[&7];
        assert!((e.extra[2] - 0.5).abs() < 1e-5, "{}", e.extra[2]);
        let p = posed(&l, e, 0.0);
        let t = ((0.45f32 - 0.16) / 1.8).clamp(0.0, 1.0);
        let want = (t * PI).sin() * (t * PI * 11.0).sin() * 0.15 * PI;
        assert!((part(&p, "body")[5] - want).abs() < 1e-5, "{} ≠ {want}", part(&p, "body")[5]);
        assert!(part(&p, "real_head")[5] != 0.0);
        // Tail of a wild wolf: π/5.
        assert!((part(&p, "tail")[3] - PI / 5.0).abs() < 1e-6);
        for _ in 0..40 {
            all.tick();
        }
        let e = &all.map[&7];
        assert_eq!(e.extra[2..5], [0.0; 3]);
        // Stop by event 56.
        fire(&mut all, 8);
        all.tick();
        fire(&mut all, 56);
        assert_eq!(all.map[&7].extra[2..5], [0.0; 3]);
    }

    /// Sitting wolf (18 & 1): sitting pose; tamed (18 & 4) hurt: lower tail.
    #[test]
    fn wolf_sits_and_tail_follows_health() {
        let l = test_layer(&WOLF);
        let mut all = spawn("wolf");
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(18, Meta::Byte(5));
        e.meta.insert(9, Meta::Float(20.0));
        let p = posed(&l, e, 0.0);
        assert_eq!(part(&p, "body")[3], PI / 4.0);
        assert_eq!(part(&p, "right_hind_leg")[3], PI * 3.0 / 2.0);
        assert!((part(&p, "upper_body")[3] - PI * 2.0 / 5.0).abs() < 1e-6);
        assert!((part(&p, "tail")[3] - (0.55 - 0.5 * 0.4) * PI).abs() < 1e-6);
    }

    /// Goat: head lowered 1 tick per tick (event 58) up to 30°, raised by 2 per tick
    /// (59); absent horn (19 false) hidden.
    #[test]
    fn goat_ram_head_and_horns() {
        let l = test_layer(&[("head", None), ("left_horn", Some(0)), ("right_horn", Some(0))]);
        let mut all = spawn("goat");
        fire(&mut all, 58);
        for _ in 0..25 {
            all.tick();
        }
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(19, Meta::Bool(false));
        let p = posed(&l, e, 0.0);
        assert!((part(&p, "head")[3] - deg(30.0)).abs() < 1e-6);
        assert!(!p.visible[1] && p.visible[2]);
        fire(&mut all, 59);
        for _ in 0..3 {
            all.tick();
        }
        assert_eq!(all.map[&7].extra[0], 14.0);
    }

    /// Armadillo: rolled (state 1) hidden after 5 ticks in the state (body not drawn, head kept),
    /// cube visible; at rest the cube is hidden.
    #[test]
    fn armadillo_hides_in_shell() {
        let l = test_layer(&[
            ("body", None),
            ("tail", Some(0)),
            ("head", Some(0)),
            ("right_hind_leg", None),
            ("left_hind_leg", None),
            ("cube", None),
        ]);
        let mut all = spawn("armadillo");
        all.tick();
        let p = posed(&l, &all.map[&7], 0.0);
        assert!(!p.visible[5] && p.visible[1]);
        all.map.get_mut(&7).unwrap().meta.insert(18, Meta::Variant(1, 0));
        for _ in 0..7 {
            all.tick();
        }
        let e = &all.map[&7];
        assert!(State::started(e.extra[3]));
        let p = posed(&l, e, 0.0);
        assert!(p.visible[5] && !p.visible[1] && !p.visible[3]);
        // Body shrunk, head compensated: its composed matrix is unchanged.
        let b = part(&p, "body");
        let h = part(&p, "head");
        assert!((b[6] * h[6] - 1.0).abs() < 1e-6);
    }

    /// Panda: roll (23 & 4) stopped locally after 32 ticks like the client; sneeze
    /// counted locally.
    #[test]
    fn panda_local_counters() {
        let mut all = spawn("panda");
        all.map.get_mut(&7).unwrap().meta.insert(23, Meta::Byte(4 | 2));
        for _ in 0..33 {
            all.tick();
        }
        let e = &all.map[&7];
        assert_eq!(e.meta_byte(23) & 4, 0);
        assert_eq!(e.meta_byte(23) & 2, 0);
        assert_eq!(e.meta_int(19), Some(0));
    }

    /// Frog: "croaks" pose (8) seen at the next tick, start at the reception tick, inflated body
    /// visible while the state runs; stopped by another pose.
    #[test]
    fn frog_croak_follows_pose() {
        let l = test_layer(&[("body", None), ("croaking_body", Some(0))]);
        let mut all = spawn("frog");
        for _ in 0..3 {
            all.tick();
        }
        assert!(!posed(&l, &all.map[&7], 0.0).visible[1]);
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(8));
        all.tick();
        let e = &all.map[&7];
        assert_eq!(State::start_of(e.extra[2]), Some(3.0));
        assert!(posed(&l, e, 0.0).visible[1]);
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(0));
        all.tick();
        assert!(!State::started(all.map[&7].extra[2]));
    }

    /// Sleeping fox (19 & 32): legs hidden, head turned by −2π/3.
    #[test]
    fn fox_sleeps() {
        let l = test_layer(&[
            ("head", None),
            ("body", None),
            ("tail", Some(1)),
            ("right_hind_leg", None),
            ("left_hind_leg", None),
            ("right_front_leg", None),
            ("left_front_leg", None),
        ]);
        let mut all = spawn("fox");
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(19, Meta::Byte(32));
        let p = posed(&l, e, 0.0);
        assert!(p.visible[3..7].iter().all(|v| !v));
        assert_eq!(part(&p, "head")[4], -PI * 2.0 / 3.0);
        assert_eq!(part(&p, "body")[5], -PI / 2.0);
    }

    /// Own target walk: creaking ×25 up to 3, camel ×6 smoothed at 0.2, otherwise ×4 bounded to 1.
    #[test]
    fn walk_targets_per_kind() {
        let all = spawn("creaking");
        assert_eq!(walk_target(&all.map[&7], 0.1), (2.5, 0.4));
        let all = spawn("cow");
        assert_eq!(walk_target(&all.map[&7], 0.1), (0.4, 0.4));
    }

    /// Recomputed walk of a ridden mount: same target as the common tick (camel ×6 at 0.2).
    #[test]
    fn redo_walk_uses_kind_target() {
        let mut all = spawn("camel");
        let e = all.map.get_mut(&7).unwrap();
        e.extra[8] = -1.0;
        e.pos_o = e.pos;
        e.pos[0] += 0.1;
        e.walk_speed_o = 0.0;
        e.walk_speed = 0.0;
        e.redo_walk();
        assert!((e.walk_speed - 0.6 * 0.2).abs() < 1e-6, "{}", e.walk_speed);
    }

    fn shown(p: &Pose<'_>, name: &str) -> bool {
        p.visible[p.index(name).unwrap()]
    }

    /// Camel reins and equine saddle lines: visible only with a passenger.
    /// Before: always hidden.
    #[test]
    fn reins_shown_when_ridden() {
        let l = test_layer(&[("head", None), ("reins", Some(0))]);
        let mut all = spawn("camel");
        assert!(!shown(&posed(&l, &all.map[&7], 0.0), "reins"));
        all.map.get_mut(&7).unwrap().passengers.push(8);
        assert!(shown(&posed(&l, &all.map[&7], 0.0), "reins"));
        let l = test_layer(&[
            ("head_parts", None),
            ("body", None),
            ("left_saddle_line", Some(0)),
            ("right_saddle_line", Some(0)),
        ]);
        for kind in ["horse", "donkey", "zombie_horse"] {
            let mut all = spawn(kind);
            let p = posed(&l, &all.map[&7], 0.0);
            assert!(!shown(&p, "left_saddle_line") && !shown(&p, "right_saddle_line"), "{kind}");
            all.map.get_mut(&7).unwrap().passengers.push(8);
            let p = posed(&l, &all.map[&7], 0.0);
            assert!(shown(&p, "left_saddle_line") && shown(&p, "right_saddle_line"), "{kind}");
        }
    }

    /// Equine stride in water: phase × 0.2 (cos(0.2 × pos × 0.6662 + π)). Before: × 1.
    #[test]
    fn equine_stride_in_water() {
        let l = test_layer(&[("head_parts", None), ("body", None), ("left_hind_leg", None)]);
        let mut all = spawn("horse");
        let e = all.map.get_mut(&7).unwrap();
        e.walk_pos = 3.0;
        e.walk_speed = 0.5;
        e.walk_speed_o = 0.5;
        let dry = part(&posed(&l, e, 1.0), "left_hind_leg")[3];
        assert!((dry + (3.0f32 * 0.6662 + PI).cos() * 0.5 * 0.5).abs() < 1e-6);
        e.in_water = true;
        let wet = part(&posed(&l, e, 1.0), "left_hind_leg")[3];
        assert!((wet + (0.2 * 3.0f32 * 0.6662 + PI).cos() * 0.5 * 0.5).abs() < 1e-6, "{wet}");
    }

    /// Turtle placed on the bottom of the water: swims (front legs in z), not the land walk (y).
    #[test]
    fn turtle_on_seabed_swims() {
        let l = test_layer(&[("right_front_leg", None), ("left_front_leg", None)]);
        let mut all = spawn("turtle");
        let e = all.map.get_mut(&7).unwrap();
        e.on_ground = true;
        e.in_water = true;
        e.walk_pos = 2.0;
        e.walk_speed = 0.5;
        e.walk_speed_o = 0.5;
        let p = posed(&l, e, 1.0);
        let sw = (2.0f32 * 0.6662 * 0.6).cos() * 0.5 * 0.5;
        assert_eq!(part(&p, "right_front_leg")[4], 0.0);
        assert!((part(&p, "right_front_leg")[5] + sw).abs() < 1e-6);
        e.in_water = false;
        let p = posed(&l, e, 1.0);
        assert_eq!(part(&p, "right_front_leg")[5], 0.0);
        assert_ne!(part(&p, "right_front_leg")[4], 0.0);
    }

    /// Frog on the bottom of the water (on the ground) still: aquatic rest started. Before: the water
    /// was treated as "off the ground", never at the bottom.
    #[test]
    fn frog_idles_in_water_on_ground() {
        let mut all = spawn("frog");
        let e = all.map.get_mut(&7).unwrap();
        e.on_ground = true;
        e.in_water = true;
        all.tick();
        assert!(State::started(all.map[&7].extra[4]));
        let e = all.map.get_mut(&7).unwrap();
        e.in_water = false;
        all.tick();
        assert!(!State::started(all.map[&7].extra[4]));
    }

    /// Worried panda (gene 2) sitting during the thunderstorm: head 2.1707964, front legs −0.9,
    /// yaw shaking cos(age × 1.25) × π × 0.05; normal gene: ordinary sitting pose.
    #[test]
    fn panda_scared_by_thunder() {
        set_thundering(true);
        let l = test_layer(&[("head", None), ("body", None), ("right_front_leg", None), ("left_front_leg", None)]);
        let mut all = spawn("panda");
        let e = all.map.get_mut(&7).unwrap();
        e.extra[0] = 1.0;
        e.extra[1] = 1.0;
        e.age = 10;
        let calm = part(&posed(&l, e, 0.0), "head")[3];
        let mut calm_m = Mat4::IDENTITY;
        rotations(e, 0.0, &mut calm_m);
        e.meta.insert(21, Meta::Byte(2));
        assert!(panda_scared(e));
        let p = posed(&l, e, 0.0);
        assert_eq!(part(&p, "head")[3], 2.170_796_4);
        assert_ne!(calm, 2.170_796_4);
        assert_eq!(part(&p, "right_front_leg")[3], -0.9);
        assert_eq!(part(&p, "left_front_leg")[3], -0.9);
        let mut m = Mat4::IDENTITY;
        rotations(e, 0.0, &mut m);
        let shake = (f64::from(10.0f32 * 1.25).cos() * std::f64::consts::PI * 0.05) as f32;
        let want = calm_m * Mat4::from_rotation_y(deg(shake));
        assert!(m.abs_diff_eq(want, 1e-6), "{m:?}");
    }

    /// Cat lying (21) near a sleeping player (bed position, 14): offset by 0.15 × lying
    /// in x after the rotation; awake or far player: nothing.
    #[test]
    fn cat_on_sleeping_player() {
        let mut all = spawn("cat");
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(21, Meta::Bool(true));
        e.extra[0] = 1.0;
        e.extra[1] = 1.0;
        let mut player = e.clone();
        player.kind = "player".into();
        player.meta = Default::default();
        player.meta.insert(14, Meta::BlockPos(Some([2, 64, 0])));
        player.pos = [2.5, 64.2, 0.5];
        all.map.insert(8, player);
        let mut alone = Mat4::IDENTITY;
        rotations(&all.map[&7], 0.0, &mut alone);
        cat_track(&mut all);
        assert_eq!(all.map[&7].extra[6], 1.0);
        let mut m = Mat4::IDENTITY;
        rotations(&all.map[&7], 0.0, &mut m);
        assert!(m.abs_diff_eq(alone * Mat4::from_translation(Vec3::new(0.15, 0.0, 0.0)), 1e-6));
        // Outside the block widened by 2 (x ≥ 3 + 0.1): nothing.
        all.map.get_mut(&8).unwrap().pos[0] = 3.11;
        cat_track(&mut all);
        assert_eq!(all.map[&7].extra[6], 0.0);
        all.map.get_mut(&8).unwrap().pos[0] = 3.09;
        cat_track(&mut all);
        assert_eq!(all.map[&7].extra[6], 1.0);
        all.map.get_mut(&8).unwrap().meta.insert(14, Meta::BlockPos(None));
        cat_track(&mut all);
        assert_eq!(all.map[&7].extra[6], 0.0);
    }

    /// Micro-bench (`--ignored --nocapture`): per-tick cost of water contact (1,000 equines
    /// in water + 1,000 ignored cows) and of cat tracking without a sleeping player.
    #[test]
    #[ignore]
    fn bench_water_and_cat_track() {
        let data = rc_data::load_cached("26.3").expect("26.3 data in cache");
        let water = data
            .states
            .iter()
            .position(|s| data.fluids.get(usize::from(s.fluid)).is_some_and(|f| f.contains("water")))
            .expect("water") as u32;
        let mut all = spawn("horse");
        let proto = all.map[&7].clone();
        for i in 0..2000 {
            let mut e = proto.clone();
            e.pos = [f64::from(i % 50) * 3.0, 64.0, f64::from(i / 50) * 3.0];
            if i % 2 == 1 {
                e.kind = "cow".into();
            }
            if i % 4 == 0 {
                e.kind = "cat".into();
            }
            all.map.insert(100 + i, e);
        }
        let state = |_: i32, y: i32, _: i32| if y <= 64 { water } else { 0 };
        let n = 2000;
        let t = std::time::Instant::now();
        for _ in 0..n {
            super::super::water_track(&mut all, &data, &data.entity_dims, &state);
        }
        let w = t.elapsed().as_secs_f64() / f64::from(n) * 1e6;
        let t = std::time::Instant::now();
        for _ in 0..n {
            cat_track(&mut all);
        }
        let c = t.elapsed().as_secs_f64() / f64::from(n) * 1e6;
        assert!(all.map[&101].in_water || all.map[&101].kind != "horse");
        println!("water_track {w:.1} µs/tick, cat_track {c:.1} µs/tick (2,001 entities)");
    }

    /// Shadow radii of the game's renderers (13 kinds wrong before).
    #[test]
    fn shadow_radii_match_renderers() {
        for (k, r) in [
            ("allay", 0.4),
            ("armadillo", 0.4),
            ("bat", 0.25),
            ("bee", 0.4),
            ("cave_spider", 0.56),
            ("creaking", 0.6),
            ("dolphin", 0.7),
            ("iron_golem", 0.7),
            ("panda", 0.9),
            ("phantom", 0.75),
            ("sniffer", 1.1),
            ("tadpole", 0.14),
            ("warden", 0.9),
        ] {
            assert_eq!(super::super::shadow_of(k), (r, 1.0), "{k}");
        }
    }

    /// Jukebox (1010) whose block widened by 3 touches the parrot's box: party pose (legs
    /// spread by π/9, circle cos/sin of the age, head straight that rolls); too far for the box:
    /// nothing. End (1011): party over, jukebox kept; centre at 3.46 or block gone: forgotten.
    #[test]
    fn parrot_parties_near_jukebox() {
        const PARROT: [(&str, Option<usize>); 7] = [
            ("head", None),
            ("body", None),
            ("tail", None),
            ("left_wing", None),
            ("right_wing", None),
            ("left_leg", None),
            ("right_leg", None),
        ];
        let l = test_layer(&PARROT);
        let dims = [[0.5, 0.9, 0.54, 0.0]];
        let mut all = spawn("parrot");
        all.map.get_mut(&7).unwrap().on_ground = true;
        all.map.get_mut(&7).unwrap().age = 10;
        // Block [4, 64, 0] widened by 3: x from 1 to 8, the box (0.25..0.75) does not touch it.
        super::super::jukebox_notify(&mut all, &dims, [4, 64, 0], true);
        assert_eq!(all.map[&7].jukebox, None);
        // Block [−4, 64, 0] widened: x from −7 to 0 (strict bound), not touched either.
        super::super::jukebox_notify(&mut all, &dims, [-4, 64, 0], true);
        assert_eq!(all.map[&7].jukebox, None);
        let calm = *part(&posed(&l, &all.map[&7], 0.0), "left_leg");
        assert_eq!(calm[5], 0.0);
        super::super::jukebox_notify(&mut all, &dims, [3, 64, 0], true);
        let e = &all.map[&7];
        assert_eq!(e.jukebox, Some(([3, 64, 0], true)));
        let p = posed(&l, e, 0.0);
        let leg = (std::f64::consts::PI / 9.0) as f32;
        assert_eq!(part(&p, "left_leg")[5], -leg);
        assert_eq!(part(&p, "right_leg")[5], leg);
        let head = part(&p, "head");
        assert_eq!((head[0], head[1], head[3], head[4]), (10f32.cos(), 10f32.sin(), 0.0, 0.0));
        assert_eq!(head[5], 10f32.sin() * 0.4);
        assert_eq!(part(&p, "tail")[0], 10f32.cos());
        assert_eq!(part(&p, "left_leg")[1], 0.0, "legs outside the circle");
        // End: party over, jukebox remembered.
        super::super::jukebox_notify(&mut all, &dims, [3, 64, 0], false);
        assert_eq!(all.map[&7].jukebox, Some(([3, 64, 0], false)));
        assert_eq!(part(&posed(&l, &all.map[&7], 0.0), "left_leg")[5], 0.0);
        // Kept when less than 3.46 from the centre while the block remains a jukebox; forgotten otherwise.
        super::super::jukebox_notify(&mut all, &dims, [3, 64, 0], true);
        super::super::party_track(&mut all, &|_| true);
        assert!(all.map[&7].jukebox.is_some());
        super::super::party_track(&mut all, &|_| false);
        assert_eq!(all.map[&7].jukebox, None, "block that is no longer a jukebox");
        super::super::jukebox_notify(&mut all, &dims, [3, 64, 0], true);
        all.map.get_mut(&7).unwrap().pos[0] = 0.04;
        super::super::party_track(&mut all, &|_| true);
        assert_eq!(all.map[&7].jukebox, None, "centre at 3.46 or more");
    }

    /// Per-tick cost of `party_track` (2,001 entities, one marked parrot) and of a `jukebox_notify`.
    #[test]
    #[ignore = "bench"]
    fn bench_party_track() {
        let mut all = spawn("parrot");
        let base = all.map[&7].clone();
        for i in 0..2000 {
            let mut e = base.clone();
            e.kind = if i % 2 == 0 { "cow".into() } else { "parrot".into() };
            e.pos = [f64::from(i % 50) * 2.0, 64.0, f64::from(i / 50) * 2.0];
            all.map.insert(100 + i, e);
        }
        all.map.get_mut(&7).unwrap().jukebox = Some(([3, 64, 0], true));
        let dims = [[0.5, 0.9, 0.54, 0.0]];
        let n = 10_000;
        let t = std::time::Instant::now();
        for _ in 0..n {
            super::super::party_track(&mut all, &|_| true);
        }
        let track = t.elapsed().as_secs_f64() / f64::from(n) * 1e6;
        let t = std::time::Instant::now();
        for _ in 0..1000 {
            super::super::jukebox_notify(&mut all, &dims, [3, 64, 0], true);
        }
        let notify = t.elapsed().as_secs_f64() / 1000.0 * 1e6;
        eprintln!("party_track {track:.2} µs/tick, jukebox_notify {notify:.2} µs/event");
    }
}
