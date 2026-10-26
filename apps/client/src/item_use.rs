//! Interaction components of a stack (`docs/specs/interactions.md`, `item-components.md`):
//! network patch first (decoded here), then the item's default components (`rc-data`).

use hashbrown::HashMap;
use std::borrow::Cow;

use rc_data::{BlockSet, Consumable, ItemInfo, Tool, ToolRule, VersionData};
use rc_proto::Reader;
use rc_proto::varint::read_varint;
use rc_proto_v777::item::ItemStack;

/// Network ids of the components read for the interactions.
#[derive(Default, Clone)]
pub struct UseIds {
    tool: Option<u16>,
    consumable: Option<u16>,
    food: Option<u16>,
    attack_range: Option<u16>,
    min_charge: Option<u16>,
    piercing: Option<u16>,
    kinetic: Option<u16>,
    blocks_attacks: Option<u16>,
    attack_anim: Option<u16>,
    interact_anim: Option<u16>,
    can_break: Option<u16>,
    can_place_on: Option<u16>,
    use_effects: Option<u16>,
    dyed_color: Option<u16>,
    potion_contents: Option<u16>,
    banner_patterns: Option<u16>,
    compostable: Option<u16>,
    /// Names of the `minecraft:sound_event` registry (sounds referenced by id).
    sounds: Vec<String>,
}

impl UseIds {
    pub fn new(data: &VersionData) -> Self {
        let names = data.registry("minecraft:data_component_type").map(|r| r.entries.as_slice()).unwrap_or(&[]);
        let id =
            |n: &str| names.iter().position(|e| e == &format!("minecraft:{n}")).and_then(|i| u16::try_from(i).ok());
        UseIds {
            tool: id("tool"),
            consumable: id("consumable"),
            food: id("food"),
            attack_range: id("attack_range"),
            min_charge: id("minimum_attack_charge"),
            piercing: id("piercing_weapon"),
            kinetic: id("kinetic_weapon"),
            blocks_attacks: id("blocks_attacks"),
            attack_anim: id("attack_animation"),
            interact_anim: id("interact_animation"),
            can_break: id("can_break"),
            can_place_on: id("can_place_on"),
            use_effects: id("use_effects"),
            dyed_color: id("dyed_color"),
            potion_contents: id("potion_contents"),
            banner_patterns: id("banner_patterns"),
            compostable: id("compostable"),
            sounds: data.registry("minecraft:sound_event").map(|r| r.entries.clone()).unwrap_or_default(),
        }
    }
}

/// Synchronized block tags (`update_tags`): name → sorted block ids.
#[derive(Default)]
pub struct BlockTags(HashMap<String, Vec<u32>>);

impl BlockTags {
    pub fn set(&mut self, tags: &[(String, Vec<i32>)]) {
        self.0 = tags
            .iter()
            .map(|(n, ids)| {
                let mut v: Vec<u32> = ids.iter().filter_map(|&i| u32::try_from(i).ok()).collect();
                v.sort_unstable();
                (n.clone(), v)
            })
            .collect();
    }

    pub fn contains(&self, tag: &str, block: u32) -> bool {
        self.0.get(tag).is_some_and(|v| v.binary_search(&block).is_ok())
    }

    fn set_has(&self, set: &BlockSet, block: u32) -> bool {
        match set {
            BlockSet::Tag(t) => self.contains(t, block),
            BlockSet::Blocks(b) => b.contains(&block),
        }
    }
}

/// Component of a stack: removed, patched (network bytes) or default value of the item.
enum Comp<'a> {
    Removed,
    Patched(&'a [u8]),
    Default,
}

fn f32_be(r: &mut Reader<'_>) -> Option<f32> {
    Some(f32::from_be_bytes(r.take_array().ok()?))
}

fn boolean(r: &mut Reader<'_>) -> Option<bool> {
    Some(r.u8().ok()? != 0)
}

