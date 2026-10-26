//! Binary format of the cache (`data.bin`): little-endian, sections in a fixed order,
//! arrays prefixed by their `u32` length. Single-pass reading, no JSON parsing.

use crate::model::{
    Aabb, AmbientMood, AmbientSounds, AudioAttrs, BackgroundMusic, Biome, BlockInfo, Bound, ClientState, DimensionType,
    Ease, EnvEntry, EnvOp, MusicTrack, PacketDef, Phase, Property, Registry, SoundType, StateInfo, Timeline, Track,
    VersionData,
};
use crate::{Error, Result};

const MAGIC: &[u8; 8] = b"RCDATA\0\0";
/// To increment at each change of the format or the extractor.
pub const FORMAT: u32 = 33;

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("table too large"));
    }
    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.b.get(self.p..self.p + n).ok_or_else(|| Error::Format("truncated cache".into()))?;
        self.p += n;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("fixed size"))
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.arr()?))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.arr()?))
    }
    fn len(&mut self) -> Result<usize> {
        let n = self.u32()? as usize;
        // Safeguard against a corrupt file: no more elements than remaining bytes.
        if n > self.b.len() - self.p {
            return Err(Error::Format("invalid table length".into()));
        }
        Ok(n)
    }
    fn str(&mut self) -> Result<String> {
        let n = self.len()?;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| Error::Format("invalid string".into()))
    }
}

fn enc_creative_stack(w: &mut W, s: &crate::CreativeStack) {
    w.str(&s.item);
    w.u8(s.visibility);
    w.len(s.features.len());
    for f in &s.features {
        w.str(f);
    }
    w.len(s.added.len());
    for (t, b) in &s.added {
        w.str(t);
        w.len(b.len());
        w.0.extend_from_slice(b);
    }
    w.len(s.removed.len());
    for t in &s.removed {
        w.str(t);
    }
    w.len(s.banner.len());
    for (p, c) in &s.banner {
        w.str(p);
        w.u8(*c);
    }
}

fn dec_creative_stack(r: &mut R<'_>) -> Result<crate::CreativeStack> {
    let item = r.str()?;
    let visibility = r.u8()?;
    let n = r.len()?;
    let features = (0..n).map(|_| r.str()).collect::<Result<_>>()?;
    let n = r.len()?;
    let mut added = Vec::with_capacity(n);
    for _ in 0..n {
        let t = r.str()?;
        let l = r.len()?;
        added.push((t, r.take(l)?.to_vec()));
    }
    let n = r.len()?;
    let removed = (0..n).map(|_| r.str()).collect::<Result<_>>()?;
    let n = r.len()?;
    let banner = (0..n).map(|_| Ok((r.str()?, r.u8()?))).collect::<Result<_>>()?;
    Ok(crate::CreativeStack { item, visibility, features, added, removed, banner })
}

fn enc_creative(w: &mut W, c: &crate::CreativeData) {
    w.len(c.tabs.len());
    for t in &c.tabs {
        w.str(&t.name);
        w.str(&t.title);
        w.u8(t.row);
        w.u8(t.column);
        w.u8(t.kind as u8);
        w.u8(u8::from(t.right) | u8::from(t.scroll) << 1 | u8::from(t.show_title) << 2);
        w.str(&t.background);
        enc_creative_stack(w, &t.icon);
        w.len(t.entries.len());
        for e in &t.entries {
            match e {
                crate::CreativeEntry::Stack(s) => {
                    w.u8(0);
                    enc_creative_stack(w, s);
                }
                crate::CreativeEntry::Dynamic(d) => {
                    w.u8(1);
                    w.str(d);
                }
            }
        }
    }
    for list in [&c.paintings, &c.enchantments] {
        w.len(list.len());
        for (n, a, b) in list {
            w.str(n);
            w.u8(*a);
            w.u8(*b);
        }
    }
}

fn dec_creative(r: &mut R<'_>) -> Result<crate::CreativeData> {
    use crate::CreativeKind;
    let n = r.len()?;
    let mut tabs = Vec::with_capacity(n);
    for _ in 0..n {
        let (name, title, row, column) = (r.str()?, r.str()?, r.u8()?, r.u8()?);
        let kind = match r.u8()? {
            0 => CreativeKind::Category,
            1 => CreativeKind::Hotbar,
            2 => CreativeKind::Search,
            3 => CreativeKind::Inventory,
            _ => return Err(Error::Format("invalid creative tab".into())),
        };
        let flags = r.u8()?;
        let background = r.str()?;
        let icon = dec_creative_stack(r)?;
        let ne = r.len()?;
        let mut entries = Vec::with_capacity(ne);
        for _ in 0..ne {
            entries.push(match r.u8()? {
                0 => crate::CreativeEntry::Stack(dec_creative_stack(r)?),
                _ => crate::CreativeEntry::Dynamic(r.str()?),
            });
        }
        tabs.push(crate::CreativeTab {
            name,
            title,
            row,
            column,
            kind,
            right: flags & 1 != 0,
            scroll: flags & 2 != 0,
            show_title: flags & 4 != 0,
            background,
            icon,
            entries,
        });
    }
    let mut lists = [Vec::new(), Vec::new()];
    for l in &mut lists {
        let n = r.len()?;
        for _ in 0..n {
            l.push((r.str()?, r.u8()?, r.u8()?));
        }
    }
    let [paintings, enchantments] = lists;
    Ok(crate::CreativeData { tabs, paintings, enchantments })
}

