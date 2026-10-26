//! Reading the components of a network stack (patch + default components of the item) for
//! model evaluation and slot decorations (`docs/specs/items.md`).

use rc_data::{ItemInfo, VersionData};
use rc_proto::Reader;
use rc_proto::varint::read_varint;
use rc_proto_v777::item::{ItemCodec, ItemStack};

/// Network ids of the components read by the client.
#[derive(Default)]
pub struct ComponentIds {
    names: Vec<String>,
    damage: Option<u16>,
    max_damage: Option<u16>,
    max_stack_size: Option<u16>,
    dyed_color: Option<u16>,
    enchantments: Option<u16>,
    glint: Option<u16>,
    item_model: Option<u16>,
    potion_contents: Option<u16>,
    /// Properties imposed on the state of the placed block (also: swallowed block of the sulfur cube).
    pub block_state: Option<u16>,
    custom_model_data: Option<u16>,
    firework_explosion: Option<u16>,
    lodestone: Option<u16>,
    charged: Option<u16>,
    custom_name: Option<u16>,
    item_name: Option<u16>,
    rarity: Option<u16>,
    written_book: Option<u16>,
    base_color: Option<u16>,
    profile: Option<u16>,
    /// Maps (`maps.md`): map id.
    pub map_id: Option<u16>,
    /// Bundles: contents, bees (weight of a hive), decoder of stack templates.
    pub bundle_contents: Option<u16>,
    pub bees: Option<u16>,
    /// Equipment (camera overlay of the worn stack).
    pub equippable: Option<u16>,
    pub codec: ItemCodec,
    /// Item id of the rocket (crossbow charge), read once.
    rocket: Option<i32>,
}

const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

impl ComponentIds {
    pub fn new(data: &VersionData) -> Self {
        let names = data.registry("minecraft:data_component_type").map(|r| r.entries.clone()).unwrap_or_default();
        let effects = data.registry("minecraft:consume_effect_type").map(|r| r.entries.clone()).unwrap_or_default();
        let codec = ItemCodec::new(&names, &effects);
        let id = |n: &str| rc_data::ids::with_key(&["minecraft:", n], |k| codec.component_id(k));
        ComponentIds {
            damage: id("damage"),
            max_damage: id("max_damage"),
            max_stack_size: id("max_stack_size"),
            dyed_color: id("dyed_color"),
            enchantments: id("enchantments"),
            glint: id("enchantment_glint_override"),
            item_model: id("item_model"),
            potion_contents: id("potion_contents"),
            block_state: id("block_state"),
            custom_model_data: id("custom_model_data"),
            firework_explosion: id("firework_explosion"),
            lodestone: id("lodestone_tracker"),
            charged: id("charged_projectiles"),
            custom_name: id("custom_name"),
            item_name: id("item_name"),
            rarity: id("rarity"),
            written_book: id("written_book_content"),
            base_color: id("base_color"),
            profile: id("profile"),
            map_id: id("map_id"),
            bundle_contents: id("bundle_contents"),
            bees: id("bees"),
            equippable: id("equippable"),
            names,
            codec,
            rocket: data
                .registry("minecraft:item")
                .and_then(|r| r.entries.iter().position(|e| e == "minecraft:firework_rocket"))
                .map(|i| i as i32),
        }
    }

    pub(crate) fn of(&self, name: &str) -> Option<u16> {
        let n = name.strip_prefix("minecraft:").unwrap_or(name);
        self.names.iter().position(|e| e.strip_prefix("minecraft:").unwrap_or(e) == n).map(|i| i as u16)
    }

    /// Custom name of a stack (`custom_name` component, text NBT), if there is one.
    pub fn custom_name_nbt(&self, stack: &ItemStack) -> Option<rc_proto::nbt::Nbt> {
        let b = self.custom_name.and_then(|i| stack.component(i))?;
        rc_proto::nbt::read(&mut Reader::new(b)).ok().flatten()
    }
}

/// Stack seen by the evaluation: network patch, then item defaults.
pub struct StackRef<'a> {
    pub stack: &'a ItemStack,
    pub info: Option<&'a ItemInfo>,
    pub ids: &'a ComponentIds,
    pub data: &'a VersionData,
    pub hold: Hold,
}

/// Place of the stack at its holder: the game recognises the held, worn or stored stack by its
/// identity; here, the caller says where it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Hold {
    /// Main hand (chosen slot of the hotbar).
    pub main: bool,
    /// Off hand.
    pub off: bool,
    /// Cursor stack of the open menu (local player).
    pub carried: bool,
    /// Held by the hand that holds the line of an active fishing bobber.
    pub cast: bool,
    /// Slot of the open menu (chosen item of a bundle).
    pub menu_slot: Option<u16>,
}

impl Hold {
    pub const NONE: Hold = Hold { main: false, off: false, carried: false, cast: false, menu_slot: None };
    pub const MAIN: Hold = Hold { main: true, ..Hold::NONE };
    pub const CARRIED: Hold = Hold { carried: true, ..Hold::NONE };

    /// Stack of a hand of the local player (`main`: main), fishing line included.
    pub fn local_hand(main: bool) -> Hold {
        Hold { main, off: !main, cast: local_cast(main), ..Hold::NONE }
    }

    /// Stack held by an entity seen in the world (`slot` 0: main hand). Only the local player
    /// has a chosen slot and a use in progress published: the other holders keep only
    /// the fishing line, otherwise the local player's use would show on their items.
    pub fn worn(slot: usize, local: bool, cast: bool) -> Hold {
        Hold { main: local && slot == 0, off: local && slot == 1, cast, ..Hold::NONE }
    }
}

/// Hands of the local player published per frame: chosen hotbar slot, hand of the fishing line
/// (0 none, 1 main, 2 off).
static LOCAL_HANDS: [std::sync::atomic::AtomicU8; 2] =
    [std::sync::atomic::AtomicU8::new(0), std::sync::atomic::AtomicU8::new(0)];

/// Publishes the chosen slot and the hand that holds the fishing line (`None`: no active bobber):
/// the main one if it carries a fishing rod, otherwise the other.
pub fn set_local_hands(selected: usize, fishing: Option<bool>) {
    use std::sync::atomic::Ordering::Relaxed;
    LOCAL_HANDS[0].store(selected.min(8) as u8, Relaxed);
    LOCAL_HANDS[1].store(fishing.map_or(0, |main| if main { 1 } else { 2 }), Relaxed);
}

