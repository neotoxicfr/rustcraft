//! Interactions with the world (`docs/specs/interactions.md`): mining with local prediction and
//! sequence numbers, cracks, placing and use, attack, prolonged use, action
//! keys (drop, swap, pick a block).
//!
//! State lives in `Interact`; the logic is written as `impl App` to read the world,
//! the inventory and the player without passing everything through parameters.

use hashbrown::HashMap;

use rc_data::flags;
use rc_proto_v777::item::ItemStack;

use crate::item_use::{BlockTags, ItemView, UseIds, tool_speed};
use crate::particles::{Kind as PKind, Options as POptions};
use crate::pick::Hit;

mod fluid_use;
pub(crate) mod instrument;

/// Brush use duration (ticks).
const BRUSH_TICKS: i32 = 200;

/// Actions of `player_action`.
mod action {
    pub const START: i32 = 0;
    pub const ABORT: i32 = 2;
    pub const STOP: i32 = 3;
    pub const DROP_ALL: i32 = 4;
    pub const DROP_ONE: i32 = 5;
    pub const RELEASE: i32 = 6;
    pub const SWAP: i32 = 7;
    pub const STAB: i32 = 8;
    pub const CHANGE_FACE: i32 = 1;
}

/// Block changed locally during a predicted action (§ 0.1).
struct Pending {
    seq: i32,
    /// Last state known to the server.
    state: u32,
    /// Player's feet at the time of the change.
    player: [f64; 3],
}

/// Crack: block, stage, update tick (§ 2.1).
#[derive(Clone, Copy)]
pub struct Crack {
    pub pos: [i32; 3],
    pub stage: u8,
    tick: u64,
}

/// Prolonged use in progress (§ 5.1).
pub struct Using {
    pub hand: usize,
    pub item: i32,
    pub remaining: i32,
    pub duration: i32,
}

/// Mouse-free demonstration script (`RUSTCRAFT_ACTTEST`).
struct Script {
    steps: Vec<Vec<String>>,
    at: usize,
    wait: u32,
}

#[derive(Default)]
pub struct Interact {
    pub ids: UseIds,
    pub tags: BlockTags,
    /// Presses not consumed: left, right, middle; buttons held.
    presses: [u32; 3],
    held: [bool; 3],
    /// Keys: drop (Ctrl held?), swap hands.
    drop: Option<bool>,
    swap: bool,
    /// World sequence counter (§ 0.1) and prediction mode.
    seq: i32,
    predicting: bool,
    pending: HashMap<[i32; 3], Pending>,
    /// Current sequence at the last accepted positioning.
    teleport_seq: i32,
    /// Mining (§ 1.1).
    pub mining: bool,
    mine_pos: [i32; 3],
    mine_item: Option<ItemStack>,
    progress: f32,
    mine_ticks: f32,
    delay: i32,
    mine_face: Option<u8>,
    /// Delay after a click on nothing (`miss_delay`) and right-click delay.
    miss: i32,
    right_delay: i32,
    /// Attack cooldown (§ 4.1) and tracked main item for its reset.
    pub attack_ticks: i32,
    last_main: i32,
    /// Cracks per miner id.
    cracks: HashMap<i32, Crack>,
    pub using: Option<Using>,
    /// Living-entity flags of the local player at the last tick (bits 1 and 2).
    last_flags: i8,
    tick: u64,
    rng: u64,
    script: Option<Script>,
    /// "Invulnerable" ability (can always eat).
    pub invulnerable: bool,
    /// Tick at the start of the current mining (measure of the break time, demonstration).
    started_at: u64,
    /// Index of the full cube shape (aiming of the held light block or scaffolding).
    pub full_shape: Option<u16>,
    /// Received `minecraft:instrument` registry (goat horns).
    pub instruments: Vec<(String, Option<instrument::Instrument>)>,
}

impl Interact {
    pub fn new(data: &rc_data::VersionData) -> Self {
        let script = rc_data::env_str!("RUSTCRAFT_ACTTEST").map(|s| Script {
            steps: s.split(';').filter(|x| !x.is_empty()).map(|x| x.split(':').map(str::to_owned).collect()).collect(),
            at: 0,
            wait: 0,
        });
        let cube = [rc_data::Aabb { min: [0.0; 3], max: [1.0; 3] }];
        Interact {
            ids: UseIds::new(data),
            full_shape: (0..data.shapes.len()).find(|&i| data.shape(i as u16) == cube).map(|i| i as u16),
            script,
            rng: 0x2545_f491_4f6c_dd1d,
            last_main: i32::MIN,
            ..Default::default()
        }
    }

    /// New client world (entry, respawn, dimension change).
    pub fn reset_world(&mut self) {
        self.seq = 0;
        self.pending.clear();
        self.cracks.clear();
        self.mining = false;
        self.progress = 0.0;
        self.using = None;
        self.last_flags = 0;
    }

    /// Mouse button pressed (0 left, 1 right, 2 middle), mouse captured, no screen.
    pub fn press(&mut self, b: usize) {
        if let Some(p) = self.presses.get_mut(b) {
            *p += 1;
            self.held[b] = true;
        }
    }

    pub fn release(&mut self, b: usize) {
        if let Some(h) = self.held.get_mut(b) {
            *h = false;
        }
    }

    /// Mouse released or screen open: nothing is held any more.
    pub fn release_all(&mut self) {
        self.held = [false; 3];
    }

    /// "Drop" key (Ctrl: whole stack), handled at the tick.
    pub fn drop_key(&mut self, ctrl: bool) {
        self.drop = Some(ctrl);
    }

    /// "Swap hands" key, handled at the tick.
    pub fn swap_key(&mut self) {
        self.swap = true;
    }

    /// Mining in progress: block and progress (tutorial "punch a tree").
    pub(crate) fn mining_state(&self) -> Option<([i32; 3], f32)> {
        self.mining.then_some((self.mine_pos, self.progress))
    }

    fn rand(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `block_update` received: true if it is absorbed by a pending prediction (§ 0.1).
    pub fn server_block(&mut self, pos: [i32; 3], state: u32) -> bool {
        match self.pending.get_mut(&pos) {
            Some(p) => {
                p.state = state;
                true
            }
            None => false,
        }
    }

    /// `section_blocks_update`: removes (and stores) the changes awaiting prediction.
    pub fn filter_section(&mut self, section: [i32; 3], changes: Vec<(u16, u32)>) -> Vec<(u16, u32)> {
        if self.pending.is_empty() {
            return changes;
        }
        changes
            .into_iter()
            .filter(|&(i, state)| {
                let i = i32::from(i);
                let pos = [section[0] * 16 + (i & 15), section[1] * 16 + (i >> 8), section[2] * 16 + ((i >> 4) & 15)];
                !self.server_block(pos, state)
            })
            .collect()
    }

    /// Sequence number for an action sent outside this module (demonstrations).
    pub fn next_seq(&mut self) -> i32 {
        self.seq += 1;
        self.seq
    }

    /// Server positioning accepted.
    pub fn teleported(&mut self) {
        self.teleport_seq = self.seq;
    }

    /// `block_destruction` from another player, or local crack (§ 2.1).
    pub fn crack(&mut self, id: i32, pos: [i32; 3], stage: i32) {
        if (0..10).contains(&stage) {
            let e = self.cracks.entry(id).or_insert(Crack { pos, stage: 0, tick: 0 });
            e.pos = pos;
            e.stage = stage as u8;
            e.tick = self.tick;
        } else {
            self.cracks.remove(&id);
        }
    }

    /// Highest stage per block, within 32 blocks of the camera (§ 2.2).
    pub fn visible_cracks(&self, camera: [f64; 3]) -> Vec<Crack> {
        let mut by_pos: HashMap<[i32; 3], Crack> = HashMap::new();
        for c in self.cracks.values() {
            let d2: f64 = (0..3).map(|a| (f64::from(c.pos[a]) + 0.5 - camera[a]).powi(2)).sum();
            if d2 > 1024.0 {
                continue;
            }
            let e = by_pos.entry(c.pos).or_insert(*c);
            if c.stage > e.stage {
                *e = *c;
            }
        }
        by_pos.into_values().collect()
    }

    fn stage(&self) -> i32 {
        if self.progress > 0.0 { (self.progress * 10.0) as i32 } else { -1 }
    }
}

/// Corner of a face (unit direction) in the order bottom, top, north, south, west, east.
const FACES: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

/// Local result of a use (§ 3.1); `swing`: "predicted" success, the client swings the hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UseRes {
    Success { swing: bool },
    Fail,
    Pass,
}

/// Blocks that react to the main hand's right-click without depending on the item (menus,
/// doors, levers…): the vanilla client runs their reaction and stops there; here the server
/// runs it and sends back the state.
fn reacts_to_hand(name: &str) -> bool {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    const EXACT: [&str; 31] = [
        "chest",
        "trapped_chest",
        "ender_chest",
        "barrel",
        "furnace",
        "blast_furnace",
        "smoker",
        "crafting_table",
        "crafter",
        "enchanting_table",
        "brewing_stand",
        "beacon",
        "hopper",
        "dispenser",
        "dropper",
        "loom",
        "cartography_table",
        "grindstone",
        "smithing_table",
        "stonecutter",
        "lectern",
        "bell",
        "note_block",
        "repeater",
        "comparator",
        "daylight_detector",
        "lever",
        "respawn_anchor",
        "jukebox",
        "dragon_egg",
        "chiseled_bookshelf",
    ];
    EXACT.contains(&n)
        || (n.ends_with("anvil"))
        || n.ends_with("shulker_box")
        || n.ends_with("_button")
        || (n.ends_with("_door") && n != "iron_door")
        || (n.ends_with("_trapdoor") && n != "iron_trapdoor")
        || n.ends_with("_fence_gate")
        || n.ends_with("_bed")
        || n.ends_with("_shelf")
        || n.ends_with("command_block")
        || n == "structure_block"
        || n == "jigsaw"
}

/// (property, value) pairs of the `block_state` component of a stack, in received order; empty
/// without the component (reading stopped at the first unreadable pair).
fn item_block_states(data: &rc_data::VersionData, stack: &ItemStack) -> Vec<(String, String)> {
    let Some(b) = data
        .registry("minecraft:data_component_type")
        .and_then(|names| names.entries.iter().position(|e| e == "minecraft:block_state"))
        .and_then(|id| stack.component(u16::try_from(id).ok()?))
    else {
        return Vec::new();
    };
    let mut r = rc_proto::Reader::new(b);
    let n = rc_proto::varint::read_varint(&mut r).unwrap_or(0).clamp(0, 64);
    let mut out = Vec::new();
    for _ in 0..n {
        let (Ok(k), Ok(v)) = (rc_proto::read_str(&mut r, 32767), rc_proto::read_str(&mut r, 32767)) else { break };
        out.push((k.to_owned(), v.to_owned()));
    }
    out
}

/// Blocks of the item table → block whose placement depends on the neighbourhood (crops, stems, berries, vine,
/// cocoa, redstone dust); string holds anywhere.
const SUPPORTED_PLANTS: [&str; 12] = [
    "minecraft:wheat",
    "minecraft:carrots",
    "minecraft:potatoes",
    "minecraft:beetroots",
    "minecraft:torchflower_crop",
    "minecraft:pitcher_crop",
    "minecraft:melon_stem",
    "minecraft:pumpkin_stem",
    "minecraft:sweet_berry_bush",
    "minecraft:cave_vines",
    "minecraft:cocoa",
    "minecraft:redstone_wire",
];