fn phase_code(p: Phase) -> u8 {
    Phase::ALL.iter().position(|&x| x == p).unwrap_or(0) as u8
}

/// Independent sections of the file, decoded in parallel on load.
const SECTIONS: usize = 4;

fn enc_geometry(d: &VersionData, w: &mut W) {
    w.len(d.aabbs.len());
    for a in &d.aabbs {
        for v in a.min.iter().chain(&a.max) {
            w.f64(*v);
        }
    }
    w.len(d.shapes.len());
    for &(a, b) in &d.shapes {
        w.u32(a);
        w.u32(b);
    }
    w.len(d.states.len());
    for s in &d.states {
        for v in [s.block, s.collision, s.outline, s.occlusion, s.support, s.flags] {
            w.u16(v);
        }
        for v in [s.light_emission, s.light_dampening, s.fluid, s.fluid_amount] {
            w.u8(v);
        }
        w.u32(s.values);
        w.u8(s.seed_offset);
        w.u8(s.sturdy);
        w.u8(s.light);
        w.u8(s.sound);
        w.u8(s.map_color);
    }
}

fn enc_blocks(d: &VersionData, w: &mut W) {
    w.len(d.blocks.len());
    for b in &d.blocks {
        w.str(&b.name);
        w.u32(b.first_state);
        w.u32(b.state_count);
        w.u32(b.default_state);
        w.f32(b.friction);
        w.f32(b.speed_factor);
        w.f32(b.jump_factor);
        w.f32(b.bounce);
        w.u8(b.inside);
        w.u8(b.offset_kind);
        w.f32(b.offset_h);
        w.f32(b.offset_v);
        w.u8(u8::from(b.dynamic_shape));
        w.len(b.properties.len());
        for &p in &b.properties {
            w.u16(p);
        }
    }
    w.len(d.properties.len());
    for p in &d.properties {
        w.str(&p.name);
        w.len(p.values.len());
        for v in &p.values {
            w.str(v);
        }
    }
    w.len(d.state_values.len());
    w.0.extend_from_slice(&d.state_values);
    w.len(d.sound_types.len());
    for t in &d.sound_types {
        w.f32(t.volume);
        w.f32(t.pitch);
        for s in [&t.break_sound, &t.step, &t.place, &t.hit, &t.fall] {
            w.str(s);
        }
    }
    w.len(d.hardness.len());
    for &h in &d.hardness {
        w.f32(h);
    }
    w.len(d.needs_tool.len());
    for &t in &d.needs_tool {
        w.u8(u8::from(t));
    }
    w.len(d.sim.len());
    for &s in &d.sim {
        w.u16(s);
    }
    w.len(d.blast.len());
    for &b in &d.blast {
        w.f32(b);
    }
    w.len(d.instrument.len());
    for &i in &d.instrument {
        w.u8(i);
    }
}

fn enc_block_set(w: &mut W, s: &crate::BlockSet) {
    match s {
        crate::BlockSet::Tag(t) => {
            w.u8(0);
            w.str(t);
        }
        crate::BlockSet::Blocks(b) => {
            w.u8(1);
            w.len(b.len());
            for &v in b {
                w.u32(v);
            }
        }
    }
}

fn dec_block_set(r: &mut R<'_>) -> Result<crate::BlockSet> {
    Ok(if r.u8()? == 0 {
        crate::BlockSet::Tag(r.str()?)
    } else {
        let n = r.len()?;
        crate::BlockSet::Blocks((0..n).map(|_| r.u32()).collect::<Result<_>>()?)
    })
}