/// The hand (main or not) of the local player holds an active fishing line.
pub fn local_cast(main: bool) -> bool {
    LOCAL_HANDS[1].load(std::sync::atomic::Ordering::Relaxed) == if main { 1 } else { 2 }
}

/// Place of a slot of the open menu (`slots` of `screens::current`): chosen hotbar, off
/// hand (inventory only) and slot.
pub fn menu_hold(inv: &crate::inventory::PlayerInventory, slot: usize) -> Hold {
    let sel = usize::from(LOCAL_HANDS[0].load(std::sync::atomic::Ordering::Relaxed));
    // Index in menu 0 (hotbar 36..44, off hand 45).
    let menu = match &inv.container {
        None => Some(slot),
        Some(c) => slot.checked_sub(c.player_start()).filter(|&k| k < 36).map(|k| k + 9),
    };
    let main = menu == Some(36 + sel);
    let off = inv.container.is_none() && menu == Some(45);
    Hold { main, off, cast: (main || off) && local_cast(main), menu_slot: u16::try_from(slot).ok(), ..Hold::NONE }
}

/// Effects of a network list of effect instances: (effect, amplifier, visible).
fn read_effects(r: &mut Reader<'_>) -> Option<Vec<(usize, i32, bool)>> {
    fn details(r: &mut Reader<'_>, depth: u32) -> Option<(i32, bool)> {
        let amp = read_varint(r).ok()?;
        read_varint(r).ok()?;
        let flags = r.take(3).ok()?;
        if depth < 16 && r.u8().ok()? == 1 {
            details(r, depth + 1)?;
        }
        Some((amp, flags[1] != 0))
    }
    let n = read_varint(r).ok()?;
    (0..n)
        .map(|_| {
            let effect = read_varint(r).ok()? as usize;
            let (amp, visible) = details(r, 0)?;
            Some((effect, amp, visible))
        })
        .collect()
}

/// `equippable` component in network bytes: slot code (`ItemInfo::equip_slot`,
/// network id + 1) and camera overlay, without reading the rest of the component.
pub fn equippable_overlay(b: &[u8]) -> Option<(u8, Option<&str>)> {
    let mut r = Reader::new(b);
    // Id out of bounds: main hand, like the game.
    let slot = u8::try_from(read_varint(&mut r).ok()?).ok().filter(|&s| s < 8).unwrap_or(0) + 1;
    // Equip sound: 0 = direct sound (id, optional range), otherwise number + 1.
    if read_varint(&mut r).ok()? == 0 {
        rc_proto::read_str(&mut r, 32767).ok()?;
        if r.u8().ok()? != 0 {
            r.take(4).ok()?;
        }
    }
    // Worn appearance (optional), then overlay (optional).
    if r.u8().ok()? != 0 {
        rc_proto::read_str(&mut r, 32767).ok()?;
    }
    let overlay = if r.u8().ok()? != 0 { Some(rc_proto::read_str(&mut r, 32767).ok()?) } else { None };
    Some((slot, overlay))
}

fn varint(b: &[u8]) -> Option<i32> {
    read_varint(&mut Reader::new(b)).ok()
}

