//! Physics of the vehicles piloted by the local player (`docs/specs/mounts.md` § 3): boats and
//! rafts, equines, camels, pig and strider, happy ghast, nautilus.
//!
//! The tick receives the pilot's input set at the previous tick ([`Pilot`]); the application copies
//! the entity's position and rotations before the tick and hands them back afterwards.

use crate::{Aabb, DEG, Fluid, Input, Tables, World, below_of, collide, cos_t, is_free, l2, sin_t, support_of};

/// Kind of simulated vehicle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// Boat or raft (with or without chest).
    Boat,
    /// Horse, donkey, mule, skeleton and zombie horses. `float`: floats when ridden (tag
    /// `can_float_while_ridden`, all except the skeleton horse); `water_slowdown` 0.8 (skeleton 0.96).
    Equine {
        float: bool,
        water_slowdown: f32,
    },
    /// Camel, camel husk.
    Camel,
    Pig,
    Strider,
    HappyGhast,
    Nautilus,
}

/// Boat state (§ 3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    InWater,
    UnderWater,
    UnderFlowingWater,
    OnLand,
    #[default]
    InAir,
}

/// Pilot inputs set at the previous tick: raw keys and sideways / forward inputs
/// (±0.98, "square" correction included).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Controls {
    pub left: bool,
    pub right: bool,
    pub forward: bool,
    pub backward: bool,
    pub jump: bool,
    pub xxa: f32,
    pub zza: f32,
}

impl Controls {
    /// Inputs of a passenger (`movement.md` § 2.4: ×0.98 then "square" correction, without
    /// sneak slowdown or item-use slowdown).
    pub fn from_input(i: Input) -> Self {
        let fwd = f32::from(i8::from(i.forward) - i8::from(i.backward));
        let side = f32::from(i8::from(i.left) - i8::from(i.right));
        let len = (fwd * fwd + side * side).sqrt();
        let (mut sx, mut fz) = if len > 0.0 { (side / len, fwd / len) } else { (0.0, 0.0) };
        if sx != 0.0 || fz != 0.0 {
            sx *= 0.98;
            fz *= 0.98;
            let l = (sx * sx + fz * fz).sqrt();
            let (ax, az) = (sx.abs(), fz.abs());
            let ratio = ax.min(az) / ax.max(az);
            let nl = (l * (1.0 + ratio * ratio).sqrt()).min(1.0);
            sx = sx / l * nl;
            fz = fz / l * nl;
        }
        Controls {
            left: i.left,
            right: i.right,
            forward: i.forward,
            backward: i.backward,
            jump: i.jump,
            xxa: sx,
            zza: fz,
        }
    }
}

/// The piloting player: current rotation, previous tick's inputs, sprint.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pilot {
    pub yaw: f32,
    pub pitch: f32,
    pub controls: Controls,
    pub sprinting: bool,
}

/// Attributes of the mount (received from the server; mob defaults).
#[derive(Debug, Clone, Copy)]
pub struct Attrs {
    pub movement_speed: f64,
    pub jump_strength: f64,
    pub step_height: f64,
    pub flying_speed: f64,
    pub gravity: f64,
    pub friction_modifier: f64,
    pub air_drag_modifier: f64,
    pub movement_efficiency: f64,
    pub water_movement_efficiency: f64,
    pub bounciness: f64,
}

impl Default for Attrs {
    fn default() -> Self {
        Attrs {
            movement_speed: 0.7,
            jump_strength: 0.42,
            step_height: 0.6,
            flying_speed: 0.4,
            gravity: 0.08,
            friction_modifier: 1.0,
            air_drag_modifier: 1.0,
            movement_efficiency: 0.0,
            water_movement_efficiency: 0.0,
            bounciness: 0.0,
        }
    }
}

impl Attrs {
    /// Replaces the attribute `name` (`minecraft:…`) if it is used by the mount physics.
    pub fn set(&mut self, name: &str, v: f64) {
        let slot = match name.strip_prefix("minecraft:").unwrap_or(name) {
            "movement_speed" => &mut self.movement_speed,
            "jump_strength" => &mut self.jump_strength,
            "step_height" => &mut self.step_height,
            "flying_speed" => &mut self.flying_speed,
            "gravity" => &mut self.gravity,
            "friction_modifier" => &mut self.friction_modifier,
            "air_drag_modifier" => &mut self.air_drag_modifier,
            "movement_efficiency" => &mut self.movement_efficiency,
            "water_movement_efficiency" => &mut self.water_movement_efficiency,
            "bounciness" => &mut self.bounciness,
            _ => return,
        };
        *slot = v;
    }
}

/// What the tick asks of the application.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Out {
    /// Boat: paddle state to send (`paddle_boat`).
    pub paddles: Option<(bool, bool)>,
    /// Sitting camel: it stands up (sound, standing posture, change date).
    pub stand_up: bool,
}