/// Neighbourhood of a block to place: tags of the block below (crop, vegetation, stem-specific
/// support), raw light of the block, block above (vine, full underside), cocoa support
/// on a side, support of the redstone dust below (full top face or hopper).
#[derive(Default)]
struct PlantSupport {
    crops_below: bool,
    vegetation_below: bool,
    stem_below: bool,
    light: i32,
    vine_above: bool,
    sturdy_above: bool,
    cocoa_beside: bool,
    wire_below: bool,
}

/// Would the block hold here: crop (wheat, carrot, potato, beetroot, torchflower,
/// bottom of the pitcher) on a crop support with light 8 at least, stem on its support, berry bush
/// on a vegetation support, vine attached to a vine or to a full face above,
/// cocoa against a cocoa support on any of the 4 sides (placement tries each side), redstone
/// dust on a full face or a hopper. Any other block: true (not judged here).
fn plant_survives(block: &str, s: &PlantSupport) -> bool {
    match block {
        "minecraft:wheat"
        | "minecraft:carrots"
        | "minecraft:potatoes"
        | "minecraft:beetroots"
        | "minecraft:torchflower_crop"
        | "minecraft:pitcher_crop" => s.light >= 8 && s.crops_below,
        "minecraft:melon_stem" | "minecraft:pumpkin_stem" => s.stem_below,
        "minecraft:sweet_berry_bush" => s.vegetation_below,
        "minecraft:cave_vines" => s.vine_above || s.sturdy_above,
        "minecraft:cocoa" => s.cocoa_beside,
        "minecraft:redstone_wire" => s.wire_below,
        _ => true,
    }
}

/// Brushing: dust and sound on the 5th tick of each elapsed ten (`remaining` before the countdown).
fn brush_due(remaining: i32) -> bool {
    (BRUSH_TICKS - remaining + 1) % 10 == 5
}

/// Base velocity (x, z) of the dust depending on the brushed face: along the face, slightly
/// outwards; top and bottom: perpendicular to the gaze.
fn dust_delta(face: u8, view: [f64; 3]) -> (f64, f64) {
    match face {
        0 | 1 => (view[2], -view[0]),
        2 => (1.0, -0.1),
        3 => (-1.0, 0.1),
        4 => (-0.1, -1.0),
        _ => (0.1, 1.0),
    }
}

/// Right-click on a minecart (other than a command block), local reaction per kind: the plain one
/// is mounted empty and without sneaking (consumed, no swing), otherwise "pass"; chest and hopper
/// open their menu and the furnace takes its fuel (success, swing), sneaking included; TNT and
/// spawner have no reaction of their own.
fn minecart_use(kind: &str, sneak: bool, occupied: bool) -> UseRes {
    match kind {
        "minecart" if !sneak && !occupied => UseRes::Success { swing: false },
        "chest_minecart" | "hopper_minecart" | "furnace_minecart" => UseRes::Success { swing: true },
        _ => UseRes::Pass,
    }
}

/// "Game master" blocks: broken only by an operator.
fn game_master(name: &str) -> bool {
    matches!(
        name,
        "minecraft:command_block"
            | "minecraft:chain_command_block"
            | "minecraft:repeating_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
            | "minecraft:test_block"
            | "minecraft:test_instance_block"
    )
}

impl crate::App {
    /// Sounds that a vanilla server sends to everyone except the player acting by hand: opening
    /// or closing a wooden door, trapdoor or fence gate (pitch 0.9 to 1.0) and
    /// click of a released button being pressed. The lever and note block stay with the server.
    fn predict_hand_sound(&mut self, pos: [i32; 3], state: u32) {
        let Some(name) = self.data.block_of(state).map(|b| b.name.clone()) else { return };
        let n = name.trim_start_matches("minecraft:");
        let prop = |k: &str| self.data.state_properties(state).find(|(p, _)| *p == k).map(|(_, v)| v == "true");
        let (sound, pitch) = if n.ends_with("_door") || n.ends_with("_trapdoor") || n.ends_with("_fence_gate") {
            if n.starts_with("iron_") {
                return;
            }
            let opening = !prop("open").unwrap_or(false);
            (rc_sim::openable_sound_name(&name, opening), self.act.rand() as f32 * 0.1 + 0.9)
        } else if n.ends_with("_button") {
            if prop("powered").unwrap_or(false) {
                return;
            }
            (rc_sim::button_sound(&name, true), 1.0)
        } else {
            return;
        };
        if let Some(a) = self.audio.get() {
            let c = pos.map(|v| f64::from(v) + 0.5);
            a.play(&sound, rc_audio::Category::Block, Some(c), 1.0, pitch, None);
        }
    }

    fn act_state(&self, p: [i32; 3]) -> u32 {
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        view.state(p[0], p[1], p[2])
    }

    fn is_air(&self, state: u32) -> bool {
        self.data.state(state).is_none_or(|s| s.has(flags::AIR))
    }

