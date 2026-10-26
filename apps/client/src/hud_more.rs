//! HUD additions (`docs/specs/hud.md` § 4.5 to 4.8): air bubbles, mount health,
//! jump bar, locator bar; state of the camera overlays (freeze, portal, nausea).

use crate::gui::Gui;
use crate::sky_mesh::JavaRandom;

/// Maximum air supply of the player (ticks).
const MAX_AIR: i32 = 300;

/// What the game tick provides to the HUD (local player).
pub struct TickIn {
    /// Air supply (entity data 1), `None` until it is received.
    pub air: Option<i32>,
    /// Eyes in water.
    pub eyes_in_water: bool,
    /// Mount of the local player, if they have one.
    pub vehicle: Option<VehicleIn>,
    /// Jump key held.
    pub jump: bool,
    /// Freeze ticks (entity data 7).
    pub frozen: Option<i32>,
    /// In a Nether portal this tick.
    pub in_portal: bool,
}

/// Nausea effect of the local player: remaining duration (−1 infinite), fade factor (before, after).
struct Nausea {
    duration: i32,
    factor_o: f32,
    factor: f32,
}

/// What the frame provides to the overlays.
pub struct FrameIn<'a> {
    pub pt: f32,
    pub first_person: bool,
    pub spectator: bool,
    /// Overlays of equipped items (`equippable.camera_overlay`).
    pub equipped: Vec<&'a str>,
    /// "Distortion effects" option (0..1).
    pub screen_effect_scale: f32,
    /// State of the block that obstructs the view (see [`HudMore::view_blocking`]).
    pub block: Option<usize>,
    /// Light at the eyes (brightness of the dimension) for the water tint.
    pub eye_brightness: f32,
    /// Rotation of the player (degrees): scrolling of the water texture.
    pub yaw: f32,
    pub pitch: f32,
    /// On fire (entity flag 0x01).
    pub on_fire: bool,
    pub hud_fov: f32,
    /// Spyglass (first person): mask scale.
    pub scope: Option<f32>,
}

/// What the app provides on every frame for the overlays.
pub struct SceneIn<'a> {
    pub data: &'a rc_data::VersionData,
    pub state_at: &'a dyn Fn(i32, i32, i32) -> u32,
    pub me: Option<&'a crate::entities::Entity>,
    /// Network id of the `equippable` component (overlays of worn stacks).
    pub equippable: Option<u16>,
    pub feet: [f64; 3],
    /// Eyes of the player (outside the detached camera).
    pub eye: [f64; 3],
    /// Rotation of the player, then of the camera (degrees).
    pub yaw: f32,
    pub pitch: f32,
    pub shot: (f32, f32),
    pub fov: f32,
    pub aspect: f32,
    pub hud_fov: f32,
    pub first_person: bool,
    pub spectator: bool,
    pub hud_hidden: bool,
    pub screen_effect_scale: f32,
    pub eye_brightness: f32,
    pub pt: f32,
    /// Target duration of the client tick (s): 50 ms, or slower under `/tick rate` (advancing world).
    pub tick_len: f32,
}

/// Camera and entities for the locator bar.
pub struct LocateIn<'a> {
    /// Position and rotation of the camera (degrees), field of view of the world, aspect ratio.
    pub cam: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    /// Local player (camera entity): position of the feet and UUID.
    pub me: [f64; 3],
    pub me_uuid: u128,
    /// Entity known by UUID: (feet position, eyes), interpolated.
    pub entity: &'a dyn Fn(u128) -> Option<([f64; 3], [f64; 3])>,
}

/// Tracked waypoint (`waypoint` packet).
struct Tracked {
    id: rc_proto_v777::waypoint::WaypointId,
    style: String,
    color: Option<u32>,
    pos: rc_proto_v777::waypoint::WaypointPos,
}

/// Point to draw on the bar: sprite, horizontal offset, colour, arrow (−1 down, 1 up).
struct Dot {
    sprite: &'static str,
    offset: i32,
    color: u32,
    arrow: i8,
}

/// Waypoint styles of the game (`waypoint_style/*.json`): near and far distances, sprites.
fn waypoint_style(style: &str) -> (f32, f32, &'static [&'static str]) {
    const DEFAULT: [&str; 4] = [
        "hud/locator_bar_dot/default_0",
        "hud/locator_bar_dot/default_1",
        "hud/locator_bar_dot/default_2",
        "hud/locator_bar_dot/default_3",
    ];
    const BOWTIE: [&str; 5] = [
        "hud/locator_bar_dot/bowtie",
        "hud/locator_bar_dot/default_0",
        "hud/locator_bar_dot/default_1",
        "hud/locator_bar_dot/default_2",
        "hud/locator_bar_dot/default_3",
    ];
    match style {
        "minecraft:bowtie" => (64.0, 332.0, &BOWTIE),
        _ => (128.0, 332.0, &DEFAULT),
    }
}

/// Sprite according to distance: first one below "near", last one beyond "far",
/// otherwise interpolated over the intermediate sprites.
fn style_sprite(style: &str, d: f32) -> &'static str {
    let (near, far, s) = waypoint_style(style);
    if d < near {
        s[0]
    } else if d >= far {
        s[s.len() - 1]
    } else if s.len() == 1 {
        s[0]
    } else if s.len() == 3 {
        s[1]
    } else {
        let t = (d - near) / (far - near);
        s[(1 + (t * (s.len() - 2) as f32).floor() as usize).min(s.len() - 1)]
    }
}

/// Colour of a waypoint without colour: tint drawn from the hash (UUID or name), brightness 0.9.
fn hash_color(id: &rc_proto_v777::waypoint::WaypointId) -> u32 {
    use rc_proto_v777::waypoint::WaypointId;
    let h = match id {
        WaypointId::Uuid(u) => {
            let (msb, lsb) = ((u >> 64) as u64, *u as u64);
            let x = msb ^ lsb;
            ((x >> 32) as u32 ^ x as u32) as i32
        }
        WaypointId::Name(n) => n.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c))),
    };
    set_brightness(h as u32 & 0xff_ffff, 0.9)
}