/// Locally simulated vehicle.
#[derive(Debug, Clone)]
pub struct Mount {
    pub kind: Kind,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub width: f64,
    pub height: f64,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub fall_distance: f64,
    pub attrs: Attrs,
    pub water_height: f64,
    pub lava_height: f64,
    /// Fast-lava dimension.
    pub fast_lava: bool,
    support: Option<[i32; 3]>,
    // --- Boat ---
    pub status: Status,
    pub old_status: Status,
    water_level: f64,
    land_friction: f32,
    /// Rotation per tick (degrees), maintained by the piloting and damped by the flotation.
    pub rotation: f32,
    last_vy: f64,
    pub out_of_control: f32,
    pub paddles: (bool, bool),
    // --- Equines, camel, nautilus ---
    /// Pending jump (or dash), set on receipt of the strength (0: none).
    pub pending_jump: f32,
    /// Rearing slide allowed.
    pub allow_slide: bool,
    /// Rearing (flag 0x20 received from the server).
    pub standing: bool,
    /// Camel sitting or in posture transition ("refuses to move"); sitting outside a transition.
    pub refuse_move: bool,
    pub sitting_still: bool,
    pub dash_cooldown: i32,
    pub dashing: bool,
    // --- Pig, strider ---
    /// Acceleration in progress: (ticks elapsed, duration received).
    pub boost: Option<(i32, i32)>,
    pub suffocating: bool,
    /// Jump effect (level + 1), 0 without effect.
    pub jump_boost: i32,
}

/// Modified friction factor (`movement.md` § 4.3).
fn corrected(x: f32, m: f64) -> f32 {
    (1.0 - (1.0 - x) * m as f32).clamp(0.0, 1.0)
}

/// Look direction (yaw, pitch in degrees), tabulated sines.
pub fn view_vector(yaw: f32, pitch: f32) -> [f64; 3] {
    let f = pitch * DEG;
    let g = -yaw * DEG;
    let (h, i, j, k) = (cos_t(g), sin_t(g), cos_t(f), sin_t(f));
    [f64::from(i * j), f64::from(-k), f64::from(h * j)]
}

/// Brings an angle back into [−180, 180).
fn wrap(a: f32) -> f32 {
    let r = (a + 180.0).rem_euclid(360.0) - 180.0;
    if r >= 180.0 { r - 360.0 } else { r }
}

/// Pending jump for a received strength `n` (0..100).
pub fn pending_scale(n: i32) -> f32 {
    if n >= 90 { 1.0 } else { 0.4 + 0.4 * n as f32 / 90.0 }
}

impl Mount {
    pub fn new(kind: Kind, pos: [f64; 3], yaw: f32, pitch: f32, width: f64, height: f64) -> Self {
        Mount {
            kind,
            pos,
            vel: [0.0; 3],
            yaw,
            pitch,
            width,
            height,
            on_ground: false,
            horizontal_collision: false,
            vertical_collision: false,
            fall_distance: 0.0,
            attrs: Attrs::default(),
            water_height: 0.0,
            lava_height: 0.0,
            fast_lava: false,
            support: None,
            status: Status::InAir,
            old_status: Status::InAir,
            water_level: 0.0,
            land_friction: 0.0,
            rotation: 0.0,
            last_vy: 0.0,
            out_of_control: 0.0,
            paddles: (false, false),
            pending_jump: 0.0,
            allow_slide: false,
            standing: false,
            refuse_move: false,
            sitting_still: false,
            dash_cooldown: 0,
            dashing: false,
            boost: None,
            suffocating: false,
            jump_boost: 0,
        }
    }

    pub fn bounding_box(&self) -> Aabb {
        let r = self.width / 2.0;
        Aabb {
            min: [self.pos[0] - r, self.pos[1], self.pos[2] - r],
            max: [self.pos[0] + r, self.pos[1] + self.height, self.pos[2] + r],
        }
    }

    fn block_pos(&self) -> [i32; 3] {
        self.pos.map(|v| v.floor() as i32)
    }

    fn below(&self, t: &Tables, w: &dyn World, h: f32) -> [i32; 3] {
        below_of(self.support, self.pos, t, w, h)
    }

    fn state(w: &dyn World, p: [i32; 3]) -> u32 {
        w.state(p[0], p[1], p[2]).unwrap_or(0)
    }

    fn is_living(&self) -> bool {
        self.kind != Kind::Boat
    }

    /// Jumps ("jumping" mount): can the controlled mount jump right now?
    pub fn jumpable(&self) -> bool {
        match self.kind {
            Kind::Equine { .. } | Kind::Nautilus => true,
            Kind::Camel => !self.refuse_move,
            _ => false,
        }
    }

    /// The mount's own delay before a new jump (camel and nautilus dash).
    pub fn jump_cooldown(&self) -> i32 {
        match self.kind {
            Kind::Camel | Kind::Nautilus => self.dash_cooldown,
            _ => 0,
        }
    }

