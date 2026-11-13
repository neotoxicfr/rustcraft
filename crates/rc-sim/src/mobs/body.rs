//! Mob physics (`server-mobs.md` § 1): fluids, movement (`travel`) in
//! air, water and lava, collisions and step-up, jump, fall, mutual push.

use rc_physics::collide;

use super::env::Fluid;
use super::mob::{Ent, Mob};
use super::mth::{DEG, cos, sin};
use super::species::Kind;
use crate::server::Server;

/// `RUSTCRAFT_OPT_PUSHIDX=0`: each push finds its mob by `mob_index` (old
/// path) instead of the recorded slot.
fn push_reuses_recorded_slot() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_PUSHIDX") != Some("0"))
}

/// `RUSTCRAFT_OPT_STEPON_CACHE=0`: the block stepped on always recomputes its effect cell instead
/// of reusing the one from the last movement.
fn step_on_reuses_effect_cell() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_STEPON_CACHE") != Some("0"))
}

/// `RUSTCRAFT_OPT_TEAMIDX=0`: the push team filter walks the members of each team with the UUID
/// name recreated (old path) instead of using the scoreboard index.
fn push_uses_team_index() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_TEAMIDX") != Some("0"))
}

/// How a mob scans the list for the mobs it pushes: the cheap tests run on every mob of the list
/// before its box is computed. Every scan keeps the same mobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PushScan {
    /// Species bound around the position first, bound table fetched once for the whole scan,
    /// then liveness.
    BoundThenLiveness,
    /// `BoundThenLiveness` with the comparisons of the bound combined before one jump (27 to
    /// 70 % slower than `BoundThenLiveness` at the micro-bench, `docs/perf.md`).
    BoundAllAtOnceThenLiveness,
    /// Liveness first, then the bound with its table fetched for every mob (old path).
    LivenessThenBound,
}

#[cfg(test)]
impl PushScan {
    pub(super) const EVERY: [PushScan; 3] =
        [PushScan::LivenessThenBound, PushScan::BoundThenLiveness, PushScan::BoundAllAtOnceThenLiveness];
}

/// `RUSTCRAFT_OPT_PUSH_SCAN_BOUND_FIRST=0`: `PushScan::LivenessThenBound`; otherwise (default)
/// `BoundThenLiveness`, or `BoundAllAtOnceThenLiveness` with
/// `RUSTCRAFT_OPT_PUSH_BOUND_ALL_AT_ONCE=1`.
fn push_scan() -> PushScan {
    if !rc_data::opt_switch!("RUSTCRAFT_OPT_PUSH_SCAN_BOUND_FIRST") {
        PushScan::LivenessThenBound
    } else if rc_data::opt_switch!("RUSTCRAFT_OPT_PUSH_BOUND_ALL_AT_ONCE", default_off) {
        PushScan::BoundAllAtOnceThenLiveness
    } else {
        PushScan::BoundThenLiveness
    }
}

/// An entity pushed by a mob: the entity and its slot of the mob list when it was recorded
/// (`PLAYER_CONTACT_SLOT` for the player).
pub(crate) type PushContact = (Ent, u32);

const PLAYER_CONTACT_SLOT: u32 = u32::MAX;

/// Contacts of one push stored in place up to eight (a crowded mob rarely touches more).
type InlineContacts = smallvec::SmallVec<[PushContact; 8]>;

/// Where the contacts of one mob's push are collected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PushContactStorage {
    /// The buffer kept by `Mobs`: no allocation once it has grown.
    KeptBuffer,
    /// A list in place on the stack, on the heap past eight contacts.
    Inline,
    /// A new vector for each mob that touches something (old path).
    FreshVec,
}

/// `RUSTCRAFT_OPT_PUSH_CONTACTS_INLINE=1` (default off): `PushContactStorage::Inline`; otherwise
/// `RUSTCRAFT_OPT_PUSH_CONTACT_BUFFER` (default on): `KeptBuffer`, `=0`: `FreshVec`.
fn push_contact_storage() -> PushContactStorage {
    if rc_data::opt_switch!("RUSTCRAFT_OPT_PUSH_CONTACTS_INLINE", default_off) {
        PushContactStorage::Inline
    } else if rc_data::opt_switch!("RUSTCRAFT_OPT_PUSH_CONTACT_BUFFER") {
        PushContactStorage::KeptBuffer
    } else {
        PushContactStorage::FreshVec
    }
}

/// Team filter of a push as seen from the pushing entity (`Server::push_team`): its team rank and
/// collision rule, `None` when no team restricts collisions.
type PushTeamFilter = Option<(Option<u32>, u8)>;

/// `RUSTCRAFT_OPT_SUPPORTSHARE` (default on): the support search after a mob's movement filters the
/// block boxes the movement collected instead of walking the same cells again.
fn support_share() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SUPPORTSHARE")
}

/// `RUSTCRAFT_OPT_FREEZE_SKIP_UNFROZEN` (default on): the freezing step reads the block under the
/// feet and the freeze-immune armour only when a frozen mob needs them.
fn freeze_skip_unfrozen() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_FREEZE_SKIP_UNFROZEN")
}

/// `RUSTCRAFT_OPT_FREEZESHARE` (default on): the freezing step reuses the cell 0.2 under the feet
/// that the movement of the same tick computed for the same position and support.
fn freeze_share() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_FREEZESHARE")
}

/// `RUSTCRAFT_OPT_STEPSOUNDREF` (default on): a mob's own step sound is borrowed when it is a
/// fixed name instead of being copied into a new string at every step.
fn step_sound_ref() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_STEPSOUNDREF")
}

thread_local! {
    /// Block boxes of this thread's last mob movement, read by its support search
    /// (`RUSTCRAFT_OPT_SUPPORTSHARE`).
    static MOVE_BOXES: std::cell::RefCell<collide::CollectedBlocks> = std::cell::RefCell::new(collide::CollectedBlocks::default());
}

/// `f` with the thread's kept movement boxes (fresh ones if a movement is already using them).
fn with_move_boxes<R>(f: impl FnOnce(&mut collide::CollectedBlocks) -> R) -> R {
    MOVE_BOXES.with(|kept| match kept.try_borrow_mut() {
        Ok(mut kept) => f(&mut kept),
        Err(_) => f(&mut collide::CollectedBlocks::default()),
    })
}

/// Per-mob body state, outside physics: amethyst chime of the steps (intensity, tick of the
/// last chime), last seen size (width, height) for the refit after growth (`None` before the
/// first tick), effect cell of the last movement with the tick, position and support that
/// produced it.
#[derive(Debug, Clone, Default)]
pub struct BodyFx {
    pub crystal: f32,
    pub crystal_tick: i32,
    pub dims: Option<(f32, f32)>,
    pub effect: Option<EffectAt>,
}

/// Effect cell (0.2 below the feet) computed in a tick for a position and a support:
/// (tick, position, support, cell). Reused only in that tick: the support's state may change
/// afterwards without any movement (a mob without AI on a carpet over magma, the carpet
/// replaced by a wall: the cell becomes the wall).
pub type EffectAt = (i32, [f64; 3], Option<[i32; 3]>, [i32; 3]);

/// Species whose fall check is empty in the game (flyers): the fall does not accumulate, no
/// block is touched (farmland, bed bounce), no landing event.
fn ignores_falls(k: Kind) -> bool {
    matches!(k, Kind::Bat | Kind::Allay | Kind::Bee | Kind::HappyGhast | Kind::Parrot | Kind::Ghast | Kind::Phantom)
}

fn length_sq(v: [f64; 3]) -> f64 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

/// Pose of a player (host or guest), the game's poses a player takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlayerPose {
    #[default]
    Standing,
    FallFlying,
    Asleep,
    Swimming,
    SpinAttack,
    Crouching,
}

/// What decides the pose a player asks for: read from the installed player
/// ([`Server::player_pose`]) or from a guest's kept state ([`guest_pose`]).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PoseWish {
    pub asleep: bool,
    pub swimming: bool,
    pub fall_flying: bool,
    pub spin_attack: bool,
    pub shift_held: bool,
    pub flying: bool,
    /// Spectator or passenger: the pose asked for is taken whatever room there is.
    pub ignores_room: bool,
}

/// Tallest player box that fits where the player stands, measured once per player tick
/// ([`Server::update_player_pose`]). The boxes share their base and width, so a taller one that
/// fits means every shorter one fits too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum Headroom {
    /// Not even the flat box fits (inside a wall): the pose stays what it was.
    None,
    Flat,
    Crouching,
    /// The default before any measure: nothing forces a pose.
    #[default]
    Standing,
}

/// The room around a player at its last tick, and the pose it had then.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PoseRoom {
    pub headroom: Headroom,
    pub last: PlayerPose,
}

impl PlayerPose {
    /// The pose asked for, in the game's order: asleep, swimming, gliding, spinning with a
    /// trident, crouching (shift held while not flying), otherwise standing.
    pub(crate) fn wanted(w: PoseWish) -> PlayerPose {
        if w.asleep {
            PlayerPose::Asleep
        } else if w.swimming {
            PlayerPose::Swimming
        } else if w.fall_flying {
            PlayerPose::FallFlying
        } else if w.spin_attack {
            PlayerPose::SpinAttack
        } else if w.shift_held && !w.flying {
            PlayerPose::Crouching
        } else {
            PlayerPose::Standing
        }
    }

    /// The pose taken with `room` around, the game's end-of-tick choice: the one asked for if
    /// its box fits (always for a spectator or a passenger), otherwise crouching if that fits,
    /// otherwise flat (crawling under a one-block ceiling); unchanged when not even the flat box
    /// fits.
    pub(crate) fn resolve(w: PoseWish, room: PoseRoom) -> PlayerPose {
        if room.headroom == Headroom::None {
            return room.last;
        }
        let wanted = PlayerPose::wanted(w);
        if w.ignores_room || wanted.headroom_needed() <= room.headroom {
            wanted
        } else if room.headroom >= Headroom::Crouching {
            PlayerPose::Crouching
        } else {
            PlayerPose::Swimming
        }
    }

