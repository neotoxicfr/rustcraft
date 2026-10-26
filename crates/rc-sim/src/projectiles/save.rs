//! Saving projectiles (the game's format): chunk entities (`entities/`), clouds, and
//! the player's Ender pearls in their file (`ender_pearls`, dimension included).

use std::collections::BTreeMap;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::cloud::Cloud;
use super::{PKind, Pickup, Proj};
use crate::nbtio::{f64s, int, set};
use crate::player::Stack;
use crate::server::Server;

pub fn uuid_nbt(u: u128) -> Nbt {
    crate::nbtio::uuid_nbt(u)
}

pub fn uuid_of(v: Option<&Nbt>) -> Option<u128> {
    crate::nbtio::uuid(v)
}

fn dbl(v: [f64; 3]) -> Nbt {
    Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect())
}

fn byte(b: bool) -> Nbt {
    Nbt::Byte(i8::from(b))
}

fn flag(e: &Nbt, k: &str) -> bool {
    int(e, k).is_some_and(|v| v != 0)
}

impl Server {
    /// NBT of a projectile (+ own fields).
    pub(crate) fn proj_to_nbt(&self, p: &Proj) -> Nbt {
        let mut f: Vec<(String, Nbt)> = vec![
            ("id".into(), Nbt::String(p.kind.name().into())),
            ("Pos".into(), dbl(p.pos)),
            ("Motion".into(), dbl(p.vel)),
            ("Rotation".into(), Nbt::List(vec![Nbt::Float(p.yaw), Nbt::Float(p.pitch)])),
            ("fall_distance".into(), Nbt::Double(0.0)),
            ("Fire".into(), Nbt::Short(p.fire.clamp(-32768, 32767) as i16)),
            ("Air".into(), Nbt::Short(300)),
            ("OnGround".into(), byte(p.on_ground)),
            ("Invulnerable".into(), byte(false)),
            ("PortalCooldown".into(), Nbt::Int(0)),
            ("UUID".into(), uuid_nbt(p.uuid)),
        ];
        if p.no_gravity {
            f.push(("NoGravity".into(), byte(true)));
        }
        if p.silent {
            f.push(("Silent".into(), byte(true)));
        }
        if let Some(o) = p.owner {
            f.push(("Owner".into(), uuid_nbt(o)));
        }
        if p.left_owner {
            f.push(("LeftOwner".into(), byte(true)));
        }
        // The Eye of Ender is not a projectile: neither this field, nor an owner.
        if p.kind != PKind::EyeOfEnder {
            f.push(("HasBeenShot".into(), byte(p.has_been_shot)));
        }
        match p.kind {
            k if k.arrow_like() => {
                f.push(("life".into(), Nbt::Short(p.life as i16)));
                if let Some(s) = p.last_state {
                    f.push(("inBlockState".into(), self.states.palette_entry(&self.data, s)));
                }
                f.push(("shake".into(), Nbt::Byte(p.shake as i8)));
                f.push(("inGround".into(), byte(p.in_ground)));
                f.push(("pickup".into(), Nbt::Byte(p.pickup as i8)));
                f.push(("damage".into(), Nbt::Double(p.damage)));
                f.push(("crit".into(), byte(p.crit)));
                f.push(("PierceLevel".into(), Nbt::Byte(p.pierce as i8)));
                f.push(("SoundEvent".into(), Nbt::String(p.hit_sound.into())));
                f.push(("item".into(), p.item.to_nbt(None)));
                if let Some(w) = &p.weapon {
                    f.push(("weapon".into(), w.to_nbt(None)));
                }
                if k == PKind::SpectralArrow {
                    f.push(("Duration".into(), Nbt::Int(p.spectral_duration)));
                }
                if k == PKind::Trident {
                    f.push(("DealtDamage".into(), byte(p.dealt_damage)));
                }
            }
            PKind::Firework => {
                f.push(("Life".into(), Nbt::Int(p.fw_life)));
                f.push(("LifeTime".into(), Nbt::Int(p.fw_lifetime)));
                f.push(("FireworksItem".into(), p.item.to_nbt(None)));
                f.push(("ShotAtAngle".into(), byte(p.at_angle)));
            }
            // F15: dragon fireball (no item), shulker bullet (target, segment).
            PKind::DragonFireball => f.push(("acceleration_power".into(), Nbt::Double(p.accel))),
            PKind::ShulkerBullet => {
                if let Some(b) = &p.bullet {
                    b.save(&mut f);
                }
            }
            // F16: wither skull (no item, `dangerous` flag).
            PKind::WitherSkull => {
                f.push(("acceleration_power".into(), Nbt::Double(p.accel)));
                f.push(("dangerous".into(), byte(p.dangerous)));
            }
            k if k.hurting() => {
                f.push(("acceleration_power".into(), Nbt::Double(p.accel)));
                // F14: the wind charge writes no item.
                if !k.wind() {
                    f.push(("Item".into(), p.item.to_nbt(None)));
                }
                if k == PKind::LargeFireball {
                    f.push(("ExplosionPower".into(), Nbt::Byte(p.power as i8)));
                }
            }
            _ => f.push(("Item".into(), p.item.to_nbt(None))),
        }
        Nbt::Compound(f)
    }