/// Set of network references: 0 → named tag, n > 0 → n − 1 ids.
fn holder_set(r: &mut Reader<'_>) -> Option<BlockSet> {
    let n = read_varint(r).ok()?;
    if n == 0 {
        let s = rc_proto::read_str(r, 32767).ok()?;
        return Some(BlockSet::Tag(if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") }));
    }
    (1..n).map(|_| read_varint(r).ok().and_then(|v| u32::try_from(v).ok())).collect::<Option<_>>().map(BlockSet::Blocks)
}

fn decode_tool(b: &[u8]) -> Option<Tool> {
    let r = &mut Reader::new(b);
    let n = read_varint(r).ok()?;
    let mut rules = Vec::new();
    for _ in 0..n.clamp(0, 256) {
        let blocks = holder_set(r)?;
        let speed = if boolean(r)? { Some(f32_be(r)?) } else { None };
        let correct = if boolean(r)? { Some(boolean(r)?) } else { None };
        rules.push(ToolRule { blocks, speed, correct });
    }
    Some(Tool {
        rules,
        default_speed: f32_be(r)?,
        damage_per_block: read_varint(r).ok()? as u32,
        creative_destroy: boolean(r)?,
    })
}

fn decode_consumable(b: &[u8], sounds: &[String]) -> Option<Consumable> {
    let r = &mut Reader::new(b);
    let seconds = f32_be(r)?;
    let animation = read_varint(r).ok()? as u8;
    let sound = match read_varint(r).ok()? {
        0 => {
            let s = rc_proto::read_str(r, 32767).ok()?.to_owned();
            if boolean(r)? {
                f32_be(r)?;
            }
            s
        }
        i => sounds.get(i as usize - 1).cloned().unwrap_or_default(),
    };
    Some(Consumable { seconds, animation, sound, particles: boolean(r)? })
}

/// Stack as seen by the interactions.
pub struct ItemView<'a> {
    pub stack: &'a ItemStack,
    pub info: Option<&'a ItemInfo>,
    pub ids: &'a UseIds,
}

impl<'a> ItemView<'a> {
    pub fn new(stack: &'a ItemStack, data: &'a VersionData, ids: &'a UseIds) -> Self {
        ItemView { stack, info: usize::try_from(stack.item).ok().and_then(|i| data.items.get(i)), ids }
    }

    fn comp(&self, id: Option<u16>) -> Comp<'a> {
        let Some(id) = id else { return Comp::Default };
        if self.stack.removed.contains(&id) {
            return Comp::Removed;
        }
        self.stack.component(id).map_or(Comp::Default, Comp::Patched)
    }

    fn has(&self, id: Option<u16>, default: bool) -> bool {
        match self.comp(id) {
            Comp::Removed => false,
            Comp::Patched(_) => true,
            Comp::Default => default,
        }
    }

    pub fn tool(&self) -> Option<Cow<'a, Tool>> {
        match self.comp(self.ids.tool) {
            Comp::Removed => None,
            Comp::Patched(b) => decode_tool(b).map(Cow::Owned),
            Comp::Default => self.info.and_then(|i| i.tool.as_ref()).map(Cow::Borrowed),
        }
    }

    pub fn consumable(&self) -> Option<Cow<'a, Consumable>> {
        match self.comp(self.ids.consumable) {
            Comp::Removed => None,
            Comp::Patched(b) => decode_consumable(b, &self.ids.sounds).map(Cow::Owned),
            Comp::Default => self.info.and_then(|i| i.consumable.as_ref()).map(Cow::Borrowed),
        }
    }

    /// `food` present: "can always be eaten".
    pub fn food(&self) -> Option<bool> {
        match self.comp(self.ids.food) {
            Comp::Removed => None,
            Comp::Patched(b) => {
                let r = &mut Reader::new(b);
                read_varint(r).ok()?;
                f32_be(r)?;
                boolean(r)
            }
            Comp::Default => self.info.and_then(|i| i.food),
        }
    }

    pub fn attack_range(&self) -> Option<[f32; 6]> {
        match self.comp(self.ids.attack_range) {
            Comp::Removed => None,
            Comp::Patched(b) => {
                let r = &mut Reader::new(b);
                Some([f32_be(r)?, f32_be(r)?, f32_be(r)?, f32_be(r)?, f32_be(r)?, f32_be(r)?])
            }
            Comp::Default => self.info.and_then(|i| i.attack_range),
        }
    }

    pub fn min_attack_charge(&self) -> f32 {
        match self.comp(self.ids.min_charge) {
            Comp::Removed => 0.0,
            Comp::Patched(b) => f32_be(&mut Reader::new(b)).unwrap_or(0.0),
            Comp::Default => self.info.map_or(0.0, |i| i.min_attack_charge),
        }
    }

    pub fn piercing(&self) -> bool {
        self.has(self.ids.piercing, self.info.is_some_and(|i| i.piercing))
    }

    pub fn kinetic(&self) -> bool {
        self.has(self.ids.kinetic, self.info.is_some_and(|i| i.kinetic))
    }

    /// Component present (patch, otherwise list of the item's default components).
    fn present(&self, id: Option<u16>) -> bool {
        self.has(id, id.is_some_and(|i| self.info.is_some_and(|inf| inf.components.contains(&i))))
    }

    /// `dyed_color` present (the water cauldron washes it).
    pub fn dyed(&self) -> bool {
        self.present(self.ids.dyed_color)
    }

    /// `compostable` present (the composter accepts it).
    pub fn compostable(&self) -> bool {
        self.present(self.ids.compostable)
    }

    /// `banner_patterns` with at least one layer (the water cauldron removes one).
    pub fn banner_has_layers(&self) -> bool {
        match self.comp(self.ids.banner_patterns) {
            Comp::Patched(b) => read_varint(&mut Reader::new(b)).is_ok_and(|n| n > 0),
            _ => false,
        }
    }

    /// Potion id of `potion_contents` (none if absent, empty or removed).
    pub fn potion_id(&self) -> Option<i32> {
        match self.comp(self.ids.potion_contents) {
            Comp::Patched(b) => {
                let r = &mut Reader::new(b);
                (r.u8().ok()? == 1).then(|| read_varint(r).ok()).flatten()
            }
            _ => None,
        }
    }

    pub fn blocks_attacks(&self) -> bool {
        self.has(self.ids.blocks_attacks, self.info.is_some_and(|i| i.blocks_attacks))
    }

    fn anim(&self, id: Option<u16>, default: Option<[u8; 2]>) -> [u8; 2] {
        match self.comp(id) {
            Comp::Removed => [1, 6],
            Comp::Patched(b) => {
                let r = &mut Reader::new(b);
                match (read_varint(r), read_varint(r)) {
                    (Ok(k), Ok(d)) => [k as u8, d.clamp(0, 255) as u8],
                    _ => [1, 6],
                }
            }
            Comp::Default => default.unwrap_or([1, 6]),
        }
    }

    /// Use effects: speed multiplier, sprint allowed.
    pub fn use_effects(&self) -> (f32, bool) {
        let default = self.info.map_or((0.2, false), |i| (i.use_speed, i.use_sprint));
        match self.comp(self.ids.use_effects) {
            Comp::Patched(b) => {
                let r = &mut Reader::new(b);
                match (boolean(r), boolean(r), f32_be(r)) {
                    (Some(sprint), Some(_), Some(speed)) => (speed, sprint),
                    _ => default,
                }
            }
            Comp::Removed => (0.2, false),
            Comp::Default => default,
        }
    }

    /// Attack animation: (kind, duration).
    pub fn attack_anim(&self) -> [u8; 2] {
        self.anim(self.ids.attack_anim, self.info.map(|i| i.attack_anim))
    }

    /// Interaction animation: (kind, duration).
    pub fn interact_anim(&self) -> [u8; 2] {
        self.anim(self.ids.interact_anim, self.info.map(|i| i.interact_anim))
    }

    /// Block predicates (`can_break` / `can_place_on`): at least one predicate accepts the block.
    /// Only the block sets and the state properties are tested; a predicate that requires
    /// block-entity data or components is considered verified (the server
    /// decides).
    fn predicate(&self, id: Option<u16>, data: &VersionData, tags: &BlockTags, state: u32) -> bool {
        let Comp::Patched(b) = self.comp(id) else { return false };
        let block = data.state(state).map_or(u32::MAX, |s| u32::from(s.block));
        let r = &mut Reader::new(b);
        let Ok(n) = read_varint(r) else { return false };
        for _ in 0..n.clamp(0, 64) {
            let Some(ok) = one_predicate(r, data, tags, state, block) else { return false };
            if ok {
                return true;
            }
        }
        false
    }

    pub fn can_break(&self, data: &VersionData, tags: &BlockTags, state: u32) -> bool {
        self.predicate(self.ids.can_break, data, tags, state)
    }

    pub fn can_place_on(&self, data: &VersionData, tags: &BlockTags, state: u32) -> bool {
        self.predicate(self.ids.can_place_on, data, tags, state)
    }
}