    /// Room its box needs.
    fn headroom_needed(self) -> Headroom {
        match self {
            PlayerPose::Standing => Headroom::Standing,
            PlayerPose::Crouching => Headroom::Crouching,
            _ => Headroom::Flat,
        }
    }

    /// Box of the pose: width, height, eye height.
    pub(crate) fn dims(self) -> (f64, f64, f64) {
        match self {
            PlayerPose::Asleep => {
                let s = f64::from(0.2f32);
                (s, s, s)
            }
            PlayerPose::FallFlying | PlayerPose::Swimming | PlayerPose::SpinAttack => (0.6, 0.6, f64::from(0.4f32)),
            PlayerPose::Crouching => (0.6, 1.5, 1.27),
            PlayerPose::Standing => (0.6, 1.8, 1.62),
        }
    }

    /// Box of the pose for a player of scale `scale` (its scale attribute): every box but the
    /// sleeping one grows with it, like the game's.
    pub(crate) fn scaled_dims(self, scale: f32) -> (f64, f64, f64) {
        let (w, h, eye) = self.dims();
        if self == PlayerPose::Asleep || scale == 1.0 {
            return (w, h, eye);
        }
        let s = f64::from(scale);
        (w * s, h * s, eye * s)
    }

    /// Box of the pose for feet at `pos`, for a player of scale `scale`.
    pub(crate) fn bbox_at(self, pos: [f64; 3], scale: f32) -> rc_physics::Aabb {
        let (w, h, _) = self.scaled_dims(scale);
        let r = w / 2.0;
        rc_physics::Aabb { min: [pos[0] - r, pos[1], pos[2] - r], max: [pos[0] + r, pos[1] + h, pos[2] + r] }
    }

    /// Id of the pose in the entity data (data 6).
    pub(crate) fn network_id(self) -> i32 {
        match self {
            PlayerPose::Standing => 0,
            PlayerPose::FallFlying => 1,
            PlayerPose::Asleep => 2,
            PlayerPose::Swimming => 3,
            PlayerPose::SpinAttack => 4,
            PlayerPose::Crouching => 5,
        }
    }
}

/// Pose of a guest from its kept state (not installed), by the same rule as the host's.
pub(crate) fn guest_pose(c: &crate::lan::ctx::PlayerCtx) -> PlayerPose {
    let wish = PoseWish {
        asleep: c.vit.sleeping.is_some(),
        swimming: c.swimming,
        fall_flying: c.proj_player.fall_flying,
        spin_attack: c.proj_player.spin > 0,
        shift_held: c.sneaking,
        flying: c.flying,
        ignores_room: c.game_mode == 3 || c.ride.vehicle.is_some() || c.pcart.vehicle.is_some() || c.seat.is_some(),
    };
    PlayerPose::resolve(wish, c.vit.pose_room)
}

/// Box of a guest from its kept state: its pose's, at its scale.
pub(crate) fn guest_box(c: &crate::lan::ctx::PlayerCtx) -> rc_physics::Aabb {
    guest_pose(c).bbox_at(c.pos, c.attrs.scale())
}

/// Width, height and eye height of a guest from its kept state: its pose's, at its scale.
pub(crate) fn guest_dims(c: &crate::lan::ctx::PlayerCtx) -> (f64, f64, f64) {
    guest_pose(c).scaled_dims(c.attrs.scale())
}

/// Powder snow as seen by a mob: low shape (0.9) after a fall of more than 2.5 blocks;
/// otherwise full block for a species in `#powder_snow_walkable_mobs` (or wearing leather
/// boots) whose bottom is above the block; empty for the others.
struct SnowStand<'a> {
    /// The world the snow shapes are added to.
    base: &'a dyn rc_physics::World,
    server: &'a Server,
    bottom: f64,
    falling: bool,
    walker: bool,
}

impl rc_physics::World for SnowStand<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.base.state(x, y, z)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        let over = self.base.collision(x, y, z, out);
        if out.is_empty() && self.is_powder_snow([x, y, z], || self.base.state(x, y, z)) {
            self.push_snow_shape(y, out);
        }
        over
    }

    fn states_in(&self, first: [i32; 3], last: [i32; 3], out: &mut [u32]) -> bool {
        self.base.states_in(first, last, out)
    }

    /// The state the walk read serves the wrapped world and the snow test (no second read).
    fn collision_of_state(&self, state: Option<u32>, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        let over = self.base.collision_of_state(state, x, y, z, out);
        if out.is_empty() && self.is_powder_snow([x, y, z], || state) {
            self.push_snow_shape(y, out);
        }
        over
    }

    fn border(&self) -> Option<&rc_physics::WorldBorder> {
        self.base.border()
    }

    fn asker_position(&self) -> Option<[f64; 2]> {
        self.base.asker_position()
    }

    fn entity_colliders(&self, area: &rc_physics::Aabb, out: &mut Vec<rc_physics::Aabb>) {
        self.base.entity_colliders(area, out);
    }

    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        self.base.moving_collision(x, y, z, out)
    }

    /// The snow shapes stay inside the cell.
    fn shape_reach(&self, lo: [i32; 3], hi: [i32; 3]) -> rc_physics::Aabb {
        self.base.shape_reach(lo, hi).union(&rc_physics::Aabb::UNIT)
    }
}

impl SnowStand<'_> {
    /// Powder snow at `p` (state given by `state`): by the per-state flag of the physics tables,
    /// or by the block name (`RUSTCRAFT_OPT_STAND_SHAPE_FLAGS=0`, the old path, which compared it
    /// for every empty cell).
    fn is_powder_snow(&self, p: [i32; 3], state: impl FnOnce() -> Option<u32>) -> bool {
        if rc_data::opt_switch!("RUSTCRAFT_OPT_STAND_SHAPE_FLAGS") {
            state().is_some_and(|s| self.server.phys.is_powder_snow(s))
        } else {
            self.server.bname(p) == "minecraft:powder_snow"
        }
    }

    /// Shape of a powder snow cell of row `y` for this mob.
    fn push_snow_shape(&self, y: i32, out: &mut Vec<rc_physics::Aabb>) {
        if self.falling {
            out.push(rc_physics::Aabb { min: [0.0; 3], max: [1.0, f64::from(0.9f32), 1.0] });
        } else if self.walker && self.bottom > f64::from(y) + 1.0 - f64::from(1.0e-5f32) {
            out.push(rc_physics::Aabb { min: [0.0; 3], max: [1.0; 3] });
        }
    }
}

/// Does the entity with rule `own` (team `own_team`, code from `commands::scores::collision_code`)
/// push the one with rule `their`? Like the official game (and the client, `player_env.rs`):
/// "never" on either side blocks; with "own team" on either side, two members of the same team
/// do NOT push each other (quirk of the game, reproduced); with "other teams" on either side,
/// only members of the same team push each other.
fn team_allows(own: u8, own_team: Option<u32>, their: u8, their_team: Option<u32>) -> bool {
    if own == 1 || their == 1 {
        return false;
    }
    let same = own_team.is_some() && own_team == their_team;
    if (own == 3 || their == 3) && same {
        return false;
    }
    (own != 2 && their != 2) || same
}

fn corrected(x: f32, m: f32) -> f32 {
    (1.0 - (1.0 - x) * m).clamp(0.0, 1.0)
}

/// Slowdown of the previous tick (cobweb, berries): the movement is multiplied and the velocity
/// reset to zero (piston pushes clear it without multiplying it, `piston.rs`).
fn slowed_by_last_tick(m: &mut Mob, delta: [f64; 3]) -> [f64; 3] {
    if length_sq(m.stuck) > 1.0e-7 {
        let slowed = std::array::from_fn(|i| delta[i] * m.stuck[i]);
        m.stuck = [0.0; 3];
        m.vel = [0.0; 3];
        return slowed;
    }
    delta
}

/// Result of a mob's movement against the collisions: the movement allowed and the horizontal
/// axes it blocked.
struct Moved {
    allowed: [f64; 3],
    blocked_x: bool,
    blocked_z: bool,
}

impl Server {
    /// Fluids touched and current, start of the base tick.
    pub(crate) fn mob_fluids(&self, m: &mut Mob) {
        let w = self.world();
        let eye = m.eye_y();
        let mut flow = m.vel;
        let (water, lava, eyes) =
            rc_physics::scan_fluids_entity(&w, &self.phys, &m.bbox(), m.pos(), eye, true, self.fast_lava(), &mut flow);
        // Aquatic fauna and nautiluses are not pushed by currents.
        if super::surface::pushed_by_fluid(m.kind)
            && super::aquatic::pushed_by_fluid(m)
            && !super::sealife::handles(m.kind)
            && super::brainy::pushed_by_fluid(m.kind)
        {
            m.vel = flow;
        }
        m.water_height = water;
        m.lava_height = lava;
        m.in_water = water > 0.0;
        m.eyes_in_water = eyes;
        if m.in_water {
            m.fall_distance = 0.0; // extinguishing happens in the crossed-block pass
        }
    }

    /// Block under the feet that acts on movement.
    pub(crate) fn mob_below(&self, m: &Mob, off: f32) -> [i32; 3] {
        self.on_pos(m.pos(), m.support, off)
    }

    /// Block under an entity at `off` below its feet (single-precision offset, like the game):
    /// main supporting block at 1e-5, otherwise its column at that height, except low walls, fence gates
    /// and (offset <= 0.5) fences that keep the supporter; without a supporter, the cell under `pos`.
    /// Single rule shared by mobs, player, minecarts and predicates.
    pub(crate) fn on_pos(&self, pos: [f64; 3], support: Option<[i32; 3]>, off: f32) -> [i32; 3] {
        let y = (pos[1] - f64::from(off)).floor() as i32;
        if let Some(s) = support {
            if off <= 1.0e-5 {
                return s;
            }
            return if self.keeps_support(s, off) { s } else { [s[0], y, s[2]] };
        }
        [pos[0].floor() as i32, y, pos[2].floor() as i32]
    }