    /// Projectile re-read (new network identifier).
    pub(crate) fn proj_from_nbt(&mut self, e: &Nbt) -> Option<Proj> {
        let kind = PKind::from_name(e.get("id").and_then(Nbt::as_str)?)?;
        let pos = f64s::<3>(e.get("Pos"))?;
        let id = self.proj_new_id();
        let item_key = match kind {
            k if k.arrow_like() => "item",
            PKind::Firework => "FireworksItem",
            _ => "Item",
        };
        let item = e.get(item_key).and_then(Stack::from_nbt).unwrap_or_else(|| Stack {
            id: kind.default_item().into(),
            count: 1,
            components: None,
        });
        let mut p = Proj::new(kind, id, pos, item);
        p.vel = crate::nbtio::motion(e.get("Motion")).unwrap_or_default();
        if let Some(Nbt::List(r)) = e.get("Rotation")
            && let [Nbt::Float(y), Nbt::Float(x)] = r.as_slice()
        {
            p.yaw = *y;
            p.pitch = *x;
        }
        if let Some(u) = uuid_of(e.get("UUID")) {
            p.uuid = u;
        }
        p.fire = int(e, "Fire").unwrap_or(0) as i32;
        p.on_ground = flag(e, "OnGround");
        p.no_gravity = flag(e, "NoGravity");
        p.silent = flag(e, "Silent");
        p.owner = uuid_of(e.get("Owner"));
        p.left_owner = flag(e, "LeftOwner");
        p.has_been_shot = flag(e, "HasBeenShot");
        p.first_tick = false;
        if kind.arrow_like() {
            p.life = int(e, "life").unwrap_or(0) as i32;
            p.last_state = e.get("inBlockState").and_then(|s| self.states.state_of(&self.data, s));
            p.shake = (int(e, "shake").unwrap_or(0) & 255) as i32;
            p.in_ground = flag(e, "inGround");
            p.damage = match e.get("damage") {
                Some(Nbt::Double(d)) => *d,
                _ => 2.0,
            };
            p.pickup = match int(e, "pickup").unwrap_or(0) {
                1 => Pickup::Allowed,
                2 => Pickup::CreativeOnly,
                _ => Pickup::Disallowed,
            };
            p.crit = flag(e, "crit");
            p.pierce = int(e, "PierceLevel").unwrap_or(0) as u8;
            if let Some(s) = e.get("SoundEvent").and_then(Nbt::as_str) {
                p.hit_sound = match s {
                    "minecraft:item.crossbow.hit" => "minecraft:item.crossbow.hit",
                    "minecraft:item.trident.hit_ground" => "minecraft:item.trident.hit_ground",
                    _ => "minecraft:entity.arrow.hit",
                };
            }
            p.weapon = e.get("weapon").and_then(Stack::from_nbt);
            p.spectral_duration = int(e, "Duration").unwrap_or(200) as i32;
            p.dealt_damage = flag(e, "DealtDamage");
            if kind == PKind::Arrow {
                let c = if crate::items::component(&p.item, "minecraft:potion_contents").is_some() {
                    super::hits::potion_color(&p.item)
                } else {
                    -1
                };
                p.set_meta(11, Meta::Int(c));
            }
            if kind == PKind::Trident {
                let ctx = super::ench::Ctx { this: Some("minecraft:trident"), ..Default::default() };
                p.loyalty = self
                    .ench_value(&p.item, "minecraft:trident_return_acceleration", 0.0, &ctx)
                    .clamp(0.0, 127.0) as u8;
                p.set_meta(11, Meta::Byte(p.loyalty as i8));
                p.set_meta(12, Meta::Bool(crate::enchanting::has_any(&self.life.items, &p.item)));
            }
        } else if kind == PKind::Firework {
            p.fw_life = int(e, "Life").unwrap_or(0) as i32;
            p.fw_lifetime = int(e, "LifeTime").unwrap_or(0) as i32;
            p.at_angle = flag(e, "ShotAtAngle");
            let net = self.net(Some(&p.item));
            p.set_meta(8, Meta::Item(net));
            p.set_meta(10, Meta::Bool(p.at_angle));
        } else {
            if kind.hurting()
                && let Some(Nbt::Double(a)) = e.get("acceleration_power")
            {
                p.accel = *a;
            }
            if kind == PKind::LargeFireball {
                p.power = int(e, "ExplosionPower").unwrap_or(1) as i32;
            }
            if kind == PKind::ShulkerBullet {
                p.bullet = Some(Box::new(crate::mobs::end::bullet::BulletState::load(e)));
            }
            if kind == PKind::WitherSkull {
                p.dangerous = flag(e, "dangerous");
                p.set_meta(8, Meta::Bool(p.dangerous));
            } else if kind.carries_item() && !kind.wind() {
                // F14: the wind charge has no item; F15: nor the dragon fireball, nor the bullet.
                let net = self.net(Some(&p.item));
                p.set_meta(8, Meta::Item(net));
            }
        }
        p.sync_arrow_flags();
        p.sync_fire_flag();
        Some(p)
    }