/// Same colour in HSV with another brightness (rounding of the game).
fn set_brightness(rgb: u32, brightness: f32) -> u32 {
    let (r, g, b) = ((rgb >> 16 & 255) as i32, (rgb >> 8 & 255) as i32, (rgb & 255) as i32);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let range = (max - min) as f32;
    let sat = if max != 0 { range / max as f32 } else { 0.0 };
    let byte = |v: f32| (v * 255.0).round() as u32;
    if sat == 0.0 {
        let v = byte(brightness);
        return 0xff00_0000 | v << 16 | v << 8 | v;
    }
    let (cr, cg, cb) = ((max - r) as f32 / range, (max - g) as f32 / range, (max - b) as f32 / range);
    let mut hue = if r == max {
        cb - cg
    } else if g == max {
        2.0 + cr - cb
    } else {
        4.0 + cg - cr
    } / 6.0;
    if hue < 0.0 {
        hue += 1.0;
    }
    let seg = (hue - hue.floor()) * 6.0;
    let off = seg - seg.floor();
    let p = brightness * (1.0 - sat);
    let q = brightness * (1.0 - sat * off);
    let t = brightness * (1.0 - sat * (1.0 - off));
    let (r, g, b) = match seg as i32 {
        0 => (brightness, t, p),
        1 => (q, brightness, p),
        2 => (p, brightness, t),
        3 => (p, q, brightness),
        4 => (t, p, brightness),
        _ => (brightness, p, q),
    };
    0xff00_0000 | byte(r) << 16 | byte(g) << 8 | byte(b)
}

/// Signed angle brought into ]−180, 180].
fn wrap_degrees(a: f64) -> f64 {
    let a = a % 360.0;
    if a >= 180.0 {
        a - 360.0
    } else if a < -180.0 {
        a + 360.0
    } else {
        a
    }
}

/// "Blocks the view" class of a block (`hand-camera.md` § 1.2, screen effects).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Blocking {
    /// Full collision and solid block.
    Default,
    Never,
    Always,
    /// Snow: 8 layers.
    Snow,
    /// Stairs, slabs: occlusion shape against the near plane.
    NearPlane,
}

fn blocking_class(name: &str) -> Blocking {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    match n {
        "barrier" | "light" | "structure_void" | "moving_piston" | "mangrove_roots" => Blocking::Never,
        "soul_sand" | "mud" | "farmland" | "dirt_path" => Blocking::Always,
        "snow" => Blocking::Snow,
        _ if n.ends_with("glass") || n.contains("leaves") || n.contains("copper_grate") => Blocking::Never,
        _ if n.ends_with("_stairs") || n.ends_with("_slab") => Blocking::NearPlane,
        _ => Blocking::Default,
    }
}

/// Sound to play: to the player (player category), or ambient without position.
pub struct Sound {
    pub name: &'static str,
    pub volume: f32,
    pub pitch: f32,
    pub ambient: bool,
}

/// Mount as seen from the HUD.
pub struct VehicleIn {
    pub id: i32,
    /// Living entity (shows its health).
    pub living: bool,
    /// Health (entity data 9).
    pub health: f32,
    /// Jumping mount steered by the player: cooldown set on each jump (0 for horses).
    pub jump_cooldown: Option<i32>,
}

/// Contextual bar above the hotbar (§ 4.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bar {
    Empty,
    Experience,
    Locator,
    Jump,
}

/// Current mount: hearts, health, jump possible.
struct Vehicle {
    hearts: i32,
    health: i32,
    jumpable: bool,
}

#[derive(Default)]
pub struct HudMore {
    air: i32,
    eyes_in_water: bool,
    /// Last bubble whose popping was given a sound (0: none).
    last_bubble: i32,
    sounds: Vec<Sound>,
    vehicle: Option<Vehicle>,
    /// Maximum health of the other entities (synchronised attribute), by id.
    max_health: hashbrown::HashMap<i32, f64>,
    /// Mount jump: key at the previous tick, charge ticks, strength (0..1), cooldown.
    jump_was: bool,
    jump_ticks: i32,
    jump_scale: f32,
    jump_cooldown: i32,
    /// Jump to send to the server (strength 0..100).
    jump_send: Option<i32>,
    ticks: i32,
    /// Tick of the last change of experience progress.
    xp_changed: Option<i32>,
    frozen: i32,
    /// Portal intensity (previous tick, current).
    portal_o: f32,
    portal: f32,
    nausea: Option<Nausea>,
    /// Distortion rotation: accumulated angle and speed (degrees per tick).
    spin_time: f32,
    spin_speed: f32,
    random: Option<JavaRandom>,
    /// "Blocks the view" class per block, computed on first need.
    blocking: Vec<Blocking>,
    waypoints: Vec<Tracked>,
    /// Locator bar points of the frame, from farthest to nearest.
    dots: Vec<Dot>,
    /// Spyglass in use (local player): full-screen mask in first person.
    pub scoping: bool,
    /// Mask scale (0.5 → 1.125) and instant of the previous frame.
    scope_scale: f32,
    scope_last: Option<std::time::Instant>,
    /// Sleep counter of the local player (0..100 in bed, 100..110 waking): fade to black.
    pub sleep_timer: i32,
}

impl HudMore {
    pub fn new() -> Self {
        HudMore { air: MAX_AIR, ..Default::default() }
    }

    /// Periodic forgetting of the max healths of vanished entities.
    pub fn prune(&mut self, entities: &crate::entities::Entities) {
        if self.ticks % 256 == 0 {
            self.max_health.retain(|id, _| entities.map.contains_key(id));
        }
    }

