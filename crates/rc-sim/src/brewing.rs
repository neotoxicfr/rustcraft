//! Brewing stands (S7, `mechanics-s7.md` § 2.2): block entity simulated like furnaces (loaded with
//! its chunk, ticked, rewritten before save), the pack's `brewing` recipes, `brewing_fuel`
//! fuel (uses and speed read from the pack's providers), `has_bottle_*` state
//! of the block.

use rc_proto::nbt::Nbt;

use crate::containers::{read_items, write_items};
use crate::menus::Kind;
use crate::nbtio::{float, int};
use crate::player::Stack;
use crate::server::Server;
use crate::stations::St;

#[derive(Debug, Clone, Default)]
pub struct Brewer {
    pub items: [Option<Stack>; 5],
    pub brew_time: i32,
    pub fuel: i32,
    pub total_brew: i32,
    pub total_fuel: i32,
    pub speed: f32,
    /// Reagent at the start of brewing.
    pub ingredient: Option<String>,
    bits: Option<[bool; 3]>,
    pub dirty: bool,
}

impl Brewer {
    pub fn from_nbt(be: &Nbt) -> Brewer {
        let v = read_items(be, 5);
        let mut items: [Option<Stack>; 5] = Default::default();
        for (k, s) in v.into_iter().enumerate().take(5) {
            items[k] = s;
        }
        let brew_time = int(be, "BrewTime").unwrap_or(0) as i32;
        let ingredient = if brew_time > 0 { items[3].as_ref().map(|s| s.id.clone()) } else { None };
        Brewer {
            brew_time,
            total_brew: int(be, "total_brew_time").unwrap_or(400) as i32,
            fuel: int(be, "Fuel").unwrap_or(0) as i32,
            total_fuel: int(be, "total_fuel").unwrap_or(20) as i32,
            speed: float(be, "speed_multiplier").unwrap_or(1.0),
            ingredient,
            items,
            bits: None,
            dirty: false,
        }
    }

    pub fn write(&self, be: &mut Nbt) {
        use crate::nbtio::set;
        set(be, "BrewTime", Nbt::Int(self.brew_time));
        set(be, "total_brew_time", Nbt::Int(self.total_brew));
        write_items(be, &self.items);
        set(be, "Fuel", Nbt::Int(self.fuel));
        set(be, "total_fuel", Nbt::Int(self.total_fuel));
        set(be, "speed_multiplier", Nbt::Float(self.speed));
    }

    /// Menu data: time left, fuel, total time, total fuel.
    pub fn data(&self) -> [i32; 4] {
        [self.brew_time, self.fuel, self.total_brew, self.total_fuel]
    }
}