    /// The supporting block `s` stays the block under the feet at offset `off`: a wall or a fence
    /// gate always, a fence up to half a block. One state read and the physics table
    /// (`RUSTCRAFT_OPT_MOBSTATES`), or three reads by tag and name (old path).
    fn keeps_support(&self, s: [i32; 3], off: f32) -> bool {
        if super::env::mob_states_on() {
            return self.phys.keeps_support(self.st(s), off);
        }
        let fence = self.btag_id(s, crate::tag_bits::TagId::Fences);
        (f64::from(off) <= 0.5 && fence)
            || self.btag_id(s, crate::tag_bits::TagId::Walls)
            || self.bname(s).ends_with("_fence_gate")
    }

    pub(crate) fn friction_at(&self, p: [i32; 3]) -> f32 {
        self.friction_of_state(self.st(p))
    }

    fn friction_of_state(&self, s: u32) -> f32 {
        self.phys.friction.get(s as usize).copied().unwrap_or(0.6)
    }

    fn speed_factor(&self, m: &Mob) -> f32 {
        // Soul Speed active, movement efficiency 1: no slowdown.
        if m.boots.soul_active {
            return 1.0;
        }
        let here = m.block_pos();
        let s = self.st(here);
        let f = self.phys.speed_factor.get(s as usize).copied().unwrap_or(1.0);
        if self.speed_factor_here_only(s) || f != 1.0 {
            return f;
        }
        let b = self.st(self.mob_below(m, 0.500_001));
        self.phys.speed_factor.get(b as usize).copied().unwrap_or(1.0)
    }

    /// Water or bubble column: the speed factor of the occupied block is final, the block below
    /// is not read. By a per-state flag of the physics tables (`RUSTCRAFT_OPT_MOBSTATES`) or by
    /// the block name (old path).
    fn speed_factor_here_only(&self, s: u32) -> bool {
        if super::env::mob_states_on() {
            return self.phys.speed_factor_here_only(s);
        }
        let name = self.state_name(s);
        name == "minecraft:water" || name == "minecraft:bubble_column"
    }

    fn jump_factor(&self, m: &Mob) -> f32 {
        let here = self.phys.jump_factor.get(self.st(m.block_pos()) as usize).copied().unwrap_or(1.0);
        let below = self.phys.jump_factor.get(self.st(self.mob_below(m, 0.500_001)) as usize).copied().unwrap_or(1.0);
        if here == 1.0 { below } else { here }
    }

    pub(crate) fn on_climbable(&self, m: &Mob) -> bool {
        if m.kind.base() == Kind::Spider {
            return m.climbing;
        }
        self.phys.climbable(self.st(m.block_pos()))
    }

    pub(crate) fn jump_from_ground(&mut self, m: &mut Mob) {
        // Cubes: imposed vertical velocity (no threshold or maximum), magma: + size × 0.1.
        if m.kind.is_cube() {
            let mut p = m.attrs.jump.value() as f32 * self.jump_factor(m);
            if m.kind == Kind::MagmaCube {
                p += m.cube.size as f32 * 0.1;
            }
            m.vel[1] = f64::from(p);
            m.needs_sync = true;
            return;
        }
        let p = m.attrs.jump.value() as f32 * super::surface::jump_mult(self, m) * self.jump_factor(m);
        if p <= 1.0e-5 {
            return;
        }
        m.vel[1] = f64::from(p).max(m.vel[1]);
        super::surface::after_jump(self, m);
        super::brainy::after_jump(m); // Sniffer
        m.needs_sync = true;
    }

    /// Acceleration relative to the yaw.
    fn move_relative(m: &mut Mob, speed: f32) {
        let input = [f64::from(m.xxa), f64::from(m.yya), f64::from(m.zza)];
        let len = length_sq(input);
        if len < 1.0e-7 {
            return;
        }
        let v = if len > 1.0 { input.map(|x| x / len.sqrt()) } else { input };
        let v = v.map(|x| x * f64::from(speed));
        let (s, c) = (sin(m.yaw * DEG), cos(m.yaw * DEG));
        m.vel[0] += v[0] * f64::from(c) - v[2] * f64::from(s);
        m.vel[1] += v[1];
        m.vel[2] += v[2] * f64::from(c) + v[0] * f64::from(s);
    }

    /// Collisions, flags, fall, step, speed factor.
    pub(crate) fn mob_move(&mut self, m: &mut Mob, delta: [f64; 3]) {
        let delta = slowed_by_last_tick(m, delta);
        let moved = self.move_and_collide(m, delta);
        let effect_pos = self.mob_below(m, 0.2);
        m.body_fx.effect = Some((m.tick_count, m.pos(), m.support, effect_pos));
        self.land_and_fall(m, moved.allowed, effect_pos);
        self.bounce(m, delta, &moved, effect_pos);
        self.accumulate_steps(m, moved.allowed, effect_pos);
        let f = self.speed_factor(m);
        m.vel[0] *= f64::from(f);
        m.vel[2] *= f64::from(f);
    }

    /// Movement against the collisions, then position, collision flags and supporting block.
    fn move_and_collide(&self, m: &mut Mob, delta: [f64; 3]) -> Moved {
        let start = m.bbox();
        // The movement meets the solid entities the mob collides with, like its free-room tests.
        let world = self.world_for_room(self.room_for_mob(m));
        // The strider settles on lava sources (own shape above them): one envelope for the
        // movement and the support search.
        let lava = super::nether::strider::lava_stand(self, &world, m, start.min[1]);
        let collision_world: &dyn rc_physics::World = match &lava {
            Some(l) => l,
            None => &world,
        };
        let snow = Self::powder_snow_rule(m);
        if super::move_stats::enabled() {
            self.count_movement(m, &start, delta, snow.0 || snow.1, lava.is_some());
        }
        with_move_boxes(|kept| {
            let allowed = self.resolve_through_snow(m, collision_world, &start, delta, snow, kept);
            let allowed_len_sq = length_sq(allowed);
            if allowed_len_sq > 1.0e-7 || length_sq(delta) - allowed_len_sq < 1.0e-7 {
                let from = m.pos();
                m.add_pos(allowed);
                m.inside.record(from, m.pos(), delta); // read back by the crossed-block pass
            }
            let blocked_x = (delta[0] - allowed[0]).abs() >= 1.0e-5;
            let blocked_z = (delta[2] - allowed[2]).abs() >= 1.0e-5;
            m.horizontal_collision = blocked_x || blocked_z;
            // The server is authoritative except for a mount the player pilots: ground and vertical
            // impact recomputed even without a vertical component (horizontal piston push).
            if delta[1].abs() > 0.0 || !Self::player_controls(m) {
                m.vertical_collision = delta[1] != allowed[1];
                let below = m.vertical_collision && delta[1] < 0.0;
                m.on_ground = below;
                m.support = if below { Self::support_after_move(collision_world, kept, m) } else { None };
            }
            Moved { allowed, blocked_x, blocked_z }
        })
    }

    /// Counts the movement for the rest rate (`RUSTCRAFT_PERFLOG`): its inputs and the zone its
    /// collision search may read.
    fn count_movement(&self, m: &Mob, start: &rc_physics::Aabb, delta: [f64; 3], snow_envelope: bool, lava_envelope: bool) {
        let step = m.step_height();
        let inputs = super::move_stats::MoveInputs::new(start, delta, m.on_ground, step, snow_envelope, lava_envelope);
        let zone = start.expand_towards(delta).expand_towards([0.0, step.max(0.0), 0.0]);
        super::move_stats::movement(m.id, self.tick, inputs, &zone);
    }

    /// Main supporting block of a mob that just landed: taken from the block boxes its movement
    /// kept when they can tell it (`RUSTCRAFT_OPT_SUPPORTSHARE`), otherwise by a new walk.
    fn support_after_move(collision_world: &dyn rc_physics::World, kept: &collide::CollectedBlocks, m: &Mob) -> Option<[i32; 3]> {
        let (b, pos) = (m.bbox(), m.pos());
        let support = if support_share() {
            rc_physics::support_of_kept(collision_world, kept, &b, pos)
        } else {
            rc_physics::support_of(collision_world, &b, pos)
        };
        if super::move_stats::enabled() {
            super::move_stats::support_search(support_share() && kept.answers_support_of(&b));
        }
        // Every movement of the test suite proves the shared answer against the walk (the
        // measurement benches turn the check off).
        #[cfg(test)]
        if super::index::verify() {
            assert_eq!(support, rc_physics::support_of(collision_world, &b, pos), "support from the kept boxes, box {b:?}");
        }
        support
    }

    /// How powder snow carries the mob: (falling onto it from more than 2.5 blocks, walks on it:
    /// species of `#powder_snow_walkable_mobs` or leather boots worn).
    fn powder_snow_rule(m: &Mob) -> (bool, bool) {
        let falling = m.fall_distance > 2.5;
        let walker = m.kind.powder_snow_walkable() || m.equipment[2].as_ref().is_some_and(|s| s.id == "minecraft:leather_boots");
        (falling, walker)
    }

    /// `resolve` through the mob's own powder snow shape when it falls onto snow or walks on it
    /// (`snow`, from `powder_snow_rule`; the support search does not see that shape: nothing
    /// kept); otherwise on `collision_world`, its block boxes kept in `kept` for the support search.
    fn resolve_through_snow(
        &self,
        m: &Mob,
        collision_world: &dyn rc_physics::World,
        start: &rc_physics::Aabb,
        delta: [f64; 3],
        (falling, walker): (bool, bool),
        kept: &mut collide::CollectedBlocks,
    ) -> [f64; 3] {
        if falling || walker {
            kept.forget();
            let snow = SnowStand { base: collision_world, server: self, bottom: start.min[1], falling, walker };
            collide::resolve(&snow, start, delta, m.on_ground, m.step_height())
        } else if support_share() {
            collide::resolve_keeping(collision_world, start, delta, m.on_ground, m.step_height(), kept)
        } else {
            collide::resolve(collision_world, start, delta, m.on_ground, m.step_height())
        }
    }