    /// Changes a block of the client world; remembered if a predicted action is in progress (§ 0.1).
    fn act_set_block(&mut self, p: [i32; 3], state: u32) {
        let old = self.act_state(p);
        if self.act.predicting {
            let player = self.player.pos;
            let seq = self.act.seq;
            self.act.pending.entry(p).and_modify(|e| e.seq = seq).or_insert(Pending { seq, state: old, player });
        }
        let (cx, cz) = (p[0] >> 4, p[2] >> 4);
        self.video.mark_player_change(cx, cz);
        let sy = (p[1] - self.world.min_y) >> 4;
        let idx = rc_world::block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize);
        self.update_section(cx, cz, sy, &[(idx as u16, state)], |s, t| t.air.get(s as usize).copied().unwrap_or(true));
    }

    /// `block_changed_ack` (§ 0.1): end of the predictions up to `seq`.
    pub fn act_ack(&mut self, seq: i32) {
        let done: Vec<[i32; 3]> = self.act.pending.iter().filter(|(_, p)| p.seq <= seq).map(|(k, _)| *k).collect();
        for pos in done {
            let Some(p) = self.act.pending.remove(&pos) else { continue };
            if self.act_state(pos) != p.state {
                self.act_set_block(pos, p.state);
                if self.act.teleport_seq < p.seq && self.collides_with(pos, p.state) {
                    self.player.pos = p.player;
                }
            }
        }
    }

    fn collides_with(&self, pos: [i32; 3], state: u32) -> bool {
        let Some(info) = self.data.state(state) else { return false };
        let s = self.player.attribute("minecraft:scale").unwrap_or(1.0);
        let (hw, h) = (0.3 * s, 1.8 * s);
        let p = self.player.pos;
        self.data.shape(info.collision).iter().any(|b| {
            let lo = [b.min[0] + f64::from(pos[0]), b.min[1] + f64::from(pos[1]), b.min[2] + f64::from(pos[2])];
            let hi = [b.max[0] + f64::from(pos[0]), b.max[1] + f64::from(pos[1]), b.max[2] + f64::from(pos[2])];
            lo[0] < p[0] + hw
                && hi[0] > p[0] - hw
                && lo[1] < p[1] + h
                && hi[1] > p[1]
                && lo[2] < p[2] + hw
                && hi[2] > p[2] - hw
        })
    }

    fn main_stack(&self) -> Option<&ItemStack> {
        self.inventory.hotbar(self.hud.selected)
    }

    fn hand_stack(&self, hand: usize) -> Option<&ItemStack> {
        if hand == 0 { self.main_stack() } else { self.inventory.offhand() }
    }

    /// Possible sending of `set_carried_item` before an action (`hud.md` § 6.2).
    fn sync_carried(&mut self) {
        if let Some(slot) = self.hud.take_carried() {
            let _ = self.session.link.send_carried_item(slot);
        }
    }

    fn effect_amp(&self, name: &str) -> Option<i32> {
        let names = &self.hud_extra.effect_names;
        self.hud_extra
            .effects
            .iter()
            .find(|e| usize::try_from(e.id).ok().and_then(|i| names.get(i)).is_some_and(|n| n == name))
            .map(|e| e.amplifier)
    }

    /// Mining gain per tick (§ 1.2).
    fn mine_gain(&self, state: u32) -> f32 {
        let hardness = self.data.hardness.get(state as usize).copied().unwrap_or(0.0);
        if hardness == -1.0 {
            return 0.0;
        }
        let block = self.data.state(state).map_or(u32::MAX, |s| u32::from(s.block));
        let view = self.main_stack().map(|s| ItemView::new(s, &self.data, &self.act.ids));
        let tool = view.as_ref().and_then(|v| v.tool());
        let (mut v, correct) = tool_speed(tool.as_deref(), &self.act.tags, block);
        let attr = |n: &str, d: f64| self.player.attribute(n).unwrap_or(d) as f32;
        if v > 1.0 {
            v += attr("minecraft:mining_efficiency", 0.0);
        }
        let (haste, conduit) = (self.effect_amp("minecraft:haste"), self.effect_amp("minecraft:conduit_power"));
        if haste.is_some() || conduit.is_some() {
            let a = haste.unwrap_or(0).max(conduit.unwrap_or(0));
            v *= 1.0 + (a + 1) as f32 * 0.2;
        }
        if let Some(f) = self.effect_amp("minecraft:mining_fatigue") {
            v *= 0.3f64.powi(f + 1) as f32;
        }
        v *= attr("minecraft:block_break_speed", 1.0);
        if self.player.eyes_in_water {
            v *= attr("minecraft:submerged_mining_speed", 0.2);
        }
        if !self.player.on_ground {
            v /= 5.0;
        }
        let needs = self.data.needs_tool.get(state as usize).copied().unwrap_or(false);
        let good = !needs || correct;
        v / hardness / if good { 30.0 } else { 100.0 }
    }

    /// Restricted action (§ 1.6): spectator, or adventure without a suitable `can_break`.
    fn restricted(&self, pos: [i32; 3]) -> bool {
        match self.env.game_mode {
            3 => true,
            2 => {
                let state = self.act_state(pos);
                self.main_stack().is_none_or(|s| {
                    !ItemView::new(s, &self.data, &self.act.ids).can_break(&self.data, &self.act.tags, state)
                })
            }
            _ => false,
        }
    }

    /// Local swing of a hand (kind, duration) if no swing is in its first half.
    fn act_swing(&mut self, hand: usize, anim: [u8; 2]) {
        if let Some(me) = &mut self.entities.me {
            let busy = me.swing.is_some_and(|s| s.3 >= 0 && s.3 < s.2 / 2);
            if !busy {
                me.swing = Some((hand as i32, i32::from(anim[0]), i32::from(anim[1]), 0));
            }
        }
    }

    fn attack_anim(&self) -> [u8; 2] {
        self.main_stack().map_or([1, 6], |s| ItemView::new(s, &self.data, &self.act.ids).attack_anim())
    }

    /// Reset of the attack cooldown and of the hand swap (§ 4.1).
    fn reset_attack(&mut self) {
        self.act.attack_ticks = 0;
        if let Some(h) = &mut self.camera.hand {
            h.reset_swap();
        }
    }

    /// Attack strength `force(pt)` (§ 4.1).
    pub fn attack_strength(&self, pt: f32) -> f32 {
        let speed = self.player.attribute("minecraft:attack_speed").unwrap_or(4.0);
        let delay = (1.0 / speed * 20.0) as f32;
        ((self.act.attack_ticks as f32 + pt) / delay).clamp(0.0, 1.0)
    }

    fn begin_predict(&mut self) -> i32 {
        self.act.seq += 1;
        self.act.predicting = true;
        self.act.seq
    }

    fn end_predict(&mut self) {
        self.act.predicting = false;
    }

    /// Predicted local break (§ 1.4); returns true if the block was removed.
    fn local_break(&mut self, pos: [i32; 3]) -> bool {
        if self.restricted(pos) {
            return false;
        }
        if self.env.game_mode == 1
            && self.main_stack().is_some_and(|s| {
                let v = ItemView::new(s, &self.data, &self.act.ids);
                v.tool().is_some_and(|t| !t.creative_destroy) || self.item_name(s.item) == "minecraft:debug_stick"
            })
        {
            return false;
        }
        let state = self.act_state(pos);
        if self.is_air(state) || self.data.block_of(state).is_some_and(|b| game_master(&b.name)) {
            return false;
        }
        // Event 2001 played locally: "broken" sound and block pieces.
        if let Some(a) = self.audio.get() {
            let r = &mut self.world_random;
            crate::world_sounds::level_event(a, &self.data, r, 2001, pos, state as i32, false, self.eye, state);
        }
        self.break_particles(pos, state);
        // The block becomes its fluid alone (air, or source water if it was waterlogged).
        let fluid = self.fluid_only(state);
        self.act_set_block(pos, fluid);
        true
    }

    fn item_name(&self, item: i32) -> &str {
        usize::try_from(item).ok().and_then(|i| self.item_names.get(i)).map_or("", String::as_str)
    }

    /// "Fluid only" state of a state (air, or source water for a waterlogged block).
    fn fluid_only(&self, state: u32) -> u32 {
        let Some(info) = self.data.state(state) else { return 0 };
        let water = self.data.fluids.get(usize::from(info.fluid)).is_some_and(|f| f.ends_with("water"));
        if info.has(flags::FLUID_SOURCE) && water {
            return self.data.blocks.iter().find(|b| b.name == "minecraft:water").map_or(0, |b| b.default_state);
        }
        0
    }

    /// Helper C (`particles.md` § 1.4): break pieces on each box of the shape.
    fn break_particles(&mut self, pos: [i32; 3], state: u32) {
        let Some(info) = self.data.state(state) else { return };
        let o = POptions { state, ..Default::default() };
        // Shape placed like the model (flowers, bamboo: block offset).
        let shift = crate::crack::shape_shift(&self.data, state, pos);
        for b in self.data.shape(info.outline).to_vec() {
            let b = rc_data::Aabb {
                min: std::array::from_fn(|a| b.min[a] + shift[a]),
                max: std::array::from_fn(|a| b.max[a] + shift[a]),
            };
            let l = [0, 1, 2].map(|a| (b.max[a] - b.min[a]).min(1.0));
            let c = l.map(|v| ((v / 0.25).ceil() as i32).max(2));
            for i in 0..c[0] {
                for j in 0..c[1] {
                    for k in 0..c[2] {
                        let rel = [
                            (f64::from(i) + 0.5) / f64::from(c[0]),
                            (f64::from(j) + 0.5) / f64::from(c[1]),
                            (f64::from(k) + 0.5) / f64::from(c[2]),
                        ];
                        let p = [0, 1, 2].map(|a| f64::from(pos[a]) + b.min[a] + rel[a] * l[a]);
                        self.particles.spawn_direct(PKind::Block, &o, p, rel.map(|r| r - 0.5));
                    }
                }
            }
        }
    }

    /// Helper F (§ 1.4): one mining piece on a face.
    fn hit_particle(&mut self, pos: [i32; 3], state: u32, face: u8) {
        let Some(info) = self.data.state(state) else { return };
        if info.has(flags::AIR) {
            return;
        }
        let boxes = self.data.shape(info.outline);
        if boxes.is_empty() {
            return;
        }
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for b in boxes {
            for a in 0..3 {
                lo[a] = lo[a].min(b.min[a]);
                hi[a] = hi[a].max(b.max[a]);
            }
        }
        // Shape placed like the model (flowers, bamboo: block offset).
        let shift = crate::crack::shape_shift(&self.data, state, pos);
        for a in 0..3 {
            (lo[a], hi[a]) = (lo[a] + shift[a], hi[a] + shift[a]);
        }
        let mut p = [0.0; 3];
        for a in 0..3 {
            p[a] = f64::from(pos[a]) + self.act.rand() * (hi[a] - lo[a] - 0.2) + 0.1 + lo[a];
        }
        let f = usize::from(face.min(5));
        let axis = [1, 1, 2, 2, 0, 0][f];
        p[axis] = f64::from(pos[axis]) + if f % 2 == 0 { lo[axis] - 0.1 } else { hi[axis] + 0.1 };
        let o = POptions { state, power: 0.2, scale: 0.6, ..Default::default() };
        self.particles.spawn_direct(PKind::Block, &o, p, [0.0; 3]);
    }

    /// Vertices of the frame's cracks (§ 2.2): opaque blocks, then translucent ones.
    pub fn crack_vertices(&self) -> (Vec<crate::crack::Vertex>, Vec<crate::crack::Vertex>) {
        let (mut early, mut late) = (Vec::new(), Vec::new());
        let origin = [self.shot.pos[0].floor() as i32, self.shot.pos[2].floor() as i32];
        for c in self.act.visible_cracks(self.shot.pos) {
            let state = self.act_state(c.pos);
            if self.is_air(state) {
                continue;
            }
            let mut v = Vec::new();
            if crate::crack::geometry(&self.models, &self.data, c.pos, state, c.stage, origin, &mut v) {
                late.extend(v);
            } else {
                early.extend(v);
            }
        }
        (early, late)
    }

    /// Particles of world events 2001, 2014 (break) and 2019, 2020 (mining).
    pub fn act_level_particles(&mut self, kind: i32, pos: [i32; 3], data: i32) {
        match kind {
            2001 | 2014 => {
                let state = u32::try_from(data).unwrap_or(0);
                if !self.is_air(state) {
                    self.break_particles(pos, state);
                }
            }
            2019 | 2020 => {
                let state = self.act_state(pos);
                self.hit_particle(pos, state, data.clamp(0, 5) as u8);
            }
            _ => {}
        }
    }

    /// `start_mining` (§ 1.3): returns false if refused.
    fn start_destroy(&mut self, pos: [i32; 3], face: u8) -> bool {
        // Outside the world border: local refusal, nothing sent.
        if self.restricted(pos) || !self.weather.border.block_within(pos) {
            return false;
        }
        let link = self.session.link.clone();
        if self.env.game_mode == 1 {
            let seq = self.begin_predict();
            self.local_break(pos);
            let _ = link.send_player_action(action::START, pos, face, seq);
            self.end_predict();
            self.act.delay = 5;
            return true;
        }
        let main = self.main_stack().cloned();
        if !self.act.mining || pos != self.act.mine_pos || main != self.act.mine_item {
            if self.act.mining {
                let _ = link.send_player_action(action::ABORT, self.act.mine_pos, face, 0);
            }
            let seq = self.begin_predict();
            let state = self.act_state(pos);
            let air = self.is_air(state);
            if !air && self.mine_gain(state) >= 1.0 {
                self.local_break(pos);
            } else {
                self.act.mining = true;
                self.act.mine_pos = pos;
                self.act.mine_face = Some(face);
                self.act.mine_item = main;
                self.act.progress = 0.0;
                self.act.mine_ticks = 0.0;
                self.act.started_at = self.act.tick;
                let me = self.player_id;
                let stage = self.act.stage();
                self.act.crack(me, pos, stage);
                // Hit effects: "hit" sound and a mining piece on the face.
                if !air {
                    if let (Some(a), Some(t)) = (self.audio.get(), crate::world_sounds::sound_type(&self.data, state)) {
                        let c = pos.map(|v| f64::from(v) + 0.5);
                        a.play(&t.hit, rc_audio::Category::Block, Some(c), (t.volume + 1.0) / 8.0, t.pitch * 0.5, None);
                    }
                    self.hit_particle(pos, state, face);
                }
            }
            let _ = link.send_player_action(action::START, pos, face, seq);
            self.end_predict();
        }
        true
    }

    /// `continue_mining` (§ 1.3).
    fn continue_destroy(&mut self, pos: [i32; 3], face: u8) -> bool {
        self.sync_carried();
        if self.act.delay > 0 {
            self.act.delay -= 1;
            return true;
        }
        let link = self.session.link.clone();
        // Creative outside the border: survival path (which refuses at the start of mining).
        if self.env.game_mode == 1 && self.weather.border.block_within(pos) {
            let seq = self.begin_predict();
            self.local_break(pos);
            let _ = link.send_player_action(action::START, pos, face, seq);
            self.end_predict();
            self.act.delay = 5;
            return true;
        }
        let main = self.main_stack().cloned();
        if pos != self.act.mine_pos || main != self.act.mine_item {
            return self.start_destroy(pos, face);
        }
        let state = self.act_state(pos);
        if self.is_air(state) {
            self.act.mining = false;
            return false;
        }
        self.act.progress += self.mine_gain(state);
        self.act.mine_ticks += 1.0;
        if self.act.script.is_some() && (self.act.mine_ticks as i32) % 20 == 0 {
            eprintln!(
                "mining: progress {:.4} after {} ticks (gain {:.6}, ground {})",
                self.act.progress,
                self.act.mine_ticks,
                self.mine_gain(state),
                self.player.on_ground
            );
        }
        if self.act.progress >= 1.0 {
            self.act.mining = false;
            let seq = self.begin_predict();
            self.local_break(pos);
            let _ = link.send_player_action(action::STOP, pos, face, seq);
            self.end_predict();
            if self.act.script.is_some() {
                eprintln!(
                    "mining: block {pos:?} broken in {} ticks ({} mining ticks)",
                    self.act.tick - self.act.started_at,
                    self.act.mine_ticks
                );
            }
            self.act.mine_face = None;
            self.act.progress = 0.0;
            self.act.mine_ticks = 0.0;
            self.act.delay = 5;
        } else if self.act.mine_face != Some(face) {
            self.act.mine_face = Some(face);
            let _ = link.send_player_action(action::CHANGE_FACE, pos, face, 0);
        }
        let me = self.player_id;
        let stage = self.act.stage();
        self.act.crack(me, pos, stage);
        true
    }

    /// `stop_mining` (§ 1.3), also on every received `player_position`.
    pub(crate) fn stop_destroy(&mut self) {
        if self.act.mining {
            let _ = self.session.link.send_player_action(action::ABORT, self.act.mine_pos, 0, 0);
            self.act.mining = false;
            self.act.progress = 0.0;
            let me = self.player_id;
            self.act.crack(me, self.act.mine_pos, -1);
            self.reset_attack();
        }
    }

    /// Left click (§ 1.3): returns true if an instant break took place.
    fn start_attack(&mut self, hit: Hit) -> bool {
        // Hands busy (piloted boat, `mounts.md` § 1.6): nothing.
        if self.act.miss > 0 || self.riding.hands_busy {
            return false;
        }
        let link = self.session.link.clone();
        if self.env.game_mode == 3 {
            let target = if let Hit::Entity { id, .. } = hit { Some(id) } else { None };
            let _ = link.send_spectator_action(target);
            return false;
        }
        if let Some(s) = self.main_stack() {
            let v = ItemView::new(s, &self.data, &self.act.ids);
            let charge = v.min_attack_charge();
            if charge > 0.0 && self.attack_strength(0.0) < charge {
                return false;
            }
            if v.piercing() {
                // Lunge (§ 4.4): aiming and damage on the server side, no `punch`.
                self.sync_carried();
                let _ = link.send_player_action(action::STAB, [0; 3], 0, 0);
                self.reset_attack();
                let anim = self.attack_anim();
                self.act_swing(0, anim);
                return false;
            }
        }
        let mut instant = false;
        match hit {
            Hit::Entity { id, at } => self.attack_entity(id, at),
            Hit::Block { pos, face, .. } if !self.is_air(self.act_state(pos)) => {
                self.start_destroy(pos, face);
                instant = self.is_air(self.act_state(pos));
            }
            _ => {
                if self.env.game_mode != 1 {
                    self.act.miss = 10;
                }
                self.reset_attack();
            }
        }
        let anim = self.attack_anim();
        self.act_swing(0, anim);
        let _ = link.send_punch();
        instant
    }

    /// Attack cooldown delay of the held item, in ticks (§ 4.1).
    fn attack_delay(&self) -> f32 {
        let speed = self.player.attribute("minecraft:attack_speed").unwrap_or(4.0);
        (1.0 / speed * 20.0) as f32
    }

    /// Aimed point within the item's range (`attack_range`, § 4.1); true without the component.
    fn in_attack_range(&self, at: [f64; 3]) -> bool {
        let Some(r) = self.main_stack().and_then(|s| ItemView::new(s, &self.data, &self.act.ids).attack_range()) else {
            return true;
        };
        let (min, max) = if self.env.game_mode == 1 { (r[2], r[3]) } else { (r[0], r[1]) };
        let d: f64 = (0..3).map(|a| (at[a] - self.eye[a]).powi(2)).sum::<f64>().sqrt();
        d >= f64::from(min - r[4]) && d <= f64::from(max + r[4])
    }

    /// Attack indicator of the frame (`hud.md` § 2): strength, and "full" on a living entity.
    pub fn attack_indicator(&self, hit: Hit) -> (f32, bool) {
        let force = self.attack_strength(0.0);
        let full = match hit {
            Hit::Entity { id, at } if force >= 1.0 => {
                self.entities.map.get(&id).is_some_and(|e| e.class != crate::entities::Kind::Other)
                    && self.attack_delay() > 5.0
                    && self.in_attack_range(at)
            }
            _ => false,
        };
        (force, full)
    }

    /// Attack on an entity (§ 4.1): `attack` packet, cooldown reset. Damage,
    /// sounds and particles of mobs come from the server.
    fn attack_entity(&mut self, id: i32, at: [f64; 3]) {
        if !self.in_attack_range(at) {
            return;
        }
        self.sync_carried();
        let _ = self.session.link.send_attack(id);
        if self.act.script.is_some() {
            eprintln!("attack: entity {id}, strength {:.2}", self.attack_strength(0.5));
        }
        self.reset_attack();
        if self.env.game_mode == 1 {
            self.act.delay = 5;
        }
    }

    /// Interaction with an entity (§ 4.2). The entity's local reaction is not simulated:
    /// only the cases where the vanilla client always stops (mounting, trading, frames) return
    /// a success; otherwise "pass".
    fn interact_entity(&mut self, hand: usize, id: i32, at: [f64; 3]) -> UseRes {
        let Some(e) = self.entities.map.get(&id) else { return UseRes::Pass };
        let rel = [at[0] - e.pos[0], at[1] - e.pos[1], at[2] - e.pos[2]];
        let kind = e.kind.clone();
        let occupied = !e.passengers.is_empty();
        self.sync_carried();
        let sneak = self.sneak_key();
        let _ = self.session.link.send_interact(id, hand as i32, rel, sneak);
        if self.env.game_mode == 3 {
            return UseRes::Pass;
        }
        // Command block minecart: screen for a creative operator, otherwise nothing.
        if kind == "command_block_minecart" {
            return if self.op_use_minecart(id) { UseRes::Success { swing: true } } else { UseRes::Pass };
        }
        if kind.ends_with("minecart") {
            return minecart_use(&kind, sneak, occupied);
        }
        let rides = kind.ends_with("boat")
            || kind.ends_with("raft")
            || matches!(
                kind.as_str(),
                "horse" | "donkey" | "mule" | "camel" | "llama" | "skeleton_horse" | "zombie_horse"
            );
        let trades = matches!(kind.as_str(), "villager" | "wandering_trader");
        if (rides || trades) && !sneak || matches!(kind.as_str(), "item_frame" | "glow_item_frame") {
            return UseRes::Success { swing: true };
        }
        UseRes::Pass
    }

    fn sneak_key(&self) -> bool {
        self.binds.down(crate::keys::SNEAK)
    }

    /// Block placed by a block item: same name as the item, or that of the shared table (seeds,
    /// carrot, redstone, string…).
    fn block_of_item(&self, item: i32) -> Option<&rc_data::BlockInfo> {
        let name = rc_data::rules::placed_block(self.item_name(item));
        if name.is_empty() {
            return None;
        }
        self.data.blocks.iter().find(|b| b.name == name)
    }

    /// Predicted placement only for a full single-state block (uncertain point 1); the others
    /// wait for the server's `block_update`.
    fn simple_block(&self, b: &rc_data::BlockInfo) -> bool {
        b.state_count == 1
            && self
                .data
                .state(b.default_state)
                .is_some_and(|s| s.has(flags::FULL_COLLISION) && s.has(flags::SOLID_RENDER))
    }

    /// `use_item_on` (§ 3.2): predicted action, packet after the local execution.
    fn use_item_on(&mut self, hand: usize, pos: [i32; 3], face: u8, at: [f64; 3], inside: bool) -> UseRes {
        self.sync_carried();
        // Block outside the border: local failure, no prediction or packet.
        if !self.weather.border.block_within(pos) {
            return UseRes::Fail;
        }
        let seq = self.begin_predict();
        let rel = [0, 1, 2].map(|a| (at[a] - f64::from(pos[a])) as f32);
        let r = self.use_item_on_local(hand, pos, face, f64::from(rel[1]));
        let _ = self.session.link.send_use_item_on_hand(hand as i32, pos, i32::from(face), rel, inside, seq);
        self.end_predict();
        if self.act.script.is_some() {
            eprintln!("demo: use_item_on hand {hand} → {r:?}");
        }
        r
    }

    fn use_item_on_local(&mut self, hand: usize, pos: [i32; 3], face: u8, rel_y: f64) -> UseRes {
        if self.env.game_mode == 3 {
            return UseRes::Success { swing: false };
        }
        let stack = self.hand_stack(hand).cloned();
        let bypass = self.sneak_key() && (self.main_stack().is_some() || self.inventory.offhand().is_some());
        let clicked = self.act_state(pos);
        // Operator blocks (creative, permission ≥ 2): screen opened locally (`op_screens.rs`).
        if !bypass && hand == 0 && self.op_use_block(pos, clicked) {
            return UseRes::Success { swing: true };
        }
        if !bypass && hand == 0 && self.data.block_of(clicked).is_some_and(|b| reacts_to_hand(&b.name)) {
            // Block reaction (chest, door, lever…): the server runs it; the sounds it
            // excludes for the acting player (doors, trapdoors, fence gates, pressed button) are played here.
            self.predict_hand_sound(pos, clicked);
            return UseRes::Success { swing: true };
        }
        // Blocks whose reaction depends on the item or the state (cake, candles, pot…).
        if !bypass
            && hand == 0
            && let Some(r) = self.predict_block_use(stack.as_ref(), pos, clicked, rel_y)
        {
            return r;
        }
        let Some(stack) = stack else { return UseRes::Pass };
        // Item on cooldown: no use on the block.
        if self.on_cooldown(&stack) {
            return UseRes::Pass;
        }
        // Lily pad, frogspawn: nothing on a block, placement goes through their ray (`use_item`).
        if matches!(fluid_use::RayItem::of(self.item_name(stack.item)), Some(fluid_use::RayItem::OnWater { .. })) {
            return UseRes::Pass;
        }
        // Tools on a block (axe, hoe, shovel, wax, bone meal): predicted sound and swing.
        if let Some(r) = self.predict_tool_use(&stack, pos, face, clicked) {
            return r;
        }
        if self.env.game_mode == 2
            && !ItemView::new(&stack, &self.data, &self.act.ids).can_place_on(&self.data, &self.act.tags, clicked)
        {
            return UseRes::Pass;
        }
        // Brush: brushing started if the aim hits a block; consumed in all cases.
        if self.item_name(stack.item) == "minecraft:brush" {
            if matches!(self.brush_hit(), Hit::Block { .. }) {
                self.start_using(hand, stack.item, BRUSH_TICKS);
            }
            return UseRes::Success { swing: false };
        }
        let Some(block) = self.block_of_item(stack.item).cloned() else { return UseRes::Pass };
        // "Game master" items: no placement state without creative or permission 2 (failure).
        if game_master(&block.name) && !(self.env.game_mode == 1 && self.debug.op_level >= 2) {
            return self.place_failed(hand, &stack);
        }
        let replaceable = |s: u32| self.data.state(s).is_some_and(|i| i.has(flags::REPLACEABLE));
        let target = if replaceable(clicked) {
            pos
        } else {
            let d = FACES[usize::from(face.min(5))];
            [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]]
        };
        let ly = target[1] - self.world.min_y;
        if !replaceable(self.act_state(target)) || !(0..self.world.height).contains(&ly) {
            return self.place_failed(hand, &stack);
        }
        // Plants, cocoa, redstone: placement refused where the block does not hold; the edible item
        // (carrot, potato, berries) is then eaten, the others fail without a swing.
        if self.plant_fails(&block.name, target) {
            return self.place_failed(hand, &stack);
        }
        // Test block: mode carried by the item (`block_state`), otherwise start.
        let test_block = block.name == "minecraft:test_block";
        if !self.simple_block(&block) && !test_block {
            return UseRes::Success { swing: true };
        }
        // Properties of the `block_state` component applied to the placed state.
        let state = item_block_states(&self.data, &stack)
            .iter()
            .fold(block.default_state, |s, (k, v)| self.with_prop(s, k, v).unwrap_or(s));
        if self.collides_with(target, state) || self.entity_blocks(target, state) {
            return self.place_failed(hand, &stack);
        }
        self.commit_place(hand, target, state);
        UseRes::Success { swing: true }
    }

    /// Placement refused: the consumable item is used normally (edible seeds eaten),
    /// otherwise failure.
    fn place_failed(&mut self, hand: usize, stack: &ItemStack) -> UseRes {
        self.start_consuming(hand, stack).unwrap_or(UseRes::Fail)
    }

    /// Block of `SUPPORTED_PLANTS` placed at `target` that would not hold there (`plant_survives`).
    /// False for any other block.
    fn plant_fails(&self, block: &str, target: [i32; 3]) -> bool {
        if !SUPPORTED_PLANTS.contains(&block) {
            return false;
        }
        let [x, y, z] = target;
        let below = self.act_state([x, y - 1, z]);
        let above = self.act_state([x, y + 1, z]);
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let stem_tag = match block {
            "minecraft:melon_stem" => "minecraft:supports_melon_stem",
            _ => "minecraft:supports_pumpkin_stem",
        };
        let around = PlantSupport {
            crops_below: self.state_tag("minecraft:supports_crops", below),
            vegetation_below: self.state_tag("minecraft:supports_vegetation", below),
            stem_below: self.state_tag(stem_tag, below),
            light: view.raw_brightness(x, y, z, self.env.has_sky, 0),
            vine_above: self
                .data
                .block_of(above)
                .is_some_and(|b| b.name == "minecraft:cave_vines" || b.name == "minecraft:cave_vines_plant"),
            sturdy_above: self.data.state(above).is_some_and(|i| i.sturdy & 1 != 0),
            cocoa_beside: [[0, -1], [0, 1], [-1, 0], [1, 0]]
                .iter()
                .any(|[dx, dz]| self.state_tag("minecraft:supports_cocoa", self.act_state([x + dx, y, z + dz]))),
            wire_below: self.data.state(below).is_some_and(|i| i.sturdy & 2 != 0)
                || self.data.block_of(below).is_some_and(|b| b.name == "minecraft:hopper"),
        };
        !plant_survives(block, &around)
    }

    /// Block of the state in a synchronised tag (name with or without namespace).
    fn state_tag(&self, tag: &str, state: u32) -> bool {
        let Some(info) = self.data.state(state) else { return false };
        let b = u32::from(info.block);
        self.act.tags.contains(tag, b) || self.act.tags.contains(tag.trim_start_matches("minecraft:"), b)
    }

    /// The stack is on cooldown (group of its `use_cooldown` component, otherwise its item).
    fn on_cooldown(&self, stack: &ItemStack) -> bool {
        crate::hud::cooldown_group(stack, &self.item_names, self.component_ids.of("use_cooldown"))
            .is_some_and(|g| self.cooldowns.is_on_cooldown(&g))
    }

    /// Start of the consumption of a consumable stack (§ 3.3); `None` if it is not one.
    fn start_consuming(&mut self, hand: usize, stack: &ItemStack) -> Option<UseRes> {
        let view = ItemView::new(stack, &self.data, &self.act.ids);
        let c = view.consumable()?;
        let can_eat = self.hud.food_level() < 20
            || view.food().is_none_or(|always| always)
            || self.env.game_mode == 1
            || self.act.invulnerable;
        if !can_eat {
            return Some(UseRes::Fail);
        }
        let duration = (c.seconds * 20.0) as i32;
        if duration > 0 {
            self.start_using(hand, stack.item, duration);
        }
        Some(UseRes::Success { swing: false })
    }

    /// Predicted placement of a block item: block, placement sound (the server excludes the author) and stack
    /// decremented outside creative.
    pub(crate) fn commit_place(&mut self, hand: usize, target: [i32; 3], state: u32) {
        self.act_set_block(target, state);
        if let (Some(a), Some(t)) = (self.audio.get(), crate::world_sounds::sound_type(&self.data, state)) {
            let c = target.map(|v| f64::from(v) + 0.5);
            a.play(&t.place, rc_audio::Category::Block, Some(c), (t.volume + 1.0) / 2.0, t.pitch * 0.8, None);
        }
        if !self.world_sounds.instabuild {
            let slot = if hand == 0 { 36 + self.hud.selected } else { 45 };
            if let Some(Some(s)) = self.inventory.menu.get_mut(slot) {
                s.count -= 1;
                if s.count <= 0 {
                    self.inventory.menu[slot] = None;
                }
            }
        }
    }

    /// An entity that blocks construction (mobs, boats, minecarts, falling blocks,
    /// TNT, crystals) meets the collision shape of the state placed at `p`. The local player is
    /// counted separately (`collides_with`).
    pub(crate) fn entity_blocks(&self, p: [i32; 3], state: u32) -> bool {
        let Some(info) = self.data.state(state) else { return false };
        let shape = self.data.shape(info.collision);
        if shape.is_empty() {
            return false;
        }
        let table = self.renderer.as_ref().and_then(|r| r.entities.as_ref()).map_or(&[][..], |e| e.dims.as_slice());
        let local = self.entities.local;
        let o = p.map(f64::from);
        self.entities.map.iter().any(|(&id, e)| {
            let k = e.kind.as_str();
            let building = e.class != crate::entities::Kind::Other
                || k.ends_with("boat")
                || k.ends_with("raft")
                || k.ends_with("minecart")
                || matches!(k, "falling_block" | "tnt" | "end_crystal");
            if id == local || !building {
                return false;
            }
            let s = e.size(table);
            let hw = f64::from(s.w) / 2.0;
            let q = e.pos;
            shape.iter().any(|c| {
                q[0] - hw < c.max[0] + o[0]
                    && q[0] + hw > c.min[0] + o[0]
                    && q[1] < c.max[1] + o[1]
                    && q[1] + f64::from(s.h) > c.min[1] + o[1]
                    && q[2] - hw < c.max[2] + o[2]
                    && q[2] + hw > c.min[2] + o[2]
            })
        })
    }

    /// `use_item` (§ 3.3): packet first, then local use.
    fn use_item(&mut self, hand: usize) -> UseRes {
        if self.env.game_mode == 3 {
            return UseRes::Pass;
        }
        self.sync_carried();
        let seq = self.begin_predict();
        let _ = self.session.link.send_use_item(hand as i32, seq, self.yaw, self.pitch);
        let r = self.use_item_local(hand);
        self.end_predict();
        if self.act.script.is_some() {
            eprintln!("demo: use_item hand {hand} → {r:?}");
        }
        r
    }

    fn use_item_local(&mut self, hand: usize) -> UseRes {
        let Some(stack) = self.hand_stack(hand).cloned() else { return UseRes::Pass };
        // Item on cooldown: the packet goes out, nothing happens here.
        if self.on_cooldown(&stack) {
            return UseRes::Pass;
        }
        let name = self.item_name(stack.item).to_owned();
        // Writable book: editor opened locally (`special-screens.md` § 4.2).
        if name.strip_prefix("minecraft:").unwrap_or(&name) == "writable_book" {
            self.open_book_edit(hand as i32);
            return UseRes::Success { swing: false };
        }
        // Boats, buckets, bottle, lily pad: own ray, fluids included (`fluid_use.rs`).
        if let Some(r) = self.ray_item_use(hand, &name) {
            return r;
        }
        if let Some(r) = self.start_consuming(hand, &stack) {
            return r;
        }
        let view = ItemView::new(&stack, &self.data, &self.act.ids);
        if view.blocks_attacks() || view.kinetic() {
            self.start_using(hand, stack.item, 72000);
            return UseRes::Success { swing: false };
        }
        let ammo = || {
            self.world_sounds.instabuild
                || self.inventory.menu.iter().flatten().any(|s| {
                    matches!(
                        self.item_name(s.item),
                        "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:tipped_arrow"
                    )
                })
        };
        match name.as_str() {
            "minecraft:bow" => {
                if !ammo() {
                    return UseRes::Fail;
                }
                self.start_using(hand, stack.item, 72000);
                UseRes::Success { swing: false }
            }
            "minecraft:crossbow" => {
                let charged = view
                    .stack
                    .component(self.component_ids_charged())
                    .is_some_and(|b| b.first().is_some_and(|&n| n > 0));
                if charged {
                    UseRes::Success { swing: false }
                } else if ammo() {
                    self.start_using(hand, stack.item, 72000);
                    UseRes::Success { swing: false }
                } else {
                    UseRes::Fail
                }
            }
            "minecraft:trident" => {
                self.start_using(hand, stack.item, 72000);
                UseRes::Success { swing: false }
            }
            "minecraft:spyglass" => {
                if let Some(a) = self.audio.get() {
                    a.play(
                        "minecraft:item.spyglass.use",
                        rc_audio::Category::Player,
                        Some(self.player.pos),
                        1.0,
                        1.0,
                        None,
                    );
                }
                self.start_using(hand, stack.item, 1200);
                UseRes::Success { swing: false }
            }
            "minecraft:goat_horn" => self.play_instrument(hand, &stack),
            "minecraft:snowball"
            | "minecraft:egg"
            | "minecraft:blue_egg"
            | "minecraft:brown_egg"
            | "minecraft:ender_pearl"
            | "minecraft:ender_eye"
            | "minecraft:splash_potion"
            | "minecraft:lingering_potion"
            | "minecraft:experience_bottle"
            | "minecraft:wind_charge"
            | "minecraft:fishing_rod" => UseRes::Success { swing: true },
            _ if self.data.items.get(stack.item as usize).is_some_and(|i| (2..=5).contains(&i.equip_slot)) => {
                // Swappable equipment (armor): the server does the swap.
                UseRes::Success { swing: true }
            }
            _ => UseRes::Pass,
        }
    }

    fn component_ids_charged(&self) -> u16 {
        self.data
            .registry("minecraft:data_component_type")
            .and_then(|r| r.entries.iter().position(|e| e == "minecraft:charged_projectiles"))
            .map_or(u16::MAX, |i| i as u16)
    }

    /// Instrument of a stack: its component (removed: none), otherwise the horn's default one.
    fn instrument_of(&self, stack: &ItemStack) -> Option<instrument::Instrument> {
        let id = self.component_ids.of("instrument");
        if let Some(b) = id.and_then(|i| stack.component(i)) {
            let sounds = self.data.registry("minecraft:sound_event").map_or(&[][..], |r| r.entries.as_slice());
            return instrument::decode(b, &self.act.instruments, sounds);
        }
        if id.is_some_and(|i| stack.removed.contains(&i)) {
            return None;
        }
        self.act.instruments.iter().find(|(n, _)| n == instrument::DEFAULT).and_then(|(_, i)| i.clone())
    }

    /// Goat horn: use started, instrument sound played here (the server excludes the author),
    /// local cooldown of the instrument's duration; without an instrument, failure.
    fn play_instrument(&mut self, hand: usize, stack: &ItemStack) -> UseRes {
        let Some(ins) = self.instrument_of(stack) else { return UseRes::Fail };
        let ticks = (ins.seconds * 20.0).floor() as i32;
        self.start_using(hand, stack.item, ticks);
        // Sound attached to the player: `tick_audio` re-aligns it on them throughout its duration.
        if let Some(a) = self.audio.get()
            && let Some(h) =
                a.play(&ins.sound, rc_audio::Category::Record, Some(self.player.pos), ins.range / 16.0, 1.0, None)
        {
            self.entity_sounds.push((h, self.entities.local));
        }
        if ins.seconds > 0.0
            && let Some(g) = crate::hud::cooldown_group(stack, &self.item_names, self.component_ids.of("use_cooldown"))
        {
            self.cooldowns.apply(&g, ticks);
        }
        UseRes::Success { swing: false }
    }

    /// Brush aim: blocks by their collision shape (border included) along the
    /// gaze over the block reach, then the targetable entities cut off before the block.
    fn brush_hit(&self) -> Hit {
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let data = &self.data;
        let range = self.player.attribute("minecraft:block_interaction_range").unwrap_or(4.5);
        let v = rc_physics::ray::view_vector(self.pitch, self.yaw);
        let to = [0, 1, 2].map(|a| self.eye[a] + v[a] * range);
        let shape = |x: i32, y: i32, z: i32| {
            crate::pick::plain(data.state(view.state(x, y, z)).map_or(&[][..], |i| data.shape(i.collision)))
        };
        let interaction = |x: i32, y: i32, z: i32| rc_physics::ray::interaction_shape(data, view.state(x, y, z));
        let local = self.entities.local;
        let ents = self.pick_candidates(range + 8.0).filter(|c| c.id != local);
        crate::pick::pick_along(self.eye, to, &shape, &interaction, self.weather.border.bounds(), ents)
    }

    /// Brushing tick (`remaining` before the countdown): aim lost → local stop without a packet;
    /// every 10 ticks (5th), dust of the block and brushing sound played here.
    fn brush_tick(&mut self, remaining: i32) {
        let Hit::Block { pos, face, at, .. } = self.brush_hit() else {
            self.act.using = None;
            return;
        };
        if !brush_due(remaining) {
            return;
        }
        let state = self.act_state(pos);
        let left = self.options.s("mainHand") == "left";
        let main = self.act.using.as_ref().is_none_or(|u| u.hand == 0);
        let flip = if left == main { -1.0 } else { 1.0 };
        let (dx, dz) = dust_delta(face, rc_physics::ray::view_vector(self.pitch, self.yaw));
        let o = POptions { state, ..Default::default() };
        let p = [
            at[0] - if face == 4 { f64::from(1.0e-6_f32) } else { 0.0 },
            at[1],
            at[2] - if face == 2 { f64::from(1.0e-6_f32) } else { 0.0 },
        ];
        let count = 7 + (self.act.rand() * 5.0) as i32;
        for _ in 0..count {
            let (rx, rz) = (self.act.rand(), self.act.rand());
            self.particles.spawn(PKind::Block, &o, p, [dx * flip * 3.0 * rx, 0.0, dz * flip * 3.0 * rz]);
        }
        let sound = match self.data.block_of(state).map(|b| b.name.as_str()) {
            Some("minecraft:suspicious_sand") => "minecraft:item.brush.brushing.sand",
            Some("minecraft:suspicious_gravel") => "minecraft:item.brush.brushing.gravel",
            _ => "minecraft:item.brush.brushing.generic",
        };
        if let Some(a) = self.audio.get() {
            let c = pos.map(|v| f64::from(v) + 0.5);
            a.play(sound, rc_audio::Category::Block, Some(c), 1.0, 1.0, None);
        }
    }

    /// End of a spyglass use (release or end): stowing sound, local player.
    fn spyglass_stop(&mut self, item: i32) {
        if self.item_name(item) == "minecraft:spyglass"
            && let Some(a) = self.audio.get()
        {
            a.play("minecraft:item.spyglass.stop_using", rc_audio::Category::Player, Some(self.player.pos), 1.0, 1.0, None);
        }
    }

    /// Start of a prolonged use (§ 5.1).
    fn start_using(&mut self, hand: usize, item: i32, duration: i32) {
        if self.act.script.is_some() {
            eprintln!("use: start hand {hand}, item {item}, {duration} ticks");
        }
        self.act.using = Some(Using { hand, item, remaining: duration, duration });
    }

    /// Right click (§ 3.1).
    fn start_use(&mut self, hit: Hit) {
        if self.act.mining {
            return;
        }
        self.act.right_delay = 4;
        if self.riding.hands_busy {
            return;
        }
        if self.act.script.is_some() {
            let held: Vec<_> = (0..2).map(|h| self.hand_stack(h).map(|s| self.item_name(s.item).to_owned())).collect();
            eprintln!("demo: right click, aim {hit:?}, hands {held:?}");
        }
        for hand in 0..2 {
            let anim =
                self.hand_stack(hand).map_or([1, 6], |s| ItemView::new(s, &self.data, &self.act.ids).interact_anim());
            // Entity whose block is outside the border: right click abandoned (no hand).
            if let Hit::Entity { id, .. } = hit
                && self.entities.map.get(&id).is_some_and(|e| {
                    !self.weather.border.block_within(e.pos.map(|v| v.floor() as i32))
                })
            {
                return;
            }
            if let Hit::Entity { id, at } = hit
                && let UseRes::Success { swing } = self.interact_entity(hand, id, at)
            {
                if swing {
                    self.act_swing(hand, anim);
                }
                return;
            }
            if let Hit::Block { pos, face, at, inside } = hit {
                let before = self.hand_stack(hand).map_or(0, |s| s.count);
                match self.use_item_on(hand, pos, face, at, inside) {
                    UseRes::Success { swing } => {
                        if swing {
                            self.act_swing(hand, anim);
                        }
                        let after = self.hand_stack(hand).map(|s| s.count);
                        if (after.is_some_and(|c| c != before) || self.world_sounds.instabuild)
                            && let Some(h) = &mut self.camera.hand
                        {
                            h.bounce(hand);
                        }
                        return;
                    }
                    UseRes::Fail => return,
                    UseRes::Pass => {}
                }
            }
            // Failure or "pass" of the item: next hand.
            if self.hand_stack(hand).is_some()
                && let UseRes::Success { swing } = self.use_item(hand)
            {
                if swing {
                    self.act_swing(hand, anim);
                }
                if let Some(h) = &mut self.camera.hand {
                    h.bounce(hand);
                }
                return;
            }
        }
    }

    /// Continuous mining (§ 1.3).
    fn continue_attack(&mut self, down: bool, hit: Hit) {
        if !down {
            self.act.miss = 0;
        }
        let piercing = self.main_stack().is_some_and(|s| ItemView::new(s, &self.data, &self.act.ids).piercing());
        if self.act.miss > 0 || self.act.using.is_some() || piercing {
            return;
        }
        match hit {
            Hit::Block { pos, face, .. } if down && !self.is_air(self.act_state(pos)) => {
                if self.continue_destroy(pos, face) {
                    let anim = self.attack_anim();
                    self.act_swing(0, anim);
                    let _ = self.session.link.send_punch();
                }
            }
            Hit::Block { .. } if down => {}
            _ => self.stop_destroy(),
        }
    }

    /// Interaction tick, before the player physics (§ 1.3, 3.1, 6).
    pub fn act_tick(&mut self) {
        self.act.tick += 1;
        // Tooltip context: peaceful difficulty, operator.
        crate::tooltips::set_context(self.exact.difficulty.0 == 0, self.debug.op_level >= 2);
        if self.act.right_delay > 0 {
            self.act.right_delay -= 1;
        }
        self.act.attack_ticks += 1;
        let main = self.main_stack().map_or(-1, |s| s.item);
        if main != self.act.last_main {
            self.act.last_main = main;
            self.act.attack_ticks = 0;
        }
        if self.act.tick.is_multiple_of(20) {
            let now = self.act.tick;
            self.act.cracks.retain(|_, c| now - c.tick <= 400);
        }
        self.act_script();
        let blocked = self.screens.open || self.menus.is_open() || self.chat.input.is_some() || self.death.is_some();
        // The demonstration plays without a captured mouse (window without focus).
        let grabbed = self.grabbed || self.act.script.is_some() || self.riding.scripted();
        if blocked || !grabbed {
            if blocked {
                self.act.miss = 10000;
            }
            self.act.presses = [0; 3];
            self.act.release_all();
        }
        let hit = self.pick();
        // Action keys (§ 6): swap hands, then drop.
        let (swap, drop) = (std::mem::take(&mut self.act.swap), self.act.drop.take());
        if !blocked && self.env.game_mode != 3 {
            if swap {
                let _ = self.session.link.send_player_action(action::SWAP, [0; 3], 0, 0);
            }
            if let Some(all) = drop {
                self.drop_selected(all);
            }
        }
        let mut instant = false;
        if self.act.using.is_some() {
            // Prolonged use: button release, clicks emptied without effect (§ 3.1, 5.1).
            if !blocked && !self.act.held[1] {
                self.release_using();
            }
            self.act.presses = [0; 3];
        }
        while self.act.presses[0] > 0 {
            self.act.presses[0] -= 1;
            if self.act.script.is_some() {
                eprintln!("demo: left click, aim {hit:?}, eye {:?}", self.eye);
            }
            instant |= self.start_attack(hit);
        }
        while self.act.presses[1] > 0 {
            self.act.presses[1] -= 1;
            self.start_use(hit);
        }
        while self.act.presses[2] > 0 {
            self.act.presses[2] -= 1;
            self.pick_block(hit);
        }
        if self.act.held[1] && grabbed && !blocked && self.act.right_delay == 0 && self.act.using.is_none() {
            self.start_use(hit);
        }
        let down = !blocked && !instant && self.act.held[0] && grabbed;
        self.continue_attack(down, hit);
        if self.act.miss > 0 {
            self.act.miss -= 1;
        }
        self.sync_using_flags();
        self.tick_using();
        crate::stack::set_using(
            self.act.using.as_ref().map(|u| (u.item, u.duration - u.remaining, u.remaining, u.hand)),
        );
        // Slowdown from use for the tick physics (`movement.md` § 2.4).
        self.player.using_item =
            self.act.using.as_ref().and_then(|u| {
                self.hand_stack(u.hand).map(|s| ItemView::new(s, &self.data, &self.act.ids).use_effects())
            });
    }

    /// Drop (Q, Ctrl+Q): predicted removal in the selected slot, then `player_action` (§ 6).
    fn drop_selected(&mut self, all: bool) {
        let slot = 36 + self.hud.selected;
        let removed = match self.inventory.menu.get_mut(slot) {
            Some(Some(s)) => {
                if all || s.count <= 1 {
                    self.inventory.menu[slot] = None;
                } else {
                    s.count -= 1;
                }
                true
            }
            _ => false,
        };
        self.sync_carried();
        let a = if all { action::DROP_ALL } else { action::DROP_ONE };
        let _ = self.session.link.send_player_action(a, [0; 3], 0, 0);
        if removed {
            self.act_swing(0, [1, 6]);
        }
    }

    /// Middle click (§ 6): asks the server for the item of the aimed block or entity.
    fn pick_block(&mut self, hit: Hit) {
        let with_data = self.mods.control_key();
        match hit {
            Hit::Block { pos, .. } => {
                let _ = self.session.link.send_pick_block(pos, with_data);
            }
            Hit::Entity { id, .. } => {
                let _ = self.session.link.send_pick_entity(id, with_data);
            }
            Hit::Miss => {}
        }
    }

    /// Release of the use button (§ 5.1).
    fn release_using(&mut self) {
        self.sync_carried();
        let _ = self.session.link.send_player_action(action::RELEASE, [0; 3], 0, 0);
        if self.act.script.is_some() {
            eprintln!(
                "use: released after {} ticks",
                self.act.using.as_ref().map_or(0, |u| u.duration - u.remaining)
            );
        }
        // Same item still in hand: end effect of the released item.
        if let Some(u) = self.act.using.take()
            && self.hand_stack(u.hand).is_some_and(|s| s.item == u.item)
        {
            self.spyglass_stop(u.item);
        }
    }

    /// Use duration of a stack started by the server (entity flags).
    fn use_duration(&self, stack: &ItemStack) -> i32 {
        let view = ItemView::new(stack, &self.data, &self.act.ids);
        if let Some(c) = view.consumable() {
            return (c.seconds * 20.0) as i32;
        }
        match self.item_name(stack.item) {
            "minecraft:spyglass" => 1200,
            "minecraft:brush" => BRUSH_TICKS,
            "minecraft:goat_horn" => self.instrument_of(stack).map_or(0, |i| (i.seconds * 20.0).floor() as i32),
            _ => 72000,
        }
    }

    /// Living-entity flags of the local player (bit 1 "using", bit 2 off hand):
    /// the server starts or ends the use (end of a meal included).
    fn sync_using_flags(&mut self) {
        let flags = self.entities.me.as_ref().map_or(0, |m| m.meta_byte(8));
        if flags == self.act.last_flags {
            return;
        }
        self.act.last_flags = flags;
        let server_using = flags & 1 != 0;
        if server_using && self.act.using.is_none() {
            let hand = usize::from(flags & 2 != 0);
            if let Some(s) = self.hand_stack(hand).cloned() {
                let d = self.use_duration(&s);
                self.start_using(hand, s.item, d);
            }
        } else if !server_using && self.act.using.is_some() {
            if self.act.script.is_some() {
                eprintln!("use: ended by the server");
            }
            self.act.using = None;
        }
    }

    /// Use tick (§ 5.1, 5.2): same item in the hand, consumption effects, countdown.
    fn tick_using(&mut self) {
        let Some(u) = &self.act.using else { return };
        let (hand, item) = (u.hand, u.item);
        let Some(stack) = self.hand_stack(hand).cloned().filter(|s| s.item == item) else {
            self.act.using = None;
            return;
        };
        let view = ItemView::new(&stack, &self.data, &self.act.ids);
        if let Some(c) = view.consumable().map(|c| c.into_owned())
            && let Some(u) = &self.act.using
        {
            let elapsed = u.duration - u.remaining;
            if elapsed > (u.duration as f32 * 0.21875) as i32 && u.remaining % 4 == 0 {
                self.consume_effects(&stack, &c, 5);
            }
        }
        if self.item_name(item) == "minecraft:brush"
            && let Some(remaining) = self.act.using.as_ref().map(|u| u.remaining)
        {
            self.brush_tick(remaining);
        }
        if let Some(u) = &mut self.act.using
            && u.remaining > 0
        {
            u.remaining -= 1;
        }
    }

    /// End of use confirmed by the server (entity event 9 of the local player): same item
    /// still in hand → last bite (16 particles and consumable sound), use ended.
    pub(crate) fn finish_use_event(&mut self) {
        let Some(u) = &self.act.using else { return };
        let (hand, item) = (u.hand, u.item);
        let stack = self.hand_stack(hand).cloned().filter(|s| s.item == item);
        if let Some(stack) = stack {
            let view = ItemView::new(&stack, &self.data, &self.act.ids);
            if let Some(c) = view.consumable().map(|c| c.into_owned()) {
                self.consume_effects(&stack, &c, 16);
            }
            self.spyglass_stop(item);
        }
        self.act.using = None;
    }

    /// Particles of the item (`count`) and consumable sound (§ 5.2), local player.
    fn consume_effects(&mut self, stack: &ItemStack, c: &rc_data::Consumable, count: u32) {
        let (yr, pr) = (self.yaw.to_radians(), self.pitch.to_radians());
        // Rotation around X then Y, like the game's vectors.
        let rot = |v: [f64; 3]| {
            let (s, k) = (f64::from(-pr).sin(), f64::from(-pr).cos());
            let v = [v[0], v[1] * k + v[2] * s, v[2] * k - v[1] * s];
            let (s, k) = (f64::from(-yr).sin(), f64::from(-yr).cos());
            [v[0] * k + v[2] * s, v[1], v[2] * k - v[0] * s]
        };
        if c.particles {
            let o = POptions { item: stack.item, ..Default::default() };
            let eye = [self.player.pos[0], self.player.pos[1] + self.player.eye_height(), self.player.pos[2]];
            for _ in 0..count {
                let d = rot([(self.act.rand() - 0.5) * 0.1, self.act.rand() * 0.1 + 0.1, 0.0]);
                let y = -self.act.rand() * 0.6 - 0.3;
                let p = rot([(self.act.rand() - 0.5) * 0.3, y, 0.6]);
                let pos = [p[0] + eye[0], p[1] + eye[1], p[2] + eye[2]];
                self.particles.spawn(PKind::Item, &o, pos, [d[0], d[1] + 0.05, d[2]]);
            }
        }
        let drink = c.animation == 2;
        let (volume, pitch) = if drink {
            (0.5, self.act.rand() as f32 * 0.1 + 0.9)
        } else {
            let v = if self.act.rand() < 0.5 { 0.5 } else { 1.0 };
            (v, (self.act.rand() as f32 - self.act.rand() as f32) * 0.2 + 1.0)
        };
        if let Some(a) = self.audio.get() {
            a.play(&c.sound, rc_audio::Category::Player, Some(self.player.pos), volume, pitch, None);
        }
    }

    /// Mouse-free demonstration: `RUSTCRAFT_ACTTEST="wait:200;look:0:90;hold:0:200;…"`.
    /// `hold:b:n` holds button b for n ticks, `click:b` one press, `slot:i`, `key:q|Q|f`.
    fn act_script(&mut self) {
        let Some(s) = &mut self.act.script else { return };
        if s.wait > 0 {
            s.wait -= 1;
            if s.wait == 0 && s.at > 0 {
                // End of a `hold`: release.
                if let Some(st) = s.steps.get(s.at - 1)
                    && st[0] == "hold"
                {
                    let b = st.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                    self.act.release(b);
                }
            }
            return;
        }
        let Some(step) = s.steps.get(s.at).cloned() else { return };
        s.at += 1;
        let num = |i: usize| step.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        eprintln!("demo: {} (tick {})", step.join(":"), self.act.tick);
        match step[0].as_str() {
            "wait" => s.wait = num(1) as u32,
            "look" => {
                self.yaw = num(1) as f32;
                self.pitch = num(2) as f32;
            }
            "hold" => {
                s.wait = num(2) as u32;
                self.act.press(num(1) as usize);
            }
            "click" => {
                self.act.press(num(1) as usize);
                self.act.release(num(1) as usize);
            }
            "press" => self.act.press(num(1) as usize),
            "release" => self.act.release(num(1) as usize),
            "slot" => self.hud.selected = (num(1) as usize).min(8),
            "key" => match step.get(1).map(String::as_str) {
                Some("q") => self.act.drop = Some(false),
                Some("Q") => self.act.drop = Some(true),
                Some("f") => self.act.swap = true,
                _ => {}
            },
            // Screenshot now (with `RUSTCRAFT_CAPTURE`), then closing.
            "shot" => {
                let now = self.started.elapsed().as_secs_f64();
                if let Some(c) = &mut self.capture {
                    c.1 = now;
                }
            }
            // Advancements and statistics screens (`progress_app.rs`).
            s if s.starts_with("prog") => self.progress_script(&step),
            "cmd" => {
                let c = step[1..].join(":");
                let _ = self.session.link.send_command(&c);
            }
            _ => {}
        }
    }
}

