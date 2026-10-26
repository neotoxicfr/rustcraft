//! Tools to check the entities that move by themselves on the client side (test scripts):
//! - `RUSTCRAFT_CAPSEQ=<folder>;<start s>;<count>;<interval ms>`: sequence of captures
//!   `<folder>/NNN.bmp`, then the client closes;
//! - `RUSTCRAFT_MOTIONLOG=1`: displayed position, velocity and ground state of items, orbs, falling
//!   blocks, TNT and projectiles at every tick (error output).

use std::sync::Mutex;
use std::time::Instant;

use crate::entities::Entities;

struct Seq {
    dir: std::path::PathBuf,
    next_at: f64,
    left: u32,
    step: f64,
    index: u32,
}

static SEQ: Mutex<Option<Option<Seq>>> = Mutex::new(None);

fn parse() -> Option<Seq> {
    let v = rc_data::env_str!("RUSTCRAFT_CAPSEQ")?;
    let mut it = v.split(';');
    let dir = std::path::PathBuf::from(it.next()?);
    let start: f64 = it.next()?.parse().ok()?;
    let left: u32 = it.next()?.parse().ok()?;
    let step = it.next()?.parse::<f64>().ok()? / 1000.0;
    let _ = std::fs::create_dir_all(&dir);
    Some(Seq { dir, next_at: start, left, step, index: 0 })
}

/// Next capture of the sequence if its time has come; true when the sequence is over.
pub fn frame(r: &mut crate::render::Renderer, started: Instant) -> bool {
    let Ok(mut g) = SEQ.lock() else { return false };
    let seq = g.get_or_insert_with(parse);
    let Some(s) = seq else { return false };
    if s.left == 0 {
        return true;
    }
    if started.elapsed().as_secs_f64() >= s.next_at {
        r.request_capture(s.dir.join(format!("{:03}.bmp", s.index)));
        s.index += 1;
        s.left -= 1;
        s.next_at += s.step;
    }
    false
}

fn logging() -> bool {
    rc_data::env_flag!("RUSTCRAFT_MOTIONLOG")
}

/// Resync received from the server for an entity that moves by itself: difference between the simulated
/// position and that of the packet (`RUSTCRAFT_MOTIONLOG` trace).
pub fn log_resync(id: i32, e: &crate::entities::Entity, to: [f64; 3]) {
    if !logging() || e.class != crate::entities::Kind::Other {
        return;
    }
    let d: [f64; 3] = std::array::from_fn(|i| to[i] - e.pos[i]);
    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    eprintln!(
        "resync: {id} {} age {} difference {n:.5} ({:.5} {:.5} {:.5}) towards {:.4} {:.4} {:.4}",
        e.kind, e.age, d[0], d[1], d[2], to[0], to[1], to[2]
    );
}

/// Trace of one tick of the entities that move by themselves (after their tick).
pub fn log_tick(ents: &Entities) {
    if !logging() {
        return;
    }
    const KINDS: &[&str] = &[
        "item",
        "experience_orb",
        "falling_block",
        "tnt",
        "arrow",
        "spectral_arrow",
        "trident",
        "snowball",
        "egg",
        "ender_pearl",
        "splash_potion",
        "lingering_potion",
        "experience_bottle",
        "firework_rocket",
        "llama_spit",
        "fireball",
        "small_fireball",
        "dragon_fireball",
        "wither_skull",
        "wind_charge",
        "breeze_wind_charge",
        "shulker_bullet",
        "eye_of_ender",
        "fishing_bobber",
    ];
    for (id, e) in &ents.map {
        if KINDS.contains(&e.kind.as_str()) {
            let (p, v) = (e.pos, e.velocity);
            eprintln!(
                "motion: {id} {} age {} pos {:.4} {:.4} {:.4} vel {:.4} {:.4} {:.4} ground {}",
                e.kind, e.age, p[0], p[1], p[2], v[0], v[1], v[2], e.on_ground
            );
        }
    }
}