    /// Projectiles and clouds read with a chunk: made live.
    pub(crate) fn adopt_projectiles(&mut self, loaded: &[(i32, i32)]) {
        for c in loaded {
            if !self.proj.adopted.insert(*c) {
                continue;
            }
            let Some(list) = self.entities.get_mut(c) else { continue };
            type Ents = Vec<(i32, Nbt)>;
            let (mine, rest): (Ents, Ents) = std::mem::take(list).into_iter().partition(|(_, e)| {
                e.get("id")
                    .and_then(Nbt::as_str)
                    .is_some_and(|n| PKind::from_name(n).is_some() || n == "minecraft:area_effect_cloud")
            });
            *list = rest;
            if mine.is_empty() {
                continue;
            }
            self.life.drop_chunks.insert(*c);
            for (_, e) in mine {
                if e.get("id").and_then(Nbt::as_str) == Some("minecraft:area_effect_cloud") {
                    let id = self.proj_new_id();
                    let cl = Cloud::from_nbt(id, &e);
                    self.cloud_add(cl);
                } else if let Some(p) = self.proj_from_nbt(&e) {
                    self.proj_add(p);
                }
            }
        }
    }

    /// Projectiles to write into the entity chunks (the player's pearls excluded: they go
    /// into their file).
    pub(crate) fn projectiles_into_chunks(&self, by_chunk: &mut BTreeMap<(i32, i32), Vec<Nbt>>) {
        for p in self.proj.list.iter().filter(|p| !p.removed) {
            if p.kind == PKind::EnderPearl && p.owner == Some(self.uuid) {
                continue;
            }
            let c = ((p.pos[0].floor() as i32) >> 4, (p.pos[2].floor() as i32) >> 4);
            by_chunk.entry(c).or_default().push(self.proj_to_nbt(p));
        }
        for cl in self.proj.clouds.iter().filter(|c| !c.removed) {
            let c = ((cl.pos[0].floor() as i32) >> 4, (cl.pos[2].floor() as i32) >> 4);
            by_chunk.entry(c).or_default().push(cl.to_nbt());
        }
    }

    /// Player's pearls written in their file (`ender_pearls`), including those of parked
    /// dimensions (passed through a portal or left behind).
    pub(crate) fn proj_player_nbt(&self, root: &mut Nbt) {
        let here = std::iter::once((&self.dim.name, &self.proj.list));
        let parked = self.dims.parked.iter().map(|w| (&w.dim.name, &w.proj.list));
        let pearls: Vec<Nbt> = here
            .chain(parked)
            .flat_map(|(dim, list)| {
                list.iter()
                    .filter(|p| !p.removed && p.kind == PKind::EnderPearl && p.owner == Some(self.uuid))
                    .map(move |p| (dim, p))
            })
            .map(|(dim, p)| {
                let mut n = self.proj_to_nbt(p);
                set(&mut n, "ender_pearl_dimension", Nbt::String(dim.clone()));
                n
            })
            .chain(self.proj_player.pending_pearls.iter().cloned())
            .collect();
        if pearls.is_empty() {
            crate::nbtio::remove(root, "ender_pearls");
        } else {
            set(root, "ender_pearls", Nbt::List(pearls));
        }
    }

    /// On the first tick: pearls from the player's file put back in their dimension (this one).
    pub(crate) fn proj_restore_pearls(&mut self) {
        if !self.proj_player.pearls_read {
            self.proj_player.pearls_read = true;
            let list = crate::nbtio::list(self.pdata.root.get("ender_pearls")).to_vec();
            self.proj_player.pending_pearls = list;
        }
        if self.proj_player.pending_pearls.is_empty() || !self.player_ready() {
            return;
        }
        let dim = self.dim.name.clone();
        let (here, other): (Vec<Nbt>, Vec<Nbt>) = std::mem::take(&mut self.proj_player.pending_pearls)
            .into_iter()
            .partition(|n| n.get("ender_pearl_dimension").and_then(Nbt::as_str).is_none_or(|d| d == dim));
        self.proj_player.pending_pearls = other;
        for n in here {
            if let Some(p) = self.proj_from_nbt(&n) {
                self.proj_add(p);
            }
        }
    }
}
