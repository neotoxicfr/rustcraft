//! Block-entity rendering (`docs/specs/block-entities.md` § 3–5) with the entity engine:
//! layers baked once, one instance per layer and per block entity, part matrices
//! recomputed only for the current pose.

use hashbrown::HashMap;
use std::f32::consts::PI;

use glam::{EulerRot, Mat4, Vec3};

use rc_proto_v777::item::ItemStack;

use super::{
    EntityRenderer, HeldEnv, Instance, PASS_CUTOUT_CULL, PASS_SOLID, PartMat, Pass, Pose, SKINS, default_skin, deg,
    items_in, part_matrix,
};
use crate::block_entities::{BlockEntities, BlockEntity, Data, Kind, Stack, dye_rgb, head_key};
use crate::block_fx::{BeamVertex, PortalVertex};
use crate::block_text::SignDraw;

/// Neutral overlay (no damage, no white).
const NEUTRAL: u32 = 10 << 16;
/// Full light (block and sky 15).
const FULL: u32 = 0xf0f0;

/// What the block-entity rendering reads from the frame.
pub struct BlockFrame<'a> {
    pub pt: f32,
    /// Integer origin of the camera (x, z): the matrices are relative to it.
    pub origin: [i32; 2],
    pub cam: [f64; 3],
    pub game_time: i64,
    pub min_y: i32,
    /// Raw light (block, sky) of a position.
    pub light: &'a dyn Fn(i32, i32, i32) -> (u8, u8),
    /// Box within the field of view and in a reached section (frame relative to the origin).
    pub visible: &'a dyn Fn([f32; 3], [f32; 3]) -> bool,
    pub items: Option<&'a rc_assets::item::ItemModels>,
    /// Effective render distance (chunks), range of the beams.
    pub render_distance: i32,
    /// Top of the world (y excluded): height of the gateway beam.
    pub top: i32,
    /// Block state of a position (gateway faces).
    pub state: &'a dyn Fn(i32, i32, i32) -> u32,
    /// The player is looking through a spyglass: beams at their normal radius.
    pub scoping: bool,
    /// Biome colours of a block (grass, foliage, dry foliage, water; RGB), unblended:
    /// tint of moving blocks.
    pub biome_tints: &'a dyn Fn(i32, i32, i32) -> [u32; 4],
}

/// Beam widening (beacon, test instance): flat distance from the camera to the
/// centre of the block over 96, at least 1; 1 with the spyglass.
fn beam_scale(pos: [i32; 3], f: &BlockFrame<'_>) -> f32 {
    if f.scoping {
        return 1.0;
    }
    let (dx, dz) = (f.cam[0] - (f64::from(pos[0]) + 0.5), f.cam[2] - (f64::from(pos[2]) + 0.5));
    ((dx * dx + dz * dz).sqrt() as f32 / 96.0).max(1.0)
}

/// Block-entity rendering state kept from one frame to the next.
#[derive(Default)]
pub struct BlockCache {
    /// Visibility of the frame's sections (column x, section, column z).
    sections: HashMap<(i32, i32, i32), bool>,
    /// Christmas chests (date read once, at load).
    pub(super) christmas: Option<bool>,
    /// Block entities drawn in the last frame.
    pub drawn: usize,
    /// Visible sign faces (text drawn separately).
    pub texts: Vec<SignDraw>,
    /// Beams and portals of the frame.
    pub fx: crate::block_fx::FxFrame,
}

/// Step of a 3D direction (0 down, 1 up, 2 north, 3 south, 4 west, 5 east).
/// Layer offset of the cutout skulls (towards the camera, around the eye), like the game.
fn skull_layering(f: &BlockFrame<'_>) -> Mat4 {
    let eye = Vec3::new(
        (f.cam[0] - f64::from(f.origin[0])) as f32,
        f.cam[1] as f32,
        (f.cam[2] - f64::from(f.origin[1])) as f32,
    );
    crate::render::view_layering(eye, 1.0)
}

pub fn step(d: u8) -> [i32; 3] {
    match d {
        0 => [0, -1, 0],
        1 => [0, 1, 0],
        2 => [0, 0, -1],
        3 => [0, 0, 1],
        4 => [-1, 0, 0],
        5 => [1, 0, 0],
        _ => [0, 0, 0],
    }
}

/// Yaw angle of a horizontal direction (south 0, west 90, north 180, east 270).
pub fn y_rot(d: u8) -> f32 {
    match d {
        4 => 90.0,
        2 => 180.0,
        5 => 270.0,
        _ => 0.0,
    }
}

fn clockwise(d: u8) -> u8 {
    match d {
        2 => 5,
        5 => 3,
        3 => 4,
        4 => 2,
        d => d,
    }
}

fn counter_clockwise(d: u8) -> u8 {
    match d {
        2 => 4,
        4 => 3,
        3 => 5,
        5 => 2,
        d => d,
    }
}

fn opposite(d: u8) -> u8 {
    match d {
        0 => 1,
        1 => 0,
        2 => 3,
        3 => 2,
        4 => 5,
        5 => 4,
        d => d,
    }
}

fn pack(b: u8, s: u8) -> u32 {
    (u32::from(b) * 16) | ((u32::from(s) * 16) << 8)
}

/// ARGB colour → RGBA word of the instance.
fn rgba(c: u32) -> u32 {
    (c >> 16 & 0xff) | (c & 0xff00) | (c & 0xff) << 16 | (c & 0xff00_0000)
}

/// Local date of the day (system time zone, like the game) between 24 and 26 December.
pub(super) fn is_christmas() -> bool {
    if let Some(v) = rc_data::env_os!("RUSTCRAFT_CHRISTMAS") {
        return v == "1";
    }
    let ms = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64);
    let (_, m, d, ..) = rc_sim::storage::local_datetime(ms);
    m == 12 && (24..=26).contains(&d)
}

const SHULKER_SPRITES: [&str; 17] = [
    "entity/shulker/shulker_white",
    "entity/shulker/shulker_orange",
    "entity/shulker/shulker_magenta",
    "entity/shulker/shulker_light_blue",
    "entity/shulker/shulker_yellow",
    "entity/shulker/shulker_lime",
    "entity/shulker/shulker_pink",
    "entity/shulker/shulker_gray",
    "entity/shulker/shulker_light_gray",
    "entity/shulker/shulker_cyan",
    "entity/shulker/shulker_purple",
    "entity/shulker/shulker_blue",
    "entity/shulker/shulker_brown",
    "entity/shulker/shulker_green",
    "entity/shulker/shulker_red",
    "entity/shulker/shulker_black",
    "entity/shulker/shulker",
];