    pub fn tick(&mut self, t: &TickIn) {
        self.ticks = self.ticks.wrapping_add(1);
        self.air = t.air.unwrap_or(MAX_AIR);
        self.eyes_in_water = t.eyes_in_water;
        self.vehicle = t.vehicle.as_ref().map(|v| {
            // Max health rounded, two points per heart, 30 hearts at most.
            let max = self.max_health.get(&v.id).copied().unwrap_or(20.0) as f32;
            Vehicle {
                hearts: if v.living { (((max + 0.5) as i32) / 2).min(30) } else { 0 },
                health: v.health.ceil() as i32,
                jumpable: v.jump_cooldown.is_some(),
            }
        });
        self.tick_jump(t.vehicle.as_ref().and_then(|v| v.jump_cooldown), t.jump);
        self.frozen = t.frozen.unwrap_or(0);
        // Nausea effect: duration then fade (entry 150 ticks, exit 20, 60 ticks ahead).
        if let Some(n) = &mut self.nausea {
            if n.duration > 0 {
                n.duration -= 1;
            }
            n.factor_o = n.factor;
            let on = n.duration == -1 || n.duration > 60;
            let step = 1.0 / if on { 150.0 } else { 20.0 };
            n.factor += (f32::from(u8::from(on)) - n.factor).clamp(-step, step);
        }
        // Portal: +0.0125 per tick inside (sound at the start), −0.05 outside.
        self.portal_o = self.portal;
        let step = if t.in_portal {
            if self.portal == 0.0 {
                let r = self.random.get_or_insert_with(|| JavaRandom::new(0x5eed_1234));
                let pitch = r.float() * 0.4 + 0.8;
                self.sounds.push(Sound { name: "minecraft:block.portal.trigger", volume: 0.25, pitch, ambient: true });
            }
            0.0125
        } else if self.portal > 0.0 {
            -0.05
        } else {
            0.0
        };
        self.portal = (self.portal + step).clamp(0.0, 1.0);
        // Rotation speed: weighted mean of the portal (20) and the nausea (7).
        let n = self.nausea.as_ref().map_or(0.0, |n| n.factor);
        if self.portal > 0.0 || n > 0.0 {
            self.spin_speed = (self.portal * 20.0 + n * 7.0) / (self.portal + n);
            self.spin_time += self.spin_speed;
        } else {
            self.spin_speed = 0.0;
        }
    }

    /// Effect added or updated (only nausea counts here); without the fade flag
    /// (0x08), the factor is set at once; an update keeps the current factor.
    pub fn effect(&mut self, name: Option<&str>, duration: i32, flags: i8) {
        if name != Some("minecraft:nausea") {
            return;
        }
        match &mut self.nausea {
            Some(n) => n.duration = duration,
            None => {
                let on = duration == -1 || duration > 60;
                let f = if flags & 8 == 0 && on { 1.0 } else { 0.0 };
                self.nausea = Some(Nausea { duration, factor_o: f, factor: f });
            }
        }
    }

    pub fn remove_effect(&mut self, name: Option<&str>) {
        if name == Some("minecraft:nausea") {
            self.nausea = None;
        }
    }

    fn nausea_at(&self, pt: f32) -> f32 {
        self.nausea.as_ref().map_or(0.0, |n| n.factor_o + (n.factor - n.factor_o) * pt)
    }

    fn portal_at(&self, pt: f32) -> f32 {
        self.portal_o + (self.portal - self.portal_o) * pt
    }

    /// Distortion of the world view (portal, nausea): rotation around the axis (0, √½, √½),
    /// stretch in x by 1/skew, inverse rotation.
    pub fn distortion(&self, pt: f32, screen_effect_scale: f32) -> glam::Mat4 {
        let i = self.portal_at(pt).max(self.nausea_at(pt)) * screen_effect_scale * screen_effect_scale;
        if i <= 0.0 {
            return glam::Mat4::IDENTITY;
        }
        let skew = (5.0 / (i * i + 5.0) - i * 0.04).powi(2);
        let axis = glam::Vec3::new(0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
        let angle = (self.spin_time + pt * self.spin_speed).to_radians();
        glam::Mat4::from_axis_angle(axis, angle)
            * glam::Mat4::from_scale(glam::Vec3::new(1.0 / skew, 1.0, 1.0))
            * glam::Mat4::from_axis_angle(axis, -angle)
    }

    /// State of the block that obstructs the view: 8 points around the eyes (width × 0.8, ±0.05 in height);
    /// `near` = corners of the near plane (world frame), for stairs and slabs.
    pub fn view_blocking(
        &mut self,
        data: &rc_data::VersionData,
        state_at: impl Fn(i32, i32, i32) -> u32,
        feet: [f64; 3],
        eye_y: f64,
        near: [[f64; 3]; 4],
    ) -> Option<usize> {
        if self.blocking.len() != data.blocks.len() {
            self.blocking = data.blocks.iter().map(|b| blocking_class(&b.name)).collect();
        }
        let width = 0.6;
        let lo: [f64; 3] = std::array::from_fn(|a| near.iter().map(|c| c[a]).fold(f64::MAX, f64::min));
        let hi: [f64; 3] = std::array::from_fn(|a| near.iter().map(|c| c[a]).fold(f64::MIN, f64::max));
        for i in 0..8 {
            let p = [
                feet[0] + (f64::from(i & 1) - 0.5) * width * 0.8,
                eye_y + (f64::from((i >> 1) & 1) - 0.5) * 0.1,
                feet[2] + (f64::from((i >> 2) & 1) - 0.5) * width * 0.8,
            ];
            let b = p.map(|v| v.floor() as i32);
            let id = state_at(b[0], b[1], b[2]);
            let Some(st) = data.state(id) else { continue };
            if st.flags & rc_data::flags::AIR != 0 {
                continue;
            }
            let block = st.block as usize;
            let blocks = match self.blocking.get(block).copied().unwrap_or(Blocking::Default) {
                Blocking::Never => false,
                Blocking::Always => true,
                Blocking::Default => {
                    st.flags & rc_data::flags::FULL_COLLISION != 0 && st.flags & rc_data::flags::LEGACY_SOLID != 0
                }
                Blocking::Snow => data.state_properties(id).any(|kv| kv == ("layers", "8")),
                Blocking::NearPlane => data.shape(st.occlusion).iter().any(|s| {
                    (0..3).all(|a| {
                        let o = f64::from(b[a]);
                        o + s.min[a] < hi[a] && o + s.max[a] > lo[a]
                    })
                }),
            };
            if blocks {
                return Some(id as usize);
            }
        }
        None
    }

    /// Overlays and distortion of the frame. The block that obstructs the view is looked up at the
    /// player's eyes with the camera's near plane (except spectator, who passes through blocks).
    pub fn scene<'a>(&mut self, s: &SceneIn<'a>) -> (crate::overlays::Frame<'a>, glam::Mat4) {
        let block = if s.spectator {
            None
        } else {
            let (right, up, fwd) = crate::camera::axes(s.shot.0, s.shot.1);
            let (right, up, fwd) = (right.as_dvec3(), up.as_dvec3(), fwd.as_dvec3());
            let hh = 0.05 * (f64::from(s.fov).to_radians() / 2.0).tan();
            let hw = hh * f64::from(s.aspect);
            let eye = glam::DVec3::from_array(s.eye);
            let near = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .map(|(a, b)| (eye + fwd * 0.05 + right * (a * hw) + up * (b * hh)).to_array());
            self.view_blocking(s.data, s.state_at, s.feet, s.eye[1], near)
        };
        // Spyglass mask: scale brought closer by 1.125 (half the difference per tick). The
        // step follows the cadence (game ticks elapsed), without being cut in pause or frozen, and without
        // bound (a frame longer than 100 ms overshoots the target, like the game).
        let now = std::time::Instant::now();
        let elapsed = self.scope_last.map_or(0.0, |l| (now - l).as_secs_f32());
        self.scope_last = Some(now);
        self.scope_scale = scope_step(self.scope_scale, elapsed, s.tick_len);
        let scope = (s.first_person && self.scoping).then_some(self.scope_scale);
        if s.first_person && !self.scoping {
            self.scope_scale = 0.5;
        }
        let mut f = self.overlays(FrameIn {
            scope,
            pt: s.pt,
            first_person: s.first_person,
            spectator: s.spectator,
            equipped: s.me.map(|me| equipped_overlays(&me.equipment, s.data, s.equippable)).unwrap_or_default(),
            screen_effect_scale: s.screen_effect_scale,
            block,
            eye_brightness: s.eye_brightness,
            yaw: s.yaw,
            pitch: s.pitch,
            on_fire: s.me.is_some_and(|me| me.flags() & 1 != 0),
            hud_fov: s.hud_fov,
        });
        // HUD hidden (F1): no camera overlays; the screen effects remain.
        if s.hud_hidden {
            f.gui.clear();
        }
        (f, self.distortion(s.pt, s.screen_effect_scale))
    }