/// Interaction components of an item (`interactions.md`): tool, consumable, weapons.
fn enc_item_use(w: &mut W, i: &crate::ItemInfo) {
    match &i.tool {
        None => w.u8(0),
        Some(t) => {
            w.u8(1);
            w.len(t.rules.len());
            for rule in &t.rules {
                enc_block_set(w, &rule.blocks);
                w.f32(rule.speed.unwrap_or(f32::NAN));
                w.u8(rule.correct.map_or(2, u8::from));
            }
            w.f32(t.default_speed);
            w.u32(t.damage_per_block);
            w.u8(u8::from(t.creative_destroy));
        }
    }
    match &i.consumable {
        None => w.u8(0),
        Some(c) => {
            w.u8(1);
            w.f32(c.seconds);
            w.u8(c.animation);
            w.str(&c.sound);
            w.u8(u8::from(c.particles));
        }
    }
    w.u8(i.food.map_or(0, |f| 1 + u8::from(f)));
    match i.attack_range {
        None => w.u8(0),
        Some(a) => {
            w.u8(1);
            for v in a {
                w.f32(v);
            }
        }
    }
    w.f32(i.min_attack_charge);
    w.u8(u8::from(i.piercing)
        | u8::from(i.kinetic) << 1
        | u8::from(i.blocks_attacks) << 2
        | u8::from(i.use_sprint) << 3);
    for v in i.attack_anim.iter().chain(&i.interact_anim) {
        w.u8(*v);
    }
    w.f32(i.use_speed);
    match &i.kinetic_params {
        None => w.u8(0),
        Some(k) => {
            w.u8(1);
            w.u32(k.contact_cooldown as u32);
            w.u32(k.delay as u32);
            for c in &k.conditions {
                match c {
                    None => w.u8(0),
                    Some((t, s, r)) => {
                        w.u8(1);
                        w.u32(*t as u32);
                        w.f32(*s);
                        w.f32(*r);
                    }
                }
            }
            w.f32(k.forward_movement);
            w.f32(k.damage_multiplier);
        }
    }
    w.len(i.attributes.len());
    for a in &i.attributes {
        w.str(&a.attribute);
        w.str(&a.id);
        w.f64(a.amount);
        w.u8(a.operation);
        w.u8(a.slot);
    }
    w.len(i.components.len());
    for &c in &i.components {
        w.u32(u32::from(c));
    }
}

