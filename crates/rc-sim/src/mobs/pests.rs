//! Small pests (family F01): silverfish (waking of siblings in infested blocks,
//! merging into stone, exit from a broken infested block). The endermite has only common
//! goals and its lifespan (`variants.rs`).

use rc_proto::nbt::Nbt;

use super::goals::Goal;
use super::mob::Mob;
use super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

/// Host blocks of infested blocks.
const HOSTS: [&str; 7] = [
    "stone",
    "cobblestone",
    "stone_bricks",
    "mossy_stone_bricks",
    "cracked_stone_bricks",
    "chiseled_stone_bricks",
    "deepslate",
];

/// Directions in the game's order (down, up, north, south, west, east).
const DIRS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

/// Hurt silverfish: arms the waking of siblings (~20 ticks adjusted).
pub fn notify_hurt(m: &mut Mob) {
    if m.x.friends == 0 {
        m.x.friends = super::mth::reduced(20);
    }
}

pub(super) fn is_host(name: &str) -> bool {
    HOSTS.contains(&name.trim_start_matches("minecraft:"))
}

fn is_infested(name: &str) -> bool {
    name.trim_start_matches("minecraft:").strip_prefix("infested_").is_some_and(|h| HOSTS.contains(&h))
}

impl Server {
    /// Same state with another block (properties copied, e.g. deepslate axis).
    fn swap_block(&self, state: u32, name: &str) -> Option<u32> {
        let props: Vec<(String, Nbt)> =
            self.data.state_properties(state).map(|(k, v)| (k.to_owned(), Nbt::String(v.to_owned()))).collect();
        let entry = Nbt::Compound(vec![
            ("Name".into(), Nbt::String(name.to_owned())),
            ("Properties".into(), Nbt::Compound(props)),
        ]);
        self.states.state_of(&self.data, &entry)
    }

    /// Silverfish coming out of an infested block (no finalisation, spawn animation).
    fn spawn_infestation(&mut self, p: [i32; 3]) {
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let mut m = self.new_mob(Kind::Silverfish, pos, 0.0);
        m.head_yaw = 0.0;
        m.body_yaw = 0.0;
        self.send_changes(&mut m);
        self.mob_event(&m, 20);
        self.add_mob(m);
    }

    /// Infested block broken by the player: silverfish except with silk touch.
    pub(crate) fn infested_broken(&mut self, p: [i32; 3], block: &str, tool: Option<&Stack>) {
        if !is_infested(block) || !self.rule("minecraft:block_drops", true) {
            return;
        }
        if tool.is_some_and(|t| self.life.items.enchantment(t, "minecraft:silk_touch") > 0) {
            return;
        }
        self.spawn_infestation(p);
    }

    /// End of the countdown: infested blocks woken from the inside out.
    pub(crate) fn silverfish_wake_tick(&mut self, m: &mut Mob) {
        m.x.friends -= 1;
        if m.x.friends > 0 {
            return;
        }
        let base = m.block_pos();
        let griefing = self.rule("minecraft:mob_griefing", true);
        let next = |v: i32| if v <= 0 { 1 } else { 0 } - v;
        let mut y = 0;
        while (-5..=5).contains(&y) {
            let mut x = 0;
            while (-10..=10).contains(&x) {
                let mut z = 0;
                while (-10..=10).contains(&z) {
                    let p = [base[0] + x, base[1] + y, base[2] + z];
                    let name = self.bname(p).to_owned();
                    if is_infested(&name) {
                        if griefing {
                            // Event 2001, loot (the silverfish) then removal.
                            let by = super::recent::vibration::Src::Living(super::mob::Ent::Mob(m.id));
                            self.destroy_block_by(p, true, 512, Some(by));
                        } else {
                            let host = format!("minecraft:{}", name.trim_start_matches("minecraft:infested_"));
                            if let Some(s) = self.swap_block(self.st(p), &host) {
                                self.set_block(p, s, crate::blocks::flag::ALL);
                            }
                        }
                        if m.rng.next_bool() {
                            return;
                        }
                    }
                    z = next(z);
                }
                x = next(x);
            }
            y = next(y);
        }
    }

    /// Wandering or merging: direction draw, compatible neighbouring host.
    pub(crate) fn silverfish_merge_can_use(
        &mut self,
        m: &mut Mob,
        merge: &mut bool,
        dir: &mut usize,
        stroll: &mut Goal,
    ) -> bool {
        if m.target.is_some() || !m.nav.is_done() {
            return false;
        }
        if self.rule("minecraft:mob_griefing", true) && m.rng.next_int_bound(super::mth::reduced(10)) == 0 {
            *dir = m.rng.next_int_bound(6) as usize;
            let p = self.merge_pos(m, *dir);
            if is_host(self.bname(p)) {
                *merge = true;
                return true;
            }
        }
        *merge = false;
        self.goal_can_use(stroll, m)
    }

    fn merge_pos(&self, m: &Mob, dir: usize) -> [i32; 3] {
        let b = super::mth::block_of([m.pos[0], m.pos[1] + 0.5, m.pos[2]]);
        let d = DIRS[dir];
        [b[0] + d[0], b[1] + d[1], b[2] + d[2]]
    }

    /// Merging: the block becomes infested, the silverfish disappears.
    pub(crate) fn silverfish_merge(&mut self, m: &mut Mob, dir: usize) {
        let p = self.merge_pos(m, dir);
        let name = self.bname(p).to_owned();
        if !is_host(&name) {
            return;
        }
        let infested = format!("minecraft:infested_{}", name.trim_start_matches("minecraft:"));
        if let Some(s) = self.swap_block(self.st(p), &infested) {
            self.set_block(p, s, crate::blocks::flag::ALL);
        }
        self.mob_event(m, 20);
        m.removed = true;
    }
}