impl StackRef<'_> {
    fn bytes(&self, id: Option<u16>) -> Option<&[u8]> {
        id.and_then(|i| self.stack.component(i))
    }

    /// Bytes of a patch component, by name (`minecraft:lore`…).
    pub fn patch(&self, name: &str) -> Option<&[u8]> {
        self.bytes(self.ids.of(name))
    }

    /// List `which` of the `custom_model_data` component (0 floats, 1 booleans, 2 strings,
    /// 3 colours), each preceded by its count: reader placed on its first element.
    fn custom_model_data_list(&self, which: usize) -> Option<(Reader<'_>, usize)> {
        let mut r = Reader::new(self.bytes(self.ids.custom_model_data)?);
        for list in 0..=which {
            let n = usize::try_from(read_varint(&mut r).ok()?).ok()?;
            if list == which {
                return Some((r, n));
            }
            match list {
                0 | 1 => {
                    r.take(if list == 0 { n.checked_mul(4)? } else { n }).ok()?;
                }
                _ => {
                    for _ in 0..n {
                        rc_proto::read_str(&mut r, 32767).ok()?;
                    }
                }
            }
        }
        None
    }

    fn removed(&self, id: Option<u16>) -> bool {
        id.is_some_and(|i| self.stack.removed.contains(&i))
    }

    fn int(&self, id: Option<u16>, default: i32) -> i32 {
        if self.removed(id) {
            return 0;
        }
        self.bytes(id).and_then(varint).unwrap_or(default)
    }

    /// `item_model` id of the stack (patch, otherwise item default).
    /// Item model (`item_model` component, otherwise the type's), borrowed without copy.
    pub fn model(&self) -> Option<std::borrow::Cow<'_, str>> {
        if self.removed(self.ids.item_model) {
            return None;
        }
        match self.bytes(self.ids.item_model) {
            Some(b) => rc_proto::read_str(&mut Reader::new(b), 32767).ok().map(std::borrow::Cow::Borrowed),
            None => self.info.map(|i| std::borrow::Cow::Borrowed(i.model.as_str())).filter(|m| !m.is_empty()),
        }
    }

    fn text(&self, id: Option<u16>) -> Option<rc_text::Component> {
        let b = self.bytes(id)?;
        rc_proto::nbt::read(&mut Reader::new(b)).ok().flatten().as_ref().and_then(rc_text::from_nbt)
    }

    pub(crate) fn item_name(&self) -> &str {
        usize::try_from(self.stack.item)
            .ok()
            .and_then(|i| self.data.registry("minecraft:item")?.entries.get(i))
            .map_or("", String::as_str)
    }

    /// Full id of the item (`minecraft:stone`).
    pub(crate) fn item_id(&self) -> &str {
        self.item_name()
    }

    /// Effective rarity (`items.md` part 3 § 2.4): 0 common … 3 epic.
    fn rarity(&self) -> u8 {
        let base =
            self.bytes(self.ids.rarity).and_then(varint).map_or_else(|| self.info.map_or(0, |i| i.rarity), |r| r as u8);
        let enchanted = self.bytes(self.ids.enchantments).and_then(varint).is_some_and(|n| n > 0);
        if !enchanted {
            return base;
        }
        match base {
            0 | 1 => 2,
            _ => 3,
        }
    }

    /// Displayed name (`items.md` part 3 § 3) with the rarity colour, in italic if it is
    /// custom.
    pub fn display_name(&self) -> rc_text::Component {
        use rc_text::{Arg, Component};
        let custom = self.text(self.ids.custom_name);
        let italic = custom.is_some();
        let key = self.info.map_or("", |i| i.name_key.as_str());
        let name = custom
            .or_else(|| {
                // Raw title of a written book.
                let b = self.bytes(self.ids.written_book)?;
                let t = rc_proto::read_str(&mut Reader::new(b), 32).ok()?;
                (!t.trim().is_empty()).then(|| Component::text(t))
            })
            .or_else(|| match self.item_name() {
                "minecraft:compass" if self.bytes(self.ids.lodestone).is_some() => {
                    Some(Component::translate("item.minecraft.lodestone_compass", vec![]))
                }
                "minecraft:potion"
                | "minecraft:splash_potion"
                | "minecraft:lingering_potion"
                | "minecraft:tipped_arrow" => {
                    let suffix = self.potion_suffix().unwrap_or_else(|| "empty".into());
                    Some(Component::translate(format!("{key}.effect.{suffix}"), vec![]))
                }
                "minecraft:shield" => {
                    let c = self.bytes(self.ids.base_color).and_then(varint)?;
                    Some(Component::translate(format!("{key}.{}", DYES.get(c as usize)?), vec![]))
                }
                "minecraft:player_head" => {
                    let n = self.profile_name()?;
                    Some(Component::translate(format!("{key}.named"), vec![Arg::Text(n)]))
                }
                _ => None,
            })
            .or_else(|| self.text(self.ids.item_name))
            .unwrap_or_else(|| Component::translate(key, vec![]));
        let mut out = Component::text("");
        out.style.color = Some([0xff_ffff, 0xff_ff55, 0x55_ffff, 0xff_55ff][self.rarity().min(3) as usize]);
        if italic {
            out.style.italic = Some(true);
        }
        out.extra.push(name);
        out
    }

    /// Suffix of a potion's name: custom name of the contents, otherwise base potion without
    /// `long_` / `strong_`.
    fn potion_suffix(&self) -> Option<String> {
        let b = self.bytes(self.ids.potion_contents)?;
        let mut r = Reader::new(b);
        let potion = if r.u8().ok()? == 1 { Some(read_varint(&mut r).ok()? as usize) } else { None };
        if r.u8().ok()? == 1 {
            r.take(4).ok()?;
        }
        read_effects(&mut r)?;
        if r.u8().ok()? == 1 {
            return Some(rc_proto::read_str(&mut r, 32767).ok()?.to_owned());
        }
        let name = self.data.registry("minecraft:potion")?.entries.get(potion?)?;
        let base = name.strip_prefix("minecraft:").unwrap_or(name);
        Some(base.strip_prefix("long_").or_else(|| base.strip_prefix("strong_")).unwrap_or(base).to_owned())
    }

    fn profile_name(&self) -> Option<String> {
        let b = self.bytes(self.ids.profile)?;
        let mut r = Reader::new(b);
        if r.u8().ok()? == 1 {
            r.take(16).ok()?;
            return Some(rc_proto::read_str(&mut r, 16).ok()?.to_owned());
        }
        (r.u8().ok()? == 1).then(|| rc_proto::read_str(&mut r, 16).ok().map(str::to_owned)).flatten()
    }

    /// Durability bar (`hud.md` § 3.5): width and colour, if the stack is damaged.
    pub fn durability_bar(&self) -> Option<(i32, u32)> {
        let max = rc_assets::item::StackView::max_damage(self);
        let damage = rc_assets::item::StackView::damage(self);
        if max <= 0 || damage <= 0 {
            // Bundle: fill bar (`bundle.rs`).
            return crate::bundle::bar(&crate::bundle::Ctx { ids: self.ids, data: self.data }, self.stack);
        }
        let w = (13.0 - damage as f32 * 13.0 / max as f32).round().clamp(0.0, 13.0) as i32;
        let hue = ((max - damage) as f32 / max as f32).max(0.0) / 3.0;
        Some((w, hsv(hue, 1.0, 1.0)))
    }
}