fn dec_item_use(r: &mut R<'_>, i: &mut crate::ItemInfo) -> Result<()> {
    if r.u8()? != 0 {
        let n = r.len()?;
        let mut rules = Vec::with_capacity(n);
        for _ in 0..n {
            let blocks = dec_block_set(r)?;
            let speed = Some(r.f32()?).filter(|s| !s.is_nan());
            let correct = match r.u8()? {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            };
            rules.push(crate::ToolRule { blocks, speed, correct });
        }
        i.tool = Some(crate::Tool {
            rules,
            default_speed: r.f32()?,
            damage_per_block: r.u32()?,
            creative_destroy: r.u8()? != 0,
        });
    }
    if r.u8()? != 0 {
        i.consumable =
            Some(crate::Consumable { seconds: r.f32()?, animation: r.u8()?, sound: r.str()?, particles: r.u8()? != 0 });
    }
    i.food = match r.u8()? {
        0 => None,
        v => Some(v == 2),
    };
    if r.u8()? != 0 {
        i.attack_range = Some([r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?]);
    }
    i.min_attack_charge = r.f32()?;
    let bits = r.u8()?;
    (i.piercing, i.kinetic, i.blocks_attacks, i.use_sprint) =
        (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
    i.attack_anim = [r.u8()?, r.u8()?];
    i.interact_anim = [r.u8()?, r.u8()?];
    i.use_speed = r.f32()?;
    if r.u8()? != 0 {
        let (contact_cooldown, delay) = (r.u32()? as i32, r.u32()? as i32);
        let mut conditions = [None; 3];
        for c in &mut conditions {
            if r.u8()? != 0 {
                *c = Some((r.u32()? as i32, r.f32()?, r.f32()?));
            }
        }
        i.kinetic_params = Some(crate::KineticParams {
            contact_cooldown,
            delay,
            conditions,
            forward_movement: r.f32()?,
            damage_multiplier: r.f32()?,
        });
    }
    let n = r.len()?;
    for _ in 0..n {
        i.attributes.push(crate::AttributeMod {
            attribute: r.str()?,
            id: r.str()?,
            amount: r.f64()?,
            operation: r.u8()?,
            slot: r.u8()?,
        });
    }
    let n = r.len()?;
    for _ in 0..n {
        i.components.push(r.u32()? as u16);
    }
    Ok(())
}

fn enc_misc(d: &VersionData, w: &mut W) {
    w.len(d.fluids.len());
    for f in &d.fluids {
        w.str(f);
    }
    w.len(d.packets.len());
    for p in &d.packets {
        w.u8(phase_code(p.phase));
        w.u8(u8::from(p.bound == Bound::ToServer));
        w.i32(p.id);
        w.str(&p.name);
    }
    w.len(d.dimension_types.len());
    for t in &d.dimension_types {
        w.str(&t.name);
        w.i32(t.min_y);
        w.i32(t.height);
        w.u8(u8::from(t.nether_light));
        enc_env(w, &t.attributes);
        w.len(t.timelines.len());
        for n in &t.timelines {
            w.str(n);
        }
        w.u8(t.skybox);
        w.u8(u8::from(t.has_skylight));
        w.u8(u8::from(t.has_ceiling));
        w.str(&t.default_clock);
        w.f32(t.ambient_light);
        enc_audio(w, &t.audio);
    }
    w.len(d.client_states.len());
    for c in &d.client_states {
        w.u8(c.skip_self);
        for k in c.tint_kind {
            w.u8(k);
        }
        for v in c.tint_value {
            w.u32(v);
        }
        w.u8(c.particle_tint.0);
        w.u32(c.particle_tint.1);
        w.u8(c.dust.0);
        w.u32(c.dust.1);
        for v in c.entity_tint {
            w.u32(v);
        }
    }
    w.len(d.biomes.len());
    for b in &d.biomes {
        w.str(&b.name);
        w.f32(b.temperature);
        w.f32(b.downfall);
        for c in [b.grass_color, b.foliage_color, b.dry_foliage_color, b.sky_color] {
            w.u32(c.map_or(0, |c| c | 0xff00_0000));
        }
        w.u32(b.water_color);
        w.u8(b.grass_modifier);
        enc_env(w, &b.attributes);
        w.u8(u8::from(b.has_precipitation) | (u8::from(b.frozen) << 1));
        enc_audio(w, &b.audio);
    }
    w.len(d.timelines.len());
    for t in &d.timelines {
        w.str(&t.name);
        w.str(&t.clock);
        w.u32(t.period);
        w.len(t.tracks.len());
        for k in &t.tracks {
            w.str(&k.attr);
            w.u8(k.op as u8);
            let (kind, c) = match k.ease {
                Ease::Linear => (0, [0.0; 4]),
                Ease::Constant => (1, [0.0; 4]),
                Ease::Bezier(c) => (2, c),
            };
            w.u8(kind);
            for v in c {
                w.f32(v);
            }
            w.len(k.keys.len());
            for (tick, arg) in &k.keys {
                w.u32(*tick);
                for v in arg {
                    w.f32(*v);
                }
            }
        }
    }
}

fn enc_env(w: &mut W, e: &[EnvEntry]) {
    w.len(e.len());
    for x in e {
        w.str(&x.attr);
        w.u8(x.op as u8);
        for v in x.arg {
            w.f32(v);
        }
    }
}

fn enc_track(w: &mut W, t: Option<&MusicTrack>) {
    w.u8(u8::from(t.is_some()));
    if let Some(t) = t {
        w.str(&t.sound);
        w.i32(t.min_delay);
        w.i32(t.max_delay);
        w.u8(u8::from(t.replace));
    }
}

fn dec_track(r: &mut R<'_>) -> Result<Option<MusicTrack>> {
    if r.u8()? == 0 {
        return Ok(None);
    }
    Ok(Some(MusicTrack { sound: r.str()?, min_delay: r.i32()?, max_delay: r.i32()?, replace: r.u8()? != 0 }))
}

fn enc_audio(w: &mut W, a: &AudioAttrs) {
    w.u8(u8::from(a.music.is_some()));
    if let Some(m) = &a.music {
        enc_track(w, m.default.as_ref());
        enc_track(w, m.creative.as_ref());
        enc_track(w, m.underwater.as_ref());
    }
    w.u8(u8::from(a.ambient.is_some()));
    if let Some(s) = &a.ambient {
        w.u8(u8::from(s.loop_sound.is_some()));
        if let Some(l) = &s.loop_sound {
            w.str(l);
        }
        w.u8(u8::from(s.mood.is_some()));
        if let Some(m) = &s.mood {
            w.str(&m.sound);
            w.i32(m.tick_delay);
            w.i32(m.block_search_extent);
            w.f64(m.offset);
        }
        w.len(s.additions.len());
        for (n, c) in &s.additions {
            w.str(n);
            w.f64(*c);
        }
    }
}

fn dec_audio(r: &mut R<'_>) -> Result<AudioAttrs> {
    let music = if r.u8()? != 0 {
        Some(BackgroundMusic { default: dec_track(r)?, creative: dec_track(r)?, underwater: dec_track(r)? })
    } else {
        None
    };
    let ambient = if r.u8()? != 0 {
        let loop_sound = if r.u8()? != 0 { Some(r.str()?) } else { None };
        let mood = if r.u8()? != 0 {
            Some(AmbientMood { sound: r.str()?, tick_delay: r.i32()?, block_search_extent: r.i32()?, offset: r.f64()? })
        } else {
            None
        };
        let n = r.len()?;
        let additions = (0..n).map(|_| Ok((r.str()?, r.f64()?))).collect::<Result<_>>()?;
        Some(AmbientSounds { loop_sound, mood, additions })
    } else {
        None
    };
    Ok(AudioAttrs { ambient, music })
}

fn dec_op(r: &mut R<'_>) -> Result<EnvOp> {
    EnvOp::ALL.get(r.u8()? as usize).copied().ok_or_else(|| Error::Format("invalid attribute operation".into()))
}

fn dec_f4(r: &mut R<'_>) -> Result<[f32; 4]> {
    Ok([r.f32()?, r.f32()?, r.f32()?, r.f32()?])
}

fn dec_env(r: &mut R<'_>) -> Result<Vec<EnvEntry>> {
    let n = r.len()?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(EnvEntry { attr: r.str()?, op: dec_op(r)?, arg: dec_f4(r)? });
    }
    Ok(out)
}

