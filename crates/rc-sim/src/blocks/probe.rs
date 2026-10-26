//! Verification probe (§ 4.4): builds a setup with game commands (`setblock`,
//! `fill`), advances tick by tick and reads the states of an area. The same script is replayed on
//! the official server by `tools/testmap/blocks_probe.py` to compare.
//!
//! Launch: `RUSTCRAFT_SOLO_WORLD=<world copy> RUSTCRAFT_PROBE=<script>
//! cargo test --release -p rc-sim probe_script -- --ignored --nocapture`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rc_proto::nbt::Nbt;

use super::{Dir, P, flag, rel};
use crate::server::Server;

pub(crate) struct Probe {
    pub s: Server,
    session: rc_client::Session,
    region: Option<(P, P)>,
    last: Vec<u32>,
    pub out: String,
    pub ticks: i64,
    watch: Vec<P>,
}

/// Block state written like the game (`minecraft:name[p=v,…]`, sorted properties).
pub(crate) fn state_string(s: &Server, st: u32) -> String {
    let name = s.data.block_of(st).map_or("?", |b| b.name.as_str()).to_owned();
    let props: Vec<String> = s.data.state_properties(st).map(|(k, v)| format!("{k}={v}")).collect();
    if props.is_empty() { name } else { format!("{name}[{}]", props.join(",")) }
}

impl Probe {
    pub fn open(world: &Path, data: Arc<rc_data::VersionData>) -> Probe {
        crate::tests::solo_slot();
        let uuid = rc_client::offline_uuid("S3");
        let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
        let opts = crate::Options {
            world: world.to_owned(),
            name: "S3".into(),
            uuid,
            info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
            sim_distance: 12,
        };
        let s = match Server::open(opts, uuid, data, peer, Arc::new(AtomicBool::new(false))) {
            Ok(s) => s,
            Err((_, e)) => panic!("opening: {e}"),
        };
        Probe { s, session, region: None, last: Vec::new(), out: String::new(), ticks: 0, watch: Vec::new() }
    }

    fn drain(&self) {
        while self.session.events.try_recv().is_ok() {}
    }

    /// Events sent to the client since the last read.
    pub(crate) fn take_events(&self) -> Vec<rc_client::Event> {
        let mut out = Vec::new();
        while let Ok(e) = self.session.events.try_recv() {
            out.push(e);
        }
        out
    }

    fn load_box(&mut self, a: P, b: P) {
        let mut want = Vec::new();
        for cx in (a[0].min(b[0]) >> 4) - 1..=(a[0].max(b[0]) >> 4) + 1 {
            for cz in (a[2].min(b[2]) >> 4) - 1..=(a[2].max(b[2]) >> 4) + 1 {
                want.push((cx, cz));
            }
        }
        self.s.ensure_loaded(&want);
    }