/// HSV → RGB (like the hue-saturation-value to red-green-blue conversion).
fn hsv(h: f32, s: f32, v: f32) -> u32 {
    let i = ((h * 6.0) as i32).rem_euclid(6);
    let f = h * 6.0 - (h * 6.0).floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match i {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let c = |x: f32| (x * 255.0).clamp(0.0, 255.0) as u32;
    c(r) << 16 | c(g) << 8 | c(b)
}

/// Use in progress of the local player: (item, ticks elapsed, ticks remaining), `item` < 0 without use.
static USING: [std::sync::atomic::AtomicI32; 4] = [
    std::sync::atomic::AtomicI32::new(-1),
    std::sync::atomic::AtomicI32::new(0),
    std::sync::atomic::AtomicI32::new(0),
    std::sync::atomic::AtomicI32::new(0),
];

/// Publishes the use in progress (bow drawn, shield raised, food…) for the item models
/// that depend on it (`using_item`, `use_duration`, `use_cycle`, crossbow charge) and for the
/// hand pose: (item, ticks elapsed, ticks remaining, hand).
pub fn set_using(u: Option<(i32, i32, i32, usize)>) {
    use std::sync::atomic::Ordering::Relaxed;
    let (item, used, left, hand) = u.unwrap_or((-1, 0, 0, 0));
    USING[1].store(used, Relaxed);
    USING[2].store(left, Relaxed);
    USING[3].store(hand as i32, Relaxed);
    USING[0].store(item, Relaxed);
}

/// Use in progress: (item, ticks elapsed, ticks remaining, hand).
pub fn using_state() -> Option<(i32, i32, i32, usize)> {
    use std::sync::atomic::Ordering::Relaxed;
    let item = USING[0].load(Relaxed);
    (item >= 0).then(|| (item, USING[1].load(Relaxed), USING[2].load(Relaxed), USING[3].load(Relaxed) as usize))
}

impl rc_assets::item::StackView for StackRef<'_> {
    /// Stack of the local player's hand in use (same hand, same item).
    fn using(&self) -> Option<(i32, i32)> {
        use std::sync::atomic::Ordering::Relaxed;
        let hand = USING[3].load(Relaxed);
        let held = if hand == 0 { self.hold.main } else { self.hold.off };
        if !held || USING[0].load(Relaxed) != self.stack.item {
            return None;
        }
        Some((USING[1].load(Relaxed), USING[2].load(Relaxed)))
    }

    fn fishing_cast(&self) -> bool {
        self.hold.cast
    }

    fn carried(&self) -> bool {
        self.hold.carried
    }

    /// Item chosen with the wheel of the bundle of the hovered slot (`bundle.rs`).
    fn bundle_selected(&self, f: &mut dyn FnMut(&str, &str, &dyn rc_assets::item::StackView)) -> bool {
        let Some(slot) = self.hold.menu_slot else { return false };
        let Ok(k) = usize::try_from(crate::bundle::selected(usize::from(slot))) else { return false };
        let Some(items) = self.patch("minecraft:bundle_contents").and_then(|b| crate::bundle::decode(self.ids, b))
        else {
            return false;
        };
        let Some(inner) = items.get(k) else { return false };
        let info = usize::try_from(inner.item).ok().and_then(|i| self.data.items.get(i));
        let view = StackRef { stack: inner, info, ids: self.ids, data: self.data, hold: Hold::NONE };
        // Stack without an item model: nothing to draw, but an item is chosen.
        if let Some(model) = view.model() {
            f(view.item_name(), &model, &view);
        }
        true
    }

    fn special_data(&self, kind: &str) -> Vec<u32> {
        let name = self.item_name();
        let name = name.strip_prefix("minecraft:").unwrap_or(name);
        if kind == "minecraft:player_head" {
            return crate::item_special::head_data(self.patch("minecraft:profile"));
        }
        // Copper golem statue: pose (item state property) and oxidation (name).
        if kind == "minecraft:copper_golem_statue" {
            let pose = match self.block_state("copper_golem_pose").as_deref() {
                Some("running") => 1,
                Some("sitting") => 2,
                Some("star") => 3,
                _ => 0,
            };
            let base = name.strip_prefix("waxed_").unwrap_or(name);
            let ox = ["exposed_", "weathered_", "oxidized_"]
                .iter()
                .position(|p| base.starts_with(p))
                .map_or(0, |i| i as u32 + 1);
            return vec![pose, ox];
        }
        // Decorated pot: four faces (back, left, right, front), item id + 1.
        if kind == "minecraft:decorated_pot" {
            let Some(b) = self.patch("minecraft:pot_decorations") else { return Vec::new() };
            let mut r = Reader::new(b);
            return (0..4)
                .map(|_| match r.u8() {
                    Ok(1) => self.ids.codec.read_template(&mut r).map_or(0, |s| (s.item + 1) as u32),
                    _ => 0,
                })
                .collect();
        }
        crate::item_special::data(
            kind,
            name,
            self.patch("minecraft:banner_patterns"),
            self.patch("minecraft:base_color"),
        )
    }

    fn has(&self, component: &str) -> bool {
        let id = self.ids.of(component);
        if self.removed(id) {
            return false;
        }
        id.is_some_and(|i| self.stack.component(i).is_some())
            || match component.strip_prefix("minecraft:").unwrap_or(component) {
                "max_damage" | "damage" => self.info.is_some_and(|i| i.max_damage > 0),
                _ => false,
            }
    }

    fn patched(&self, component: &str) -> bool {
        self.ids.of(component).is_some_and(|i| self.stack.component(i).is_some())
    }

    fn damage(&self) -> i32 {
        self.int(self.ids.damage, 0)
    }

    fn max_damage(&self) -> i32 {
        self.int(self.ids.max_damage, self.info.map_or(0, |i| i.max_damage as i32))
    }

    fn count(&self) -> i32 {
        self.stack.count
    }

    fn max_stack_size(&self) -> i32 {
        self.int(self.ids.max_stack_size, self.info.map_or(64, |i| i32::from(i.max_stack_size)))
    }

    fn dyed_color(&self) -> Option<u32> {
        let b = self.bytes(self.ids.dyed_color)?;
        Some(u32::from_be_bytes(b.get(..4)?.try_into().ok()?))
    }

    fn potion_color(&self) -> Option<u32> {
        // option(potion), option(custom colour), custom effects; otherwise mean of the visible
        // effects (of the potion then custom ones) weighted by amplifier + 1.
        let b = self.bytes(self.ids.potion_contents)?;
        let mut r = Reader::new(b);
        let potion = if r.u8().ok()? == 1 { Some(read_varint(&mut r).ok()? as usize) } else { None };
        if r.u8().ok()? == 1 {
            return Some(u32::from_be_bytes(r.take(4).ok()?.try_into().ok()?));
        }
        let custom = read_effects(&mut r)?;
        let base = potion.and_then(|p| self.data.potions.get(p)).into_iter().flatten();
        let all = base.map(|e| (usize::from(e.effect), i32::from(e.amplifier), e.visible)).chain(custom);
        let (mut sum, mut total) = ([0i64; 3], 0i64);
        for (effect, amp, visible) in all {
            if !visible {
                continue;
            }
            let c = self.data.effect_colors.get(effect).copied().unwrap_or(0);
            let w = i64::from(amp) + 1;
            for (k, s) in sum.iter_mut().enumerate() {
                *s += w * i64::from((c >> (16 - 8 * k)) & 0xff);
            }
            total += w;
        }
        (total > 0).then(|| sum.iter().fold(0u32, |a, s| a << 8 | (s / total) as u32 & 0xff))
    }

    fn charge_type(&self) -> &'static str {
        charge_type(self.ids, self.bytes(self.ids.charged))
    }

    fn crossbow_charge(&self) -> i32 {
        crate::item_regs::crossbow_charge(self.bytes(self.ids.enchantments))
    }

    fn trim_material(&self) -> Option<String> {
        crate::item_regs::material_id(self.patch("minecraft:trim")?)
    }

    fn block_state(&self, property: &str) -> Option<String> {
        let b = self.bytes(self.ids.block_state)?;
        let mut r = Reader::new(b);
        let n = read_varint(&mut r).ok()?;
        for _ in 0..n {
            let k = rc_proto::read_str(&mut r, 32767).ok()?;
            let v = rc_proto::read_str(&mut r, 32767).ok()?;
            if k == property {
                return Some(v.to_owned());
            }
        }
        None
    }

    fn custom_model_data_flag(&self, i: usize) -> Option<bool> {
        let (mut r, n) = self.custom_model_data_list(1)?;
        (i < n).then(|| r.take(i + 1).ok().map(|s| s[i] != 0)).flatten()
    }

    fn custom_model_data_float(&self, i: usize) -> Option<f32> {
        let (mut r, n) = self.custom_model_data_list(0)?;
        (i < n).then(|| r.take((i + 1) * 4).ok().and_then(|s| s[i * 4..].try_into().ok()).map(f32::from_be_bytes))?
    }

    fn custom_model_data_string(&self, i: usize) -> Option<String> {
        let (mut r, n) = self.custom_model_data_list(2)?;
        if i >= n {
            return None;
        }
        for _ in 0..i {
            rc_proto::read_str(&mut r, 32767).ok()?;
        }
        rc_proto::read_str(&mut r, 32767).ok().map(str::to_owned)
    }

    fn custom_model_data_color(&self, i: usize) -> Option<u32> {
        let (mut r, n) = self.custom_model_data_list(3)?;
        (i < n).then(|| r.take((i + 1) * 4).ok().and_then(|s| s[i * 4..].try_into().ok()).map(u32::from_be_bytes))?
    }

    /// Explosion colours: shape, then the list of colours (32-bit integers). Truncated
    /// list: no colour (the component would not decode).
    fn firework_colors(&self, f: &mut dyn FnMut(u32)) {
        let Some(b) = self.bytes(self.ids.firework_explosion) else { return };
        let mut r = Reader::new(b);
        let list = read_varint(&mut r)
            .and_then(|_| read_varint(&mut r))
            .ok()
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| r.take(n.checked_mul(4)?).ok());
        for &c in list.unwrap_or_default().as_chunks::<4>().0 {
            f(u32::from_be_bytes(c));
        }
    }

    fn glint(&self) -> bool {
        if !self.removed(self.ids.glint)
            && let Some(b) = self.bytes(self.ids.glint)
        {
            return b.first() == Some(&1);
        }
        match self.info.map_or(0, |i| i.glint) {
            1 => return false,
            2 => return true,
            _ => {}
        }
        self.bytes(self.ids.enchantments).and_then(varint).is_some_and(|n| n > 0)
            || self.bytes(self.ids.lodestone).is_some()
    }

    fn selected(&self) -> bool {
        self.hold.main
    }

    fn lodestone(&self) -> Option<(String, [i32; 3])> {
        // option(dimension + packed position), then the tracking boolean.
        let b = self.bytes(self.ids.lodestone)?;
        let mut r = Reader::new(b);
        if r.u8().ok()? != 1 {
            return None;
        }
        let dim = rc_proto::read_str(&mut r, 32767).ok()?.to_owned();
        let v = i64::from_be_bytes(r.take(8).ok()?.try_into().ok()?);
        Some((dim, [(v >> 38) as i32, (v << 52 >> 52) as i32, (v << 26 >> 38) as i32]))
    }
}