fn enc_registries(d: &VersionData, w: &mut W) {
    w.len(d.registries.len());
    for r in &d.registries {
        w.str(&r.name);
        w.len(r.entries.len());
        for e in &r.entries {
            w.str(e);
        }
    }
    w.len(d.items.len());
    for i in &d.items {
        w.u8(i.max_stack_size);
        w.u32(i.max_damage);
        w.u8(i.rarity);
        w.str(&i.name_key);
        w.str(&i.model);
        w.u8(i.glint);
        w.u8(i.equip_slot);
        w.str(&i.equip_asset);
        w.str(&i.equip_overlay);
        enc_item_use(w, i);
    }
    w.len(d.potions.len());
    for p in &d.potions {
        w.len(p.len());
        for e in p {
            w.u32(u32::from(e.effect));
            w.u8(e.amplifier);
            w.u8(u8::from(e.visible));
            w.i32(e.duration);
        }
    }
    w.len(d.effect_colors.len());
    for &c in &d.effect_colors {
        w.u32(c);
    }
    w.len(d.entity_dims.len());
    for v in &d.entity_dims {
        for &x in v {
            w.f32(x);
        }
    }
    w.len(d.entity_models.len());
    for l in &d.entity_models {
        w.str(&l.name);
        w.u32(u32::from(l.texture[0]) | u32::from(l.texture[1]) << 16);
        w.len(l.parts.len());
        for p in &l.parts {
            w.str(&p.name);
            w.u32(p.parent);
            for v in p.pose {
                w.f32(v);
            }
            w.len(p.cubes.len());
            for c in &p.cubes {
                for v in c.uv.iter().chain(&c.from).chain(&c.size).chain(&c.grow).chain(&c.tex_scale) {
                    w.f32(*v);
                }
                w.u8(u8::from(c.mirror));
                w.u8(c.faces);
            }
        }
    }
    w.len(d.entity_animations.len());
    for a in &d.entity_animations {
        w.str(&a.name);
        w.f32(a.length);
        w.u8(u8::from(a.looping));
        w.len(a.channels.len());
        for c in &a.channels {
            w.str(&c.part);
            w.u8(c.target as u8);
            w.len(c.keyframes.len());
            for k in &c.keyframes {
                w.f32(k.time);
                for v in k.pre.iter().chain(&k.post) {
                    w.f32(*v);
                }
                w.u8(u8::from(k.catmull));
            }
        }
    }
    enc_creative(w, &d.creative);
}

/// `tag`: fingerprint of the extractor that produced the data (checked on load).
pub fn encode(d: &VersionData, tag: u64) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(2 << 20));
    w.0.extend_from_slice(MAGIC);
    w.u32(FORMAT);
    w.0.extend_from_slice(&tag.to_le_bytes());
    w.str(&d.version);
    w.i32(d.protocol);
    let encoders: [fn(&VersionData, &mut W); SECTIONS] = [enc_geometry, enc_blocks, enc_misc, enc_registries];
    let parts = encoders.map(|f| {
        let mut p = W(Vec::new());
        f(d, &mut p);
        p.0
    });
    for p in &parts {
        w.len(p.len());
    }
    for p in parts {
        w.0.extend_from_slice(&p);
    }
    w.0
}

type Geometry = (Vec<Aabb>, Vec<(u32, u32)>, Vec<StateInfo>);
type Blocks =
    (Vec<BlockInfo>, Vec<Property>, Vec<u8>, Vec<SoundType>, Vec<f32>, Vec<bool>, Vec<u16>, Vec<f32>, Vec<u8>);
type Misc = (Vec<String>, Vec<PacketDef>, Vec<DimensionType>, Vec<ClientState>, Vec<Biome>, Vec<Timeline>);

fn done(r: &R<'_>) -> Result<()> {
    if r.p == r.b.len() { Ok(()) } else { Err(Error::Format("extra bytes in a section".into())) }
}

/// Size of a state record: 6 × u16, 4 × u8, 1 × u32, 4 × u8.
const STATE_BYTES: usize = 25;

fn dec_geometry(b: &[u8], parallel: bool) -> Result<Geometry> {
    let mut r = R { b, p: 0 };
    let n = r.len()?;
    let mut aabbs = Vec::with_capacity(n);
    for _ in 0..n {
        let c = [r.f64()?, r.f64()?, r.f64()?, r.f64()?, r.f64()?, r.f64()?];
        aabbs.push(Aabb { min: [c[0], c[1], c[2]], max: [c[3], c[4], c[5]] });
    }
    let n = r.len()?;
    let mut shapes = Vec::with_capacity(n);
    for _ in 0..n {
        let (a, b) = (r.u32()?, r.u32()?);
        if a > b || b as usize > aabbs.len() {
            return Err(Error::Format("shape outside table".into()));
        }
        shapes.push((a, b));
    }
    // States: fixed-size records, decoded independently (in parallel if requested).
    let n = r.len()?;
    let raw = r.take(n.checked_mul(STATE_BYTES).ok_or_else(|| Error::Format("too many states".into()))?)?;
    let shape_count = shapes.len();
    let state = |c: &[u8; STATE_BYTES]| -> Result<StateInfo> {
        let u16_at = |i: usize| u16::from_le_bytes([c[i], c[i + 1]]);
        let s = StateInfo {
            block: u16_at(0),
            collision: u16_at(2),
            outline: u16_at(4),
            occlusion: u16_at(6),
            support: u16_at(8),
            flags: u16_at(10),
            light_emission: c[12],
            light_dampening: c[13],
            fluid: c[14],
            fluid_amount: c[15],
            values: u32::from_le_bytes([c[16], c[17], c[18], c[19]]),
            seed_offset: c[20],
            sturdy: c[21],
            light: c[22],
            sound: c[23],
            map_color: c[24],
        };
        if [s.collision, s.outline, s.occlusion, s.support].iter().any(|&i| i as usize >= shape_count) {
            return Err(Error::Format("invalid shape index".into()));
        }
        Ok(s)
    };
    let records = raw.as_chunks::<STATE_BYTES>().0;
    let states = if parallel {
        use rayon::prelude::*;
        // Batches of 4096 states: collecting element by element costs more than the decoding.
        let parts: Vec<Result<Vec<StateInfo>>> =
            records.par_chunks(4096).map(|c| c.iter().map(state).collect()).collect();
        let mut states = Vec::with_capacity(records.len());
        for p in parts {
            states.extend(p?);
        }
        states
    } else {
        records.iter().map(state).collect::<Result<Vec<_>>>()?
    };
    done(&r)?;
    Ok((aabbs, shapes, states))
}