/// A block predicate; `None` if the bytes cannot be read (the rest is then abandoned).
fn one_predicate(r: &mut Reader<'_>, data: &VersionData, tags: &BlockTags, state: u32, block: u32) -> Option<bool> {
    let mut ok = true;
    if boolean(r)? {
        let set = holder_set(r)?;
        ok &= tags.set_has(&set, block);
    }
    if boolean(r)? {
        let props: Vec<(&str, &str)> = data.state_properties(state).collect();
        let n = read_varint(r).ok()?;
        for _ in 0..n.clamp(0, 64) {
            let name = rc_proto::read_str(r, 32767).ok()?;
            let value = props.iter().find(|p| p.0 == name).map(|p| p.1);
            if boolean(r)? {
                let exact = rc_proto::read_str(r, 32767).ok()?;
                ok &= value == Some(exact);
            } else {
                let min = if boolean(r)? { Some(rc_proto::read_str(r, 32767).ok()?) } else { None };
                let max = if boolean(r)? { Some(rc_proto::read_str(r, 32767).ok()?) } else { None };
                let cmp = |a: &str, b: &str| match (a.parse::<i64>(), b.parse::<i64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    _ => a.cmp(b),
                };
                ok &= value
                    .is_some_and(|v| min.is_none_or(|m| cmp(v, m).is_ge()) && max.is_none_or(|m| cmp(v, m).is_le()));
            }
        }
    }
    // Block-entity data, exact and partial components: not evaluated locally. If there are any,
    // the rest of the bytes is not read and this predicate decides alone (verified if the start is).
    let nbt = boolean(r)?;
    if nbt || read_varint(r).ok()? != 0 || read_varint(r).ok()? != 0 {
        r.take_rest();
        return Some(ok);
    }
    Some(ok)
}