    /// Fall distance and landing: the strider in lava resets it, the flyers skip it all.
    fn land_and_fall(&mut self, m: &mut Mob, allowed: [f64; 3], effect_pos: [i32; 3]) {
        let lava_reset = super::nether::strider::fall_reset(m);
        if lava_reset {
            m.fall_distance = 0.0;
        }
        let falls = !ignores_falls(m.kind);
        let before = m.fall_distance;
        if falls && !m.in_water && allowed[1] < 0.0 && !lava_reset {
            m.fall_distance -= f64::from(allowed[1] as f32);
        }
        // Landing dust: before the block's rule, on the fall from before this movement (none for
        // the flyers, which skip the whole fall check).
        if falls && m.on_ground && before > 0.0 {
            self.fall_dust(m.pos(), effect_pos, before, m.attrs.safe_fall.value());
        }
        if falls && m.on_ground && !lava_reset {
            if m.fall_distance > 0.0 {
                let on = self.st(effect_pos);
                let (w, h) = m.dims_wh();
                // Once trampled, the mob is raised onto the dirt before its damage (fall sound
                // and landing at the new height).
                self.trample_farmland(effect_pos, on, m.fall_distance, Some(&mut *m), w, h);
                self.turtle_egg_fall_on(effect_pos, Some(m.kind)); // turtle eggs crack before the damage
                self.fall_on(m, on, m.fall_distance);
                // Landing: state of the main supporting block, otherwise of the block at 0.2.
                let st = self.st(m.support.unwrap_or(effect_pos));
                self.game_event_src(
                    "minecraft:hit_ground",
                    m.pos(),
                    Some(super::recent::vibration::Src::Living(Ent::Mob(m.id))),
                    Some(st),
                );
            }
            m.fall_distance = 0.0;
        }
    }

    /// Velocity after a collision: the entity's own bounce (full sulfur cube) off walls, on the
    /// ground the maximum with that of the block; otherwise the blocked axis stops (no bounce for
    /// mobs outside bouncing blocks).
    fn bounce(&mut self, m: &mut Mob, delta: [f64; 3], moved: &Moved, effect_pos: [i32; 3]) {
        if !((delta[1].abs() > 0.0 && m.vertical_collision) || m.horizontal_collision) {
            return;
        }
        let effect_state = self.st(effect_pos);
        let block_bounce = f64::from(self.phys.bounce.get(effect_state as usize).copied().unwrap_or(0.0));
        let own = m.bounciness();
        let cur = m.vel;
        let mut v = cur;
        if moved.blocked_x {
            v[0] = -cur[0] * own;
        }
        if moved.blocked_z {
            v[2] = -cur[2] * own;
        }
        if m.vertical_collision {
            let mut r = own;
            if delta[1] < 0.0 {
                let suppressed = || {
                    if super::env::mob_states_on() {
                        self.block_has_tag(effect_state, crate::tag_bits::TagId::SuppressesBounce)
                    } else {
                        self.btag_id(effect_pos, crate::tag_bits::TagId::SuppressesBounce)
                    }
                };
                r = if -cur[1] > m.gravity() && !suppressed() {
                    r.max(block_bounce)
                } else {
                    0.0
                };
            }
            v[1] = if r > 0.0 {
                let portion = moved.allowed[1] / cur[1];
                let comp = portion * m.gravity();
                let air = if m.omnidirectional_air() { 0.91f32 } else { 0.98f32 };
                let air = f64::from(corrected(air, m.air_drag_modifier()));
                let drag = 1.0 + (air - 1.0) * portion;
                (comp - cur[1]) * drag * r
            } else {
                0.0
            };
        }
        m.vel = v;
        // Vertical bounce: `bounce` event.
        if m.vertical_collision && v[1] != 0.0 {
            self.mob_game_event(m, "minecraft:bounce");
        }
    }

    /// Walked distance, and the step (sound, vibration) or swim it makes when one is due.
    fn accumulate_steps(&mut self, m: &mut Mob, allowed: [f64; 3], effect_pos: [i32; 3]) {
        let h = (allowed[0] * allowed[0] + allowed[2] * allowed[2]).sqrt();
        let full = length_sq(allowed).sqrt();
        let on = self.mob_below(m, 1.0e-5);
        let on_st = self.st(on);
        let climbing = self.climbable_state(on_st);
        m.move_dist += if climbing { (full * f64::from(0.6f32)) as f32 } else { (h * f64::from(0.6f32)) as f32 };
        m.surf.fly_dist += (full * f64::from(0.6f32)) as f32;
        // The state of `on` is already read (old path: read again for each air test).
        let on_air = if super::env::mob_states_on() { self.state_is_air(on_st) } else { self.is_air(on) };
        if m.move_dist > m.next_step && !on_air {
            // Swimming (swim sound) when the aquatic mob is not on the ground.
            if super::sealife::swim_step(self, m) {
                return;
            }
            // Step on the ground: sound of the block at 0.2 under the feet, vibration of the supporting block (the game
            // emits it whatever the emission when both blocks coincide); otherwise swimming
            // in water; otherwise the step remains due.
            let sounds = super::aquatic::splash::emits_sounds(m.kind);
            let same = on == effect_pos;
            let effect_state = self.st(effect_pos);
            let mut produced = self.mob_step_effects(m, effect_pos, effect_state, sounds, same);
            if !same {
                produced |= self.mob_step_effects(m, on, on_st, false, true);
            }
            if produced {
                m.next_step = super::surface::next_step(m)
                    .or_else(|| super::nether::next_step(m))
                    .or_else(|| super::recent::next_step(m))
                    .unwrap_or_else(|| m.move_dist.trunc() + 1.0);
            } else if m.in_water {
                m.next_step = super::surface::next_step(m).unwrap_or_else(|| m.move_dist.trunc() + 1.0);
                if sounds {
                    self.mob_swim_sound(m);
                }
                self.mob_game_event(m, "minecraft:swim");
            }
        } else if on_air {
            self.mob_flap(m);
        }
    }

    /// Climbable block for step accumulation: `climbable` tag or powder snow (per-state flag of
    /// the physics tables, `RUSTCRAFT_OPT_MOBSTATES`; old path: block name).
    pub(crate) fn climbable_state(&self, st: u32) -> bool {
        self.phys.climbable(st)
            || if super::env::mob_states_on() {
                self.phys.is_powder_snow(st)
            } else {
                self.data.block_of(st).is_some_and(|b| b.name == "minecraft:powder_snow")
            }
    }

    /// Effects of a step on a block: nothing on air; on the
    /// ground or climbing, not swimming: sound and vibration according to the flags.
    fn mob_step_effects(&mut self, m: &mut Mob, p: [i32; 3], st: u32, sound: bool, vibrate: bool) -> bool {
        if self.view().is_air(st) {
            return false;
        }
        let climb = self.climbable_state(st);
        if !super::aquatic::splash::grounded_step(m, climb) {
            return false;
        }
        if sound {
            self.step_sound(m, p, st);
        }
        if vibrate {
            self.game_event_src(
                "minecraft:step",
                m.pos(),
                Some(super::recent::vibration::Src::Living(Ent::Mob(m.id))),
                Some(st),
            );
        }
        true
    }

    /// The flap of wings above air: species' own flap, then
    /// `flap` event.
    fn mob_flap(&mut self, m: &mut Mob) {
        let flapping = match m.kind {
            Kind::Parrot | Kind::Chicken => m.surf.fly_dist > m.surf.next_flap,
            Kind::Bee => !m.on_ground && m.tick_count % 2 == 0,
            Kind::Allay => !m.on_ground,
            Kind::Bat => m.surf.bat_flags & 1 == 0 && m.tick_count % 10 == 0,
            Kind::Phantom => (m.id.wrapping_mul(3).wrapping_add(m.tick_count)) % 25 == 0,
            _ => false,
        };
        if !flapping {
            return;
        }
        match m.kind {
            Kind::Parrot => super::surface::parrot::on_flap(self, m),
            Kind::Chicken => m.surf.next_flap = m.surf.fly_dist + m.surf.flap_speed / 2.0,
            _ => {}
        }
        self.mob_game_event(m, "minecraft:flap");
    }