/// Crossbow charge (bytes of the `charged_projectiles` component): "rocket" if one of the
/// ammunitions is a rocket, "arrow" otherwise, "none" without ammunition.
fn charge_type(ids: &ComponentIds, bytes: Option<&[u8]>) -> &'static str {
    let Some(b) = bytes else { return "none" };
    let mut r = Reader::new(b);
    let n = read_varint(&mut r).unwrap_or(0);
    if n <= 0 {
        return "none";
    }
    for _ in 0..n {
        // Unreadable template: the charge remains an arrow (at least one ammunition).
        let Ok(s) = ids.codec.read_template(&mut r) else { break };
        if Some(s.item) == ids.rocket {
            return "rocket";
        }
    }
    "arrow"
}

/// World and holder seen by the evaluation (local player).
#[derive(Clone)]
pub struct HudEnv {
    pub dimension: String,
    /// Interface clock (real time × 20): sliders, cache keys.
    pub tick: i64,
    /// World time: oscillation of the needles (compass, clock) and draw without target; follows
    /// the cadence, stops in pause and under freeze.
    pub game_time: i64,
    pub sun_angle: f32,
    pub pos: [f64; 3],
    pub body_yaw: f32,
    pub spawn: Option<(String, [i32; 3])>,
    /// Last death location of the local player (spawn info).
    pub last_death: Option<(String, [i32; 3])>,
    /// Bindings held down (bit `i` = `options::KEYS[i]`).
    pub keys: u64,
    /// The camera follows an entity other than the local player.
    pub camera_elsewhere: bool,
    /// The world advances at this tick (not frozen, or step by step in progress): damped swing of the
    /// compass of the local player.
    pub ticking: bool,
    /// RGB colour of the local player's team (`team` tint of the items they wear).
    pub team: Option<u32>,
}

impl crate::App {
    /// Item evaluation environment of the local player (bar, menus, cursor).
    pub(crate) fn hud_env(&self) -> HudEnv {
        HudEnv {
            dimension: self.dimension.clone(),
            tick: (self.started.elapsed().as_secs_f64() * 20.0) as i64,
            game_time: self.env.env.clocks.game_time,
            sun_angle: self.sun_angle,
            pos: self.player.pos,
            // Body rotation not yet tracked: the head's.
            body_yaw: self.yaw,
            spawn: self.spawn.clone(),
            last_death: self.last_death.clone(),
            keys: self.binds.down_mask(),
            camera_elsewhere: self.exact.camera.is_some(),
            ticking: self.exact.rate.runs,
            team: self.scores.team_rgb(&self.local_name),
        }
    }

    /// Publishes the hands of the local player (chosen slot, hand of an active bobber's line).
    pub(crate) fn publish_local_hands(&self) {
        let me = self.player_id;
        let fishing = self.entities.map.values().any(|e| e.kind == "fishing_bobber" && e.data == me);
        let main_rod = self
            .inventory
            .hotbar(self.hud.selected)
            .and_then(|s| usize::try_from(s.item).ok())
            .and_then(|i| self.item_names.get(i))
            .is_some_and(|n| n == "minecraft:fishing_rod" || n == "fishing_rod");
        set_local_hands(self.hud.selected, fishing.then_some(main_rod));
    }
}

impl HudEnv {
    /// Empty environment (out-of-world screens).
    pub fn empty() -> HudEnv {
        HudEnv {
            dimension: String::new(),
            tick: 0,
            game_time: 0,
            sun_angle: 0.0,
            pos: [0.0; 3],
            body_yaw: 0.0,
            spawn: None,
            last_death: None,
            keys: 0,
            camera_elsewhere: false,
            ticking: true,
            team: None,
        }
    }
}