fn dec_blocks(b: &[u8]) -> Result<Blocks> {
    let mut r = R { b, p: 0 };
    let n = r.len()?;
    let mut blocks = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let (first_state, state_count, default_state) = (r.u32()?, r.u32()?, r.u32()?);
        let (friction, speed_factor, jump_factor) = (r.f32()?, r.f32()?, r.f32()?);
        let (bounce, inside) = (r.f32()?, r.u8()?);
        let (offset_kind, offset_h, offset_v) = (r.u8()?, r.f32()?, r.f32()?);
        let dynamic_shape = r.u8()? != 0;
        let np = r.len()?;
        let properties = (0..np).map(|_| r.u16()).collect::<Result<_>>()?;
        blocks.push(BlockInfo {
            name,
            first_state,
            state_count,
            default_state,
            friction,
            speed_factor,
            jump_factor,
            bounce,
            inside,
            offset_kind,
            offset_h,
            offset_v,
            dynamic_shape,
            properties,
        });
    }
    let n = r.len()?;
    let mut properties = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let nv = r.len()?;
        let values = (0..nv).map(|_| r.str()).collect::<Result<_>>()?;
        properties.push(Property { name, values });
    }
    let n = r.len()?;
    let state_values = r.take(n)?.to_vec();
    let n = r.len()?;
    let mut sound_types = Vec::with_capacity(n);
    for _ in 0..n {
        sound_types.push(SoundType {
            volume: r.f32()?,
            pitch: r.f32()?,
            break_sound: r.str()?,
            step: r.str()?,
            place: r.str()?,
            hit: r.str()?,
            fall: r.str()?,
        });
    }
    let n = r.len()?;
    let hardness = (0..n).map(|_| r.f32()).collect::<Result<_>>()?;
    let n = r.len()?;
    let needs_tool = (0..n).map(|_| r.u8().map(|v| v != 0)).collect::<Result<_>>()?;
    let n = r.len()?;
    let sim = (0..n).map(|_| r.u16()).collect::<Result<_>>()?;
    let n = r.len()?;
    let blast = (0..n).map(|_| r.f32()).collect::<Result<_>>()?;
    let n = r.len()?;
    let instrument = (0..n).map(|_| r.u8()).collect::<Result<_>>()?;
    done(&r)?;
    Ok((blocks, properties, state_values, sound_types, hardness, needs_tool, sim, blast, instrument))
}