/// Items that a flower pot accepts: name of the filled pot (azaleas: "bush").
fn potted_name(item: &str) -> String {
    match item {
        "azalea" => "minecraft:potted_azalea_bush".to_owned(),
        "flowering_azalea" => "minecraft:potted_flowering_azalea_bush".to_owned(),
        _ => format!("minecraft:potted_{item}"),
    }
}

impl crate::App {
    fn block_exists(&self, name: &str) -> bool {
        self.data.blocks.iter().any(|b| b.name == name)
    }

    fn state_prop(&self, s: u32, key: &str) -> Option<String> {
        self.data.state_properties(s).find(|(k, _)| *k == key).map(|(_, v)| v.to_owned())
    }

    /// Predicted reaction of blocks whose response depends on the item or the state, like the
    /// game's client: arm swing (or action without a swing); `None` lets the item act.
    fn predict_block_use(
        &mut self,
        stack: Option<&ItemStack>,
        pos: [i32; 3],
        clicked: u32,
        rel_y: f64,
    ) -> Option<UseRes> {
        let name = self.data.block_of(clicked)?.name.clone();
        let n = name.trim_start_matches("minecraft:");
        let item = stack.map(|s| self.item_name(s.item).to_owned()).unwrap_or_default();
        let it = item.trim_start_matches("minecraft:");
        let lit = self.state_prop(clicked, "lit").as_deref() == Some("true");
        let int = |k: &str| self.state_prop(clicked, k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let can_eat = self.hud.food_level() < 20 || self.env.game_mode == 1 || self.act.invulnerable;
        let igniter = it == "flint_and_steel" || it == "fire_charge";
        let swing = Some(UseRes::Success { swing: true });
        let quiet = Some(UseRes::Success { swing: false });
        match n {
            "tnt" if igniter => swing,
            "cake" => {
                // Candle placed (whole cake) or slice eaten.
                if (it.ends_with("candle") && int("bites") == 0) || can_eat {
                    swing
                } else if stack.is_none() {
                    quiet
                } else {
                    None
                }
            }
            _ if n.ends_with("candle_cake") => {
                if igniter {
                    None
                } else if (rel_y > 0.5 && stack.is_none() && lit) || can_eat {
                    swing
                } else {
                    None
                }
            }
            _ if n.ends_with("candle") => {
                (stack.is_none() && self.env.game_mode != 2 && lit).then_some(UseRes::Success { swing: true })
            }
            "flower_pot" => {
                if !it.is_empty() && self.block_exists(&potted_name(it)) {
                    swing
                } else {
                    quiet
                }
            }
            _ if n.starts_with("potted_") => {
                if !it.is_empty() && self.block_exists(&potted_name(it)) {
                    quiet
                } else {
                    swing
                }
            }
            // Compostable below level 8: success on both sides (even at level 7, without effect);
            // at level 8, bare-handed harvest.
            "composter" => {
                let compostable = stack.is_some_and(|s| ItemView::new(s, &self.data, &self.act.ids).compostable());
                (int("level") == 8 || compostable).then_some(UseRes::Success { swing: true })
            }
            "cauldron" | "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => {
                let level = int("level");
                let full = n == "lava_cauldron" || level == 3;
                let view = stack.map(|s| ItemView::new(s, &self.data, &self.act.ids));
                let water_potion = view.as_ref().and_then(ItemView::potion_id).is_some_and(|p| {
                    self.data
                        .registry("minecraft:potion")
                        .and_then(|r| r.entries.get(usize::try_from(p).ok()?))
                        .is_some_and(|e| e == "minecraft:water")
                });
                // Lava or powder snow bucket under water: consumed without an arm swing.
                let under_water = || {
                    let up = self.act_state([pos[0], pos[1] + 1, pos[2]]);
                    self.data.state(up).is_some_and(|i| {
                        self.data.fluids.get(usize::from(i.fluid)).is_some_and(|f| f.ends_with("water"))
                    })
                };
                match it {
                    "water_bucket" => swing,
                    "lava_bucket" | "powder_snow_bucket" => {
                        if under_water() {
                            quiet
                        } else {
                            swing
                        }
                    }
                    "bucket" => (n != "cauldron" && full).then_some(UseRes::Success { swing: true }),
                    "glass_bottle" => (n == "water_cauldron").then_some(UseRes::Success { swing: true }),
                    // Only a water bottle acts; any other potion goes to use (drinking).
                    "potion" => ((n == "cauldron" || (n == "water_cauldron" && level < 3)) && water_potion)
                        .then_some(UseRes::Success { swing: true }),
                    _ if n == "water_cauldron"
                        && view.as_ref().is_some_and(|v| {
                            (v.dyed() && is_dye_washable(it))
                                || (it.ends_with("_banner") && v.banner_has_layers())
                                || (it.ends_with("_shulker_box") && it != "shulker_box")
                        }) =>
                    {
                        swing
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Predicted tool on a block: axe (stripping, scraping, unwaxing), hoe
    /// (tilling), shovel (path), honeycomb (wax), bone meal; sound and particles played
    /// for the player (the server sends them to the others only), arm swing.
    fn predict_tool_use(&mut self, stack: &ItemStack, pos: [i32; 3], face: u8, clicked: u32) -> Option<UseRes> {
        let item = self.item_name(stack.item).to_owned();
        let it = item.trim_start_matches("minecraft:");
        let name = self.data.block_of(clicked)?.name.clone();
        let n = name.trim_start_matches("minecraft:");
        let above_air = self.is_air(self.act_state([pos[0], pos[1] + 1, pos[2]]));
        let not_down = face != 0;
        let (sound, fx) = if it.ends_with("_axe") || it.ends_with("_hoe") || it.ends_with("_shovel") {
            // Shield in the off hand without sneaking: the intent is to parry.
            let shield =
                self.inventory.offhand().is_some_and(|o| ItemView::new(o, &self.data, &self.act.ids).blocks_attacks());
            if shield && !self.sneak_key() {
                return None;
            }
            if it.ends_with("_axe") {
                if !n.starts_with("stripped_")
                    && rc_data::ids::with_key(&["minecraft:stripped_", n], |k| self.block_exists(k))
                {
                    ("minecraft:item.axe.strip", 0)
                } else if (n.starts_with("exposed_") || n.starts_with("weathered_") || n.starts_with("oxidized_"))
                    && rc_data::ids::with_key(&["minecraft:waxed_", n], |k| self.block_exists(k))
                {
                    ("minecraft:item.axe.scrape", 3005)
                } else if n.starts_with("waxed_") {
                    ("minecraft:item.axe.wax_off", 3004)
                } else {
                    return None;
                }
            } else if it.ends_with("_hoe") {
                let tills =
                    (not_down && above_air && matches!(n, "grass_block" | "dirt_path" | "dirt" | "coarse_dirt"))
                        || n == "rooted_dirt";
                if !tills {
                    return None;
                }
                ("minecraft:item.hoe.till", 0)
            } else {
                let path = not_down
                    && above_air
                    && matches!(n, "grass_block" | "dirt" | "podzol" | "coarse_dirt" | "mycelium" | "rooted_dirt");
                if !path {
                    return None;
                }
                ("minecraft:item.shovel.flatten", 0)
            }
        } else if it == "honeycomb" {
            if n.starts_with("waxed_") || !rc_data::ids::with_key(&["minecraft:waxed_", n], |k| self.block_exists(k)) {
                return None;
            }
            ("minecraft:item.honeycomb.wax_on", 3003)
        } else if it == "bone_meal" {
            return self.bone_meal_target(clicked, n, above_air).then_some(UseRes::Success { swing: true });
        } else {
            return None;
        };
        if let Some(a) = self.audio.get() {
            let c = pos.map(|v| f64::from(v) + 0.5);
            a.play(sound, rc_audio::Category::Block, Some(c), 1.0, 1.0, None);
        }
        if fx != 0 {
            self.fx_level_event(fx, pos, 0);
        }
        Some(UseRes::Success { swing: true })
    }

    /// Valid bone meal target seen from the client (arm swing; the particles come
    /// from the server).
    fn bone_meal_target(&self, s: u32, n: &str, above_air: bool) -> bool {
        let int = |k: &str| self.state_prop(s, k).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        match n {
            "wheat" | "carrots" | "potatoes" => int("age") < 7,
            "beetroots" => int("age") < 3,
            "torchflower_crop" => int("age") < 2,
            "pumpkin_stem" | "melon_stem" => int("age") < 7,
            "sweet_berry_bush" => int("age") < 3,
            "cocoa" => int("age") < 2,
            "short_grass" | "fern" | "grass_block" | "moss_block" | "pale_moss_block" => above_air,
            "sunflower" | "lilac" | "rose_bush" | "peony" | "pink_petals" | "wildflowers" => true,
            "red_mushroom" | "brown_mushroom" | "azalea" | "flowering_azalea" | "mangrove_propagule" => true,
            _ => n.ends_with("_sapling"),
        }
    }
}

/// Items whose dye washes off in a water cauldron (tag `cauldron_can_remove_dye`).
fn is_dye_washable(it: &str) -> bool {
    matches!(
        it,
        "leather_helmet"
            | "leather_chestplate"
            | "leather_leggings"
            | "leather_boots"
            | "leather_horse_armor"
            | "wolf_armor"
    )
}

#[cfg(test)]
mod game_master_tests {
    use super::*;

    #[test]
    fn block_state_component_of_a_test_block_item() {
        let mut data = rc_data::VersionData::default();
        data.registries.push(rc_data::model::Registry::new(
            "minecraft:data_component_type",
            vec!["minecraft:custom_data".into(), "minecraft:block_state".into()],
        ));
        let mut value = Vec::new();
        rc_proto::varint::write_varint(&mut value, 2);
        for s in ["powered", "true", "mode", "fail"] {
            rc_proto::write_str(&mut value, s);
        }
        let stack = ItemStack { item: 1, count: 1, added: vec![(1, value.into())], removed: Vec::new() };
        let pairs = |v: &[(&str, &str)]| v.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect::<Vec<_>>();
        assert_eq!(item_block_states(&data, &stack), pairs(&[("powered", "true"), ("mode", "fail")]));
        let bare = ItemStack { item: 1, count: 1, added: Vec::new(), removed: Vec::new() };
        assert!(item_block_states(&data, &bare).is_empty());
    }

    /// Carrot or berries clicked where the plant does not hold: placement refused (the item is eaten).
    #[test]
    fn edible_plants_need_their_support() {
        let s = |crops, veg, light, vine, sturdy| PlantSupport {
            crops_below: crops,
            vegetation_below: veg,
            light,
            vine_above: vine,
            sturdy_above: sturdy,
            ..PlantSupport::default()
        };
        assert!(plant_survives("minecraft:carrots", &s(true, true, 8, false, false)));
        assert!(!plant_survives("minecraft:carrots", &s(true, true, 7, false, false)));
        assert!(!plant_survives("minecraft:potatoes", &s(false, true, 15, false, false)));
        assert!(plant_survives("minecraft:sweet_berry_bush", &s(false, true, 0, false, false)));
        assert!(!plant_survives("minecraft:sweet_berry_bush", &s(true, false, 15, false, false)));
        assert!(plant_survives("minecraft:cave_vines", &s(false, false, 0, true, false)));
        assert!(plant_survives("minecraft:cave_vines", &s(false, false, 0, false, true)));
        assert!(!plant_survives("minecraft:cave_vines", &s(true, true, 15, false, false)));
        assert!(plant_survives("minecraft:stone", &s(false, false, 0, false, false)));
        // The blocks do come from the items of the shared table; only string holds anywhere.
        for item in [
            "carrot",
            "potato",
            "sweet_berries",
            "glow_berries",
            "wheat_seeds",
            "beetroot_seeds",
            "torchflower_seeds",
            "pitcher_pod",
            "melon_seeds",
            "pumpkin_seeds",
            "cocoa_beans",
            "redstone",
        ] {
            let b = rc_data::rules::placed_block(&format!("minecraft:{item}")).to_owned();
            assert!(SUPPORTED_PLANTS.contains(&b.as_str()), "{item}");
        }
        assert!(!SUPPORTED_PLANTS.contains(&rc_data::rules::placed_block("minecraft:string")));
    }

    /// Non-edible seeds, cocoa, redstone: refused where the block does not hold (bare stone,
    /// without a useful neighbour), accepted on their support.
    #[test]
    fn seeds_cocoa_and_wire_need_their_support() {
        let stone = PlantSupport { light: 15, ..PlantSupport::default() };
        for b in SUPPORTED_PLANTS {
            assert!(!plant_survives(b, &stone), "{b} on stone");
        }
        let farmland = PlantSupport { crops_below: true, light: 8, ..PlantSupport::default() };
        for b in ["wheat", "beetroots", "torchflower_crop", "pitcher_crop"] {
            assert!(plant_survives(&format!("minecraft:{b}"), &farmland), "{b}");
            let dark = PlantSupport { light: 7, ..farmland };
            assert!(!plant_survives(&format!("minecraft:{b}"), &dark), "{b} at light 7");
        }
        // Stem: its own tag, no light condition.
        let stem = PlantSupport { stem_below: true, ..PlantSupport::default() };
        assert!(plant_survives("minecraft:melon_stem", &stem));
        assert!(plant_survives("minecraft:pumpkin_stem", &stem));
        assert!(!plant_survives("minecraft:melon_stem", &farmland));
        assert!(plant_survives("minecraft:cocoa", &PlantSupport { cocoa_beside: true, ..PlantSupport::default() }));
        assert!(plant_survives("minecraft:redstone_wire", &PlantSupport { wire_below: true, ..PlantSupport::default() }));
        assert!(plant_survives("minecraft:tripwire", &PlantSupport::default()));
    }

    /// Brush: dust on ticks 5, 15, 25… of the use; dust delta per face.
    #[test]
    fn brush_rhythm_and_dust() {
        let due: Vec<i32> = (0..=BRUSH_TICKS).rev().filter(|&r| brush_due(r)).map(|r| BRUSH_TICKS - r + 1).take(3).collect();
        assert_eq!(due, vec![5, 15, 25]);
        assert!(brush_due(BRUSH_TICKS - 4));
        assert_eq!(dust_delta(2, [0.0; 3]), (1.0, -0.1));
        assert_eq!(dust_delta(5, [0.0; 3]), (0.1, 1.0));
        assert_eq!(dust_delta(1, [0.6, -0.8, 0.0]), (0.0, -0.6));
    }

    /// Local reaction of a minecart: the plain one is mounted empty and standing (no swing), chest,
    /// hopper and furnace always react (swing), TNT and spawner never.
    #[test]
    fn minecart_reactions_by_kind() {
        let ok = UseRes::Success { swing: false };
        let swing = UseRes::Success { swing: true };
        assert_eq!(minecart_use("minecart", false, false), ok);
        assert_eq!(minecart_use("minecart", true, false), UseRes::Pass);
        assert_eq!(minecart_use("minecart", false, true), UseRes::Pass);
        for k in ["chest_minecart", "hopper_minecart", "furnace_minecart"] {
            assert_eq!(minecart_use(k, true, true), swing, "{k}");
        }
        assert_eq!(minecart_use("tnt_minecart", false, false), UseRes::Pass);
        assert_eq!(minecart_use("spawner_minecart", false, false), UseRes::Pass);
    }

    #[test]
    fn game_master_items() {
        for n in ["test_block", "test_instance_block", "structure_block", "jigsaw", "command_block"] {
            assert!(game_master(&format!("minecraft:{n}")), "{n}");
        }
        assert!(!game_master("minecraft:light"));
    }
}