impl rc_assets::item::EvalEnv for HudEnv {
    fn dimension(&self) -> Option<String> {
        Some(self.dimension.clone())
    }

    // Clock of the item properties: the world time, not the interface clock `tick`.
    #[allow(clippy::misnamed_getters)]
    fn tick(&self) -> i64 {
        self.game_time
    }

    fn sun_angle(&self) -> f32 {
        self.sun_angle
    }

    fn holder_pos(&self) -> [f64; 3] {
        self.pos
    }

    fn body_yaw(&self) -> f32 {
        self.body_yaw
    }

    fn spawn(&self) -> Option<(String, [i32; 3])> {
        self.spawn.clone()
    }

    fn random(&self) -> f32 {
        // Draw per tick (needles without target), reproducible.
        let mut x = (self.game_time as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        x ^= x >> 31;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }

    fn now_ms(&self) -> Option<i64> {
        // Bench and trials: Christmas forced (`RUSTCRAFT_CHRISTMAS=1` → 25/12 noon UTC, `=0` → 1 July).
        if let Some(v) = rc_data::env_os!("RUSTCRAFT_CHRISTMAS") {
            return Some(if v == "1" { 1_798_200_000_000 } else { 1_782_907_200_000 });
        }
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
    }

    fn utc_offset_secs(&self, _ms: i64) -> i64 {
        if rc_data::env_os!("RUSTCRAFT_CHRISTMAS").is_some() {
            return 0;
        }
        rc_sim::storage::local_offset_secs()
    }

    fn last_death(&self) -> Option<(String, [i32; 3])> {
        self.last_death.clone()
    }

    fn key_down(&self, keybind: &str) -> bool {
        crate::options::KEYS.iter().position(|k| k.0 == keybind).is_some_and(|i| i < 64 && self.keys >> i & 1 == 1)
    }

    fn view_entity(&self) -> bool {
        !self.camera_elsewhere
    }

    fn local_ticking(&self) -> bool {
        self.ticking
    }

    fn team_color(&self) -> Option<u32> {
        self.team
    }
}

/// Environment of the local player in a given display context (first-person hand).
pub struct InContext<'a>(pub &'a HudEnv, pub &'static str);

impl rc_assets::item::EvalEnv for InContext<'_> {
    fn display_context(&self) -> &str {
        self.1
    }
    fn dimension(&self) -> Option<String> {
        self.0.dimension()
    }
    fn tick(&self) -> i64 {
        self.0.tick()
    }
    fn sun_angle(&self) -> f32 {
        self.0.sun_angle()
    }
    fn holder_pos(&self) -> [f64; 3] {
        self.0.holder_pos()
    }
    fn body_yaw(&self) -> f32 {
        self.0.body_yaw()
    }
    fn spawn(&self) -> Option<(String, [i32; 3])> {
        self.0.spawn()
    }
    fn random(&self) -> f32 {
        self.0.random()
    }
    fn now_ms(&self) -> Option<i64> {
        self.0.now_ms()
    }
    fn utc_offset_secs(&self, ms: i64) -> i64 {
        self.0.utc_offset_secs(ms)
    }
    fn last_death(&self) -> Option<(String, [i32; 3])> {
        self.0.last_death()
    }
    fn key_down(&self, keybind: &str) -> bool {
        self.0.key_down(keybind)
    }
    fn view_entity(&self) -> bool {
        self.0.view_entity()
    }
    fn local_ticking(&self) -> bool {
        self.0.local_ticking()
    }
    fn team_color(&self) -> Option<u32> {
        self.0.team_color()
    }
}

/// Item worn outside the interface (third-person hand, head, ground, frame, mouth of a
/// mob): world of the frame (`world`, the HUD's) and carrying entity. Any entity carries: the
/// needles (compass, clock) read its position and visual rotation; the properties with a
/// condition or a choice see only a living holder (`living`); the death location and the
/// damped swing apply only to the local player.
pub struct OwnerEnv<'a> {
    pub world: &'a HudEnv,
    pub ctx: &'static str,
    /// Network id of the holder (seed of the needles without target, `seed`).
    pub id: i32,
    pub pos: [f64; 3],
    /// Visual rotation (degrees): body of a living entity, own rotation of an item on the ground, direction
    /// of a frame.
    pub yaw: f32,
    /// Kind of the living holder, without namespace (`zombie`), `None` for a non-living one.
    pub living: Option<&'a str>,
    pub right_handed: bool,
    /// Holder = local player.
    pub local: bool,
    /// Holder = entity followed by the camera (otherwise, the local player).
    pub camera: bool,
}

impl<'a> OwnerEnv<'a> {
    /// Non-living holder `id` (item on the ground, frame, projectile) at `pos`, visual rotation `yaw`.
    pub fn new(world: &'a HudEnv, ctx: &'static str, id: i32, pos: [f64; 3], yaw: f32) -> Self {
        OwnerEnv { world, ctx, id, pos, yaw, living: None, right_handed: true, local: false, camera: false }
    }

    /// Seed of the evaluation: id of the holder, plus the rank of the context for a living entity
    /// (two hands, the head and the mouth of the same mob do not spin together).
    pub fn seed(&self) -> i32 {
        if self.living.is_some() { living_seed(self.id, self.ctx) } else { self.id }
    }
}

/// Seed of an item worn by the living entity `id` in context `ctx`: id plus the rank of the
/// context in the game's order (none, hands in third then first person, head,
/// interface, ground, fixed, shelf).
pub fn living_seed(id: i32, ctx: &str) -> i32 {
    const RANKS: [&str; 10] = [
        "none",
        "thirdperson_lefthand",
        "thirdperson_righthand",
        "firstperson_lefthand",
        "firstperson_righthand",
        "head",
        "gui",
        "ground",
        "fixed",
        "on_shelf",
    ];
    id.wrapping_add(RANKS.iter().position(|&c| c == ctx).unwrap_or(0) as i32)
}

