//! Bell ringing on the server side (bell block entity): on ringing, living entities
//! within 48 blocks are recorded (at most one search every 60 ticks); 5 ticks
//! later, if there is a pillager of the tag `raiders` within 32 blocks, the bell resonates (sound), then
//! 40 ticks later the pillagers within 48 blocks get the glow for 60 ticks.

use super::mob::Ent;
use crate::server::Server;

/// State of a rung bell.
#[derive(Debug, Clone, Default)]
pub struct Bell {
    pub pos: [i32; 3],
    ticks: i32,
    shaking: bool,
    resonating: bool,
    resonation: i32,
    last_search: i64,
    /// Entities recorded at the last search (`None`: never searched).
    nearby: Option<Vec<Ent>>,
}

/// Block centre within `d`.
fn near(b: [i32; 3], p: [f64; 3], d: f64) -> bool {
    let c = b.map(|v| f64::from(v) + 0.5);
    (0..3).map(|i| (c[i] - p[i]) * (c[i] - p[i])).sum::<f64>() < d * d
}

impl Server {
    /// Block event 1 of a bell: entities recorded, shake reset
    /// to zero.
    pub(crate) fn bell_rung(&mut self, p: [i32; 3]) {
        let t = self.time;
        let i = match self.mobs.bells.iter().position(|b| b.pos == p) {
            Some(i) => i,
            None => {
                self.mobs.bells.push(Bell { pos: p, ..Bell::default() });
                self.mobs.bells.len() - 1
            }
        };
        let mut b = std::mem::take(&mut self.mobs.bells[i]);
        if t > b.last_search + 60 || b.nearby.is_none() {
            b.last_search = t;
            let area = rc_physics::Aabb {
                min: [f64::from(p[0]) - 48.0, f64::from(p[1]) - 48.0, f64::from(p[2]) - 48.0],
                max: [f64::from(p[0]) + 49.0, f64::from(p[1]) + 49.0, f64::from(p[2]) + 49.0],
            };
            let mut list: Vec<Ent> =
                self.mobs.list.iter().flatten().filter(|m| m.touches(&area)).map(|m| Ent::Mob(m.id)).collect();
            if self.ent_bbox(Ent::Player).is_some_and(|bb| bb.intersects(&area)) {
                list.push(Ent::Player);
            }
            b.nearby = Some(list);
        }
        // "Bell heard" memory of living villagers within 32 blocks (record kept).
        let gt = self.time;
        for e in b.nearby.iter().flatten() {
            let Ent::Mob(id) = *e else { continue };
            let Some(o) = self.mobs.get_mut(id) else { continue };
            if !o.alive() || o.removed || !near(p, o.pos, 32.0) {
                continue;
            }
            if let Some(br) = o.npc.as_mut().and_then(|n| n.brain.as_mut()) {
                br.mem.heard_bell = Some(gt);
            }
        }
        b.resonation = 0;
        b.ticks = 0;
        b.shaking = true;
        self.mobs.bells[i] = b;
    }

    /// Living pillager of the tag `raiders` within `d` of the bell's centre.
    fn bell_raider(&self, p: [i32; 3], e: Ent, d: f64) -> bool {
        let Ent::Mob(id) = e else { return false };
        self.mob_ref(id)
            .is_some_and(|m| m.alive() && !m.removed && super::illagers::is_raider(m.kind) && near(p, m.pos, d))
    }

    /// Tick of rung bells; a bell whose block has disappeared is forgotten.
    pub(crate) fn bells_tick(&mut self) {
        let mut i = 0;
        while i < self.mobs.bells.len() {
            let mut b = std::mem::take(&mut self.mobs.bells[i]);
            if self.bname(b.pos) != "minecraft:bell" {
                self.mobs.bells.swap_remove(i);
                continue;
            }
            // Same test as the other block entities: outside the border or out of tick range,
            // the bell is kept but frozen.
            if !self.be_ticking(b.pos) {
                self.mobs.bells[i] = b;
                i += 1;
                continue;
            }
            if b.shaking {
                b.ticks += 1;
            }
            if b.ticks >= 50 {
                b.shaking = false;
                b.ticks = 0;
            }
            let list = b.nearby.clone().unwrap_or_default();
            if b.ticks >= 5 && b.resonation == 0 && list.iter().any(|&e| self.bell_raider(b.pos, e, 32.0)) {
                b.resonating = true;
                self.block_sound(b.pos, "minecraft:block.bell.resonate", 1.0, 1.0);
            }
            if b.resonating {
                if b.resonation < 40 {
                    b.resonation += 1;
                } else {
                    for e in list {
                        if self.bell_raider(b.pos, e, 48.0)
                            && let Ent::Mob(id) = e
                            && let Some(k) = self.mob_index(id)
                        {
                            let mut m = self.mobs.list[k].take().expect("mob");
                            self.give_mob_effect(&mut m, "minecraft:glowing", 60, 0);
                            self.mobs.list[k] = Some(m);
                        }
                    }
                    b.resonating = false;
                }
            }
            self.mobs.bells[i] = b;
            i += 1;
        }
    }
}