    /// Sleep counter (one tick): rises up to 100 in bed, then 100 → 110 → 0 on waking.
    pub fn tick_sleep(&mut self, sleeping: bool) {
        if sleeping {
            self.sleep_timer = (self.sleep_timer + 1).min(100);
        } else if self.sleep_timer > 0 {
            self.sleep_timer += 1;
            if self.sleep_timer >= 110 {
                self.sleep_timer = 0;
            }
        }
    }

    /// Sleep fade: flat 0x101020 of opacity 220 × (counter / 100), decreasing over the
    /// 10 ticks of waking.
    pub fn draw_sleep(&self, gui: &mut Gui) {
        if self.sleep_timer <= 0 {
            return;
        }
        let t = self.sleep_timer as f32;
        let mut a = t / 100.0;
        if a > 1.0 {
            a = 1.0 - (t - 100.0) / 10.0;
        }
        let alpha = ((220.0 * a) as i32).clamp(0, 255) as u32;
        gui.next_layer();
        gui.fill(0.0, 0.0, gui.width, gui.height, alpha << 24 | 0x10_1020);
    }

    /// Screen effects and camera overlays of the frame (`overlays.rs`).
    fn overlays<'a>(&self, f: FrameIn<'a>) -> crate::overlays::Frame<'a> {
        use crate::overlays::GuiOverlay;
        let mut out = crate::overlays::Frame { hud_fov: f.hud_fov, block: f.block, ..Default::default() };
        if f.first_person && !f.spectator {
            if self.eyes_in_water {
                let b = f.eye_brightness;
                out.water = Some(([b, b, b, 0.1], -f.yaw / 64.0, f.pitch / 64.0));
            }
            out.fire = f.on_fire;
        }
        if let Some(scale) = f.scope {
            out.gui.push(GuiOverlay::Scope(scale));
        } else if f.first_person {
            for id in &f.equipped {
                out.gui.push(GuiOverlay::Texture(id, 1.0));
            }
        }
        if self.frozen > 0 {
            let a = self.frozen.min(140) as f32 / 140.0;
            out.gui.push(GuiOverlay::Texture("minecraft:misc/powder_snow_outline", a));
        }
        let portal = self.portal_at(f.pt);
        let nausea = self.nausea_at(f.pt);
        if portal > 0.0 {
            let a = if portal < 1.0 { portal.powi(4) * 0.8 + 0.2 } else { portal };
            out.gui.push(GuiOverlay::Portal(a));
        } else if nausea > 0.0 && f.screen_effect_scale < 1.0 {
            out.gui.push(GuiOverlay::Nausea(nausea * (1.0 - f.screen_effect_scale)));
        }
        out
    }

    /// Mount jump charge (key held), sent on release.
    fn tick_jump(&mut self, cooldown_on_jump: Option<i32>, jump: bool) {
        let was = std::mem::replace(&mut self.jump_was, jump);
        if self.jump_cooldown > 0 {
            self.jump_cooldown -= 1;
        }
        match cooldown_on_jump {
            Some(cd) if self.jump_cooldown == 0 => {
                if self.jump_ticks < 0 {
                    self.jump_ticks += 1;
                    if self.jump_ticks == 0 {
                        self.jump_scale = 0.0;
                    }
                }
                if was && !jump {
                    self.jump_ticks = -10;
                    self.jump_send = Some((self.jump_scale * 100.0).floor() as i32);
                    self.jump_cooldown = cd;
                } else if !was && jump {
                    self.jump_ticks = 0;
                    self.jump_scale = 0.0;
                } else if was {
                    self.jump_ticks += 1;
                    self.jump_scale = if self.jump_ticks < 10 {
                        self.jump_ticks as f32 * 0.1
                    } else {
                        0.8 + 2.0 / (self.jump_ticks - 9) as f32 * 0.1
                    };
                }
            }
            Some(_) => self.jump_scale = 0.0,
            None => {
                self.jump_scale = 0.0;
                self.jump_cooldown = 0;
            }
        }
    }

    /// Mount jump to send (`player_command`, strength 0..100).
    pub fn take_jump(&mut self) -> Option<i32> {
        self.jump_send.take()
    }

    /// Attributes of another entity: only the maximum health is used (mount health).
    pub fn attributes(&mut self, entity: i32, list: &[rc_proto_v777::play::Attribute], names: &[String]) {
        for a in list {
            if names.get(a.id as usize).is_some_and(|n| n == "minecraft:max_health") {
                self.max_health.insert(entity, attribute_value("minecraft:max_health", a));
            }
        }
    }

    /// Experience progress changed: the XP bar takes precedence over the locator for 100 ticks.
    pub fn xp_changed(&mut self) {
        self.xp_changed = Some(self.ticks);
    }

    /// Hearts of the living mount (0: no mount showing its health).
    pub fn vehicle_hearts(&self) -> i32 {
        self.vehicle.as_ref().map_or(0, |v| v.hearts)
    }

    /// Bar to display; `experience` = mode with experience (survival, adventure).
    pub fn bar(&self, experience: bool) -> Bar {
        let jumpable = self.vehicle.as_ref().is_some_and(|v| v.jumpable);
        if self.has_waypoints() {
            if jumpable && (self.jump_scale > 0.0 || self.jump_cooldown > 0) {
                Bar::Jump
            } else if experience && self.xp_changed.is_some_and(|t| t.wrapping_add(100) > self.ticks) {
                Bar::Experience
            } else {
                Bar::Locator
            }
        } else if jumpable {
            Bar::Jump
        } else if experience {
            Bar::Experience
        } else {
            Bar::Empty
        }
    }

    /// Tracked waypoints (locator bar).
    pub fn has_waypoints(&self) -> bool {
        !self.waypoints.is_empty()
    }

    /// `waypoint` packet: track (replaces), untrack, update (same kind of position).
    pub fn waypoint(&mut self, w: rc_proto_v777::waypoint::Waypoint) {
        use rc_proto_v777::waypoint::WaypointOp;
        let i = self.waypoints.iter().position(|t| t.id == w.id);
        match (w.op, i) {
            (WaypointOp::Track, Some(i)) => {
                self.waypoints[i] = Tracked { id: w.id, style: w.style, color: w.color, pos: w.pos }
            }
            (WaypointOp::Track, None) => {
                self.waypoints.push(Tracked { id: w.id, style: w.style, color: w.color, pos: w.pos })
            }
            (WaypointOp::Untrack, Some(i)) => {
                self.waypoints.remove(i);
            }
            (WaypointOp::Update, Some(i)) => {
                let t = &mut self.waypoints[i];
                if std::mem::discriminant(&t.pos) == std::mem::discriminant(&w.pos) {
                    t.pos = w.pos;
                }
            }
            _ => {}
        }
    }

    /// New connection: no waypoint any more.
    pub fn clear_waypoints(&mut self) {
        self.waypoints.clear();
        self.dots.clear();
    }

    /// Bar points for the frame: yaw angle at the camera in ]−60°, 60°], sprite
    /// according to the distance to the player, arrow if the waypoint leaves the screen in height.
    pub fn locate(&mut self, l: &LocateIn<'_>) {
        use rc_proto_v777::waypoint::{WaypointId, WaypointPos};
        self.dots.clear();
        if self.waypoints.is_empty() {
            return;
        }
        let (_, up, fwd) = crate::camera::axes(l.yaw, l.pitch);
        let (up, fwd) = (up.as_dvec3(), fwd.as_dvec3());
        let tan = f64::from(l.fov.to_radians() / 2.0).tan();
        let cam = glam::DVec3::from_array(l.cam);
        // Horizon on screen (approximate positions: chunk, direction).
        let horizon = || {
            if l.pitch <= -90.0 {
                f64::NEG_INFINITY
            } else if l.pitch >= 90.0 {
                f64::INFINITY
            } else {
                f64::from(l.pitch.to_radians()).tan() / tan
            }
        };
        let by_horizon = |h: f64| if h < -1.0 { -1 } else { i8::from(h > 1.0) };
        let mut list: Vec<(f64, Dot)> = Vec::new();
        for w in &self.waypoints {
            if w.id == WaypointId::Uuid(l.me_uuid) {
                continue;
            }
            let yaw_to = |p: [f64; 3]| {
                // Camera → waypoint direction rotated by a quarter turn, angle compared with the yaw.
                let (dx, dz) = (l.cam[0] - p[0], l.cam[2] - p[2]);
                wrap_degrees(f64::from((dx as f32).atan2(-dz as f32).to_degrees()) - f64::from(l.yaw))
            };
            let dist2 = |p: [f64; 3]| (0..3).map(|a| (l.me[a] - p[a]).powi(2)).sum::<f64>();
            let (angle, d2, arrow) = match w.pos {
                WaypointPos::Empty => continue,
                WaypointPos::Azimuth(a) => {
                    (wrap_degrees(f64::from(a.to_degrees()) - f64::from(l.yaw)), f64::INFINITY, by_horizon(horizon()))
                }
                WaypointPos::Chunk([x, z]) => {
                    // Block in the middle of the chunk, at the camera's height (truncated) or the player's.
                    let at = |y: f64| [f64::from(x * 16 + 8) + 0.5, y + 0.5, f64::from(z * 16 + 8) + 0.5];
                    (yaw_to(at(l.cam[1].trunc())), dist2(at(l.me[1].floor())), by_horizon(horizon()))
                }
                WaypointPos::Block(b) => {
                    let center = b.map(|v| f64::from(v) + 0.5);
                    // Tracked entity close to its announced position: its eyes.
                    let p = match &w.id {
                        WaypointId::Uuid(u) => (l.entity)(*u)
                            .filter(|(feet, _)| (0..3).map(|a| (feet[a].floor() as i32 - b[a]).abs()).sum::<i32>() <= 3)
                            .map_or(center, |(_, eye)| eye),
                        WaypointId::Name(_) => center,
                    };
                    let rel = glam::DVec3::from_array(p) - cam;
                    let depth = rel.dot(fwd);
                    let y = rel.dot(up) / tan / depth;
                    let behind = depth < 0.0;
                    let front = if behind { -y } else { y };
                    let arrow = if front < -1.0 {
                        -1
                    } else if front > 1.0 || (behind && y > 0.0) {
                        1
                    } else if behind && y < 0.0 {
                        -1
                    } else {
                        0
                    };
                    (yaw_to(p), dist2(center), arrow)
                }
            };
            if angle <= -60.0 || angle > 60.0 {
                continue;
            }
            let d = (d2 as f32).sqrt();
            let color = w.color.map_or_else(|| hash_color(&w.id), |c| 0xff00_0000 | c);
            let offset = (angle * 173.0 / 2.0 / 60.0).floor() as i32;
            list.push((d2, Dot { sprite: style_sprite(&w.style, d), offset, color, arrow }));
        }
        list.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.dots = list.into_iter().map(|(_, d)| d).collect();
    }

    /// Background of the locator bar.
    pub fn draw_locator_background(&self, gui: &mut Gui, left: i32, top: i32) {
        gui.sprite("hud/locator_bar_background", left as f32, top as f32, 182.0, 5.0, 0xffff_ffff);
    }

    /// Points and arrows (§ 4.8); animated arrows (10 ticks then 4).
    pub fn draw_locator(&self, gui: &mut Gui, w: i32, top: i32) {
        let middle = (w - 9 + 1) / 2;
        let frame = self.ticks.rem_euclid(14) >= 10;
        for d in &self.dots {
            let x = middle + d.offset;
            gui.sprite(d.sprite, x as f32, (top - 2) as f32, 9.0, 9.0, d.color);
            if d.arrow != 0 {
                let (dy, name) = if d.arrow < 0 {
                    (6, if frame { "hud/locator_bar_arrow_down#1" } else { "hud/locator_bar_arrow_down" })
                } else {
                    (-6, if frame { "hud/locator_bar_arrow_up#1" } else { "hud/locator_bar_arrow_up" })
                };
                gui.sprite(name, (x + 1) as f32, (top + dy) as f32, 7.0, 5.0, 0xffff_ffff);
            }
        }
    }

    /// Jump bar (§ 4.6) at (`left`, `top`).
    pub fn draw_jump(&self, gui: &mut Gui, left: i32, top: i32) {
        let (x, y) = (left as f32, top as f32);
        gui.sprite("hud/jump_bar_background", x, y, 182.0, 5.0, 0xffff_ffff);
        if self.jump_cooldown > 0 {
            gui.sprite("hud/jump_bar_cooldown", x, y, 182.0, 5.0, 0xffff_ffff);
        } else if self.jump_scale > 0.0 {
            let p = (self.jump_scale * 181.0).floor() + 1.0;
            gui.sprite_region("hud/jump_bar_progress", 182.0, 5.0, 0.0, 0.0, x, y, p, 5.0);
        }
    }

    /// Mount health (§ 4.7): rows of 10 hearts on the right, from bottom to top.
    pub fn draw_vehicle(&self, gui: &mut Gui, w: i32, h: i32) {
        let Some(v) = &self.vehicle else { return };
        let x_right = w / 2 + 91;
        let mut left = v.hearts;
        let (mut y, mut base) = (h - 39, 0);
        while left > 0 {
            let row = left.min(10);
            left -= row;
            for i in 0..row {
                let x = (x_right - i * 8 - 9) as f32;
                gui.sprite("hud/heart/vehicle_container", x, y as f32, 9.0, 9.0, 0xffff_ffff);
                if i * 2 + 1 + base < v.health {
                    gui.sprite("hud/heart/vehicle_full", x, y as f32, 9.0, 9.0, 0xffff_ffff);
                }
                if i * 2 + 1 + base == v.health {
                    gui.sprite("hud/heart/vehicle_half", x, y as f32, 9.0, 9.0, 0xffff_ffff);
                }
            }
            y -= 10;
            base += 20;
        }
    }

    /// Sounds triggered since the last call.
    pub fn take_sounds(&mut self) -> Vec<Sound> {
        std::mem::take(&mut self.sounds)
    }

    /// Air bubbles (§ 4.5), following the hunger with the same random draw; `y` = air line
    /// (already raised by 10 if hunger is displayed), `t` = HUD tick counter.
    pub fn draw_air(&mut self, gui: &mut Gui, random: &mut JavaRandom, t: i32, x_right: i32, y: i32) {
        let air = self.air.clamp(0, MAX_AIR);
        let under = self.eyes_in_water;
        if !under && air >= MAX_AIR {
            return;
        }
        // Without a mount: 0 rows, hence one notch lower (just above the hunger).
        let rows = (self.vehicle_hearts() + 9) / 10;
        let y = y - (rows - 1) * 10;
        let bubble = |off: i32| ((air + off) as f32 * 10.0 / MAX_AIR as f32).ceil() as i32;
        let full = bubble(-2);
        let popping = bubble(0);
        let empty = 10 - bubble(i32::from(air != 0 && under));
        if !under {
            self.last_bubble = 0;
        }
        for b in 1..=10 {
            let x = (x_right - (b - 1) * 8 - 9) as f32;
            if b <= full {
                gui.sprite("hud/air", x, y as f32, 9.0, 9.0, 0xffff_ffff);
            } else if full != popping && b == popping && under {
                gui.sprite("hud/air_bursting", x, y as f32, 9.0, 9.0, 0xffff_ffff);
                if self.last_bubble != b {
                    let volume = 0.5 + 0.1 * (empty - 2).max(0) as f32;
                    let pitch = 1.0 + 0.1 * (empty - 4).max(0) as f32;
                    self.sounds.push(Sound { name: "minecraft:ui.hud.bubble_pop", volume, pitch, ambient: false });
                    self.last_bubble = b;
                }
            } else if b > 10 - empty {
                let wobble = if empty == 10 && t % 2 == 0 { random.next_int(2) } else { 0 };
                gui.sprite("hud/air_empty", x, (y + wobble) as f32, 9.0, 9.0, 0xffff_ffff);
            }
        }
    }
}