impl rc_assets::item::EvalEnv for OwnerEnv<'_> {
    fn living_holder(&self) -> bool {
        self.living.is_some()
    }
    fn holder_type(&self) -> Option<String> {
        self.living.map(|k| format!("minecraft:{k}"))
    }
    fn display_context(&self) -> &str {
        self.ctx
    }
    fn dimension(&self) -> Option<String> {
        self.world.dimension()
    }
    fn tick(&self) -> i64 {
        self.world.tick()
    }
    fn sun_angle(&self) -> f32 {
        self.world.sun_angle()
    }
    fn holder_pos(&self) -> [f64; 3] {
        self.pos
    }
    fn body_yaw(&self) -> f32 {
        self.yaw
    }
    fn spawn(&self) -> Option<(String, [i32; 3])> {
        self.world.spawn()
    }
    fn random(&self) -> f32 {
        self.world.random()
    }
    fn now_ms(&self) -> Option<i64> {
        self.world.now_ms()
    }
    fn utc_offset_secs(&self, ms: i64) -> i64 {
        self.world.utc_offset_secs(ms)
    }
    fn last_death(&self) -> Option<(String, [i32; 3])> {
        if self.local { self.world.last_death() } else { None }
    }
    fn key_down(&self, keybind: &str) -> bool {
        self.world.key_down(keybind)
    }
    fn view_entity(&self) -> bool {
        self.camera
    }
    fn right_handed(&self) -> bool {
        self.right_handed
    }
    fn local_ticking(&self) -> bool {
        self.local && self.world.ticking
    }
    /// Team known only for the local player (like the HUD).
    fn team_color(&self) -> Option<u32> {
        if self.local { self.world.team } else { None }
    }
}