/// Tool speed and "correct" for a block (`interactions.md` § 1.2).
pub fn tool_speed(tool: Option<&Tool>, tags: &BlockTags, block: u32) -> (f32, bool) {
    let Some(t) = tool else { return (1.0, false) };
    let speed = t
        .rules
        .iter()
        .find(|r| r.speed.is_some() && tags.set_has(&r.blocks, block))
        .and_then(|r| r.speed)
        .unwrap_or(t.default_speed);
    let correct = t
        .rules
        .iter()
        .find(|r| r.correct.is_some() && tags.set_has(&r.blocks, block))
        .and_then(|r| r.correct)
        .unwrap_or(false);
    (speed, correct)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_tool_decodes() {
        // One rule: tag "mineable/pickaxe", speed 8, correct; default 1, 1 damage, creative.
        let mut b = vec![1, 0];
        let tag = b"minecraft:mineable/pickaxe";
        b.push(tag.len() as u8);
        b.extend_from_slice(tag);
        b.push(1);
        b.extend_from_slice(&8f32.to_be_bytes());
        b.extend_from_slice(&[1, 1]);
        b.extend_from_slice(&1f32.to_be_bytes());
        b.extend_from_slice(&[1, 1]);
        let t = decode_tool(&b).unwrap();
        assert_eq!(t.rules.len(), 1);
        assert_eq!(t.rules[0].speed, Some(8.0));
        assert_eq!(t.rules[0].correct, Some(true));
        let mut tags = BlockTags::default();
        tags.set(&[("minecraft:mineable/pickaxe".into(), vec![1, 5])]);
        assert_eq!(tool_speed(Some(&t), &tags, 1), (8.0, true));
        assert_eq!(tool_speed(Some(&t), &tags, 2), (1.0, false));
        assert_eq!(tool_speed(None, &tags, 1), (1.0, false));
    }
}