/// Mount of the local player: the tracked entity that counts them among its passengers. It is steered
/// (jump) if it is saddled and the player is its first passenger.
pub fn vehicle_of(entities: &crate::entities::Entities) -> Option<VehicleIn> {
    let local = entities.local;
    let (&id, e) = entities.map.iter().find(|(_, e)| e.passengers.contains(&local))?;
    let health = match e.meta.get(&9) {
        Some(rc_proto_v777::entity::Meta::Float(h)) => *h,
        _ => 0.0,
    };
    let controls = e.passengers.first() == Some(&local) && e.equipment[7].is_some();
    let jump_cooldown = match e.kind.as_str() {
        _ if !controls => None,
        "horse" | "donkey" | "mule" | "skeleton_horse" | "zombie_horse" => Some(0),
        "camel" | "camel_husk" => Some(55),
        "nautilus" | "zombie_nautilus" => Some(40),
        _ => None,
    };
    Some(VehicleIn { id, living: e.class != crate::entities::Kind::Other, health, jump_cooldown })
}

/// Overlays of equipped items: `equippable.camera_overlay` texture of an item worn in
/// the slot its component designates. The component is the stack's (`equippable` of the
/// patch, removed by `!equippable`), otherwise the item's; `equippable`: its id.
pub fn equipped_overlays<'a>(
    equipment: &'a [Option<rc_proto_v777::item::ItemStack>; 8],
    data: &'a rc_data::VersionData,
    equippable: Option<u16>,
) -> Vec<&'a str> {
    // Network slot → `equip_slot` code (1 main hand, 2 feet … 5 head, 6 off hand, 7 body, 8 saddle).
    const CODE: [u8; 8] = [1, 6, 2, 3, 4, 5, 7, 8];
    equipment
        .iter()
        .zip(CODE)
        .filter_map(|(s, code)| {
            let s = s.as_ref()?;
            if equippable.is_some_and(|id| s.removed.contains(&id)) {
                return None;
            }
            let (slot, overlay) = match equippable.and_then(|id| s.component(id)) {
                Some(b) => crate::stack::equippable_overlay(b)?,
                None => {
                    let info = data.items.get(usize::try_from(s.item).ok()?)?;
                    (info.equip_slot, Some(info.equip_overlay.as_str()).filter(|o| !o.is_empty()))
                }
            };
            overlay.filter(|_| slot == code)
        })
        .collect()
}