/// Days since 1970 → (year, month, day) of the Gregorian calendar (timestamp outside Windows).
#[cfg(any(test, not(windows)))]
pub(crate) fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_dates() {
        assert_eq!(super::civil(0), (1970, 1, 1));
        assert_eq!(super::civil(20_738), (2026, 10, 12));
    }

    /// Crossbow charge: a rocket anywhere in the list is enough (not only at the head).
    #[test]
    fn charge_with_any_rocket() {
        let ids = super::ComponentIds { rocket: Some(9), ..Default::default() };
        // (item, count, additions 0, removals 0) per ammunition.
        let list = |items: &[u8]| {
            let mut b = vec![items.len() as u8];
            for &i in items {
                b.extend([i, 1, 0, 0]);
            }
            b
        };
        assert_eq!(super::charge_type(&ids, Some(&list(&[3, 9, 3]))), "rocket");
        assert_eq!(super::charge_type(&ids, Some(&list(&[9]))), "rocket");
        assert_eq!(super::charge_type(&ids, Some(&list(&[3, 3]))), "arrow");
        assert_eq!(super::charge_type(&ids, Some(&list(&[]))), "none");
        assert_eq!(super::charge_type(&ids, None), "none");
    }

    /// Hands of the local player: chosen hotbar slot and fishing line (main if it
    /// carries the rod), in the inventory as in a container; keys held down.
    #[test]
    fn local_hands_and_menu_slots() {
        use super::{Hold, menu_hold, set_local_hands};
        let mut inv = crate::inventory::PlayerInventory::default();
        set_local_hands(2, Some(true));
        let h = menu_hold(&inv, 38);
        assert!(h.main && h.cast && !h.off && h.menu_slot == Some(38));
        assert!(!menu_hold(&inv, 37).main && !menu_hold(&inv, 37).cast);
        assert!(menu_hold(&inv, 45).off && !menu_hold(&inv, 45).cast);
        assert!(Hold::local_hand(true).cast && !Hold::local_hand(false).cast);
        // Line in the off hand (rod not in the main hand).
        set_local_hands(2, Some(false));
        assert!(menu_hold(&inv, 45).cast && !menu_hold(&inv, 38).cast);
        // Single chest (27 slots + 36): hotbar at 54..62, no off hand.
        inv.open(1, crate::menu::Kind::Chest(3), None);
        assert!(menu_hold(&inv, 56).main && !menu_hold(&inv, 38).main);
        set_local_hands(0, None);
        assert!(!Hold::local_hand(true).cast && !Hold::local_hand(false).cast);
        let env = super::HudEnv { keys: 1 << crate::keys::USE, ..super::HudEnv::empty() };
        use rc_assets::item::EvalEnv;
        assert!(env.key_down("key.use") && !env.key_down("key.attack") && !env.key_down("key.unknown"));
    }

    /// Item held by another entity, identical to the one the local player uses: neither use
    /// in progress nor chosen slot (only the local player has them).
    #[test]
    fn worn_by_others_ignores_local_use() {
        use rc_assets::item::StackView;
        let stack = rc_proto_v777::item::ItemStack { item: 7, count: 1, added: vec![], removed: vec![] };
        let ids = super::ComponentIds::default();
        let data = rc_data::VersionData::default();
        let view = |hold| super::StackRef { stack: &stack, info: None, ids: &ids, data: &data, hold };
        super::set_using(Some((7, 5, 15, 0)));
        let other = view(super::Hold::worn(0, false, false));
        assert_eq!(other.using(), None);
        assert!(!other.selected());
        assert!(view(super::Hold::worn(0, false, true)).fishing_cast());
        let me = view(super::Hold::worn(0, true, false));
        assert_eq!(me.using(), Some((5, 15)));
        assert!(me.selected());
        // Use in the off hand: another holder's shield does not rise.
        super::set_using(Some((7, 5, 15, 1)));
        assert_eq!(view(super::Hold::worn(1, false, false)).using(), None);
        assert_eq!(view(super::Hold::worn(1, true, false)).using(), Some((5, 15)));
        super::set_using(None);
    }

    /// First-person hand: context of the hand, but death location, position, clock and
    /// keys of the local player (recovery compass, Christmas chest, key-driven items).
    #[test]
    fn first_person_env_is_local_player() {
        use rc_assets::item::EvalEnv;
        let death = Some(("minecraft:overworld".to_string(), [10, 64, -3]));
        let hud = super::HudEnv {
            pos: [1.0, 2.0, 3.0],
            last_death: death.clone(),
            keys: 1 << crate::keys::USE,
            ..super::HudEnv::empty()
        };
        let env = super::InContext(&hud, "firstperson_righthand");
        assert_eq!(env.display_context(), "firstperson_righthand");
        assert_eq!(env.last_death(), death);
        assert_eq!(env.holder_pos(), [1.0, 2.0, 3.0]);
        assert!(env.now_ms().is_some());
        assert!(env.key_down("key.use"));
    }

    /// Items worn outside the HUD: the frame's world (dimension, world time, sun,
    /// spawn) and the position and rotation of the holder; an item on the ground is carried without being
    /// living (oriented needles, no hand or type); death location and damped swing
    /// reserved for the local player, the swing to the advancing world; seed per holder.
    #[test]
    fn owner_env_reads_world_and_holder() {
        use rc_assets::item::EvalEnv;
        let world = super::HudEnv {
            dimension: "minecraft:the_nether".into(),
            game_time: 1234,
            sun_angle: 90.0,
            spawn: Some(("minecraft:overworld".into(), [0, 64, 0])),
            last_death: Some(("minecraft:the_nether".into(), [5, 70, 5])),
            ..super::HudEnv::empty()
        };
        let ground = super::OwnerEnv::new(&world, "ground", 40, [3.0, 65.0, -2.0], 150.0);
        // Seed: id alone for a non-living entity, plus the rank of the context for a living one.
        assert_eq!(ground.seed(), 40);
        assert!(ground.has_holder() && !ground.living_holder());
        assert_eq!(ground.holder_type(), None);
        assert_eq!(ground.dimension().as_deref(), Some("minecraft:the_nether"));
        assert_eq!((ground.tick(), ground.sun_angle()), (1234, 90.0));
        assert_eq!((ground.holder_pos(), ground.body_yaw()), ([3.0, 65.0, -2.0], 150.0));
        assert_eq!(ground.spawn(), world.spawn);
        assert_eq!(ground.last_death(), None);
        assert!(!ground.local_ticking() && !ground.view_entity());
        let zombie = super::OwnerEnv {
            living: Some("zombie"),
            right_handed: false,
            ..super::OwnerEnv::new(&world, "head", 40, [0.0; 3], 10.0)
        };
        assert_eq!(zombie.holder_type().as_deref(), Some("minecraft:zombie"));
        assert!(zombie.living_holder() && !zombie.right_handed());
        assert_eq!(zombie.seed(), 45);
        let me = super::OwnerEnv {
            living: Some("player"),
            local: true,
            camera: true,
            ..super::OwnerEnv::new(&world, "thirdperson_righthand", 40, [0.0; 3], 0.0)
        };
        assert_eq!(me.last_death(), world.last_death);
        assert!(me.local_ticking() && me.view_entity());
        assert_eq!(me.display_context(), "thirdperson_righthand");
        assert_eq!(me.seed(), 42);
        let ranks: Vec<i32> = ["thirdperson_lefthand", "firstperson_lefthand", "firstperson_righthand", "ground"]
            .iter()
            .map(|c| super::living_seed(i32::MAX, c))
            .collect();
        assert_eq!(ranks, [i32::MIN, i32::MIN + 2, i32::MIN + 3, i32::MIN + 6]);
        // Frozen world (`/tick freeze`): no more swing, neither in the hand nor in the interface.
        let frozen = super::HudEnv { ticking: false, ..world.clone() };
        let frozen_me = super::OwnerEnv { local: true, ..super::OwnerEnv::new(&frozen, "head", 1, [0.0; 3], 0.0) };
        assert!(!frozen_me.local_ticking());
        assert!(!frozen.local_ticking() && !super::InContext(&frozen, "firstperson_righthand").local_ticking());
        assert!(world.local_ticking() && super::InContext(&world, "firstperson_righthand").local_ticking());
    }

    /// Tints read from the stack (network format of the components): colours of the explosion of a
    /// firework star; strings and colours of `custom_model_data` after the floats
    /// and booleans; team of the local player passed by their environment.
    #[test]
    fn tint_components() {
        use rc_assets::item::{EvalEnv, StackView};
        let ids = super::ComponentIds { custom_model_data: Some(1), firework_explosion: Some(2), ..Default::default() };
        let data = rc_data::VersionData::default();
        // Shape 1, colours [0xb3312c, 0x3b511a], fade [0x123456], trail, twinkle.
        let mut fw = vec![1, 2, 0x00, 0xb3, 0x31, 0x2c, 0x00, 0x3b, 0x51, 0x1a, 1, 0, 0x12, 0x34, 0x56, 1, 0];
        // Floats [1.5], booleans [true, false], strings ["a", "bc"], colours [0xff0000, 0x00ff00].
        let mut cmd = vec![1];
        cmd.extend(1.5f32.to_be_bytes());
        cmd.extend([2, 1, 0, 2, 1, b'a', 2, b'b', b'c', 2, 0, 0xff, 0, 0, 0, 0, 0xff, 0]);
        let stack = |cmd: &[u8], fw: &[u8]| rc_proto_v777::item::ItemStack {
            item: 0,
            count: 1,
            added: vec![(1, cmd.into()), (2, fw.into())],
            removed: vec![],
        };
        let s = stack(&cmd, &fw);
        let view = super::StackRef { stack: &s, info: None, ids: &ids, data: &data, hold: super::Hold::NONE };
        let mut colors = Vec::new();
        view.firework_colors(&mut |c| colors.push(c));
        assert_eq!(colors, [0xb3_312c, 0x3b_511a]);
        assert_eq!(view.custom_model_data_float(0), Some(1.5));
        assert_eq!((view.custom_model_data_flag(0), view.custom_model_data_flag(1)), (Some(true), Some(false)));
        assert_eq!(view.custom_model_data_flag(2), None);
        assert_eq!(view.custom_model_data_string(0).as_deref(), Some("a"));
        assert_eq!(view.custom_model_data_string(1).as_deref(), Some("bc"));
        assert_eq!(view.custom_model_data_string(2), None);
        assert_eq!(view.custom_model_data_color(0), Some(0xff_0000));
        assert_eq!(view.custom_model_data_color(1), Some(0x00_ff00));
        assert_eq!(view.custom_model_data_color(2), None);
        // Truncated colour list: no colour, like an unreadable component.
        fw.truncate(8);
        cmd.truncate(cmd.len() - 1);
        let s = stack(&cmd, &fw);
        let view = super::StackRef { stack: &s, info: None, ids: &ids, data: &data, hold: super::Hold::NONE };
        let mut n = 0;
        view.firework_colors(&mut |_| n += 1);
        assert_eq!(n, 0);
        assert_eq!(view.custom_model_data_color(1), None);
        let hud = super::HudEnv { team: Some(0x55_55ff), ..super::HudEnv::empty() };
        assert_eq!(super::InContext(&hud, "gui").team_color(), Some(0x55_55ff));
        assert_eq!(super::HudEnv::empty().team_color(), None);
    }

    #[test]
    fn durability_hue() {
        assert_eq!(super::hsv(1.0 / 3.0, 1.0, 1.0), 0x00ff00);
        assert_eq!(super::hsv(0.0, 1.0, 1.0), 0xff0000);
    }
}