    /// Jump strength received from the pilot (§ 3.3, 3.4, 3.7), the mount being saddled.
    pub fn on_jump(&mut self, mut n: i32) {
        match self.kind {
            Kind::Equine { .. } => {}
            Kind::Camel if self.dash_cooldown <= 0 && self.on_ground => {}
            Kind::Nautilus if self.dash_cooldown <= 0 => {
                self.pending_jump = pending_scale(n);
                return;
            }
            _ => return,
        }
        if n < 0 {
            n = 0;
        } else {
            self.allow_slide = true;
        }
        self.pending_jump = pending_scale(n);
    }

    /// Acceleration decided by the server (received metadata): duration `total` in ticks.
    pub fn start_boost(&mut self, total: i32) {
        self.boost = Some((0, total));
    }

    /// One tick of the vehicle under local authority.
    pub fn tick(&mut self, p: &Pilot, w: &dyn World, t: &Tables) -> Out {
        match self.kind {
            Kind::Boat => self.tick_boat(p, w, t),
            Kind::Strider => {
                let lw = LavaFloor { w, t, feet: self.pos[1] };
                let out = self.tick_living(p, &lw, t);
                self.float_strider(w, t);
                out
            }
            _ => self.tick_living(p, w, t),
        }
    }

    // ---------- Boat ----------

    fn tick_boat(&mut self, p: &Pilot, w: &dyn World, t: &Tables) -> Out {
        self.old_status = self.status;
        self.status = self.boat_status(w, t);
        if matches!(self.status, Status::UnderWater | Status::UnderFlowingWater) {
            self.out_of_control += 1.0;
        } else {
            self.out_of_control = 0.0;
        }
        // Base tick: fluids and currents.
        self.update_fluids(w, t, true);
        self.float_boat(w, t);
        self.control_boat(&p.controls);
        self.do_move(self.vel, w, t, 0.0);
        Out { paddles: Some(self.paddles), stand_up: false }
    }

    fn water_at(t: &Tables, w: &dyn World, x: i32, y: i32, z: i32) -> Option<(f32, bool)> {
        let s = Self::state(w, [x, y, z]);
        if t.fluids.fluid(s) != Fluid::Water {
            return None;
        }
        let above = Self::state(w, [x, y + 1, z]);
        Some((t.fluids.height_at(s, above), t.fluids.source.get(s as usize).copied().unwrap_or(false)))
    }

    fn boat_status(&mut self, w: &dyn World, t: &Tables) -> Status {
        let b = self.bounding_box();
        // Underwater.
        let top = b.max[1] + 0.001;
        let mut under = false;
        for x in b.min[0].floor() as i32..b.max[0].ceil() as i32 {
            for y in b.max[1].floor() as i32..top.ceil() as i32 {
                for z in b.min[2].floor() as i32..b.max[2].ceil() as i32 {
                    if let Some((h, source)) = Self::water_at(t, w, x, y, z)
                        && top < f64::from(y) + f64::from(h)
                    {
                        if !source {
                            self.water_level = b.max[1];
                            return Status::UnderFlowingWater;
                        }
                        under = true;
                    }
                }
            }
        }
        if under {
            self.water_level = b.max[1];
            return Status::UnderWater;
        }
        // In water.
        let mut in_water = false;
        self.water_level = -f64::MAX;
        for x in b.min[0].floor() as i32..b.max[0].ceil() as i32 {
            for y in b.min[1].floor() as i32..(b.min[1] + 0.001).ceil() as i32 {
                for z in b.min[2].floor() as i32..b.max[2].ceil() as i32 {
                    if let Some((h, _)) = Self::water_at(t, w, x, y, z) {
                        let level = y as f32 + h;
                        self.water_level = self.water_level.max(f64::from(level));
                        in_water |= b.min[1] < f64::from(level);
                    }
                }
            }
        }
        if in_water {
            return Status::InWater;
        }
        let f = self.ground_friction(w, t);
        if f > 0.0 {
            self.land_friction = f;
            Status::OnLand
        } else {
            Status::InAir
        }
    }

    /// Average friction of the blocks under the boat (NaN without a block).
    fn ground_friction(&self, w: &dyn World, t: &Tables) -> f32 {
        let bb = self.bounding_box();
        let slab = Aabb { min: [bb.min[0], bb.min[1] - 0.001, bb.min[2]], max: [bb.max[0], bb.min[1], bb.max[2]] };
        let (x0, x1) = (slab.min[0].floor() as i32 - 1, slab.max[0].ceil() as i32 + 1);
        let (y0, y1) = (slab.min[1].floor() as i32 - 1, slab.max[1].ceil() as i32 + 1);
        let (z0, z1) = (slab.min[2].floor() as i32 - 1, slab.max[2].ceil() as i32 + 1);
        let (mut sum, mut n) = (0.0f32, 0i32);
        let mut cell = Vec::new();
        for x in x0..x1 {
            for z in z0..z1 {
                let edges = i32::from(x == x0 || x == x1 - 1) + i32::from(z == z0 || z == z1 - 1);
                if edges == 2 {
                    continue;
                }
                for y in y0..y1 {
                    if edges > 0 && (y == y0 || y == y1 - 1) {
                        continue;
                    }
                    let s = Self::state(w, [x, y, z]);
                    if t.lily_pad(s) {
                        continue;
                    }
                    cell.clear();
                    w.collision(x, y, z, &mut cell);
                    if cell.iter().any(|c| c.offset(f64::from(x), f64::from(y), f64::from(z)).intersects(&slab)) {
                        sum += t.friction.get(s as usize).copied().unwrap_or(0.6);
                        n += 1;
                    }
                }
            }
        }
        sum / n as f32
    }