fn dec_misc(b: &[u8]) -> Result<Misc> {
    let mut r = R { b, p: 0 };
    let n = r.len()?;
    let fluids = (0..n).map(|_| r.str()).collect::<Result<_>>()?;
    let n = r.len()?;
    let mut packets = Vec::with_capacity(n);
    for _ in 0..n {
        let phase = *Phase::ALL.get(r.u8()? as usize).ok_or_else(|| Error::Format("invalid phase".into()))?;
        let bound = if r.u8()? != 0 { Bound::ToServer } else { Bound::ToClient };
        packets.push(PacketDef { phase, bound, id: r.i32()?, name: r.str()? });
    }
    let n = r.len()?;
    let mut dims = Vec::with_capacity(n);
    for _ in 0..n {
        let (name, min_y, height, nether_light) = (r.str()?, r.i32()?, r.i32()?, r.u8()? != 0);
        let attributes = dec_env(&mut r)?;
        let k = r.len()?;
        let mut timelines = Vec::with_capacity(k);
        for _ in 0..k {
            timelines.push(r.str()?);
        }
        dims.push(DimensionType {
            name,
            min_y,
            height,
            nether_light,
            attributes,
            timelines,
            skybox: r.u8()?,
            has_skylight: r.u8()? != 0,
            has_ceiling: r.u8()? != 0,
            default_clock: r.str()?,
            ambient_light: r.f32()?,
            audio: dec_audio(&mut r)?,
        });
    }
    let n = r.len()?;
    let mut client = Vec::with_capacity(n);
    for _ in 0..n {
        let skip_self = r.u8()?;
        let tint_kind = [r.u8()?, r.u8()?, r.u8()?, r.u8()?];
        let tint_value = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        let particle_tint = (r.u8()?, r.u32()?);
        let dust = (r.u8()?, r.u32()?);
        let entity_tint = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        client.push(ClientState { skip_self, tint_kind, tint_value, particle_tint, dust, entity_tint });
    }
    let n = r.len()?;
    let mut biomes = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let (temperature, downfall) = (r.f32()?, r.f32()?);
        let opt = |v: u32| (v != 0).then_some(v & 0xff_ffff);
        let (grass, foliage, dry, sky) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        biomes.push(Biome {
            name,
            temperature,
            downfall,
            grass_color: opt(grass),
            foliage_color: opt(foliage),
            dry_foliage_color: opt(dry),
            sky_color: opt(sky),
            water_color: r.u32()?,
            grass_modifier: r.u8()?,
            attributes: dec_env(&mut r)?,
            has_precipitation: false,
            frozen: false,
            audio: Default::default(),
        });
        let b = biomes.last_mut().expect("added above");
        let flags = r.u8()?;
        (b.has_precipitation, b.frozen) = (flags & 1 != 0, flags & 2 != 0);
        b.audio = dec_audio(&mut r)?;
    }
    let n = r.len()?;
    let mut timelines = Vec::with_capacity(n);
    for _ in 0..n {
        let (name, clock, period) = (r.str()?, r.str()?, r.u32()?);
        let k = r.len()?;
        let mut tracks = Vec::with_capacity(k);
        for _ in 0..k {
            let (attr, op) = (r.str()?, dec_op(&mut r)?);
            let kind = r.u8()?;
            let c = dec_f4(&mut r)?;
            let ease = match kind {
                0 => Ease::Linear,
                1 => Ease::Constant,
                2 => Ease::Bezier(c),
                _ => return Err(Error::Format("invalid timeline curve".into())),
            };
            let m = r.len()?;
            let mut keys = Vec::with_capacity(m);
            for _ in 0..m {
                keys.push((r.u32()?, dec_f4(&mut r)?));
            }
            tracks.push(Track { attr, op, ease, keys });
        }
        timelines.push(Timeline { name, clock, period, tracks });
    }
    done(&r)?;
    Ok((fluids, packets, dims, client, biomes, timelines))
}

type Registries = (
    Vec<Registry>,
    Vec<crate::ItemInfo>,
    Vec<Vec<crate::PotionEffect>>,
    Vec<u32>,
    Vec<[f32; 4]>,
    Vec<crate::EntityLayer>,
    Vec<crate::KeyframeAnimation>,
    crate::CreativeData,
);

fn dec_registries(b: &[u8]) -> Result<Registries> {
    let mut r = R { b, p: 0 };
    let n = r.len()?;
    let mut regs = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let ne = r.len()?;
        let entries = (0..ne).map(|_| r.str()).collect::<Result<_>>()?;
        regs.push(Registry::new(name, entries));
    }
    let n = r.len()?;
    let mut items = Vec::with_capacity(n);
    for _ in 0..n {
        let mut item = crate::ItemInfo {
            max_stack_size: r.u8()?,
            max_damage: r.u32()?,
            rarity: r.u8()?,
            name_key: r.str()?,
            model: r.str()?,
            glint: r.u8()?,
            equip_slot: r.u8()?,
            equip_asset: r.str()?,
            equip_overlay: r.str()?,
            ..Default::default()
        };
        dec_item_use(&mut r, &mut item)?;
        items.push(item);
    }
    let n = r.len()?;
    let mut potions = Vec::with_capacity(n);
    for _ in 0..n {
        let k = r.len()?;
        let mut effects = Vec::with_capacity(k);
        for _ in 0..k {
            effects.push(crate::PotionEffect {
                effect: r.u32()? as u16,
                amplifier: r.u8()?,
                visible: r.u8()? != 0,
                duration: r.i32()?,
            });
        }
        potions.push(effects);
    }
    let n = r.len()?;
    let effect_colors = (0..n).map(|_| r.u32()).collect::<Result<_>>()?;
    let n = r.len()?;
    let entity_dims = (0..n).map(|_| Ok([r.f32()?, r.f32()?, r.f32()?, r.f32()?])).collect::<Result<_>>()?;
    let n = r.len()?;
    let mut entity_models = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let t = r.u32()?;
        let np = r.len()?;
        let mut parts = Vec::with_capacity(np);
        for _ in 0..np {
            let name = r.str()?;
            let parent = r.u32()?;
            let mut pose = [0.0; 9];
            for v in &mut pose {
                *v = r.f32()?;
            }
            let nc = r.len()?;
            let mut cubes = Vec::with_capacity(nc);
            for _ in 0..nc {
                let mut v = [0.0f32; 13];
                for x in &mut v {
                    *x = r.f32()?;
                }
                cubes.push(crate::EntityCube {
                    uv: [v[0], v[1]],
                    from: [v[2], v[3], v[4]],
                    size: [v[5], v[6], v[7]],
                    grow: [v[8], v[9], v[10]],
                    tex_scale: [v[11], v[12]],
                    mirror: r.u8()? != 0,
                    faces: r.u8()?,
                });
            }
            parts.push(crate::EntityPart { name, parent, pose, cubes });
        }
        entity_models.push(crate::EntityLayer { name, texture: [t as u16, (t >> 16) as u16], parts });
    }
    let n = r.len()?;
    let mut entity_animations = Vec::with_capacity(n);
    for _ in 0..n {
        let (name, length, looping) = (r.str()?, r.f32()?, r.u8()? != 0);
        let nc = r.len()?;
        let mut channels = Vec::with_capacity(nc);
        for _ in 0..nc {
            let part = r.str()?;
            let target = match r.u8()? {
                0 => crate::AnimTarget::Position,
                1 => crate::AnimTarget::Rotation,
                2 => crate::AnimTarget::Scale,
                _ => return Err(Error::Format("invalid animation target".into())),
            };
            let nk = r.len()?;
            let mut keyframes = Vec::with_capacity(nk);
            for _ in 0..nk {
                let time = r.f32()?;
                let mut v = [0.0f32; 6];
                for x in &mut v {
                    *x = r.f32()?;
                }
                let catmull = r.u8()? != 0;
                keyframes.push(crate::Keyframe { time, pre: [v[0], v[1], v[2]], post: [v[3], v[4], v[5]], catmull });
            }
            if keyframes.is_empty() {
                return Err(Error::Format("empty animation channel".into()));
            }
            channels.push(crate::AnimChannel { part, target, keyframes });
        }
        entity_animations.push(crate::KeyframeAnimation { name, length, looping, channels });
    }
    let creative = dec_creative(&mut r)?;
    done(&r)?;
    Ok((regs, items, potions, effect_colors, entity_dims, entity_models, entity_animations, creative))
}