    /// State and properties explicitly given from a string `minecraft:name[p=v]`.
    fn parse_state(&self, text: &str) -> (u32, Vec<(String, String)>) {
        let (name, props) = match text.split_once('[') {
            Some((n, rest)) => (n, rest.trim_end_matches(']')),
            None => (text, ""),
        };
        let name = if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") };
        let given: Vec<(String, String)> = props
            .split(',')
            .filter(|x| !x.is_empty())
            .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())))
            .collect();
        let entry = Nbt::Compound(vec![
            ("Name".into(), Nbt::String(name.clone())),
            (
                "Properties".into(),
                Nbt::Compound(given.iter().map(|(k, v)| (k.clone(), Nbt::String(v.clone()))).collect()),
            ),
        ]);
        let st = self.s.states.state_of(&self.s.data, &entry).unwrap_or_else(|| panic!("unknown state: {text}"));
        (st, given)
    }

    /// `/setblock` (replace mode, non-strict).
    fn setblock(&mut self, p: P, text: &str) -> Option<u32> {
        let (base, given) = self.parse_state(text);
        let old = self.s.get(p);
        let mut st = self.s.update_from_neighbour_shapes(base, p);
        if self.s.v().is_air(st) {
            st = base;
        }
        for (k, val) in &given {
            st = self.s.v().set(st, k, val);
        }
        self.s.set_block(p, st, flag::CLIENTS | flag::SKIP_BE_SIDE_EFFECTS).then_some(old)
    }

    fn after_set(&mut self, p: P, old: u32) {
        let s = &mut self.s;
        let st = s.get(p);
        let nb = s.v().block(st);
        if s.v().block(old) != nb {
            s.affect_after_removal(old, p, false);
        }
        s.update_neighbors_at(p, nb);
        if s.v().has_analog(st) {
            s.update_neighbour_for_output_signal(p, nb);
        }
    }

    /// One game tick: time then block phases, broadcast.
    pub fn tick(&mut self) {
        self.s.time += 1;
        for c in &mut self.s.clocks {
            c.1 += 1;
        }
        self.s.tick_world_blocks();
        self.s.flush_block_changes();
        self.ticks += 1;
        self.record();
        self.drain();
    }

    fn region_states(&self) -> Vec<(P, u32)> {
        let Some((a, b)) = self.region else { return Vec::new() };
        let mut v = Vec::new();
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    let p = [x, y, z];
                    v.push((p, self.s.get(p)));
                }
            }
        }
        v
    }

    fn record(&mut self) {
        let now = self.region_states();
        for (i, (p, st)) in now.iter().enumerate() {
            if self.last.get(i) != Some(st) {
                let text = state_string(&self.s, *st);
                self.out.push_str(&format!("{} {} {} {} {}\n", self.ticks, p[0], p[1], p[2], text));
            }
        }
        self.last = now.into_iter().map(|e| e.1).collect();
        if !self.watch.is_empty() {
            let counts: Vec<String> = self
                .watch
                .iter()
                .map(|&p| self.s.be_item_counts(p).iter().map(|e| e.1).sum::<i32>().to_string())
                .collect();
            self.out.push_str(&format!("# items {} {}\n", self.ticks, counts.join(" ")));
        }
    }

    /// Runs a script (one command per line; `#` comment).
    pub fn run(&mut self, script: &str) {
        for line in script.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let w: Vec<&str> = line.split_whitespace().collect();
            let n = |i: usize| -> i32 { w[i].parse().unwrap_or_else(|_| panic!("number expected: {line}")) };
            match w[0] {
                "region" => {
                    let (a, b) = ([n(1), n(2), n(3)], [n(4), n(5), n(6)]);
                    self.load_box(a, b);
                    self.region = Some((a, b));
                    self.last.clear();
                    self.record();
                }
                "setblock" => {
                    let p = [n(1), n(2), n(3)];
                    self.load_box(p, p);
                    if let Some(old) = self.setblock(p, w[4]) {
                        self.after_set(p, old);
                    }
                }
                "fill" => {
                    let (a, b) = ([n(1), n(2), n(3)], [n(4), n(5), n(6)]);
                    self.load_box(a, b);
                    let mut done = Vec::new();
                    for z in a[2].min(b[2])..=a[2].max(b[2]) {
                        for y in a[1].min(b[1])..=a[1].max(b[1]) {
                            for x in a[0].min(b[0])..=a[0].max(b[0]) {
                                if let Some(old) = self.setblock([x, y, z], w[7]) {
                                    done.push(([x, y, z], old));
                                }
                            }
                        }
                    }
                    for (p, old) in done {
                        self.after_set(p, old);
                    }
                }
                "use" => {
                    let p = [n(1), n(2), n(3)];
                    let st = self.s.get(p);
                    self.s.probe_use(p, st);
                }
                "tick" | "wait" => {
                    for _ in 0..n(1) {
                        self.tick();
                    }
                }
                "data" => {
                    // `data merge block x y z {Items:[{Slot:0b,id:"…",count:N},…]}` (items only).
                    let p = [n(3), n(4), n(5)];
                    let text = w[6..].join(" ");
                    let mut items = Vec::new();
                    for part in text.split("{Slot:").skip(1) {
                        let slot: i8 = part.split('b').next().and_then(|x| x.parse().ok()).unwrap_or(0);
                        let id = part.split("id:\"").nth(1).and_then(|x| x.split('"').next()).unwrap_or("");
                        let count: i32 = part
                            .split("count:")
                            .nth(1)
                            .and_then(|x| x.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok())
                            .unwrap_or(1);
                        items.push(Nbt::Compound(vec![
                            ("Slot".into(), Nbt::Byte(slot)),
                            ("id".into(), Nbt::String(id.to_owned())),
                            ("count".into(), Nbt::Int(count)),
                        ]));
                    }
                    if let Some(be) = self.s.be_get_mut(p) {
                        crate::nbtio::set(be, "Items", Nbt::List(items));
                    }
                    self.s.reload_live_be(p);
                    let st = self.s.get(p);
                    let b = self.s.v().block(st);
                    self.s.update_neighbour_for_output_signal(p, b);
                }
                "watchitems" => {
                    self.watch = (1..w.len()).step_by(3).map(|i| [n(i), n(i + 1), n(i + 2)]).collect();
                }
                "player" => {
                    let f = |i: usize| -> f64 { w[i].parse().unwrap_or(0.0) };
                    self.s.pos = [f(1), f(2), f(3)];
                    self.s.last_good = self.s.pos;
                    self.s.yaw = f(4) as f32;
                    self.s.pitch = f(5) as f32;
                }
                "save" => self.s.save(true),
                // Survival, item in hand: `hold <item> [damage]`.
                "hold" => {
                    self.s.game_mode = 0;
                    let components = w.get(2).map(|d| {
                        rc_proto::nbt::Nbt::Compound(vec![(
                            "minecraft:damage".into(),
                            rc_proto::nbt::Nbt::Int(d.parse().unwrap_or(0)),
                        )])
                    });
                    let slot = 36 + self.s.held as usize;
                    self.s.slots[slot] = Some(crate::player::Stack { id: w[1].to_owned(), count: 1, components });
                }
                // Break by the player (loot and survival wear included).
                "break" => {
                    self.s.try_break([n(1), n(2), n(3)]);
                    let held = self.s.slots[36 + self.s.held as usize].clone();
                    self.out.push_str(&format!("# tool {held:?}\n"));
                }
                "mark" => {
                    self.out.push_str(&format!("# {} {}\n", self.ticks, w[1..].join(" ")));
                }
                // Items on the ground (official only: singleplayer does not create them yet).
                "kill" => {}
                _ => panic!("unknown command: {line}"),
            }
        }
    }
}

impl Server {
    /// Right click without an item (lever, button…) for the probe.
    pub(crate) fn probe_use(&mut self, p: P, s: u32) {
        match self.v().kind(s) {
            super::Kind::Lever => self.lever_pull(p, s),
            super::Kind::Button { .. } => {
                self.button_press(p, s, Some(crate::mobs::mob::Ent::Player));
            }
            _ => {
                let _ = rel(p, Dir::Up);
            }
        }
    }
}