/// Player in a Nether portal: their box (0.6 × 1.8, shrunk by 1e−5) touches the shape
/// of a portal block (central plate 4/16 thick along the axis).
pub fn in_portal(data: &rc_data::VersionData, state_at: impl Fn(i32, i32, i32) -> u32, feet: [f64; 3]) -> bool {
    let e = 1e-5;
    let lo = [feet[0] - 0.3 + e, feet[1] + e, feet[2] - 0.3 + e];
    let hi = [feet[0] + 0.3 - e, feet[1] + 1.8 - e, feet[2] + 0.3 - e];
    for x in lo[0].floor() as i32..=hi[0].floor() as i32 {
        for y in lo[1].floor() as i32..=hi[1].floor() as i32 {
            for z in lo[2].floor() as i32..=hi[2].floor() as i32 {
                let id = state_at(x, y, z);
                if data.block_of(id).is_none_or(|b| b.name != "minecraft:nether_portal") {
                    continue;
                }
                let along_x = data.state_properties(id).any(|kv| kv == ("axis", "x"));
                let (a, o) = if along_x { (2, z) } else { (0, x) };
                let (s0, s1) = (f64::from(o) + 6.0 / 16.0, f64::from(o) + 10.0 / 16.0);
                if lo[a] < s1 && hi[a] > s0 {
                    return true;
                }
            }
        }
    }
    false
}