/// Decodes the cache; sections in parallel if the pool has more than one thread.
pub fn decode(bytes: &[u8], tag: u64) -> Result<VersionData> {
    decode_with(bytes, tag, rayon::current_num_threads() > 1)
}

pub fn decode_with(bytes: &[u8], tag: u64, parallel: bool) -> Result<VersionData> {
    let mut r = R { b: bytes, p: 0 };
    if r.take(8)? != MAGIC {
        return Err(Error::Format("not a Rustcraft cache".into()));
    }
    let format = r.u32()?;
    if format != FORMAT {
        return Err(Error::Stale(format!("format {format}, expected {FORMAT}")));
    }
    if u64::from_le_bytes(r.arr()?) != tag {
        return Err(Error::Stale("extractor modified".into()));
    }
    let (version, protocol) = (r.str()?, r.i32()?);
    let mut lens = [0usize; SECTIONS];
    for l in &mut lens {
        *l = r.u32()? as usize;
    }
    let mut parts: [&[u8]; SECTIONS] = [&[]; SECTIONS];
    for (p, &l) in parts.iter_mut().zip(&lens) {
        *p = r.take(l)?;
    }
    if r.p != bytes.len() {
        return Err(Error::Format("extra bytes at the end of the cache".into()));
    }
    let [pg, pb, pm, pr] = parts;
    let ((geo, blocks), (misc, regs)) = if parallel {
        rayon::join(
            || rayon::join(|| dec_geometry(pg, true), || dec_blocks(pb)),
            || rayon::join(|| dec_misc(pm), || dec_registries(pr)),
        )
    } else {
        ((dec_geometry(pg, false), dec_blocks(pb)), (dec_misc(pm), dec_registries(pr)))
    };
    let (
        (aabbs, shapes, states),
        (blocks, properties, state_values, sound_types, hardness, needs_tool, sim, blast, instrument),
        (fluids, packets, dimension_types, client_states, biomes, timelines),
        (registries, items, potions, effect_colors, entity_dims, entity_models, entity_animations, creative),
    ) = (geo?, blocks?, misc?, regs?);
    let d = VersionData {
        version,
        protocol,
        aabbs,
        shapes,
        states,
        blocks,
        properties,
        state_values,
        fluids,
        packets,
        registries,
        dimension_types,
        client_states,
        biomes,
        timelines,
        items,
        potions,
        effect_colors,
        entity_dims,
        entity_models,
        entity_animations,
        creative,
        sound_types,
        hardness,
        needs_tool,
        sim,
        blast,
        instrument,
        lookup: Default::default(),
    };
    // Cross-consistency: referenced blocks and property values exist.
    for s in &d.states {
        let b = d.blocks.get(s.block as usize).ok_or_else(|| Error::Format("invalid block".into()))?;
        let end = s.values as usize + b.properties.len();
        if end > d.state_values.len() {
            return Err(Error::Format("property values outside the table".into()));
        }
        for (i, &p) in b.properties.iter().enumerate() {
            let prop = d.properties.get(p as usize).ok_or_else(|| Error::Format("invalid property".into()))?;
            if d.state_values[s.values as usize + i] as usize >= prop.values.len() {
                return Err(Error::Format("invalid property value".into()));
            }
        }
    }
    Ok(d)
}
