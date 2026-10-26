//! Saving minecarts in the game's format (the write without identifier then own fields, in the
//! game's write order); reading (`load`), adoption of minecarts read with a chunk, summoning.

use std::collections::BTreeMap;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::{CKind, Cart};
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind;
use crate::nbtio::{int, list, set, uuid_nbt};
use crate::player::Stack;
use crate::server::Server;

/// Keys read in the structure (never kept in the raw compound).
const SIMULATED: [&str; 26] = [
    "id",
    "Pos",
    "Motion",
    "Rotation",
    "fall_distance",
    "Fire",
    "OnGround",
    "Invulnerable",
    "UUID",
    "Silent",
    "NoGravity",
    "Passengers",
    "DisplayState",
    "DisplayOffset",
    "FlippedRotation",
    "HasTicked",
    "Items",
    "LootTable",
    "LootTableSeed",
    "PushX",
    "PushZ",
    "Fuel",
    "fuse",
    "explosion_power",
    "explosion_speed_factor",
    "Enabled",
];

/// Common entity fields kept as is, in the order the game writes them.
const COMMON: [&str; 7] = ["CustomName", "CustomNameVisible", "Glowing", "TicksFrozen", "HasVisualFire", "Tags", "data"];
/// Fields written before the UUID (`Air`, `PortalCooldown`, `invulnerable_time`).
const BEFORE_UUID: [&str; 2] = ["PortalCooldown", "invulnerable_time"];

fn dbl(v: [f64; 3]) -> Nbt {
    Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect())
}

fn byte(b: bool) -> Nbt {
    Nbt::Byte(i8::from(b))
}

fn flag(e: &Nbt, k: &str) -> bool {
    int(e, k).is_some_and(|v| v != 0)
}

fn double(e: &Nbt, k: &str) -> Option<f64> {
    match e.get(k)? {
        Nbt::Double(d) => Some(*d),
        Nbt::Float(f) => Some(f64::from(*f)),
        n => n.as_i64().map(|v| v as f64),
    }
}

impl Server {
    /// NBT of a minecart (the write without identifier + `id`), passengers included.
    pub(crate) fn cart_to_nbt(&self, c: &Cart) -> Nbt {
        let kept = match &c.nbt {
            Nbt::Compound(f) => f.as_slice(),
            _ => &[],
        };
        let get = |k: &str| kept.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let mut f: Vec<(String, Nbt)> = vec![
            ("id".into(), Nbt::String(c.kind.name().into())),
            ("Pos".into(), dbl(c.pos)),
            ("Motion".into(), dbl(c.vel)),
            ("Rotation".into(), Nbt::List(vec![Nbt::Float(c.yaw), Nbt::Float(c.pitch)])),
            ("fall_distance".into(), Nbt::Double(c.fall_distance)),
            ("Fire".into(), Nbt::Short(c.fire.clamp(-32768, 32767) as i16)),
            ("Air".into(), get("Air").unwrap_or(Nbt::Short(300))),
            ("OnGround".into(), byte(c.on_ground)),
            ("Invulnerable".into(), byte(c.invulnerable)),
            ("PortalCooldown".into(), Nbt::Int(self.dims.ents.get(&c.id).map_or(0, |p| p.cooldown))),
        ];
        if let Some(v) = get("invulnerable_time") {
            f.push(("invulnerable_time".into(), v));
        }
        f.push(("UUID".into(), uuid_nbt(c.uuid)));
        for k in COMMON {
            if let Some(v) = get(k) {
                f.push((k.into(), v));
            }
            // Silent and no-gravity are written after the name (the game's order).
            if k == "CustomNameVisible" {
                if c.silent {
                    f.push(("Silent".into(), byte(true)));
                }
                if c.no_gravity {
                    f.push(("NoGravity".into(), byte(true)));
                }
            }
        }
        // The write of the minecart's own fields.
        if let Some(s) = c.display {
            f.push(("DisplayState".into(), self.states.palette_entry(&self.data, s)));
        }
        if c.display_offset != c.kind.default_offset() {
            f.push(("DisplayOffset".into(), Nbt::Int(c.display_offset)));
        }
        f.push(("FlippedRotation".into(), byte(c.flipped)));
        f.push(("HasTicked".into(), byte(c.first_tick)));
        match c.kind {
            CKind::Chest | CKind::Hopper => {
                match &c.loot {
                    Some(t) => {
                        f.push(("LootTable".into(), Nbt::String(t.clone())));
                        if c.loot_seed != 0 {
                            f.push(("LootTableSeed".into(), Nbt::Long(c.loot_seed)));
                        }
                    }
                    None => {
                        let items = c
                            .items
                            .iter()
                            .enumerate()
                            .filter_map(|(i, s)| s.as_ref().map(|s| s.to_nbt(Some(i as i8))))
                            .collect();
                        f.push(("Items".into(), Nbt::List(items)));
                    }
                }
                if c.kind == CKind::Hopper {
                    f.push(("Enabled".into(), byte(c.enabled)));
                }
            }
            CKind::Furnace => {
                f.push(("PushX".into(), Nbt::Double(c.push[0])));
                f.push(("PushZ".into(), Nbt::Double(c.push[2])));
                f.push(("Fuel".into(), Nbt::Short(c.fuel as i16)));
            }
            CKind::Tnt => {
                f.push(("fuse".into(), Nbt::Int(c.fuse)));
                if c.power_base != 4.0 {
                    f.push(("explosion_power".into(), Nbt::Float(c.power_base)));
                }
                if c.speed_factor != 1.0 {
                    f.push(("explosion_speed_factor".into(), Nbt::Float(c.speed_factor)));
                }
            }
            CKind::Rideable | CKind::Spawner | CKind::Command => {}
        }
        // Other fields kept (generator, command block, team…), in the order read.
        for (k, v) in kept {
            let known = k == "Air" || COMMON.contains(&k.as_str()) || BEFORE_UUID.contains(&k.as_str());
            if !known {
                f.push((k.clone(), v.clone()));
            }
        }
        let mut passengers: Vec<Nbt> = Vec::new();
        for p in &c.passengers {
            if let Ent::Mob(pid) = p
                && let Some(o) = self.mob_ref(*pid)
                && !o.removed
                && !o.dead
            {
                passengers.push(self.save_with_passengers(o));
            }
        }
        passengers.extend(c.foreign.iter().cloned());
        if !passengers.is_empty() {
            f.push(("Passengers".into(), Nbt::List(passengers)));
        }
        Nbt::Compound(f)
    }