impl Server {
    fn loot_values(
        &mut self,
        f: impl FnOnce(&crate::loot::Loot<'_>, &mut crate::rng::Rng) -> (i32, f32),
    ) -> (i32, f32) {
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, name: &str| {
            let id = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
        let mut rng = self.life.rng;
        let out = f(&loot, &mut rng);
        self.life.rng = rng;
        out
    }

    pub(crate) fn ensure_brewer(&mut self, pos: [i32; 3]) {
        if self.life.brewers.contains_key(&pos) {
            return;
        }
        let b = self.block_entity(pos).map_or_else(Brewer::default, Brewer::from_nbt);
        self.life.brewers.insert(pos, b);
    }

    /// Brewing stands of the chunks that have just been loaded.
    pub(crate) fn discover_brewers(&mut self, loaded: &[(i32, i32)]) {
        let mut found = Vec::new();
        for p in loaded {
            let Some(Some(c)) = self.chunks.get(p) else { continue };
            for be in &c.block_entities {
                if be.get("id").and_then(Nbt::as_str) != Some("minecraft:brewing_stand") {
                    continue;
                }
                let (Some(x), Some(y), Some(z)) = (int(be, "x"), int(be, "y"), int(be, "z")) else { continue };
                found.push([x as i32, y as i32, z as i32]);
            }
        }
        for p in found {
            self.ensure_brewer(p);
        }
    }

    pub(crate) fn brewers_tick(&mut self) {
        let positions: Vec<[i32; 3]> = self.life.brewers.keys().copied().collect();
        for p in positions {
            if self.block_name_s7(p).as_deref() != Some("minecraft:brewing_stand") {
                self.life.brewers.remove(&p);
                continue;
            }
            // Same test as the other block entities: border and chunk within tick range.
            if self.be_ticking(p) {
                self.brewer_tick(p);
            }
        }
        if let Some(m) = &mut self.life.menu
            && let (Kind::Station(St::Brewing), Some(p)) = (m.kind, m.block)
            && let Some(b) = self.life.brewers.get(&p)
        {
            // Contents re-read by `pull_menu`.
            m.data = b.data().to_vec();
        }
    }

    pub(crate) fn block_name_s7(&self, p: [i32; 3]) -> Option<String> {
        self.state_at(p).and_then(|s| self.data.block_of(s)).map(|b| b.name.clone())
    }

    fn brewer_tick(&mut self, p: [i32; 3]) {
        let Some(mut b) = self.life.brewers.remove(&p) else { return };
        // The "to save" marking of this tick (neighbouring comparators); `dirty` itself keeps the write due to the
        // block entity.
        let mut changed = false;
        let at = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        // Fuel.
        if b.fuel <= 0
            && let Some(fuel) = b.items[4].clone()
            && let Some(c) = self
                .life
                .items
                .default_component(&fuel, "minecraft:brewing_fuel")
                .or_else(|| crate::items::component(&fuel, "minecraft:brewing_fuel").map(nbt_to_json))
        {
            let (uses, speed) = self.loot_values(|loot, rng| {
                let ctx =
                    crate::loot::Ctx { block: Some(("minecraft:brewing_stand", Vec::new())), ..Default::default() };
                let u = if c["uses"].is_null() { 0 } else { loot.int(&c["uses"], &ctx, rng) };
                let s =
                    if c["speed_multiplier"].is_null() { 1.0 } else { loot.float(&c["speed_multiplier"], &ctx, rng) };
                (u, s)
            });
            b.fuel = uses;
            b.total_fuel = uses;
            b.speed = speed;
            let rem = self.life.items.remainder(&fuel.id);
            let mut f = fuel;
            f.count -= 1;
            b.items[4] = if f.count > 0 {
                if let Some(r) = rem {
                    self.spawn_item_at(at, r);
                }
                Some(f)
            } else {
                rem
            };
            b.dirty = true;
            changed = true;
        }
        let brewable = self.brewable(&b);
        if b.brew_time > 0 {
            b.brew_time -= 1;
            let done = b.brew_time == 0;
            let same_ing = b.items[3].as_ref().map(|s| s.id.clone()) == b.ingredient;
            if done && brewable {
                self.do_brew(&mut b, p);
            } else if !brewable || !same_ing {
                b.brew_time = 0;
            }
            b.dirty = true;
            changed = true;
        } else if brewable && b.fuel > 0 {
            let speed = if b.speed > 0.0 { b.speed } else { 1.0 };
            b.fuel -= 1;
            b.brew_time = (400.0f32 / speed).ceil() as i32;
            b.total_brew = b.brew_time;
            b.ingredient = b.items[3].as_ref().map(|s| s.id.clone());
            b.dirty = true;
            changed = true;
        }
        let bits = [b.items[0].is_some(), b.items[1].is_some(), b.items[2].is_some()];
        if b.bits != Some(bits) {
            b.bits = Some(bits);
            if let Some(mut s) = self.state_at(p) {
                for (k, on) in bits.iter().enumerate() {
                    if let Some(n) = self.with_prop(s, &format!("has_bottle_{k}"), if *on { "true" } else { "false" }) {
                        s = n;
                    }
                }
                if Some(s) != self.state_at(p) {
                    self.set_block(p, s, 2);
                }
            }
        }
        self.life.brewers.insert(p, b);
        if changed {
            self.container_changed(p);
        }
    }

    fn brewable(&self, b: &Brewer) -> bool {
        let Some(ing) = &b.items[3] else { return false };
        let d = &self.life.stations;
        let items = &self.life.items;
        if !d.is_reagent(items, ing) {
            return false;
        }
        b.items[..3].iter().flatten().any(|bottle| d.brew(items, bottle, ing).is_some())
    }

    fn do_brew(&mut self, b: &mut Brewer, p: [i32; 3]) {
        let Some(ing) = b.items[3].clone() else { return };
        for k in 0..3 {
            if let Some(bottle) = &b.items[k]
                && let Some(r) = self.life.stations.brew(&self.life.items, bottle, &ing)
            {
                b.items[k] = Some(crate::stations::brew_output(r));
            }
        }
        let rem = self.life.items.remainder(&ing.id);
        let mut i = ing;
        i.count -= 1;
        b.items[3] = if i.count > 0 {
            if let Some(r) = rem {
                self.spawn_item_at([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])], r);
            }
            Some(i)
        } else {
            rem
        };
        self.level_event(1035, p, 0);
    }

    /// Modified brewing stands rewritten into their block entity.
    pub(crate) fn flush_brewers(&mut self) {
        let dirty: Vec<([i32; 3], Brewer)> = self
            .life
            .brewers
            .iter_mut()
            .filter(|(_, b)| b.dirty)
            .map(|(p, b)| {
                b.dirty = false;
                (*p, b.clone())
            })
            .collect();
        for (p, b) in dirty {
            if let Some(be) = self.block_entity_mut(p, "minecraft:brewing_stand") {
                b.write(be);
            }
        }
    }

    /// Block broken: contents thrown out.
    pub(crate) fn brewer_broken(&mut self, p: [i32; 3]) -> bool {
        let Some(b) = self.life.brewers.remove(&p) else { return false };
        let at = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        for s in b.items.into_iter().flatten() {
            self.drop_contents(at, s);
        }
        true
    }

    /// Contents of the open menu copied into the brewing stand.
    pub(crate) fn brewer_sync_back(&mut self, p: [i32; 3], own: &[Option<Stack>]) {
        if let Some(b) = self.life.brewers.get_mut(&p) {
            for (k, s) in own.iter().enumerate().take(5) {
                b.items[k] = s.clone();
            }
            b.dirty = true;
        }
    }
}

/// NBT → JSON (component placed on the stack, rare).
fn nbt_to_json(n: &Nbt) -> serde_json::Value {
    use serde_json::Value;
    match n {
        Nbt::String(s) => Value::String(s.clone()),
        Nbt::Compound(f) => Value::Object(f.iter().map(|(k, v)| (k.clone(), nbt_to_json(v))).collect()),
        Nbt::List(l) => Value::Array(l.iter().map(nbt_to_json).collect()),
        Nbt::Float(f) => serde_json::json!(*f),
        Nbt::Double(d) => serde_json::json!(*d),
        o => o.as_i64().map_or(Value::Null, |i| serde_json::json!(i)),
    }
}