    /// Water surface above the boat (splashdown).
    fn water_level_above(&self, w: &dyn World, t: &Tables) -> f32 {
        let b = self.bounding_box();
        let (x0, x1) = (b.min[0].floor() as i32, b.max[0].ceil() as i32);
        let (z0, z1) = (b.min[2].floor() as i32, b.max[2].ceil() as i32);
        let y0 = b.max[1].floor() as i32;
        let y1 = (b.max[1] - self.last_vy).ceil() as i32;
        'layer: for y in y0..y1 {
            let mut h = 0.0f32;
            for x in x0..x1 {
                for z in z0..z1 {
                    if let Some((wh, _)) = Self::water_at(t, w, x, y, z) {
                        h = h.max(wh);
                    }
                    if h >= 1.0 {
                        continue 'layer;
                    }
                }
            }
            if h < 1.0 {
                // Position of the last block examined in the layer (same y).
                return y as f32 + h;
            }
        }
        (y1 + 1) as f32
    }

    fn float_boat(&mut self, w: &dyn World, t: &Tables) {
        let g = 0.04f64;
        let mut vspeed = -g;
        let mut buoyancy = 0.0f64;
        let k;
        if self.old_status == Status::InAir && !matches!(self.status, Status::InAir | Status::OnLand) {
            // Splashdown: placed on the surface if there is room, without damping this tick.
            self.water_level = self.pos[1] + self.height;
            let target = f64::from(self.water_level_above(w, t)) - self.height + 0.101;
            if is_free(w, &self.bounding_box().offset(0.0, target - self.pos[1], 0.0)) {
                self.pos[1] = target;
                self.vel[1] = 0.0;
                self.last_vy = 0.0;
            }
            self.status = Status::InWater;
            return;
        }
        match self.status {
            Status::InWater => {
                buoyancy = (self.water_level - self.pos[1]) / self.height;
                k = 0.9;
            }
            Status::UnderFlowingWater => {
                vspeed = -7.0e-4;
                k = 0.9;
            }
            Status::UnderWater => {
                buoyancy = f64::from(0.01f32);
                k = 0.45;
            }
            Status::InAir => k = 0.9,
            Status::OnLand => {
                k = self.land_friction;
                self.land_friction /= 2.0;
            }
        }
        let kf = f64::from(k);
        self.vel[0] *= kf;
        self.vel[1] += vspeed;
        self.vel[2] *= kf;
        self.rotation *= k;
        if buoyancy > 0.0 {
            self.vel[1] = (self.vel[1] + buoyancy * (g / 0.65)) * 0.75;
        }
    }

    fn control_boat(&mut self, c: &Controls) {
        let mut a = 0.0f32;
        if c.left {
            self.rotation -= 1.0;
        }
        if c.right {
            self.rotation += 1.0;
        }
        if c.right != c.left && !c.forward && !c.backward {
            a += 0.005;
        }
        self.yaw += self.rotation;
        if c.forward {
            a += 0.04;
        }
        if c.backward {
            a -= 0.005;
        }
        self.vel[0] += f64::from(sin_t(-self.yaw * DEG) * a);
        self.vel[2] += f64::from(cos_t(self.yaw * DEG) * a);
        self.paddles = ((c.right && !c.left) || c.forward, (c.left && !c.right) || c.forward);
    }

    // ---------- Living mounts ----------

    fn tick_living(&mut self, p: &Pilot, w: &dyn World, t: &Tables) -> Out {
        let mut out = Out::default();
        // Base tick: fluids and currents (the nautilus is not pushed).
        self.update_fluids(w, t, self.kind != Kind::Nautilus);
        // Stop thresholds of the mobs.
        for a in [0, 2] {
            if self.vel[a].abs() < 0.003 {
                self.vel[a] = 0.0;
            }
        }
        if self.vel[1].abs() < 0.003 {
            self.vel[1] = 0.0;
        }
        // Piloted mount: input, setting, speed, movement.
        let input = self.ridden_input(p);
        self.tick_ridden(p, input, w, t, &mut out);
        let speed = self.ridden_speed(p);
        self.travel(input, speed, w, t);
        // End of the own tick.
        match self.kind {
            Kind::Equine { .. } if !self.standing => self.allow_slide = false,
            Kind::Camel => {
                if !self.standing {
                    self.allow_slide = false;
                }
                let liquid = self.water_height > 0.0 || self.lava_height > 0.0;
                if self.dashing && self.dash_cooldown < 50 && (self.on_ground || liquid) {
                    self.dashing = false;
                }
                if self.dash_cooldown > 0 {
                    self.dash_cooldown -= 1;
                }
            }
            Kind::Nautilus => {
                if self.dashing && self.dash_cooldown < 35 {
                    self.dashing = false;
                }
                if self.dash_cooldown > 0 {
                    self.dash_cooldown -= 1;
                }
            }
            _ => {}
        }
        out
    }

    fn ridden_input(&self, p: &Pilot) -> [f64; 3] {
        let c = &p.controls;
        match self.kind {
            Kind::Equine { .. } | Kind::Camel => {
                if self.kind == Kind::Camel && self.refuse_move {
                    return [0.0; 3];
                }
                if self.on_ground && self.pending_jump == 0.0 && self.standing && !self.allow_slide {
                    return [0.0; 3];
                }
                let side = c.xxa * 0.5;
                let fwd = if c.zza <= 0.0 { c.zza * 0.25 } else { c.zza };
                [f64::from(side), 0.0, f64::from(fwd)]
            }
            Kind::Pig | Kind::Strider => [0.0, 0.0, 1.0],
            Kind::HappyGhast | Kind::Nautilus => {
                let (mut fwd, mut up) = (0.0f32, 0.0f32);
                if c.zza != 0.0 {
                    let mut fl = cos_t(p.pitch * DEG);
                    let mut ul = -sin_t(p.pitch * DEG);
                    if c.zza < 0.0 {
                        fl *= -0.5;
                        ul *= -0.5;
                    }
                    up = ul;
                    fwd = fl;
                }
                if self.kind == Kind::Nautilus {
                    return [f64::from(c.xxa), f64::from(up), f64::from(fwd)];
                }
                if c.jump {
                    up += 0.5;
                }
                let k = f64::from(3.9f32) * self.attrs.flying_speed;
                [f64::from(c.xxa) * k, f64::from(up) * k, f64::from(fwd) * k]
            }
            Kind::Boat => [0.0; 3],
        }
    }

    fn set_rot(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw % 360.0;
        self.pitch = pitch % 360.0;
    }

    fn tick_ridden(&mut self, p: &Pilot, input: [f64; 3], w: &dyn World, t: &Tables, out: &mut Out) {
        match self.kind {
            Kind::Equine { .. } | Kind::Camel => {
                if !(self.kind == Kind::Camel && self.refuse_move) {
                    self.set_rot(p.yaw, p.pitch * 0.5);
                }
                if self.on_ground {
                    if self.pending_jump > 0.0 {
                        let a = self.pending_jump;
                        if self.kind == Kind::Camel {
                            self.dash(a, w, t);
                        } else {
                            self.horse_jump(a, input, w, t);
                        }
                    }
                    self.pending_jump = 0.0;
                }
                if self.kind == Kind::Camel && p.controls.zza > 0.0 && self.sitting_still {
                    out.stand_up = true;
                }
            }
            Kind::Pig | Kind::Strider => {
                self.set_rot(p.yaw, p.pitch * 0.5);
                if let Some((n, total)) = &mut self.boost {
                    let was = *n;
                    *n += 1;
                    if was > *total {
                        self.boost = None;
                    }
                }
            }
            Kind::HappyGhast | Kind::Nautilus => {
                let k = if self.kind == Kind::HappyGhast { 0.08 } else { 0.5 };
                let yaw = self.yaw + wrap(p.yaw - self.yaw) * k;
                self.set_rot(yaw, p.pitch * 0.5);
                if self.kind == Kind::Nautilus {
                    if self.pending_jump > 0.0 {
                        let a = self.pending_jump;
                        let look = view_vector(p.yaw, p.pitch);
                        let s = f64::from(if self.water_height > 0.0 { 1.2f32 } else { 0.5 } * a)
                            * self.attrs.movement_speed
                            * f64::from(self.speed_factor(w, t));
                        for (v, l) in self.vel.iter_mut().zip(look) {
                            *v += l * s;
                        }
                        self.dash_cooldown = 40;
                        self.dashing = true;
                    }
                    self.pending_jump = 0.0;
                }
            }
            Kind::Boat => {}
        }
    }

    fn jump_factor(&self, w: &dyn World, t: &Tables) -> f32 {
        let here = Self::state(w, self.block_pos());
        let f = t.jump_factor.get(here as usize).copied().unwrap_or(1.0);
        if f == 1.0 {
            let b = Self::state(w, self.below(t, w, 0.500_001));
            t.jump_factor.get(b as usize).copied().unwrap_or(1.0)
        } else {
            f
        }
    }

    fn jump_power(&self, amount: f32, w: &dyn World, t: &Tables) -> f32 {
        let boost = if self.jump_boost > 0 { 0.1 * self.jump_boost as f32 } else { 0.0 };
        self.attrs.jump_strength as f32 * amount * self.jump_factor(w, t) + boost
    }

    fn horse_jump(&mut self, a: f32, input: [f64; 3], w: &dyn World, t: &Tables) {
        self.vel[1] = f64::from(self.jump_power(a, w, t));
        if input[2] > 0.0 {
            let (s, c) = (sin_t(self.yaw * DEG), cos_t(self.yaw * DEG));
            self.vel[0] += f64::from(-0.4 * s * a);
            self.vel[2] += f64::from(0.4 * c * a);
        }
    }

    fn dash(&mut self, a: f32, w: &dyn World, t: &Tables) {
        let momentum = f64::from(self.jump_power(1.0, w, t));
        let look = view_vector(self.yaw, self.pitch);
        let h = (look[0] * look[0] + look[2] * look[2]).sqrt();
        let dir = if h < 1.0e-5 { [0.0; 3] } else { [look[0] / h, 0.0, look[2] / h] };
        let k = f64::from(22.2222f32 * a) * self.attrs.movement_speed * f64::from(self.speed_factor(w, t));
        self.vel[0] += dir[0] * k;
        self.vel[1] += f64::from(1.4285f32 * a) * momentum;
        self.vel[2] += dir[2] * k;
        self.dash_cooldown = 55;
        self.dashing = true;
    }

    fn boost_factor(&self) -> f32 {
        match self.boost {
            Some((n, total)) => 1.0 + 1.15 * crate::sin_t(n as f32 / total as f32 * std::f32::consts::PI),
            None => 1.0,
        }
    }

    fn ridden_speed(&self, p: &Pilot) -> f32 {
        let s = self.attrs.movement_speed;
        match self.kind {
            Kind::Camel => s as f32 + if p.sprinting && self.dash_cooldown == 0 { 0.1 } else { 0.0 },
            Kind::Pig => (s * 0.225 * f64::from(self.boost_factor())) as f32,
            Kind::Strider => {
                (s * f64::from(if self.suffocating { 0.35f32 } else { 0.55 }) * f64::from(self.boost_factor())) as f32
            }
            Kind::Nautilus => (if self.water_height > 0.0 { 0.0325 } else { 0.02 }) * s as f32,
            _ => s as f32,
        }
    }

    /// Acceleration relative to the yaw (`movement.md` § 4.3).
    fn move_relative(&mut self, speed: f32, input: [f64; 3]) {
        let l2i = l2(input);
        if l2i < 1.0e-7 {
            return;
        }
        let mut v = input;
        if l2i > 1.0 {
            let l = l2i.sqrt();
            v = v.map(|c| c / l);
        }
        v = v.map(|c| c * f64::from(speed));
        let (s, c) = (f64::from(sin_t(self.yaw * DEG)), f64::from(cos_t(self.yaw * DEG)));
        self.vel[0] += v[0] * c - v[2] * s;
        self.vel[1] += v[1];
        self.vel[2] += v[2] * c + v[0] * s;
    }

    fn travel(&mut self, mut input: [f64; 3], speed: f32, w: &dyn World, t: &Tables) {
        let in_water = self.water_height > 0.0;
        let in_lava = self.lava_height > 0.0;
        match self.kind {
            Kind::HappyGhast => {
                let s = self.attrs.flying_speed as f32 * 5.0 / 3.0;
                self.move_relative(s, input);
                self.do_move(self.vel, w, t, self.step());
                let k = if in_water {
                    f64::from(0.8f32)
                } else if in_lava {
                    0.5
                } else {
                    f64::from(0.91f32)
                };
                self.vel = self.vel.map(|v| v * k);
                return;
            }
            Kind::Camel if self.refuse_move && self.on_ground => {
                self.vel[0] = 0.0;
                self.vel[2] = 0.0;
                input = [0.0, input[1], 0.0];
            }
            _ => {}
        }
        // The fluid at the feet block: the strider stands on lava.
        let here = t.fluids.fluid(Self::state(w, self.block_pos()));
        let stands = self.kind == Kind::Strider && here == Fluid::Lava;
        if (in_water || in_lava) && !stands {
            self.travel_fluid(input, speed, w, t);
        } else {
            self.travel_air(input, speed, w, t);
        }
    }

    fn step(&self) -> f64 {
        self.attrs.step_height.max(1.0)
    }

    fn travel_air(&mut self, input: [f64; 3], speed: f32, w: &dyn World, t: &Tables) {
        let fb = if self.on_ground {
            let s = Self::state(w, self.below(t, w, 0.500_001));
            corrected(t.friction.get(s as usize).copied().unwrap_or(0.6), self.attrs.friction_modifier)
        } else {
            1.0
        };
        let a = if self.on_ground {
            if f64::from(fb) > 0.6 { speed * (0.216_000_02 / (fb * fb * fb)) } else { speed }
        } else {
            speed * 0.1
        };
        self.move_relative(a, input);
        let on_climbable = t.climbable(Self::state(w, self.block_pos()));
        if on_climbable {
            self.fall_distance = 0.0;
            self.vel[0] = self.vel[0].clamp(-0.15, 0.15);
            self.vel[2] = self.vel[2].clamp(-0.15, 0.15);
            self.vel[1] = self.vel[1].max(-0.15);
        }
        self.do_move(self.vel, w, t, self.step());
        if self.horizontal_collision && on_climbable {
            self.vel[1] = 0.2;
        }
        let fp = self.below(t, w, 0.500_001);
        if w.state(fp[0], fp[1], fp[2]).is_some() {
            self.vel[1] -= self.attrs.gravity;
        } else {
            self.vel[1] = if self.pos[1] > -64.0 { -0.1 } else { 0.0 };
        }
        let air = corrected(0.91, self.attrs.air_drag_modifier);
        let h = f64::from(fb * air);
        self.vel[0] *= h;
        self.vel[2] *= h;
        self.vel[1] *= f64::from(corrected(0.98, self.attrs.air_drag_modifier));
    }

    fn fluid_gravity(&mut self, g: f64, falling: bool) {
        if g == 0.0 {
            return;
        }
        if falling && (self.vel[1] - 0.005).abs() >= 0.003 && (self.vel[1] - g / 16.0).abs() < 0.003 {
            self.vel[1] = -0.003;
        } else {
            self.vel[1] -= g / 16.0;
        }
    }

    fn travel_fluid(&mut self, input: [f64; 3], speed: f32, w: &dyn World, t: &Tables) {
        let falling = self.vel[1] <= 0.0;
        let y0 = self.pos[1];
        let g = self.attrs.gravity;
        if self.water_height > 0.0 {
            if self.kind == Kind::Nautilus {
                self.move_relative(speed, input);
                self.do_move(self.vel, w, t, self.step());
                self.vel = self.vel.map(|v| v * 0.9);
                return;
            }
            let mut slow = match self.kind {
                Kind::Equine { water_slowdown, .. } => water_slowdown,
                _ => 0.8,
            };
            let mut a = 0.02f32;
            let mut eff = self.attrs.water_movement_efficiency as f32;
            if !self.on_ground {
                eff *= 0.5;
            }
            if eff > 0.0 {
                slow += (0.546_000_06 - slow) * eff;
                a += (speed - a) * eff;
            }
            self.move_relative(a, input);
            self.do_move(self.vel, w, t, self.step());
            if self.horizontal_collision && t.climbable(Self::state(w, self.block_pos())) {
                self.vel[1] = 0.2;
            }
            self.vel[0] *= f64::from(slow);
            self.vel[1] *= f64::from(0.8f32);
            self.vel[2] *= f64::from(slow);
            self.fluid_gravity(g, falling);
        } else {
            self.move_relative(0.02, input);
            self.do_move(self.vel, w, t, self.step());
            if self.lava_height <= 0.4 {
                self.vel[0] *= 0.5;
                self.vel[1] *= f64::from(0.8f32);
                self.vel[2] *= 0.5;
                self.fluid_gravity(g, falling);
            } else {
                self.vel = self.vel.map(|v| v * 0.5);
            }
            if g != 0.0 {
                self.vel[1] -= g / 4.0;
            }
        }
        // Leaving the fluid.
        if self.horizontal_collision {
            let b = self.bounding_box().offset(
                self.vel[0],
                self.vel[1] + f64::from(0.6f32) - self.pos[1] + y0,
                self.vel[2],
            );
            if is_free(w, &b) {
                self.vel[1] = f64::from(0.3f32);
            }
        }
        // Mount that floats when ridden (jump threshold in fluid 0.4), in water
        // only (fluid tag `entity_floatable`).
        if matches!(self.kind, Kind::Equine { float: true, .. } | Kind::Camel) && self.water_height > 0.4 {
            self.vel[1] += f64::from(0.04f32);
        }
    }

    /// Strider in lava: carried if above the supporting surface, otherwise it rises.
    fn float_strider(&mut self, w: &dyn World, t: &Tables) {
        if self.lava_height <= 0.0 {
            return;
        }
        let p = self.block_pos();
        let above = t.fluids.fluid(Self::state(w, [p[0], p[1] + 1, p[2]]));
        if self.pos[1] > f64::from(p[1]) + 0.5 - 1.0e-5 && above != Fluid::Lava {
            self.on_ground = true;
        } else {
            self.vel = [self.vel[0] * 0.5, self.vel[1] * 0.5 + 0.05, self.vel[2] * 0.5];
        }
    }

    // ---------- Common ----------

    fn update_fluids(&mut self, w: &dyn World, t: &Tables, pushed: bool) {
        let b = self.bounding_box();
        let (water, lava, _) =
            crate::scan_fluids_entity(w, t, &b, self.pos, self.pos[1], pushed, self.fast_lava, &mut self.vel);
        self.water_height = water;
        self.lava_height = lava;
        if water > 0.0 && self.is_living() {
            self.fall_distance = 0.0;
        }
    }

    fn speed_factor(&self, w: &dyn World, t: &Tables) -> f32 {
        let here = Self::state(w, self.block_pos());
        let mut f = t.speed_factor.get(here as usize).copied().unwrap_or(1.0);
        if !t.speed_factor_here_only(here) && f == 1.0 {
            let b = Self::state(w, self.below(t, w, 0.500_001));
            f = t.speed_factor.get(b as usize).copied().unwrap_or(1.0);
        }
        if self.is_living() {
            let e = self.attrs.movement_efficiency as f32;
            f + (1.0 - f) * e
        } else {
            f
        }
    }

    /// Movement with collisions (`movement.md` § 6), flags, bounce and speed factor.
    fn do_move(&mut self, d: [f64; 3], w: &dyn World, t: &Tables, step: f64) {
        let start = self.bounding_box();
        let m = collide::resolve(w, &start, d, self.on_ground, step);
        if l2(m) > 1.0e-7 || l2(d) - l2(m) < 1.0e-7 {
            for (p, v) in self.pos.iter_mut().zip(m) {
                *p += v;
            }
        }
        let cx = (d[0] - m[0]).abs() >= 1.0e-5;
        let cz = (d[2] - m[2]).abs() >= 1.0e-5;
        self.horizontal_collision = cx || cz;
        self.vertical_collision = d[1] != m[1];
        self.on_ground = self.vertical_collision && d[1] < 0.0;
        self.support = if self.on_ground { support_of(w, &self.bounding_box(), self.pos) } else { None };
        // Fall (the boat also remembers the last vertical speed).
        if self.kind == Kind::Boat {
            self.last_vy = self.vel[1];
            let below = Self::state(w, [self.block_pos()[0], self.block_pos()[1] - 1, self.block_pos()[2]]);
            if self.on_ground {
                self.fall_distance = 0.0;
            } else if t.fluids.fluid(below) != Fluid::Water && m[1] < 0.0 {
                self.fall_distance -= m[1];
            }
        } else if self.on_ground {
            self.fall_distance = 0.0;
        } else if m[1] < 0.0 {
            self.fall_distance -= m[1];
        }
        if (d[1].abs() > 0.0 && self.vertical_collision) || self.horizontal_collision {
            self.restitute(cx, cz, m, w, t);
        }
        let f = f64::from(self.speed_factor(w, t));
        self.vel[0] *= f;
        self.vel[2] *= f;
    }

    fn restitute(&mut self, cx: bool, cz: bool, m: [f64; 3], w: &dyn World, t: &Tables) {
        let v = self.vel;
        let mut r = if self.is_living() { self.attrs.bounciness } else { 0.0 };
        if cx {
            self.vel[0] = -v[0] * r;
        }
        if cz {
            self.vel[2] = -v[2] * r;
        }
        if self.vertical_collision {
            let g = if self.is_living() { self.attrs.gravity } else { 0.04 };
            if self.on_ground {
                let s = Self::state(w, self.below(t, w, 0.2));
                r = if -v[1] > g && !t.suppresses_bounce(s) {
                    // A non-living entity (boat, raft) only gives back 0.8 of the block's momentum.
                    let mut block = t.bounce.get(s as usize).copied().unwrap_or(0.0);
                    if !self.is_living() {
                        block *= 0.8;
                    }
                    r.max(f64::from(block))
                } else {
                    0.0
                };
            }
            let (gc, drag) = if r > 0.0 {
                let p = m[1] / v[1];
                let air = if self.is_living() { f64::from(corrected(0.98, self.attrs.air_drag_modifier)) } else { 1.0 };
                (p * g, 1.0 + p * (air - 1.0))
            } else {
                (0.0, 1.0)
            };
            self.vel[1] = (gc - v[1]) * drag * r;
        }
    }
}

/// World seen by the strider: a lava source with no lava above carries a half-slab when
/// its feet are above it (§ 3.5).
struct LavaFloor<'a> {
    w: &'a dyn World,
    t: &'a Tables,
    feet: f64,
}

impl World for LavaFloor<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.w.state(x, y, z)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        let big = self.w.collision(x, y, z, out);
        let s = self.w.state(x, y, z).unwrap_or(0);
        let f = &self.t.fluids;
        if f.fluid(s) == Fluid::Lava
            && f.source.get(s as usize).copied().unwrap_or(false)
            && self.feet > f64::from(y) + 0.5 - 1.0e-5
            && f.fluid(self.w.state(x, y + 1, z).unwrap_or(0)) != Fluid::Lava
        {
            out.push(Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] });
        }
        big
    }

    fn border(&self) -> Option<&crate::WorldBorder> {
        self.w.border()
    }
}

#[cfg(test)]
mod tests;