    /// Minecart re-read (`load`) with a new network identifier; simulated passengers adopted.
    pub(crate) fn cart_from_nbt(&mut self, e: &Nbt) -> Option<Cart> {
        let kind = CKind::from_name(e.get("id").and_then(Nbt::as_str)?)?;
        self.next_entity += 1;
        let id = self.next_entity;
        let pos = crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let pos = [pos[0].clamp(-3.000_051_2e7, 3.000_051_2e7), pos[1].clamp(-2.0e7, 2.0e7), pos[2].clamp(-3.000_051_2e7, 3.000_051_2e7)];
        let mut c = Cart::new(kind, id, pos);
        c.load(self, e);
        // Portal return delay in progress.
        if let Some(cd) = int(e, "PortalCooldown").filter(|&v| v > 0) {
            self.dims.ents.insert(id, crate::dims::EntPortal { cooldown: cd as i32, proc: None });
        }
        // Passengers: simulated mobs adopted, the others kept as NBT.
        for p in list(e.get("Passengers")) {
            if p.get("id").and_then(Nbt::as_str).and_then(Kind::from_name).is_some() {
                self.next_entity += 1;
                let pid = self.next_entity;
                if let Some(mid) = self.adopt_with_passengers(pid, p, None)
                    && let Some(i) = self.mob_index(mid)
                    && let Some(m) = self.mobs.list[i].as_mut()
                {
                    m.mount.vehicle = Some(c.id);
                    c.passengers.push(Ent::Mob(mid));
                }
            } else {
                c.foreign.push(p.clone());
            }
        }
        Some(c)
    }

    /// Minecarts read with a chunk: made live.
    pub(crate) fn adopt_carts(&mut self, loaded: &[(i32, i32)]) {
        for ch in loaded {
            if !self.carts.adopted.insert(*ch) {
                continue;
            }
            let Some(l) = self.entities.get_mut(ch) else { continue };
            type Ents = Vec<(i32, Nbt)>;
            let (mine, rest): (Ents, Ents) = std::mem::take(l)
                .into_iter()
                .partition(|(_, e)| e.get("id").and_then(Nbt::as_str).and_then(CKind::from_name).is_some());
            *l = rest;
            if mine.is_empty() {
                continue;
            }
            self.life.drop_chunks.insert(*ch);
            for (_, e) in mine {
                if let Some(c) = self.cart_from_nbt(&e) {
                    // UUID already present (minecart returned with the player): duplicate ignored.
                    if self.carts.list.iter().flatten().any(|o| o.uuid == c.uuid) {
                        continue;
                    }
                    self.cart_add(c);
                }
            }
        }
    }

    /// Minecarts to write into the entity chunks (the player's leaves with them on
    /// closing).
    pub(crate) fn carts_into_chunks(&self, by_chunk: &mut BTreeMap<(i32, i32), Vec<Nbt>>) {
        for c in self.carts.list.iter().flatten().filter(|c| !c.removed) {
            if self.pcart.unloading && self.pcart.vehicle == Some(c.id) {
                continue;
            }
            let ch = ((c.pos[0].floor() as i32) >> 4, (c.pos[2].floor() as i32) >> 4);
            by_chunk.entry(ch).or_default().push(self.cart_to_nbt(c));
        }
    }