/// Chest sprites: [single, left, right] per kind.
pub(super) const CHEST_SPRITES: [[&str; 3]; 8] = [
    ["entity/chest/normal", "entity/chest/normal_left", "entity/chest/normal_right"],
    ["entity/chest/copper", "entity/chest/copper_left", "entity/chest/copper_right"],
    ["entity/chest/copper_exposed", "entity/chest/copper_exposed_left", "entity/chest/copper_exposed_right"],
    ["entity/chest/copper_weathered", "entity/chest/copper_weathered_left", "entity/chest/copper_weathered_right"],
    ["entity/chest/copper_oxidized", "entity/chest/copper_oxidized_left", "entity/chest/copper_oxidized_right"],
    ["entity/chest/trapped", "entity/chest/trapped_left", "entity/chest/trapped_right"],
    ["entity/chest/christmas", "entity/chest/christmas_left", "entity/chest/christmas_right"],
    ["entity/chest/ender", "entity/chest/ender", "entity/chest/ender"],
];

impl EntityRenderer {
    /// Instance of a baked layer with matrix `m` and the pose adjusted by `anim`.
    #[allow(clippy::too_many_arguments)]
    fn push_layer(
        &mut self,
        layer: usize,
        rect: [f32; 4],
        m: Mat4,
        pass: usize,
        color: u32,
        light: u32,
        anim: impl FnOnce(&mut Pose<'_>),
    ) {
        let (parts_buf, vis_buf, mut world_m) = std::mem::take(&mut self.scratch);
        let gl = &self.layers[layer];
        let mut pose = Pose::reset(&gl.baked, parts_buf, vis_buf);
        anim(&mut pose);
        let base = self.mats.len() as u32;
        world_m.clear();
        world_m.resize(gl.baked.parts.len(), None);
        for (i, part) in gl.baked.parts.iter().enumerate() {
            let parent = if part.parent == u32::MAX { Some(m) } else { world_m[part.parent as usize] };
            let pm = parent.filter(|_| pose.visible[i]).map(|p| p * part_matrix(&pose.parts[i]));
            world_m[i] = pm;
            self.mats.push(match pm {
                Some(pm) => PartMat::new(&pm),
                None => PartMat::zeroed(),
            });
        }
        self.instances[pass].push((
            0,
            gl.first,
            gl.count,
            Instance { base, rect, color: rgba(color), light, overlay: NEUTRAL },
        ));
        self.scratch = (pose.parts, pose.visible, world_m);
    }

    /// Layer and sprite by name; nothing if one is missing.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_named(
        &mut self,
        layer: &str,
        sprite: &str,
        m: Mat4,
        pass: usize,
        color: u32,
        light: u32,
        anim: impl FnOnce(&mut Pose<'_>),
    ) -> bool {
        let (Some(&l), Some(rect)) = (self.layer_index.get(layer), self.atlas.rect(sprite)) else { return false };
        self.push_layer(l, rect, m, pass, color, light, anim);
        true
    }

    /// Block entities of the frame (§ 5): visible sections only, range per type, from farthest
    /// to nearest (translucent order).
    pub fn push_block_entities(&mut self, bes: &BlockEntities, f: &BlockFrame<'_>) {
        if self.blocks.christmas.is_none() {
            self.blocks.christmas = Some(is_christmas());
        }
        self.blocks.sections.clear();
        self.blocks.texts.clear();
        self.blocks.fx.clear();
        // Portals of blocks drawn as entities (placed earlier, by `prepare`).
        self.blocks.fx.portals.extend_from_slice(&self.objects.portals);
        self.blocks.fx.time = (f.game_time.rem_euclid(24_000) as f32 + f.pt) / 24_000.0;
        let mut list: Vec<(f64, [i32; 3], &BlockEntity)> = Vec::new();
        for map in bes.chunks.values() {
            for (pos, e) in map {
                let c = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
                let (dx, dy, dz) = (c[0] - f.cam[0], c[1] - f.cam[1], c[2] - f.cam[2]);
                let d2 = dx * dx + dy * dy + dz * dz;
                // Ranges (§ 5); the beacon is an "off-screen" render (no section test).
                let (inside, offscreen) = match e.kind {
                    Kind::Beacon => {
                        let r = f64::from(f.render_distance * 16);
                        (dx * dx + dz * dz < r * r, true)
                    }
                    // Test instance: beam in range or box within 96 blocks.
                    Kind::TestInstance => {
                        let r = f64::from(f.render_distance * 16);
                        (dx * dx + dz * dz < r * r || d2 < 96.0 * 96.0, true)
                    }
                    Kind::EndGateway => (d2 < 256.0 * 256.0, false),
                    Kind::Piston => (d2 < 68.0 * 68.0, false),
                    _ => (d2 < 64.0 * 64.0, false),
                };
                if !inside {
                    continue;
                }
                if !offscreen {
                    let key = (pos[0] >> 4, (pos[1] - f.min_y) >> 4, pos[2] >> 4);
                    let visible = *self.blocks.sections.entry(key).or_insert_with(|| {
                        let x0 = (key.0 * 16 - f.origin[0]) as f32;
                        let z0 = (key.2 * 16 - f.origin[1]) as f32;
                        let y0 = (f.min_y + key.1 * 16) as f32;
                        (f.visible)([x0, y0, z0], [x0 + 16.0, y0 + 16.0, z0 + 16.0])
                    });
                    if !visible {
                        continue;
                    }
                }
                list.push((d2, *pos, e));
            }
        }
        list.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut drawn = 0;
        for (_, pos, e) in list {
            drawn += usize::from(self.push_block_entity(bes, pos, e, f));
        }
        self.blocks.drawn = drawn;
        self.hand_from = std::array::from_fn(|i| self.instances[i].len());
    }

    /// Light of a block entity (§ 2): that of its position, block read when the state is emitted.
    fn block_light(&self, f: &BlockFrame<'_>, pos: [i32; 3], state: u32) -> u32 {
        let (b, s) = (f.light)(pos[0], pos[1], pos[2]);
        let emission = self.source.states.get(state as usize).map_or(0, |i| i.light_emission);
        pack(b.max(emission), s)
    }

    fn push_block_entity(&mut self, bes: &BlockEntities, pos: [i32; 3], e: &BlockEntity, f: &BlockFrame<'_>) -> bool {
        let rel = Vec3::new((pos[0] - f.origin[0]) as f32, pos[1] as f32, (pos[2] - f.origin[1]) as f32);
        let t = Mat4::from_translation(rel);
        let light = self.block_light(f, pos, e.state);
        match e.kind {
            Kind::Chest | Kind::TrappedChest | Kind::EnderChest => self.push_chest_be(bes, pos, e, t, light, f),
            Kind::ShulkerBox => self.push_shulker(e, t, light, f.pt),
            Kind::Bell => self.push_bell(e, t, light, f.pt),
            Kind::Sign | Kind::HangingSign => self.push_sign(pos, e, t, light, f),
            Kind::Banner => self.push_banner(pos, e, t, light, f),
            Kind::Skull => self.push_skull(e, t, light, f),
            Kind::EnchantingTable => self.push_enchanting(e, t, light, f.pt),
            Kind::Lectern => self.push_lectern(e, t, light),
            Kind::DecoratedPot => self.push_pot(e, t, light, f),
            Kind::CopperGolemStatue => self.push_statue(e, t, light),
            Kind::Conduit => self.push_conduit(e, t, light, f.pt),
            Kind::Spawner => match &e.data {
                Data::Spawner(Some(id), ..) => self.push_spawn_entity(id, e, t, light, f.pt),
                _ => false,
            },
            Kind::TrialSpawner => match &e.data {
                Data::Trial(Some(id), _) if e.props.active => self.push_spawn_entity(id, e, t, light, f.pt),
                _ => false,
            },
            Kind::Campfire => self.push_campfire(e, t, light, f),
            Kind::Shelf => self.push_shelf(e, t, light, f),
            Kind::Vault => self.push_vault(e, t, light, f),
            Kind::Brushable => self.push_brushable(pos, e, t, f),
            Kind::Beacon => self.push_beacon(pos, e, t, f),
            Kind::TestInstance => self.push_test_beam(pos, bes.test_beam(e), t, f),
            Kind::EndPortal => self.push_end_portal(t),
            Kind::EndGateway => self.push_gateway(pos, e, t, f),
            Kind::Piston => self.push_piston(e, t, light, f.pt, (f.biome_tints)(pos[0], pos[1], pos[2])),
            // Tracked only for its client tick (harmful gas): no rendering of its own.
            Kind::PotentSulfur => false,
        }
    }

    /// Chests (§ 4.1): single layer or half of a double, lid `1 − (1 − t)³`.
    fn push_chest_be(
        &mut self,
        bes: &BlockEntities,
        pos: [i32; 3],
        e: &BlockEntity,
        t: Mat4,
        light: u32,
        f: &BlockFrame<'_>,
    ) -> bool {
        let half = if e.kind == Kind::EnderChest { 0 } else { usize::from(e.props.chest.min(2)) };
        let layer =
            ["minecraft:chest#main", "minecraft:double_chest_left#main", "minecraft:double_chest_right#main"][half];
        let sort = match e.kind {
            Kind::EnderChest => 7,
            Kind::Chest if e.sub > 0 => usize::from(e.sub.min(4)),
            _ if self.blocks.christmas == Some(true) => 6,
            Kind::TrappedChest => 5,
            _ => 0,
        };
        let open_at = |b: &BlockEntity| b.anim.open_o + (b.anim.open - b.anim.open_o) * f.pt;
        let mut open = open_at(e);
        let mut light = light;
        if half > 0 {
            let other_dir = if half == 1 { clockwise(e.props.facing) } else { counter_clockwise(e.props.facing) };
            let d = step(other_dir);
            let other = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
            if let Some(o) = bes.get(other).filter(|o| o.kind == e.kind) {
                open = open.max(open_at(o));
                let ol = self.block_light(f, other, o.state);
                light = (light & 0xff).max(ol & 0xff) | (light & 0xff00).max(ol & 0xff00);
            }
        }
        let o = 1.0 - (1.0 - open).powi(3);
        let m = t
            * Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5))
            * Mat4::from_rotation_y(deg(-y_rot(e.props.facing)))
            * Mat4::from_translation(Vec3::new(-0.5, 0.0, -0.5));
        let angle = -o * PI / 2.0;
        self.push_named(layer, CHEST_SPRITES[sort][half], m, PASS_CUTOUT_CULL, 0xffff_ffff, light, |p| {
            p.set("lid", |q| q[3] = angle);
            p.set("lock", |q| q[3] = angle);
        })
    }

    /// Shulker box (§ 4.2): lid rising half a block while rotating.
    pub(super) fn push_shulker(&mut self, e: &BlockEntity, t: Mat4, light: u32, pt: f32) -> bool {
        let rot = match e.props.facing {
            0 => Mat4::from_rotation_x(PI),
            2 => Mat4::from_euler(EulerRot::XYZ, PI / 2.0, 0.0, PI),
            3 => Mat4::from_rotation_x(PI / 2.0),
            4 => Mat4::from_euler(EulerRot::XYZ, PI / 2.0, 0.0, PI / 2.0),
            5 => Mat4::from_euler(EulerRot::XYZ, PI / 2.0, 0.0, -PI / 2.0),
            _ => Mat4::IDENTITY,
        };
        let m = t
            * Mat4::from_translation(Vec3::splat(0.5))
            * Mat4::from_scale(Vec3::splat(0.9995))
            * rot
            * Mat4::from_scale(Vec3::new(1.0, -1.0, -1.0))
            * Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0));
        let p = e.anim.open_o + (e.anim.open - e.anim.open_o) * pt;
        let sprite = SHULKER_SPRITES[usize::from(e.sub.min(16))];
        self.push_named("minecraft:shulker_box#main", sprite, m, 0, 0xffff_ffff, light, |q| {
            q.set("lid", |l| {
                l[1] = 24.0 - 8.0 * p;
                l[4] = 1.5 * PI * p;
            });
        })
    }

    /// Bell (§ 4.3): body swinging according to the struck face.
    pub(super) fn push_bell(&mut self, e: &BlockEntity, t: Mat4, light: u32, pt: f32) -> bool {
        let (mut x, mut z) = (0.0, 0.0);
        if e.anim.shaking {
            let tau = e.anim.ticks as f32 + pt;
            let b = (tau / PI).sin() / (4.0 + tau / 3.0);
            match e.anim.dir {
                2 => x = -b,
                3 => x = b,
                5 => z = -b,
                4 => z = b,
                _ => {}
            }
        }
        self.push_named("minecraft:bell#main", "entity/bell/bell_body", t, PASS_SOLID, 0xffff_ffff, light, |p| {
            p.set("bell_body", |q| {
                q[3] = x;
                q[5] = z;
            });
        })
    }

    /// Signs (§ 3): the board is a block model; only the matrices of the two faces
    /// are read here, the text is drawn separately.
    fn push_sign(&mut self, pos: [i32; 3], e: &BlockEntity, t: Mat4, light: u32, f: &BlockFrame<'_>) -> bool {
        if !matches!(e.data, Data::Sign(_)) {
            return false;
        }
        let a = if e.wall { y_rot(e.props.facing) } else { f32::from(e.props.rotation) * 22.5 };
        let hanging = e.kind == Kind::HangingSign;
        let face = |back: bool| {
            let flip = if back { Mat4::from_rotation_y(PI) } else { Mat4::IDENTITY };
            if hanging {
                t * Mat4::from_translation(Vec3::new(0.5, 0.9375, 0.5))
                    * Mat4::from_rotation_y(deg(-a))
                    * Mat4::from_translation(Vec3::new(0.0, -0.3125, 0.0))
                    * flip
                    * Mat4::from_translation(Vec3::new(0.0, -0.32, 0.073))
                    * Mat4::from_scale(Vec3::new(0.014_062_5, -0.014_062_5, 0.014_062_5))
            } else {
                let wall =
                    if e.wall { Mat4::from_translation(Vec3::new(0.0, -0.3125, -0.4375)) } else { Mat4::IDENTITY };
                t * Mat4::from_translation(Vec3::splat(0.5))
                    * Mat4::from_rotation_y(deg(-a))
                    * wall
                    * flip
                    * Mat4::from_translation(Vec3::new(0.0, 1.0 / 3.0, 0.046_666_667))
                    * Mat4::from_scale(Vec3::new(1.0 / 96.0, -1.0 / 96.0, 1.0 / 96.0))
            }
        };
        let c = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
        let d2 = (0..3).map(|i| (c[i] - f.cam[i]).powi(2)).sum();
        self.blocks.texts.push(SignDraw { pos, faces: [face(false), face(true)], light, hanging, d2 });
        true
    }

    /// Banners (§ 4.4): pole and bar, then waving flag (background, dyed base, patterns).
    pub(super) fn push_banner(
        &mut self,
        pos: [i32; 3],
        e: &BlockEntity,
        t: Mat4,
        light: u32,
        f: &BlockFrame<'_>,
    ) -> bool {
        let a = if e.wall { y_rot(e.props.facing) } else { f32::from(e.props.rotation) * 22.5 };
        let m = t
            * Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5))
            * Mat4::from_rotation_y(deg(-a))
            * Mat4::from_scale(Vec3::new(2.0 / 3.0, -2.0 / 3.0, -2.0 / 3.0));
        let (main, flag) = if e.wall {
            ("minecraft:wall_banner#main", "minecraft:wall_banner#flag")
        } else {
            ("minecraft:standing_banner#main", "minecraft:standing_banner#flag")
        };
        let white = 0xffff_ffff;
        if !self.push_named(main, "entity/banner/banner_base", m, PASS_SOLID, white, light, |_| {}) {
            return false;
        }
        let k = i64::from(pos[0]) * 7 + i64::from(pos[1]) * 9 + i64::from(pos[2]) * 13 + f.game_time;
        let phase = (k.rem_euclid(100) as f32 + f.pt) / 100.0;
        let x_rot = (-0.0125 + 0.01 * (2.0 * PI * phase).cos()) * PI;
        let wave = |p: &mut Pose<'_>| p.set("flag", |q| q[3] = x_rot);
        let argb = |d: u8| {
            let [r, g, b] = dye_rgb(d);
            0xff00_0000 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
        };
        self.push_named(flag, "entity/banner/banner_base", m, PASS_SOLID, white, light, wave);
        self.push_named(flag, "entity/banner/base", m, 1, argb(e.sub), light, wave);
        if let Data::Banner(patterns) = &e.data {
            for (sprite, color) in patterns.iter().take(16) {
                self.push_named(flag, sprite, m, 1, argb(*color), light, wave);
            }
        }
        true
    }

    /// Heads and skulls (§ 4.5): layer of the type, profile skin for the player head, dragon jaw
    /// and piglin ears animated when the block is powered.
    pub(super) fn push_skull(&mut self, e: &BlockEntity, t: Mat4, light: u32, f: &BlockFrame<'_>) -> bool {
        let pt = f.pt;
        let kind = usize::from(e.sub.min(6));
        let layer = [
            "minecraft:skeleton_skull#main",
            "minecraft:wither_skeleton_skull#main",
            "minecraft:zombie_head#main",
            "minecraft:creeper_head#main",
            "minecraft:dragon_skull#main",
            "minecraft:piglin_head#main",
            "minecraft:player_head#main",
        ][kind];
        let m = if e.wall {
            let d = step(e.props.facing);
            t * Mat4::from_translation(Vec3::new(0.5 - 0.25 * d[0] as f32, 0.25, 0.5 - 0.25 * d[2] as f32))
                * Mat4::from_rotation_y(deg(-y_rot(opposite(e.props.facing))))
                * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0))
        } else {
            t * Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5))
                * Mat4::from_rotation_y(deg(-f32::from(e.props.rotation) * 22.5))
                * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0))
        };
        let anim = e.anim.ticks as f32 + if e.props.powered { pt } else { 0.0 };
        let pose = |p: &mut Pose<'_>| match kind {
            4 => p.set("jaw", |q| q[3] = ((anim * PI * 0.2).sin() + 1.0) * 0.2),
            5 => {
                p.set("left_ear", |q| q[5] = -((anim * PI * 0.2 * 1.2).cos() + 2.5) * 0.2);
                p.set("right_ear", |q| q[5] = ((anim * PI * 0.2).cos() + 2.5) * 0.2);
            }
            _ => {}
        };
        let Some(&l) = self.layer_index.get(layer) else { return false };
        if kind == 6 {
            // Profile: downloaded skin, otherwise the default skin of its UUID; without a profile: slim Steve.
            let (rect, pass) = match &e.data {
                Data::Skull(Some(p)) => {
                    let custom = head_key(p).and_then(|k| self.custom_skins.get(&k)).map(|&i| self.arena[i][0].rect);
                    let rect = custom.or_else(|| {
                        let i = usize::from(default_skin(p.uuid.unwrap_or(0)));
                        let dir = if i < 9 { "slim" } else { "wide" };
                        self.atlas.rect(&format!("entity/player/{dir}/{}", SKINS[i % 9]))
                    });
                    (rect, 1)
                }
                _ => (self.atlas.rect("entity/player/slim/steve"), 0),
            };
            let Some(rect) = rect else { return false };
            let m = if pass == 0 { skull_layering(f) * m } else { m };
            self.push_layer(l, rect, m, pass, 0xffff_ffff, light, pose);
            return true;
        }
        let sprite = [
            "entity/skeleton/skeleton",
            "entity/skeleton/wither_skeleton",
            "entity/zombie/zombie",
            "entity/creeper/creeper",
            "entity/enderdragon/dragon",
            "entity/piglin/piglin",
        ][kind];
        let Some(rect) = self.atlas.rect(sprite) else { return false };
        self.push_layer(l, rect, skull_layering(f) * m, 0, 0xffff_ffff, light, pose);
        true
    }

    /// Book (§ 4.13): covers, pages and turning pages depending on the effective opening.
    pub(super) fn push_book(&mut self, m: Mat4, light: u32, tau: f32, f1: f32, f2: f32, open: f32) -> bool {
        let o = ((0.02 * tau).sin() * 0.1 + 1.25) * open;
        let x = o.sin();
        self.push_named(
            "minecraft:book#main",
            "entity/enchantment/enchanting_table_book",
            m,
            PASS_SOLID,
            0xffff_ffff,
            light,
            |p| {
                p.set("left_lid", |q| q[4] = PI + o);
                p.set("right_lid", |q| q[4] = -o);
                p.set("left_pages", |q| {
                    q[4] = o;
                    q[0] = x;
                });
                p.set("right_pages", |q| {
                    q[4] = -o;
                    q[0] = x;
                });
                p.set("flip_page1", |q| {
                    q[4] = o - o * 2.0 * f1;
                    q[0] = x;
                });
                p.set("flip_page2", |q| {
                    q[4] = o - o * 2.0 * f2;
                    q[0] = x;
                });
            },
        )
    }

    /// Enchanting table (§ 4.13): book that floats, turns towards the player and opens.
    fn push_enchanting(&mut self, e: &BlockEntity, t: Mat4, light: u32, pt: f32) -> bool {
        let a = &e.anim;
        let b = &a.book;
        let tau = a.ticks as f32 + pt;
        let mut diff = b[0] - b[1];
        while diff >= PI {
            diff -= 2.0 * PI;
        }
        while diff < -PI {
            diff += 2.0 * PI;
        }
        let rot = b[1] + diff * pt;
        let flip = b[4] + (b[3] - b[4]) * pt;
        let frac = |v: f32| v - v.floor();
        let f1 = (frac(flip + 0.25) * 1.6 - 0.3).clamp(0.0, 1.0);
        let f2 = (frac(flip + 0.75) * 1.6 - 0.3).clamp(0.0, 1.0);
        let open = a.open_o + (a.open - a.open_o) * pt;
        let m = t
            * Mat4::from_translation(Vec3::new(0.5, 0.75, 0.5))
            * Mat4::from_translation(Vec3::new(0.0, 0.1 + (tau * 0.1).sin() * 0.01, 0.0))
            * Mat4::from_rotation_y(-rot)
            * Mat4::from_rotation_z(deg(80.0));
        self.push_book(m, light, tau, f1, f2, open)
    }

    /// Lectern (§ 4.13): fixed open book, if there is one.
    fn push_lectern(&mut self, e: &BlockEntity, t: Mat4, light: u32) -> bool {
        if !e.props.has_book {
            return false;
        }
        let m = t
            * Mat4::from_translation(Vec3::new(0.5, 1.0625, 0.5))
            * Mat4::from_rotation_y(deg(-y_rot(clockwise(e.props.facing))))
            * Mat4::from_rotation_z(deg(67.5))
            * Mat4::from_translation(Vec3::new(0.0, -0.125, 0.0));
        self.push_book(m, light, 0.0, 0.1, 0.9, 1.2)
    }

    /// Sprite of a pot face: sherd pattern, otherwise plain face.
    fn pot_sprite(&mut self, item: Option<i32>) -> String {
        let name = item
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| self.item_names()?.get(i).cloned())
            .unwrap_or_default();
        match name.strip_prefix("minecraft:").and_then(|n| n.strip_suffix("_pottery_sherd")) {
            Some(p) => format!("entity/decorated_pot/{p}_pottery_pattern"),
            None => "entity/decorated_pot/decorated_pot_side".to_owned(),
        }
    }

    /// Decorated pot (§ 4.7): base, then each face with its pattern; Y wobble after a
    /// block event (both styles, like the game).
    pub(super) fn push_pot(&mut self, e: &BlockEntity, t: Mat4, light: u32, f: &BlockFrame<'_>) -> bool {
        let around = |m: Mat4, p: Vec3| Mat4::from_translation(p) * m * Mat4::from_translation(-p);
        let mut m = t * around(Mat4::from_rotation_y(deg(180.0 - y_rot(e.props.facing))), Vec3::splat(0.5));
        if let Some((start, dur)) = e.anim.wobble {
            let p = ((f.game_time - start) as f32 + f.pt) / dur as f32;
            if (0.0..=1.0).contains(&p) {
                let turn = (-p * 3.0 * PI).sin() * 0.125 * (1.0 - p);
                m *= around(Mat4::from_rotation_y(turn), Vec3::new(0.5, 0.0, 0.5));
            }
        }
        let white = 0xffff_ffff;
        if !self.push_named(
            "minecraft:decorated_pot_base#main",
            "entity/decorated_pot/decorated_pot_base",
            m,
            PASS_SOLID,
            white,
            light,
            |_| {},
        ) {
            return false;
        }
        let faces = match e.data {
            Data::Pot(f) => f,
            _ => [None; 4],
        };
        const SIDES: [&str; 4] = ["back", "left", "right", "front"];
        for (i, part) in SIDES.into_iter().enumerate() {
            let sprite = self.pot_sprite(faces[i]);
            self.push_named("minecraft:decorated_pot_sides#main", &sprite, m, PASS_SOLID, white, light, |p| {
                for other in SIDES {
                    if other != part {
                        p.hide(other);
                    }
                }
            });
        }
        true
    }

    /// Copper golem statue (§ 4.15): layer of the pose, oxidation texture.
    pub(super) fn push_statue(&mut self, e: &BlockEntity, t: Mat4, light: u32) -> bool {
        let layer = [
            "minecraft:copper_golem#main",
            "minecraft:copper_golem_running#main",
            "minecraft:copper_golem_sitting#main",
            "minecraft:copper_golem_star#main",
        ][usize::from(e.props.pose.min(3))];
        let sprite = [
            "entity/copper_golem/copper_golem",
            "entity/copper_golem/copper_golem_exposed",
            "entity/copper_golem/copper_golem_weathered",
            "entity/copper_golem/copper_golem_oxidized",
        ][usize::from(e.sub.min(3))];
        let m = t
            * Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5))
            * Mat4::from_rotation_y(deg(-y_rot(opposite(e.props.facing))));
        self.push_named(layer, sprite, m, 0, 0xffff_ffff, light, |p| {
            p.set("root", |q| {
                q[1] = 0.0;
                q[5] = PI;
            });
        })
    }

    /// Conduit (§ 4.12): shell only at rest; active, cage that rotates and floats, winds, eye
    /// facing the camera.
    pub(super) fn push_conduit(&mut self, e: &BlockEntity, t: Mat4, light: u32, pt: f32) -> bool {
        let a = &e.anim;
        let angle = (a.spin as f32 + if a.active { pt } else { 0.0 }) * -0.0375;
        let white = 0xffff_ffff;
        let center = t * Mat4::from_translation(Vec3::splat(0.5));
        if !a.active {
            let m = center * Mat4::from_rotation_y(deg(angle));
            return self.push_named(
                "minecraft:conduit#shell",
                "entity/conduit/base",
                m,
                PASS_SOLID,
                white,
                light,
                |_| {},
            );
        }
        let tau = a.ticks as f32 + pt;
        let mut hh = (0.1 * tau).sin() / 2.0 + 0.5;
        hh = hh * hh + hh;
        let lift = t * Mat4::from_translation(Vec3::new(0.5, 0.3 + 0.2 * hh, 0.5));
        let axis = Vec3::new(0.5, 1.0, 0.5).normalize();
        self.push_named(
            "minecraft:conduit#cage",
            "entity/conduit/cage",
            lift * Mat4::from_axis_angle(axis, angle),
            0,
            white,
            light,
            |_| {},
        );
        let phase = (a.ticks / 66).rem_euclid(3);
        let (rot, sprite) = match phase {
            1 => (Mat4::from_rotation_x(deg(90.0)), "entity/conduit/wind_vertical"),
            2 => (Mat4::from_rotation_z(deg(90.0)), "entity/conduit/wind"),
            _ => (Mat4::IDENTITY, "entity/conduit/wind"),
        };
        self.push_named("minecraft:conduit#wind", sprite, center * rot, 0, white, light, |_| {});
        let second = center * Mat4::from_scale(Vec3::splat(0.875)) * Mat4::from_euler(EulerRot::XYZ, PI, 0.0, PI);
        self.push_named("minecraft:conduit#wind", sprite, second, 0, white, light, |_| {});
        let eye = lift
            * Mat4::from_scale(Vec3::splat(0.5))
            * self.cam_rot
            * Mat4::from_rotation_z(PI)
            * Mat4::from_rotation_y(PI)
            * Mat4::from_scale(Vec3::splat(4.0 / 3.0));
        let sprite = if a.hunting { "entity/conduit/open_eye" } else { "entity/conduit/closed_eye" };
        self.push_named("minecraft:conduit#eye", sprite, eye, 0, white, light, |_| {});
        true
    }

    /// Display entity of a spawner (§ 4.14): layers of the kind at the definition pose,
    /// spinning on itself.
    fn push_spawn_entity(&mut self, id: &str, e: &BlockEntity, t: Mat4, light: u32, pt: f32) -> bool {
        let id = if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") };
        let Some(kind_id) =
            self.source.registry("minecraft:entity_type").and_then(|r| r.entries.iter().position(|n| *n == id))
        else {
            return false;
        };
        let kind = id.trim_start_matches("minecraft:").to_owned();
        let layers = self.resolve(kind_id as i32, &kind, false, 0);
        if self.arena[layers].is_empty() {
            return false;
        }
        let dims = self.dims.get(kind_id).copied().unwrap_or([0.6, 1.8, 1.6, 0.0]);
        let big = dims[0].max(dims[1]);
        let s = if big > 1.0 { 0.53125 / big } else { 0.53125 };
        let spin = (e.anim.spin_o + (e.anim.spin - e.anim.spin_o) * f64::from(pt)) as f32 * 10.0;
        let m = t
            * Mat4::from_translation(Vec3::new(0.5, 0.4, 0.5))
            * Mat4::from_rotation_y(deg(spin))
            * Mat4::from_translation(Vec3::new(0.0, -0.2, 0.0))
            * Mat4::from_rotation_x(deg(-30.0))
            * Mat4::from_scale(Vec3::splat(s))
            // Frame of the entity model (living with zero yaw).
            * Mat4::from_rotation_y(PI)
            * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0))
            * Mat4::from_translation(Vec3::new(0.0, -1.501, 0.0));
        for li in 0..self.arena[layers].len() {
            let r = self.arena[layers][li];
            let pass = r.pass.index();
            let l = if r.pass == Pass::Eyes { FULL } else { light };
            self.push_layer(r.layer, r.rect, m, pass, 0xffff_ffff, l, |_| {});
        }
        true
    }

    /// Item stack placed (campfire, shelf, vault, brushable block) in the given context.
    fn push_block_item(&mut self, f: &BlockFrame<'_>, s: Stack, ctx: usize, m: Mat4, light: u32) -> bool {
        let Some(models) = f.items else { return false };
        let stack = ItemStack { item: s.item, count: s.count, added: Vec::new(), removed: Vec::new() };
        self.push_stack(models, &stack, Some(ctx), None, m, light)
    }

    /// Box (bottom, top in y) of the model of a stack in the given context.
    fn item_bounds(&mut self, f: &BlockFrame<'_>, s: Stack, ctx: usize) -> Option<(f32, f32)> {
        let models = f.items?;
        let stack = ItemStack { item: s.item, count: s.count, added: Vec::new(), removed: Vec::new() };
        let source = self.source.clone();
        let info = usize::try_from(stack.item).ok().and_then(|i| source.items.get(i));
        let view = crate::stack::StackRef {
            stack: &stack,
            info,
            ids: &self.ids,
            data: &source,
            hold: crate::stack::Hold::NONE,
        };
        let model = view.model()?;
        let name = usize::try_from(stack.item)
            .ok()
            .and_then(|i| items_in(&source, self.item_idx())?.get(i))
            .map_or("", String::as_str);
        let env = HeldEnv(rc_assets::model::DISPLAY_CONTEXTS[ctx]);
        let (mut layers, mut key) = (Vec::new(), Vec::new());
        models.layers(name, &model, &view, &env, 0, &mut layers, &mut key);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for l in &layers {
            let b = models.baked(l.model).unwrap_or(&models.missing);
            let d =
                Mat4::from_cols_array_2d(&b.displays[ctx]).transpose() * Mat4::from_cols_array_2d(&l.local).transpose();
            for q in b.quads() {
                for p in q.pos {
                    let v = d.transform_point3(Vec3::from(p));
                    lo = lo.min(v.y);
                    hi = hi.max(v.y);
                }
            }
        }
        (lo <= hi).then_some((lo, hi))
    }

    /// Campfire (§ 4.8): four slots rotated by a quarter turn, "fixed" context.
    fn push_campfire(&mut self, e: &BlockEntity, t: Mat4, light: u32, f: &BlockFrame<'_>) -> bool {
        let Data::Items(slots, _) = &e.data else { return false };
        let base = match e.props.facing {
            4 => 1,
            2 => 2,
            5 => 3,
            _ => 0,
        };
        const DIRS: [u8; 4] = [3, 4, 2, 5];
        let mut drawn = false;
        for (i, s) in slots.iter().enumerate().take(4) {
            let Some(s) = *s else { continue };
            let d = DIRS[(i + base) % 4];
            let m = t
                * Mat4::from_translation(Vec3::new(0.5, 0.449_218_75, 0.5))
                * Mat4::from_rotation_y(deg(-y_rot(d)))
                * Mat4::from_rotation_x(deg(90.0))
                * Mat4::from_translation(Vec3::new(-0.3125, -0.3125, 0.0))
                * Mat4::from_scale(Vec3::splat(0.375));
            drawn |= self.push_block_item(f, s, 7, m, light);
        }
        drawn
    }

    /// Shelf (§ 4.8): three slots, "on shelf" context, placed on the bottom of their
    /// model (or centred).
    fn push_shelf(&mut self, e: &BlockEntity, t: Mat4, light: u32, f: &BlockFrame<'_>) -> bool {
        let Data::Items(slots, bottom) = &e.data else { return false };
        let r = deg(-y_rot(e.props.facing));
        let b = if *bottom { -0.25 } else { 0.0 };
        let mut drawn = false;
        for (i, s) in slots.iter().enumerate().take(3) {
            let Some(s) = *s else { continue };
            let Some((lo, hi)) = self.item_bounds(f, s, 8) else { continue };
            let lift = if *bottom { -lo } else { -lo - (hi - lo) / 2.0 };
            let m = t
                * Mat4::from_translation(Vec3::splat(0.5))
                * Mat4::from_rotation_y(r)
                * Mat4::from_translation(Vec3::new((i as f32 - 1.0) * 0.3125, b, -0.25))
                * Mat4::from_scale(Vec3::splat(0.25))
                * Mat4::from_translation(Vec3::new(0.0, lift, 0.0));
            drawn |= self.push_block_item(f, s, 8, m, light);
        }
        drawn
    }

    /// Vault (§ 4.8): displayed item that rotates, "ground" context.
    fn push_vault(&mut self, e: &BlockEntity, t: Mat4, light: u32, f: &BlockFrame<'_>) -> bool {
        let Data::Vault(Some(s)) = e.data else { return false };
        let spin = (e.anim.spin_o + (e.anim.spin - e.anim.spin_o) * f64::from(f.pt)) as f32;
        let m = t * Mat4::from_translation(Vec3::new(0.5, 0.4, 0.5)) * Mat4::from_rotation_y(deg(spin));
        self.push_block_item(f, s, 1, m, light)
    }

    /// Brushable block (§ 4.8): item that comes out of the struck side depending on progress,
    /// light of the neighbouring cell on that side.
    fn push_brushable(&mut self, pos: [i32; 3], e: &BlockEntity, t: Mat4, f: &BlockFrame<'_>) -> bool {
        let Data::Brushable(Some(s), Some(dir)) = e.data else { return false };
        if e.props.dusted == 0 {
            return false;
        }
        let c = f32::from(e.props.dusted) / 10.0 * 0.75;
        let (mut tx, mut ty, mut tz) = (0.5, 0.0, 0.5);
        match dir {
            5 => tx = 0.73 + c,
            4 => tx = 0.25 - c,
            1 => ty = 0.25 + c,
            0 => ty = -0.23 - c,
            2 => tz = 0.25 - c,
            _ => tz = 0.73 + c,
        }
        let turn = 11.0 + if matches!(dir, 4 | 5) { 90.0 } else { 0.0 };
        let m = t
            * Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0))
            * Mat4::from_translation(Vec3::new(tx, ty, tz))
            * Mat4::from_rotation_y(deg(75.0))
            * Mat4::from_rotation_y(deg(turn))
            * Mat4::from_scale(Vec3::splat(0.5));
        let d = step(dir);
        let (b, sk) = (f.light)(pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]);
        self.push_block_item(f, s, 7, m, pack(b, sk))
    }

    /// Beam (§ 4.9): rotating core (opaque) and halo (translucent) from `start` to
    /// `start + height`, from the centre of block `base` (camera frame).
    #[allow(clippy::too_many_arguments)]
    fn push_block_beam(
        &mut self,
        base: Vec3,
        tex: u32,
        t: f32,
        tex_scale: f32,
        start: f32,
        height: f32,
        rgb: [u8; 3],
        radii: (f32, f32),
    ) {
        let c = Vec3::new(0.5, 0.0, 0.5) + base;
        let scroll = if height < 0.0 { t } else { -t };
        let off = {
            let v = scroll * 0.2 - (scroll * 0.1).floor();
            v - v.floor()
        };
        let (y0, y1) = (start, start + height);
        let color = |a: u8| u32::from(rgb[0]) | u32::from(rgb[1]) << 8 | u32::from(rgb[2]) << 16 | u32::from(a) << 24;
        let quads = |out: &mut Vec<BeamVertex>, pts: [(f32, f32); 4], v_top: f32, v_bot: f32, col: u32| {
            for (a, b) in [(0, 1), (3, 2), (1, 3), (2, 0)] {
                let (p, q) = (pts[a], pts[b]);
                let v = |x: f32, y: f32, z: f32, u: f32, w: f32| BeamVertex {
                    pos: [c.x + x, c.y + y, c.z + z],
                    uv: [u, w],
                    color: col,
                    tex,
                };
                let corners = [
                    v(p.0, y1, p.1, 1.0, v_top),
                    v(p.0, y0, p.1, 1.0, v_bot),
                    v(q.0, y0, q.1, 0.0, v_bot),
                    v(q.0, y1, q.1, 0.0, v_top),
                ];
                out.extend([0, 1, 2, 0, 2, 3].map(|k| corners[k]));
            }
        };
        // Core: rotated square; vertices (0, r), (r, 0), (−r, 0), (0, −r).
        let (s, co) = deg(t * 2.25 - 45.0).sin_cos();
        let rot = |x: f32, z: f32| (x * co + z * s, -x * s + z * co);
        let r = radii.0;
        let core = [rot(0.0, r), rot(r, 0.0), rot(-r, 0.0), rot(0.0, -r)];
        let v_bot = -1.0 + off;
        quads(&mut self.blocks.fx.beams, core, height * tex_scale * (0.5 / r) + v_bot, v_bot, color(255));
        let g = radii.1;
        let halo = [(-g, -g), (g, -g), (-g, g), (g, g)];
        quads(&mut self.blocks.fx.halos, halo, height * tex_scale + v_bot, v_bot, color(32));
    }

    /// Beacon (§ 4.9): sections stacked from the block, the last one 2048 high; radius
    /// widened in the distance.
    fn push_beacon(&mut self, pos: [i32; 3], e: &BlockEntity, t: Mat4, f: &BlockFrame<'_>) -> bool {
        if e.anim.levels <= 0 || e.anim.beam.is_empty() {
            return false;
        }
        let base = t.w_axis.truncate();
        let time = f.game_time.rem_euclid(40) as f32 + f.pt;
        let k = beam_scale(pos, f);
        let mut y = 0.0;
        let n = e.anim.beam.len();
        for (i, &(rgb, h)) in e.anim.beam.iter().enumerate() {
            let h = if i + 1 == n { 2048.0 } else { h as f32 };
            self.push_block_beam(base, 0, time, 1.0, y, h, rgb, (0.2 * k, 0.25 * k));
            y += h;
        }
        true
    }

    /// Test instance: state beam of a single section (2048 high), like the beacon.
    fn push_test_beam(&mut self, pos: [i32; 3], rgb: Option<[u8; 3]>, t: Mat4, f: &BlockFrame<'_>) -> bool {
        let Some(rgb) = rgb else { return false };
        let time = f.game_time.rem_euclid(40) as f32 + f.pt;
        let k = beam_scale(pos, f);
        self.push_block_beam(t.w_axis.truncate(), 0, time, 1.0, 0.0, 2048.0, rgb, (0.2 * k, 0.25 * k));
        true
    }

    /// Faces of a portal cube (§ 4.10, 4.11) in the camera frame.
    fn push_portal_faces(&mut self, base: Vec3, y0: f32, y1: f32, faces: u8, layers: u32) {
        let v = |x: f32, y: f32, z: f32| PortalVertex { pos: [base.x + x, base.y + y, base.z + z], layers };
        let quad = |q: [PortalVertex; 4], out: &mut Vec<PortalVertex>| out.extend([0, 1, 2, 0, 2, 3].map(|k| q[k]));
        let out = &mut self.blocks.fx.portals;
        let defs: [(u8, [PortalVertex; 4]); 6] = [
            (0, [v(0.0, y0, 0.0), v(1.0, y0, 0.0), v(1.0, y0, 1.0), v(0.0, y0, 1.0)]),
            (1, [v(0.0, y1, 0.0), v(0.0, y1, 1.0), v(1.0, y1, 1.0), v(1.0, y1, 0.0)]),
            (2, [v(0.0, y0, 0.0), v(0.0, y1, 0.0), v(1.0, y1, 0.0), v(1.0, y0, 0.0)]),
            (3, [v(0.0, y0, 1.0), v(1.0, y0, 1.0), v(1.0, y1, 1.0), v(0.0, y1, 1.0)]),
            (4, [v(0.0, y0, 0.0), v(0.0, y0, 1.0), v(0.0, y1, 1.0), v(0.0, y1, 0.0)]),
            (5, [v(1.0, y0, 0.0), v(1.0, y1, 0.0), v(1.0, y1, 1.0), v(1.0, y0, 1.0)]),
        ];
        for (d, q) in defs {
            if faces & (1 << d) != 0 {
                quad(q, out);
            }
        }
    }

    /// End portal (§ 4.10): top and bottom faces, at y = 0.375 and 0.75.
    fn push_end_portal(&mut self, t: Mat4) -> bool {
        self.push_portal_faces(t.w_axis.truncate(), 0.375, 0.75, 0b11, 15);
        true
    }

    /// End gateway (§ 4.11): cube with visible faces, spawn or
    /// cooldown beam.
    fn push_gateway(&mut self, pos: [i32; 3], e: &BlockEntity, t: Mat4, f: &BlockFrame<'_>) -> bool {
        let mut faces = 0u8;
        for d in 0..6u8 {
            let s = step(d);
            let n = (f.state)(pos[0] + s[0], pos[1] + s[1], pos[2] + s[2]);
            if !self.full_cube.get(n as usize).copied().unwrap_or(false) {
                faces |= 1 << d;
            }
        }
        let base = t.w_axis.truncate();
        self.push_portal_faces(base, 0.0, 1.0, faces, 16);
        let a = &e.anim;
        let (p, rgb, max) = if a.age < 200 {
            (((a.age as f32 + f.pt) / 200.0).clamp(0.0, 1.0), [0xC7, 0x4E, 0xBD], (f.top - pos[1]) as f32)
        } else if a.cooldown > 0 {
            (1.0 - ((a.cooldown as f32 - f.pt) / 40.0).clamp(0.0, 1.0), [0x89, 0x32, 0xB8], 50.0)
        } else {
            return true;
        };
        let s = (PI * p).sin();
        let h = (s * max).floor();
        if h > 0.0 {
            let time = f.game_time.rem_euclid(40) as f32 + f.pt;
            self.push_block_beam(base, 1, time, s, -h, 2.0 * h, rgb, (0.15, 0.175));
        }
        true
    }

    /// Moving piston block (§ 4.17), offset according to progress: head (short until
    /// mid-course), retracting base (short head past mid-course and extended base stationary), or
    /// moved block. Nothing for air. Tints: those of the biome of the moving block (`biome`),
    /// for the head as for the base.
    fn push_piston(&mut self, e: &BlockEntity, t: Mat4, light: u32, pt: f32, biome: [u32; 4]) -> bool {
        let Data::Piston { state, facing, extending, source, .. } = e.data else { return false };
        let tint = super::objects::BlockTint::World(biome);
        if state == 0 || self.source.states.get(state as usize).is_some_and(|i| i.has(rc_data::flags::AIR)) {
            return false;
        }
        let p = e.anim.open_o + (e.anim.open - e.anim.open_o) * pt.min(1.0);
        let k = if extending { p - 1.0 } else { 1.0 - p };
        let d = step(facing);
        let moved = t * Mat4::from_translation(Vec3::new(d[0] as f32, d[1] as f32, d[2] as f32) * k);
        let ps = &self.piston_states;
        // The game tests "progress ≤ 4", always true: the head is always drawn this way.
        if let Some((f, sticky)) = ps.head_of(state) {
            let head = ps.head(f, sticky, p <= 0.5);
            return self.push_block_tinted(head, moved, light, NEUTRAL, tint);
        }
        if source
            && !extending
            && let Some((f, sticky, ext)) = ps.base(state)
        {
            let head = ps.head(f, sticky, p >= 0.5);
            let drawn = self.push_block_tinted(head, moved, light, NEUTRAL, tint);
            return self.push_block_tinted(ext, t, light, NEUTRAL, tint) || drawn;
        }
        self.push_block_tinted(state, moved, light, NEUTRAL, tint)
    }
}