/// Value of an attribute: base + additions, × (1 + Σ base multipliers), × Π (1 + totals).
/// The result is brought into the attribute's range (`name`), a NaN giving the minimum.
pub(crate) fn attribute_value(name: &str, a: &rc_proto_v777::play::Attribute) -> f64 {
    let ops = |op: i32| a.modifiers.iter().filter(move |m| m.operation == op).map(|m| m.amount);
    let base = a.base + ops(0).sum::<f64>();
    let mut v = base * (1.0 + ops(1).sum::<f64>());
    for m in ops(2) {
        v *= 1.0 + m;
    }
    rc_physics::sanitize_attribute(name, v)
}

/// Spyglass mask scale after `elapsed` seconds: brought closer by 1.125 of half
/// the difference per elapsed game tick (`tick_len`: target tick duration).
fn scope_step(scale: f32, elapsed: f32, tick_len: f32) -> f32 {
    let ticks = elapsed / tick_len.max(1e-3);
    scale + 0.5 * ticks * (1.125 - scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Overlay read from the stack's component: added to a helmet, removed from a pumpkin,
    /// ignored outside the designated slot; item default without patch.
    #[test]
    fn camera_overlay_follows_stack_components() {
        use rc_proto_v777::item::ItemStack;
        const EQ: u16 = 9;
        let data = rc_data::VersionData {
            items: vec![
                Default::default(),
                rc_data::ItemInfo {
                    equip_slot: 5,
                    equip_overlay: "minecraft:misc/pumpkinblur".into(),
                    ..Default::default()
                },
                rc_data::ItemInfo { equip_slot: 5, ..Default::default() },
            ],
            ..Default::default()
        };
        // Component: network slot, sound no. 1, no appearance, optional overlay.
        let comp = |slot: u8, overlay: Option<&str>| {
            let mut b = vec![slot, 2, 0];
            match overlay {
                Some(o) => {
                    b.extend([1, o.len() as u8]);
                    b.extend_from_slice(o.as_bytes());
                }
                None => b.push(0),
            }
            b.extend([0, 0, 0, 0, 0, 0, 0, 3]);
            b.into_boxed_slice()
        };
        let st =
            |item, added: Vec<(u16, Box<[u8]>)>, removed: Vec<u16>| Some(ItemStack { item, count: 1, added, removed });
        let head = |s| {
            let mut e: [Option<ItemStack>; 8] = Default::default();
            e[5] = s;
            e
        };
        let pumpkin = head(st(1, Vec::new(), Vec::new()));
        assert_eq!(equipped_overlays(&pumpkin, &data, Some(EQ)), ["minecraft:misc/pumpkinblur"]);
        let carved_off = head(st(1, Vec::new(), vec![EQ]));
        assert!(equipped_overlays(&carved_off, &data, Some(EQ)).is_empty());
        let helmet = head(st(2, vec![(EQ, comp(4, Some("minecraft:misc/nausea")))], Vec::new()));
        assert_eq!(equipped_overlays(&helmet, &data, Some(EQ)), ["minecraft:misc/nausea"]);
        let wrong_slot = head(st(2, vec![(EQ, comp(3, Some("minecraft:misc/nausea")))], Vec::new()));
        assert!(equipped_overlays(&wrong_slot, &data, Some(EQ)).is_empty());
        let no_overlay = head(st(1, vec![(EQ, comp(4, None))], Vec::new()));
        assert!(equipped_overlays(&no_overlay, &data, Some(EQ)).is_empty());
    }

    /// Under `/tick rate 5`, entering the spyglass is 4× slower than at 20 t/s.
    #[test]
    fn scope_zoom_follows_tick_rate() {
        let fast = scope_step(0.5, 0.05, 0.05);
        let slow = scope_step(0.5, 0.05, 0.2);
        assert!((fast - 0.8125).abs() < 1e-6);
        assert!((slow - (0.5 + 0.125 * 0.625)).abs() < 1e-6);
    }

    fn tick(h: &mut HudMore, jump: bool) {
        let v = VehicleIn { id: 1, living: true, health: 10.0, jump_cooldown: Some(0) };
        h.tick(&TickIn { air: None, eyes_in_water: false, vehicle: Some(v), jump, frozen: None, in_portal: false });
    }

    #[test]
    fn horse_jump_charges_then_fires_on_release() {
        let mut h = HudMore::new();
        tick(&mut h, false);
        tick(&mut h, true);
        assert_eq!(h.jump_scale, 0.0);
        for _ in 0..5 {
            tick(&mut h, true);
        }
        assert!((h.jump_scale - 0.5).abs() < 1e-6);
        for _ in 0..10 {
            tick(&mut h, true);
        }
        // 15 ticks held: 0.8 + 2/6 × 0.1.
        assert!((h.jump_scale - (0.8 + 2.0 / 6.0 * 0.1)).abs() < 1e-6);
        tick(&mut h, false);
        assert_eq!(h.take_jump(), Some(83));
        assert_eq!(h.bar(true), Bar::Jump);
    }

    #[test]
    fn waypoint_sprites_by_distance() {
        let d = "minecraft:default";
        assert_eq!(style_sprite(d, 127.9), "hud/locator_bar_dot/default_0");
        assert_eq!(style_sprite(d, 128.0), "hud/locator_bar_dot/default_1");
        assert_eq!(style_sprite(d, 230.0), "hud/locator_bar_dot/default_2");
        assert_eq!(style_sprite(d, 332.0), "hud/locator_bar_dot/default_3");
        assert_eq!(style_sprite(d, f32::INFINITY), "hud/locator_bar_dot/default_3");
        let b = "minecraft:bowtie";
        assert_eq!(style_sprite(b, 63.0), "hud/locator_bar_dot/bowtie");
        assert_eq!(style_sprite(b, 64.0), "hud/locator_bar_dot/default_0");
        assert_eq!(style_sprite(b, 300.0), "hud/locator_bar_dot/default_2");
        // Pure grey: brightness only.
        assert_eq!(set_brightness(0x808080, 0.9), 0xffe6_e6e6);
    }

    #[test]
    fn mount_hearts_from_max_health() {
        let mut h = HudMore::new();
        let a = rc_proto_v777::play::Attribute { id: 0, base: 53.0, modifiers: Vec::new() };
        h.attributes(1, &[a], &["minecraft:max_health".to_owned()]);
        tick(&mut h, false);
        assert_eq!(h.vehicle_hearts(), 26);
    }

    /// The scale received is brought into [0.0625, 16], a NaN giving the minimum.
    #[test]
    fn scale_attribute_is_clamped() {
        let mk = |base: f64, amount: f64| rc_proto_v777::play::Attribute {
            id: 0,
            base,
            modifiers: vec![rc_proto_v777::play::Modifier { id: "m".into(), amount, operation: 0 }],
        };
        let n = "minecraft:scale";
        assert_eq!(attribute_value(n, &mk(100.0, 0.0)), 16.0);
        assert_eq!(attribute_value(n, &mk(0.0, 0.0)), 0.0625);
        assert_eq!(attribute_value(n, &mk(1.0, -3.0)), 0.0625);
        assert_eq!(attribute_value(n, &mk(f64::NAN, 0.0)), 0.0625);
        assert_eq!(attribute_value(n, &mk(2.0, 0.0)), 2.0);
    }
}