    /// `summon` of a minecart: compound read as on load, position imposed.
    pub(crate) fn cart_summon(&mut self, ty: &str, pos: [f64; 3], nbt: Option<&Nbt>) -> Option<i32> {
        CKind::from_name(ty)?;
        let mut full = nbt.cloned().unwrap_or(Nbt::Compound(Vec::new()));
        set(&mut full, "id", Nbt::String(ty.to_owned()));
        set(&mut full, "Pos", dbl(pos));
        // The game re-reads the compound (`load`): without `HasTicked`, the "first tick" flag is false, as for
        // a re-read minecart; without a UUID, the one drawn at construction stays.
        let c = self.cart_from_nbt(&full)?;
        let id = c.id;
        self.cart_add(c);
        Some(id)
    }
}

impl Cart {
    /// `load`: common fields, then the reading of the kind's own fields.
    pub(crate) fn load(&mut self, s: &Server, e: &Nbt) {
        if let Some(p) = crate::nbtio::f64s::<3>(e.get("Pos")) {
            self.pos = [p[0].clamp(-3.000_051_2e7, 3.000_051_2e7), p[1].clamp(-2.0e7, 2.0e7), p[2].clamp(-3.000_051_2e7, 3.000_051_2e7)];
            self.old_pos = self.pos;
        }
        let m = crate::nbtio::motion(e.get("Motion")).unwrap_or_default();
        self.vel = m.map(|v| if v.abs() > 10.0 { 0.0 } else { v });
        if let Some(Nbt::List(r)) = e.get("Rotation")
            && let [Nbt::Float(y), Nbt::Float(x)] = r.as_slice()
        {
            self.yaw = y % 360.0;
            self.pitch = (x % 360.0).clamp(-90.0, 90.0);
            self.yaw_o = self.yaw;
        }
        self.needs_sync = true;
        self.fall_distance = double(e, "fall_distance").unwrap_or(0.0);
        self.fire = int(e, "Fire").unwrap_or(0) as i16 as i32;
        self.on_ground = flag(e, "OnGround");
        self.invulnerable = flag(e, "Invulnerable");
        if let Some(u) = crate::nbtio::uuid(e.get("UUID")) {
            self.uuid = u;
        }
        self.silent = flag(e, "Silent");
        self.no_gravity = flag(e, "NoGravity");
        // The reading of the minecart's own fields.
        self.display = e.get("DisplayState").and_then(|d| s.states.state_of(&s.data, d));
        self.display_offset = int(e, "DisplayOffset").map_or_else(|| self.kind.default_offset(), |v| v as i32);
        self.flipped = flag(e, "FlippedRotation");
        self.first_tick = flag(e, "HasTicked");
        match self.kind {
            CKind::Chest | CKind::Hopper => {
                self.items = vec![None; self.kind.container_size()];
                self.loot = e.get("LootTable").and_then(Nbt::as_str).map(str::to_owned);
                self.loot_seed = e.get("LootTableSeed").and_then(Nbt::as_i64).unwrap_or(0);
                if self.loot.is_none() {
                    for it in list(e.get("Items")) {
                        if let (Some(slot), Some(st)) = (int(it, "Slot"), Stack::from_nbt(it))
                            && (0..self.items.len() as i64).contains(&(slot & 255))
                        {
                            self.items[(slot & 255) as usize] = Some(st);
                        }
                    }
                }
                if self.kind == CKind::Hopper {
                    self.enabled = int(e, "Enabled").is_none_or(|v| v != 0);
                }
            }
            CKind::Furnace => {
                self.push = [double(e, "PushX").unwrap_or(0.0), 0.0, double(e, "PushZ").unwrap_or(0.0)];
                // The "lit" datum is not read: the first tick sets it.
                self.fuel = int(e, "Fuel").map_or(0, |v| v as i16 as i32);
            }
            CKind::Tnt => {
                self.fuse = int(e, "fuse").map_or(-1, |v| v as i32);
                self.power_base = crate::nbtio::float(e, "explosion_power").unwrap_or(4.0).clamp(0.0, 128.0);
                self.speed_factor = crate::nbtio::float(e, "explosion_speed_factor").unwrap_or(1.0).clamp(0.0, 128.0);
            }
            CKind::Command => {
                let cmd = e.get("Command").and_then(Nbt::as_str).unwrap_or("").to_owned();
                self.set_meta(13, Meta::String(cmd));
                let out = e.get("LastOutput").cloned().unwrap_or(Nbt::String(String::new()));
                self.set_meta(14, Meta::Text(Some(out)));
            }
            CKind::Rideable | CKind::Spawner => {}
        }
        // Raw compound: everything except the simulated keys.
        let kept: Vec<(String, Nbt)> = match e {
            Nbt::Compound(f) => f.iter().filter(|(k, _)| !SIMULATED.contains(&k.as_str())).cloned().collect(),
            _ => Vec::new(),
        };
        self.nbt = Nbt::Compound(kept);
        self.sync_hurt();
        self.sync_display();
    }
}