    /// "Fall on" block: rule of the touched block `on` (`fall_rule`), then damage.
    pub(crate) fn fall_on(&mut self, m: &mut Mob, on: u32, dist: f64) {
        // Breeze landing (sound beyond 3 blocks).
        if m.kind == Kind::Breeze {
            super::recent::breeze::on_fall(self, m, dist);
        }
        let honey = self.data.block_of(on).is_some_and(|b| b.name == "minecraft:honey_block");
        // Honey block: audible slide and jump particles (event 54), before the damage.
        if honey {
            self.mob_sound(m, "minecraft:block.honey_block.slide", 1.0, 1.0);
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Event { id: m.id, event: 54 }));
        }
        // Powder snow: no damage, species fall sound from 4 blocks (loud from 7).
        let Some((dist, mult, ty)) = self.fall_rule(on, dist, false) else {
            if dist >= 4.0 {
                let s = self.mob_fall_sound(m, dist >= 7.0);
                self.mob_sound(m, s, 1.0, 1.0);
            }
            return;
        };
        // Fall of equids (landing sound, riders hit too).
        if self.mount_fall(m, dist, mult) {
            return;
        }
        if m.kind.info().fall_immune {
            return;
        }
        let safe = m.attrs.safe_fall.value();
        // Goat −10, frog −5.
        let dmg = ((dist + 1.0e-6 - safe) * mult * m.attrs.fall_mult.value()).floor() as i32
            - super::brainy::fall_reduction(m.kind);
        if dmg > 0 {
            let s = self.mob_fall_sound(m, dmg > 4);
            self.mob_sound(m, s, 1.0, 1.0);
            self.mob_block_fall_sound(m);
            self.mob_hurt(m, ty, dmg as f32, None, None);
            // Honey: fall sound of the block replayed after the damage (volume ½, pitch ¾).
            if honey {
                self.mob_sound(m, "minecraft:block.honey_block.fall", 0.5, 0.75);
            }
        }
        // Creeper: the fall advances the fuse (1.5 tick per block, up to 5 ticks from the end).
        if m.kind == Kind::Creeper {
            m.swell = (m.swell + (dist * 1.5) as i32).min(m.fuse - 5);
        }
    }

    /// Fall sound of the species (small or big).
    fn mob_fall_sound(&self, m: &Mob, big: bool) -> &'static str {
        if m.kind.is_monster() && !super::sealife::handles(m.kind) {
            if big { "minecraft:entity.hostile.big_fall" } else { "minecraft:entity.hostile.small_fall" }
        } else if big {
            "minecraft:entity.generic.big_fall"
        } else {
            "minecraft:entity.generic.small_fall"
        }
    }

    /// Fall sound of the block touched at 0.2 under the feet: volume ½,
    /// pitch ¾ of its sound set; nothing on air.
    pub(crate) fn mob_block_fall_sound(&mut self, m: &Mob) {
        if let Some((s, v, p)) = self.block_fall_sound_at(m.pos()) {
            self.mob_sound(m, &s, v, p);
        }
    }

    /// Step sound on the effect block `on` of state `on_state`: the species' own sound (or the
    /// block's), then the amethyst chime, played whatever the step sound (even a silent one).
    fn step_sound(&mut self, m: &mut Mob, on: [i32; 3], on_state: u32) {
        // The caller already read the state (old path: read again for each use).
        let on_state = if super::env::mob_states_on() { on_state } else { self.st(on) };
        self.step_sound_own(m, on, on_state);
        let crystal = if super::env::mob_states_on() {
            self.block_has_tag(on_state, crate::tag_bits::TagId::CrystalSoundBlocks)
        } else {
            self.btag_id(on, crate::tag_bits::TagId::CrystalSoundBlocks)
        };
        if crystal && m.tick_count >= m.body_fx.crystal_tick + 20 {
            let fx = &mut m.body_fx;
            let decay = 0.997f64.powf(f64::from(m.tick_count - fx.crystal_tick)) as f32;
            fx.crystal = (fx.crystal * decay + 0.07).min(1.0);
            let pitch = 0.5 + fx.crystal * m.rng.next_float() * 1.2;
            let volume = 0.1 + m.body_fx.crystal * 1.2;
            m.body_fx.crystal_tick = m.tick_count;
            self.mob_sound(m, "minecraft:block.amethyst_block.chime", volume, pitch);
        }
    }

    fn step_sound_own(&mut self, m: &mut Mob, on: [i32; 3], on_state: u32) {
        if self.mount_step_sound(m, on) {
            return;
        }
        // Silent steps: happy ghast, allay, bee, fish and tadpole, nautiluses; sulfur
        // cube carrying an item. (The breeze only emits events: never called.)
        let silent = match m.kind {
            Kind::SulfurCube => m.has_body_item(),
            k => {
                super::recent::silent_steps(k)
                    || matches!(
                        k,
                        Kind::Allay
                            | Kind::Bee
                            | Kind::Cod
                            | Kind::Salmon
                            | Kind::Pufferfish
                            | Kind::TropicalFish
                            | Kind::Tadpole
                            | Kind::Nautilus
                            | Kind::ZombieNautilus
                    )
            }
        };
        if silent {
            return;
        }
        // Iron and copper golems (volume 1).
        if let Some((s, v)) = super::golems::step_sound(m)
            .or_else(|| super::nether::step_sound(m))
            .or_else(|| super::recent::step_sound(m))
        {
            self.mob_sound(m, s, v, 1.0);
            return;
        }
        if let Some(s) = self.own_step_sound(m) {
            self.mob_sound(m, &s, 0.15, 1.0);
            return;
        }
        // No sound of its own: step sound of the block's sound group, volume × 0.15.
        let st = on_state;
        let Some(t) = self.data.states.get(st as usize).and_then(|i| self.data.sound_types.get(usize::from(i.sound)))
        else {
            return;
        };
        if !t.step.is_empty() {
            let (s, v, p) = (t.step.clone(), t.volume * 0.15, t.pitch);
            self.mob_sound(m, &s, v, p);
        }
    }

    /// The species' own step sound (sound variant set of the pig, cow, chicken, wolf, babies
    /// included), `None` for the sulfur cube and the species that use the block's sound. A fixed
    /// name is borrowed (`RUSTCRAFT_OPT_STEPSOUNDREF`; old path: copied at every step).
    fn own_step_sound(&self, m: &Mob) -> Option<std::borrow::Cow<'static, str>> {
        use std::borrow::Cow;
        if m.kind == Kind::SulfurCube {
            return None;
        }
        if !step_sound_ref() {
            let copied = super::surface::step_sound(m)
                .map(str::to_owned)
                .or_else(|| self.variant_sound(m, "step_sound"))
                .or_else(|| m.kind.info().step_sound.map(str::to_owned));
            return copied.map(Cow::Owned);
        }
        super::surface::step_sound(m)
            .map(Cow::Borrowed)
            .or_else(|| self.variant_sound(m, "step_sound").map(Cow::Owned))
            .or_else(|| m.kind.info().step_sound.map(Cow::Borrowed))
    }

    /// Can freeze: not in `#freeze_immune_entity_types`, no `#freeze_immune_wearables` piece worn.
    pub(crate) fn mob_can_freeze(&self, m: &Mob) -> bool {
        !m.kind.freeze_immune()
            && !m.equipment[2..=6].iter().flatten().any(|s| self.item_tag(&s.id, "minecraft:freeze_immune_wearables"))
    }

    /// Freezing (server side), after the crossed-block pass, which adds 1 per tick in powder
    /// snow (up to 140): −2 outside snow or if the entity cannot freeze;
    /// slowdown proportional to freezing above a block; 1 freezing damage every
    /// 40 ticks once frozen.
    pub(crate) fn mob_freezing(&mut self, m: &mut Mob, in_snow: bool) {
        const REQUIRED: i32 = crate::inside_blocks::FREEZE_REQUIRED;
        if !freeze_skip_unfrozen() {
            return self.mob_freezing_reading_all(m, in_snow);
        }
        // Almost every mob is out of snow and not frozen: the armour, the block under the feet and
        // the damage tick are only looked at when they can change something.
        if !in_snow || !self.mob_can_freeze(m) {
            m.ticks_frozen = (m.ticks_frozen - 2).max(0);
        }
        m.attrs.speed.remove_mod("minecraft:powder_snow");
        if m.ticks_frozen > 0 && self.on_block_for_freezing(m) {
            let pct = m.ticks_frozen.min(REQUIRED) as f32 / REQUIRED as f32;
            m.attrs.speed.set_mod("minecraft:powder_snow", f64::from(-0.05 * pct), 0);
        }
        if m.tick_count % 40 == 0 && m.ticks_frozen >= REQUIRED && self.mob_can_freeze(m) {
            self.mob_hurt(m, "minecraft:freeze", 1.0, None, None);
        }
    }

    /// `mob_freezing` reading the armour and the block under the feet at every call (old path).
    fn mob_freezing_reading_all(&mut self, m: &mut Mob, in_snow: bool) {
        const REQUIRED: i32 = crate::inside_blocks::FREEZE_REQUIRED;
        let can_freeze = self.mob_can_freeze(m);
        if !in_snow || !can_freeze {
            m.ticks_frozen = (m.ticks_frozen - 2).max(0);
        }
        m.attrs.speed.remove_mod("minecraft:powder_snow");
        let on_block = self.on_block_for_freezing(m);
        if m.ticks_frozen > 0 && on_block {
            let pct = m.ticks_frozen.min(REQUIRED) as f32 / REQUIRED as f32;
            m.attrs.speed.set_mod("minecraft:powder_snow", f64::from(-0.05 * pct), 0);
        }
        if m.tick_count % 40 == 0 && m.ticks_frozen >= REQUIRED && can_freeze {
            self.mob_hurt(m, "minecraft:freeze", 1.0, None, None);
        }
    }

    /// The block 0.2 under the feet is not air (air, cave air, void air): the freezing slowdown
    /// applies. The cell is the one this tick's movement computed when the position and the
    /// support have not changed since (`RUSTCRAFT_OPT_FREEZESHARE`).
    fn on_block_for_freezing(&self, m: &Mob) -> bool {
        let cell = match m.body_fx.effect {
            Some((tick, pos, support, cell)) if freeze_share() && tick == m.tick_count && pos == m.pos() && support == m.support => {
                cell
            }
            _ => self.mob_below(m, 0.2),
        };
        if super::env::mob_states_on() {
            return !self.phys.is_air(self.st(cell));
        }
        !matches!(self.bname(cell), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
    }

    /// Fall of a living entity (the mob `mob` being ticked, otherwise the player; box `w` × `h`)
    /// onto the effect block `p` of state `st`: farmland turns back into dirt if the world
    /// random falls below the fall distance − 0.5, if the player (never in a protected zone on
    /// this server) or the `mob_griefing` rule allows it, and if the box exceeds 0.512 cubic
    /// blocks; the random is drawn only on farmland. The trampling entity is raised with the
    /// others and becomes the source of the event; fall damage follows, unchanged.
    pub(crate) fn trample_farmland(&mut self, p: [i32; 3], st: u32, dist: f64, mob: Option<&mut Mob>, w: f32, h: f32) {
        if self.view().block(st) != self.bs.t.id.farmland {
            return;
        }
        if f64::from(self.bs.rng.next_float()) < dist - 0.5
            && (mob.is_none() || self.rule("minecraft:mob_griefing", true))
            && w * w * h > 0.512
        {
            let src = match mob {
                Some(m) => {
                    Self::push_mob_up(m, p, 15.0 / 16.0, 1.0);
                    Ent::Mob(m.id)
                }
                None => Ent::Player,
            };
            self.turn_to_dirt(p, Some(src));
        }
    }

    /// Entity on the ground on the effect block `p` (0.2 below the feet), every tick after the
    /// movement; `careful`: careful step (sneaking); `by`: the mob's species (`None`: the
    /// player). True if the block burns the feet (magma); a redstone ore lights up; turtle eggs
    /// may lose one.
    pub(crate) fn step_on_block(&mut self, p: [i32; 3], careful: bool, by: Option<Kind>) -> bool {
        if careful {
            return false;
        }
        let s = self.st(p);
        let b = self.view().block(s);
        let ids = &self.bs.t.id;
        if b == ids.magma {
            return true;
        }
        if b == ids.redstone_ore || b == ids.deepslate_redstone_ore {
            self.redstone_ore_touch(p, s);
        } else if b == ids.turtle_egg {
            self.turtle_egg_step_on(p, by, careful);
        }
        false
    }

    /// A body that is not alive (ground item, orb, falling block, primed TNT) on the ground on
    /// the effect block `p` of state `s` (0.2 under it, read by its block effects), each tick:
    /// a redstone ore lights up, a sculk sensor hears a step, a shrieker answers the player behind
    /// the body (`src`, built only when a vibration block exists). Magma and turtle eggs only act
    /// on the living.
    pub(crate) fn object_step_on(
        &mut self,
        (p, s): ([i32; 3], u32),
        pos: [f64; 3],
        src: impl FnOnce() -> super::recent::vibration::Src,
    ) {
        self.redstone_ore_touch(p, s);
        if !self.mobs.rec.vib_blocks.is_empty() {
            self.sensor_step_on(p, src(), pos, false);
        }
    }

    /// Effect of the block stepped on by a mob on the ground (never sneaking): 1 `hot_floor`
    /// damage on magma (fire-immune mobs are excluded by the `is_fire` tag).
    pub(crate) fn mob_step_on(&mut self, m: &mut Mob) {
        if !m.on_ground || m.removed || super::illagers::no_physics(m) {
            return;
        }
        // Effect cell of this tick's movement if the position and the support have not changed
        // since (same computation, without re-reading the support).
        let p = match m.body_fx.effect {
            Some((tick, pos, support, e))
                if step_on_reuses_effect_cell() && tick == m.tick_count && pos == m.pos() && support == m.support =>
            {
                e
            }
            _ => self.mob_below(m, 0.2),
        };
        if self.step_on_block(p, false, Some(m.kind)) {
            self.mob_hurt(m, "minecraft:hot_floor", 1.0, None, None);
        }
    }

    /// Same effect for the player on the ground, except when sneaking.
    pub(crate) fn player_step_on(&mut self) {
        if !self.on_ground || self.game_mode == 3 || self.life.vit.dead {
            return;
        }
        let p = self.on_pos(self.pos, self.player_support(), 0.2);
        if self.step_on_block(p, self.sneaking, None) {
            self.hurt("minecraft:hot_floor", 1.0);
        }
    }

    /// `travel(input)` for a mob with effective AI.
    pub(crate) fn mob_travel(&mut self, m: &mut Mob) {
        super::mounts::camel::freeze_travel(self, m);
        // Ghast flight.
        if super::nether::travel(self, m) {
            return;
        }
        // Happy ghast flight (ridden or not).
        if super::recent::travel(self, m) {
            return;
        }
        if super::surface::travel_in_water(self, m) || super::aquatic::travel_in_water(self, m) {
            return;
        }
        // Squids (raw movement), swimmers in water.
        // Allay flight, swimming of the frog, axolotl and tadpole.
        if super::brainy::travel(self, m) {
            return;
        }
        if super::sealife::travel(self, m) {
            return;
        }
        let gravity = m.gravity();
        let falling = m.vel[1] <= 0.0;
        // The strider whose cell holds lava walks on it like on the ground.
        if (m.in_water || m.lava_height > 0.0) && !super::nether::strider::stands_on_fluid(self, m) {
            let y0 = m.pos()[1];
            if m.in_water {
                let slow = super::mounts::water_slow_down(m).unwrap_or_else(|| super::surface::water_slow_down(m));
                Self::move_relative(m, 0.02);
                let v = m.vel;
                self.mob_move(m, v);
                let mut v = m.vel;
                if m.horizontal_collision && self.on_climbable(m) {
                    v[1] = 0.2;
                }
                v = [v[0] * f64::from(slow), v[1] * f64::from(0.8f32), v[2] * f64::from(slow)];
                m.vel = Self::fluid_falling(gravity, falling, v);
            } else {
                Self::move_relative(m, 0.02);
                let v = m.vel;
                self.mob_move(m, v);
                if m.lava_height <= self.fluid_jump_threshold(m) {
                    m.vel = [m.vel[0] * 0.5, m.vel[1] * f64::from(0.8f32), m.vel[2] * 0.5];
                    m.vel = Self::fluid_falling(gravity, falling, m.vel);
                } else {
                    m.vel = m.vel.map(|v| v * 0.5);
                }
                if gravity != 0.0 {
                    m.vel[1] -= gravity / 4.0;
                }
            }
            // Leaving the fluid.
            if m.horizontal_collision {
                let b = m.bbox().offset(m.vel[0], m.vel[1] + f64::from(0.6f32) - m.pos()[1] + y0, m.vel[2]);
                if self.room_free(self.room_for_mob(m), &b) && !self.box_has_liquid(&b) {
                    m.vel[1] = f64::from(0.3f32);
                }
            }
            // `#can_float_while_ridden`: a mount carrying a passenger rises in water
            // deeper than its jump threshold (pilot or not).
            if !m.mount.passengers.is_empty()
                && m.kind.floats_while_ridden()
                && m.water_height > self.fluid_jump_threshold(m)
            {
                m.vel[1] += f64::from(0.04f32);
            }
            if m.kind == Kind::SulfurCube {
                self.sulfur_buoyancy(m);
            }
            return;
        }
        let below = self.mob_below(m, 0.500_001);
        // One read of the cell under the feet serves the friction and, after the movement, the
        // loaded test of the gravity (`RUSTCRAFT_OPT_MOBSTATES`; old path: read twice).
        let below_state = super::env::mob_states_on().then(|| self.state_at(below));
        let block_friction = if m.on_ground {
            let st = match below_state {
                Some(read) => read.unwrap_or(0),
                None => self.st(below),
            };
            corrected(self.friction_of_state(st), m.friction_modifier())
        } else {
            1.0
        };
        let speed = if m.on_ground {
            if f64::from(block_friction) > 0.6 { m.speed * (0.216_000_02 / (block_friction * block_friction * block_friction)) } else { m.speed }
        } else {
            0.02
        };
        Self::move_relative(m, speed);
        // Ladders.
        let climb = self.on_climbable(m);
        if climb {
            m.fall_distance = 0.0;
            m.vel[0] = m.vel[0].clamp(-f64::from(0.15f32), f64::from(0.15f32));
            m.vel[2] = m.vel[2].clamp(-f64::from(0.15f32), f64::from(0.15f32));
            m.vel[1] = m.vel[1].max(-f64::from(0.15f32));
        }
        let v = m.vel;
        self.mob_move(m, v);
        let mut vel_after_move = m.vel;
        if (m.horizontal_collision || m.jumping) && self.on_climbable(m) {
            vel_after_move[1] = 0.2;
        }
        let mut my = vel_after_move[1];
        let below_loaded = match below_state {
            Some(read) => read.is_some(),
            None => self.state_at(below).is_some(),
        };
        if below_loaded {
            my -= gravity;
        } else {
            my = if m.pos()[1] > f64::from(self.dim.height.min_y) { -0.1 } else { 0.0 };
        }
        let drag = m.air_drag_modifier();
        let air = corrected(0.91, drag);
        let horizontal_drag = f64::from(block_friction * air);
        let vertical = super::surface::air_vertical_friction(m)
            .unwrap_or_else(|| if m.omnidirectional_air() { air } else { corrected(0.98, drag) });
        m.vel = [vel_after_move[0] * horizontal_drag, my * f64::from(vertical), vel_after_move[2] * horizontal_drag];
        // Long jump (friction ignored).
        if super::brainy::discard_friction(m) {
            m.vel = [vel_after_move[0], my, vel_after_move[2]];
        }
        // Chicken: slowed fall.
        if m.kind == Kind::Chicken && !m.on_ground && m.vel[1] < 0.0 {
            m.vel[1] *= 0.6;
        }
    }

    fn fluid_falling(g: f64, falling: bool, v: [f64; 3]) -> [f64; 3] {
        if g == 0.0 {
            return v;
        }
        let y = if falling && (v[1] - 0.005).abs() >= 0.003 && (v[1] - g / 16.0).abs() < 0.003 {
            -0.003
        } else {
            v[1] - g / 16.0
        };
        [v[0], y, v[2]]
    }

    pub(crate) fn fluid_jump_threshold(&self, m: &Mob) -> f64 {
        if m.kind == Kind::SulfurCube {
            return f64::from(m.height()) * 0.2;
        }
        if m.eye_height() < 0.4 { 0.0 } else { 0.4 }
    }

    /// A liquid in the box.
    pub(crate) fn box_has_liquid(&self, b: &rc_physics::Aabb) -> bool {
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.ceil() as i32 - 1);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    if self.mob_fluid([x, y, z]) != Fluid::None {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Held jump (§ 1.1-6/7).
    pub(crate) fn mob_jump(&mut self, m: &mut Mob) {
        if !m.jumping {
            m.no_jump_delay = 0;
            return;
        }
        let in_lava = m.lava_height > 0.0;
        let fh = if in_lava { m.lava_height } else { m.water_height };
        let water_h = m.in_water && fh > 0.0;
        let thr = self.fluid_jump_threshold(m);
        if !water_h || (m.on_ground && fh <= thr) {
            if !in_lava || (m.on_ground && m.lava_height <= thr) {
                if (m.on_ground || (water_h && fh <= thr)) && m.no_jump_delay == 0 {
                    self.jump_from_ground(m);
                    m.no_jump_delay = 10;
                }
            } else {
                self.jump_in_liquid(m);
            }
        } else {
            self.jump_in_liquid(m);
        }
    }

    fn jump_in_liquid(&self, m: &mut Mob) {
        // The bee only rises by 0.01.
        if super::wild::jump_in_liquid(m) {
            return;
        }
        // Magma in lava: imposed velocity according to size.
        if m.kind == Kind::MagmaCube && m.lava_height > 0.0 && !(m.in_water && m.water_height > 0.0) {
            m.vel[1] = f64::from(0.22f32 + m.cube.size as f32 * 0.05);
            m.needs_sync = true;
            return;
        }
        if m.nav.can_float {
            m.vel[1] += f64::from(0.04f32);
        } else {
            m.vel[1] += 0.3;
        }
    }

    /// Team (rank) and collision rule of the mob with UUID `who`, or of the player for `None`
    /// (no team: rule 0): lookup in the scoreboard index.
    fn team_rule(&self, who: Option<u128>) -> (Option<u32>, u8) {
        if !push_uses_team_index() {
            let holder = who.map_or_else(|| self.name.clone(), crate::nbtio::uuid_text);
            let teams = &self.cmd.scores.teams;
            return teams.iter().position(|t| t.players.contains(&holder)).map_or((None, 0), |i| {
                (Some(i as u32), crate::commands::scores::collision_code(&teams[i].collision))
            });
        }
        let rule = match who {
            Some(u) => self.cmd.scores.collision_rule_by_uuid(u),
            None => self.cmd.scores.collision_rule_by_name(&self.name),
        };
        rule.map_or((None, 0), |(t, r)| (Some(t), r))
    }

    /// Team filter of the push as seen from `who` (`team_rule`): `None` when no team has a rule
    /// other than "always" (the filter then lets everything through, nothing is looked up).
    pub(super) fn push_team(&self, who: Option<u128>) -> PushTeamFilter {
        let active = if push_uses_team_index() {
            self.cmd.scores.collision_rules_active()
        } else {
            self.cmd.scores.teams.iter().any(|t| t.collision != "always")
        };
        active.then(|| self.team_rule(who))
    }

    /// The team collision rules let entity `pusher` push `pushed` (UUIDs; `None`: the player).
    pub(crate) fn teams_allow_push(&self, pusher: Option<u128>, pushed: Option<u128>) -> bool {
        self.push_team_allows(self.push_team(pusher), pushed)
    }

    /// Can `who` be pushed by the filter entity `own` (`push_team`)?
    fn push_team_allows(&self, own: PushTeamFilter, who: Option<u128>) -> bool {
        let Some((own_team, own_rule)) = own else { return true };
        let (their_team, their_rule) = self.team_rule(who);
        team_allows(own_rule, own_team, their_rule, their_team)
    }

    /// Pushable.
    pub(crate) fn mob_pushable(&self, m: &Mob) -> bool {
        m.alive()
            && !self.on_climbable(m)
            && super::surface::pushable(m)
            && super::mounts::pushable(m)
            && super::recent::pushable(m)
    }

    /// Push at the end of the AI step (§ 1.4): crowding then mutual push.
    pub(crate) fn mob_push_entities(&mut self, m: &mut Mob) {
        self.mob_push_entities_storing_in(m, push_contact_storage());
    }

    /// `mob_push_entities` with the contacts collected in `storage` (same pushes whatever it is).
    pub(super) fn mob_push_entities_storing_in(&mut self, m: &mut Mob, storage: PushContactStorage) {
        if !super::surface::pushes(m) {
            return;
        }
        match storage {
            PushContactStorage::KeptBuffer => {
                // Taken out for the call (the pushes need the whole server); a nested call would
                // find an empty vector and only lose the capacity.
                let mut contacts = std::mem::take(&mut self.mobs.push_contacts);
                self.push_touched_entities(m, &mut contacts);
                contacts.clear();
                self.mobs.push_contacts = contacts;
            }
            PushContactStorage::Inline => self.push_touched_entities(m, &mut InlineContacts::new()),
            PushContactStorage::FreshVec => self.push_touched_entities(m, &mut Vec::new()),
        }
    }

    /// Collects in `contacts` (empty on entry) the mobs and the player that `m` touches, then
    /// applies the crowding damage and the pushes, minecarts included.
    fn push_touched_entities<Contacts>(&mut self, m: &mut Mob, contacts: &mut Contacts)
    where
        Contacts: Extend<PushContact> + std::ops::Deref<Target = [PushContact]>,
    {
        let bb = m.bbox();
        // Team collision rules (only if some team has a rule other than "always").
        let team = self.push_team(Some(m.uuid));
        // Pushed entities that ride something: pushed, but the cramming only counts the others.
        let mut riders = self.collect_pushed_mobs(m, &bb, team, push_scan(), contacts);
        #[cfg(test)]
        if super::index::verify() {
            self.check_pushed_mobs_with_every_scan(m, &bb, team, contacts, riders);
        }
        let carts = self.carts_touching(&bb, team);
        if self.mob_pushes_player(m, &bb, team) {
            contacts.extend([(Ent::Player, PLAYER_CONTACT_SLOT)]);
            riders += i32::from(self.player_riding_any());
        }
        if contacts.is_empty() && carts.is_empty() {
            return;
        }
        self.cramming_hurt(m, (contacts.len() + carts.len()) as i32, riders);
        for &(entity, recorded_slot) in contacts.iter() {
            // The warden pushed against an entity gets irritated by it.
            if m.kind == Kind::Warden {
                super::recent::warden::on_push(self, m, entity);
            }
            match entity {
                Ent::Mob(id) => self.push_one_mob(m, id, recorded_slot as usize),
                Ent::Player => self.push_against_player(m),
            }
        }
        self.push_carts(m, carts);
    }

    /// The mobs that `m` (box `bb`) pushes, appended to `contacts` with their slot in the order
    /// of the list; returns how many of them ride something. Every scan keeps the same mobs: the
    /// tests of a mob have no effect and only their order changes.
    pub(super) fn collect_pushed_mobs(
        &self,
        m: &Mob,
        bb: &rc_physics::Aabb,
        team: PushTeamFilter,
        scan: PushScan,
        contacts: &mut impl Extend<PushContact>,
    ) -> i32 {
        // In a crowd nearly every mob of the list is out of reach. The bound needs only the
        // position and the species: tested first, a far mob is left without reading its health,
        // which sits on another cache line of the mob.
        match scan {
            PushScan::BoundThenLiveness => {
                let bounds = super::mob::TouchBounds::fetch();
                let alive_within_reach = |other: &Mob| bounds.may_touch(other, bb) && other.alive();
                self.collect_pushed_mobs_where(m, bb, team, contacts, alive_within_reach)
            }
            PushScan::BoundAllAtOnceThenLiveness => {
                let bounds = super::mob::TouchBounds::fetch();
                let alive_within_reach = |other: &Mob| bounds.may_touch_all_at_once(other, bb) && other.alive();
                self.collect_pushed_mobs_where(m, bb, team, contacts, alive_within_reach)
            }
            PushScan::LivenessThenBound => {
                let alive_within_reach = |other: &Mob| other.alive() && other.may_touch(bb);
                self.collect_pushed_mobs_where(m, bb, team, contacts, alive_within_reach)
            }
        }
    }

    /// `collect_pushed_mobs` with the cheap tests of the scan given as `alive_within_reach`: true
    /// for a mob that is alive and whose species bound reaches `bb` (or that has no bound).
    #[inline]
    pub(super) fn collect_pushed_mobs_where(
        &self,
        m: &Mob,
        bb: &rc_physics::Aabb,
        team: PushTeamFilter,
        contacts: &mut impl Extend<PushContact>,
        alive_within_reach: impl Fn(&Mob) -> bool,
    ) -> i32 {
        let mut riders = 0i32;
        for (slot, other) in self.mobs.list.slots().iter().enumerate() {
            if let Some(other) = other
                && alive_within_reach(other)
                && self.mob_takes_push(m, other, bb, team)
            {
                contacts.extend([(Ent::Mob(other.id), slot as u32)]);
                riders += i32::from(other.mount.vehicle.is_some());
            }
        }
        riders
    }

    /// `other`, alive and within reach, really touches the box `bb` of `m` and takes its push.
    pub(super) fn mob_takes_push(&self, m: &Mob, other: &Mob, bb: &rc_physics::Aabb, team: PushTeamFilter) -> bool {
        other.bbox().intersects(bb)
            && self.mob_pushable(other)
            && !self.same_root(m, other)
            && self.push_team_allows(team, Some(other.uuid))
    }

    /// Tests: the mobs just collected in `contacts` are those of each scan run alone.
    #[cfg(test)]
    fn check_pushed_mobs_with_every_scan(
        &self,
        m: &Mob,
        bb: &rc_physics::Aabb,
        team: PushTeamFilter,
        contacts: &[PushContact],
        riders: i32,
    ) {
        for scan in PushScan::EVERY {
            let mut alone = Vec::new();
            let riders_alone = self.collect_pushed_mobs(m, bb, team, scan, &mut alone);
            assert_eq!((contacts, riders), (&alone[..], riders_alone), "mobs pushed by {}, scan {scan:?}", m.id);
        }
    }

    /// Minecarts touched (slots of the minecart list): always pushable, the one the mob rides
    /// included (only the minecart's own push skips its passenger); counted for the cramming like
    /// the game.
    fn carts_touching(&self, bb: &rc_physics::Aabb, team: PushTeamFilter) -> Vec<usize> {
        let mut carts = Vec::new();
        for (i, c) in self.carts.list.iter().enumerate() {
            if let Some(c) = c
                && !c.removed
                && c.bbox().intersects(bb)
                && self.push_team_allows(team, Some(c.uuid))
            {
                carts.push(i);
            }
        }
        carts
    }

    /// Does `m` (box `bb`) push the player?
    fn mob_pushes_player(&self, m: &Mob, bb: &rc_physics::Aabb, team: PushTeamFilter) -> bool {
        self.game_mode != 3
            && !self.life.vit.dead
            && self.player_aabb().intersects(bb)
            && super::surface::pushes_player(m)
            && !self.player_same_root(m)
            && self.push_team_allows(team, None)
    }

    /// Crowding: past the cramming rule, one draw in four hurts the mob. The draw uses every
    /// pushed entity, the damage only those that ride nothing (a minecart never rides).
    fn cramming_hurt(&mut self, m: &mut Mob, pushed: i32, riders: i32) {
        let max = self.mob_rule_int("minecraft:max_entity_cramming", 24);
        if max > 0 && pushed > max - 1 && m.rng.next_int_bound(4) == 0 && pushed - riders > max - 1 {
            self.mob_hurt(m, "minecraft:cramming", 6.0, None, None);
        }
    }

    /// Mutual push of `m` and the mob `id`, recorded in slot `recorded_slot` during the scan.
    fn push_one_mob(&mut self, m: &mut Mob, id: i32, recorded_slot: usize) {
        // Recorded slot still valid (same identifier), otherwise search: same
        // slot as `mob_index` (identifiers are unique in the list).
        let hit = push_reuses_recorded_slot()
            && self.mobs.index.unique_ids()
            && self.mobs.list.get(recorded_slot).is_some_and(|o| o.id == id);
        let slot = if hit { Some(recorded_slot) } else { self.mob_index(id) };
        #[cfg(test)]
        if super::index::verify() {
            assert_eq!(slot, super::index::scan_id(self.mobs.list.slots(), id), "push: wrong slot of {id}");
        }
        let Some(slot) = slot else { return };
        let mut other = self.mobs.list.take(slot).expect("mob");
        // The iron golem's push (sometimes targets the pushed enemy).
        self.golem_pushed(m, &other);
        let (other_pushable, m_pushable) = (self.mob_pushable(&other), self.mob_pushable(m));
        Self::push_pair(&mut other, m, other_pushable, m_pushable);
        // A cube pushed by an iron golem is hurt by it (the cube's own reaction to a push).
        self.cube_pushed_by_golem(&mut other, m);
        // A sulfur cube with a burning archetype hurts every entity it pushes.
        if m.kind == Kind::SulfurCube && !m.cube.contacts.is_empty() {
            self.sulfur_contact_mob(m, &mut other);
        }
        self.mobs.list.put(slot, other);
    }

    /// `m` touches the player: the player's push on the server side has no visible effect (the
    /// client pushes itself); the mob, on the other hand, is pushed.
    fn push_against_player(&mut self, m: &mut Mob) {
        let (xa, za) = Self::push_vec(self.pos, m.pos());
        if let Some((xa, za)) = xa.zip(za)
            && self.mob_pushable(m)
        {
            m.vel[0] += xa;
            m.vel[2] += za;
            m.needs_sync = true;
        }
        if m.kind == Kind::SulfurCube && !m.cube.contacts.is_empty() {
            self.sulfur_contact_damage(m);
        }
    }

    /// Push between `m` and the minecarts of slots `carts`.
    fn push_carts(&mut self, m: &mut Mob, carts: Vec<usize>) {
        for i in carts {
            let Some(mut c) = self.carts.list[i].take() else { continue };
            if m.kind == Kind::Warden {
                // Touch cooldown and disturbance; a minecart is never a target (no anger).
                super::recent::warden::on_push_at(self, m, None, Some(c.pos));
            }
            if c.kind.is_boat() {
                // The boat's own push rule, except for its own passengers.
                if !c.passengers.contains(&Ent::Mob(m.id)) {
                    self.boat_push_mob(&mut c, m);
                }
            } else if !super::illagers::no_physics(m)
                && !c.passengers.contains(&Ent::Mob(m.id))
                && let Some((xa, za)) = Self::cart_pushed_by(&mut c, m.pos())
            {
                // The minecart's rule: it moves aside and gives the mob a quarter, except to its rider.
                m.vel[0] += xa;
                m.vel[2] += za;
                m.needs_sync = true;
            }
            if m.kind == Kind::SulfurCube && !m.cube.contacts.is_empty() {
                self.sulfur_contact_cart(m, &mut c);
            }
            self.carts.list[i] = Some(c);
        }
    }

    fn push_vec(from: [f64; 3], to: [f64; 3]) -> (Option<f64>, Option<f64>) {
        let mut xa = to[0] - from[0];
        let mut za = to[2] - from[2];
        let mut dd = xa.abs().max(za.abs());
        if dd < f64::from(0.01f32) {
            return (None, None);
        }
        dd = dd.sqrt();
        xa /= dd;
        za /= dd;
        let pow = (1.0 / dd).min(1.0);
        xa *= pow * f64::from(0.05f32);
        za *= pow * f64::from(0.05f32);
        (Some(xa), Some(za))
    }

    /// Mutual push of two mobs: `e` is the other, `src` the one who pushes.
    fn push_pair(e: &mut Mob, src: &mut Mob, e_pushable: bool, src_pushable: bool) {
        let (xa, za) = Self::push_vec(e.pos(), src.pos());
        let (Some(xa), Some(za)) = (xa, za) else { return };
        if e_pushable {
            e.vel[0] -= xa;
            e.vel[2] -= za;
            e.needs_sync = true;
        }
        if src_pushable {
            src.vel[0] += xa;
            src.vel[2] += za;
            src.needs_sync = true;
        }
    }

    /// The player pushes the mobs they touch (in the AI step on the server side).
    pub(crate) fn player_pushes_mobs(&mut self) {
        if self.game_mode == 3 || self.life.vit.dead {
            return;
        }
        let pb = self.player_aabb();
        let ppos = self.pos;
        let team = self.push_team(None);
        let mut idx = Vec::new();
        for (i, o) in self.mobs.list.slots().iter().enumerate() {
            if let Some(o) = o
                && o.alive()
                && o.may_touch(&pb)
                && o.bbox().intersects(&pb)
                && !self.player_same_root(o)
                && self.push_team_allows(team, Some(o.uuid))
            {
                idx.push(i);
            }
        }
        for i in idx {
            let Some(mut o) = self.mobs.list.take(i) else { continue };
            let (xa, za) = Self::push_vec(o.pos(), ppos);
            if let (Some(xa), Some(za)) = (xa, za)
                && self.mob_pushable(&o)
            {
                o.vel[0] -= xa;
                o.vel[2] -= za;
                o.needs_sync = true;
            }
            self.mobs.list.put(i, o);
        }
    }

    /// The installed player's pose ([`PlayerPose::resolve`]): the one asked for now, with the room
    /// measured at its last tick. On the server side, death does not change the pose (the dead
    /// player keeps theirs).
    pub(crate) fn player_pose(&self) -> PlayerPose {
        PlayerPose::resolve(self.pose_wish(), self.life.vit.pose_room)
    }

    /// What decides the installed player's pose.
    fn pose_wish(&self) -> PoseWish {
        let passenger =
            self.mobs.mounts.riding.vehicle.is_some() || self.pcart.vehicle.is_some() || self.deco.seat.is_some();
        PoseWish {
            asleep: self.life.vit.sleeping.is_some(),
            swimming: self.mobs.sea.player_swimming,
            fall_flying: self.proj_player.fall_flying,
            spin_attack: self.proj_player.spin > 0,
            shift_held: self.sneaking,
            flying: self.flying,
            ignores_room: self.game_mode == 3 || passenger,
        }
    }

    /// End of the installed player's own tick, like the game: the room around it measured once,
    /// its pose chosen and kept until the next tick, and shown to the other players.
    pub(crate) fn update_player_pose(&mut self) {
        let last = self.life.vit.pose_room.last;
        let headroom = self.player_headroom();
        let pose = PlayerPose::resolve(self.pose_wish(), PoseRoom { headroom, last });
        self.life.vit.pose_room = PoseRoom { headroom, last: pose };
        self.note_own_entity_value(6, rc_proto_v777::entity::Meta::Pose(pose.network_id()));
    }

    /// A player who starts riding (a mob, a minecart, a cushion, by command, or carried over to a
    /// converted mount) stands up, like any entity starting a ride in the game: the end-of-tick
    /// rule then keeps that pose when not even the flat box fits (mounted inside a wall).
    pub(crate) fn player_stands_up_to_ride(&mut self) {
        self.life.vit.pose_room.last = PlayerPose::Standing;
    }

    /// Tallest player box free of blocks, the border and solid entities at the player's feet (the
    /// game's fit test: width 0.6 and the pose's height, both times the player's scale, in single
    /// precision like the game's sizes; boxes shrunk by 1e-7). The standing box is tried first,
    /// so the usual open air costs a single test.
    fn player_headroom(&self) -> Headroom {
        let world = self.world_for_room(self.room_for_player());
        let scale = self.life.attrs.scale();
        let half_width = f64::from(0.6f32 * scale / 2.0);
        let p = self.pos;
        let fits = |height: f32| {
            let b = rc_physics::Aabb {
                min: [p[0] - half_width, p[1], p[2] - half_width],
                max: [p[0] + half_width, p[1] + f64::from(height * scale), p[2] + half_width],
            }
            .deflate(1.0e-7);
            rc_physics::is_free(&world, &b)
        };
        if fits(1.8) {
            Headroom::Standing
        } else if fits(1.5) {
            Headroom::Crouching
        } else if fits(0.6) {
            Headroom::Flat
        } else {
            Headroom::None
        }
    }

    /// Width, height and eye height of the player's pose, single source: sleeping 0.2;
    /// swimming, gliding or spin attack 0.6 (eyes 0.4); crouching 1.5 (1.27); standing 1.8
    /// (1.62).
    pub(crate) fn player_dims(&self) -> (f64, f64, f64) {
        self.player_pose().scaled_dims(self.life.attrs.scale())
    }

    pub(crate) fn player_aabb(&self) -> rc_physics::Aabb {
        self.player_pose().bbox_at(self.pos, self.life.attrs.scale())
    }

    /// "Nearby players" query of a targeting: only the feet point counts (lower bounds
    /// included, upper bounds excluded), not the player's box.
    pub(crate) fn player_feet_in(&self, area: &rc_physics::Aabb) -> bool {
        (0..3).all(|i| area.min[i] <= self.pos[i] && self.pos[i] < area.max[i])
    }

    pub(crate) fn player_eye_y(&self) -> f64 {
        self.pos[1] + self.player_eye_h()
    }

    /// Current player in water (box shrunk by 0.001 touching water) or in the rain
    /// (at the feet block, or at the top block of their box in the same column).
    pub(crate) fn player_in_water_or_rain(&self) -> bool {
        let b = self.player_aabb();
        let mut vel = [0.0; 3];
        let w = self.world();
        let (water, ..) =
            rc_physics::scan_fluids(&w, &self.phys, &b, self.pos, self.player_eye_y(), false, self.fast_lava(), &mut vel);
        if water > 0.0 {
            return true;
        }
        let feet = super::mth::block_of(self.pos);
        self.is_raining_at(feet) || self.is_raining_at([feet[0], b.max[1].floor() as i32, feet[2]])
    }

    pub(crate) fn player_eye_h(&self) -> f64 {
        self.player_dims().2
    }

    pub(crate) fn mob_rule_int(&self, name: &str, default: i32) -> i32 {
        let short = name.trim_start_matches("minecraft:");
        let v = self
            .rules
            .get(name)
            .or_else(|| self.rules.get(short))
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
        v.map_or(default, |v| v as i32)
    }
}
