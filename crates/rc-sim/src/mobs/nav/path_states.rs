//! Path type of a block state, resolved once per state (`RUSTCRAFT_OPT_PATH_TYPE_TABLE`).
//!
//! Every rule of the block path type depends on the state alone (names, `lit` and `open`
//! properties, fluid, walkability) except three block tags: speleothems, fences and walls,
//! which a data pack reload may change. The table keeps, per state, the verdict of the
//! state-only rules and which tags the rules read before reaching it; the tags are read
//! afterwards through the tag bits, so the table stays valid after a reload.

use rc_data::{VersionData, flags};

use super::PathType;

/// Tags the rules read before the per-state verdict, in the order of the rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagChecks {
    /// Verdict reached before any tag.
    None,
    /// Speleothem tag first (damage-cautious), then the verdict.
    Speleothems,
    /// Speleothem tag, then the fence and wall tags (fence), then the verdict.
    SpeleothemsFencesWalls,
}

/// Verdict of the state-only rules and the tags to read before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StateRule {
    pub(crate) verdict: PathType,
    pub(crate) checks: TagChecks,
}

impl StateRule {
    const fn early(verdict: PathType) -> StateRule {
        StateRule { verdict, checks: TagChecks::None }
    }

    const fn after_speleothems(verdict: PathType) -> StateRule {
        StateRule { verdict, checks: TagChecks::Speleothems }
    }

    const fn after_all_tags(verdict: PathType) -> StateRule {
        StateRule { verdict, checks: TagChecks::SpeleothemsFencesWalls }
    }

    /// One byte: verdict in the low 5 bits (27 path types), tag checks above.
    fn to_byte(self) -> u8 {
        let checks = match self.checks {
            TagChecks::None => 0,
            TagChecks::Speleothems => 1,
            TagChecks::SpeleothemsFencesWalls => 2,
        };
        self.verdict as u8 | (checks << 5)
    }

    fn from_byte(b: u8) -> StateRule {
        let checks = match b >> 5 {
            0 => TagChecks::None,
            1 => TagChecks::Speleothems,
            _ => TagChecks::SpeleothemsFencesWalls,
        };
        StateRule { verdict: PathType::ALL[usize::from(b & 31)], checks }
    }

    /// Final path type, the tags answered by `in_tag` (block in the speleothem, fence or wall tag).
    pub(crate) fn resolve(self, in_tag: impl Fn(crate::tag_bits::TagId) -> bool) -> PathType {
        use crate::tag_bits::TagId;
        if self.checks == TagChecks::None {
            return self.verdict;
        }
        if in_tag(TagId::Speleothems) {
            return PathType::DamageCautious;
        }
        if self.checks == TagChecks::SpeleothemsFencesWalls && (in_tag(TagId::Fences) || in_tag(TagId::Walls)) {
            return PathType::Fence;
        }
        self.verdict
    }
}

/// The path type rules of a state, tags left out (`fluid_mob`: kind of fluid as seen by mobs,
/// `land_ok`: walkable on foot). Same order and same tests as the rules by position.
pub(crate) fn state_rule(d: &VersionData, fluid_mob: &[u8], st: u32, land_ok: impl Fn(u32) -> bool) -> StateRule {
    let info = d.state(st);
    if info.is_some_and(|i| i.has(flags::AIR)) {
        return StateRule::early(PathType::Open);
    }
    let name = d.block_of(st).map_or("minecraft:air", |b| b.name.as_str()).trim_start_matches("minecraft:");
    if name.ends_with("_trapdoor") || name == "lily_pad" || name == "big_dripleaf" {
        return StateRule::early(PathType::Trapdoor);
    }
    match name {
        "powder_snow" => return StateRule::early(PathType::PowderSnow),
        "cactus" | "sweet_berry_bush" => return StateRule::early(PathType::Damaging),
        "honey_block" => return StateRule::early(PathType::StickyHoney),
        "cocoa" => return StateRule::early(PathType::Cocoa),
        "wither_rose" => return StateRule::early(PathType::DamageCautious),
        _ => {}
    }
    // The speleothem tag comes here, before lava and fire.
    let fluid =
        info.map_or(0, |i| if i.fluid == 0 { 0 } else { fluid_mob.get(i.fluid as usize).copied().unwrap_or(0) });
    if fluid == 2 {
        return StateRule::after_speleothems(PathType::Lava);
    }
    let has = |key: &str, value: &str| d.state_properties(st).any(|kv| kv == (key, value));
    let lit_campfire = (name == "campfire" || name == "soul_campfire") && has("lit", "true");
    if matches!(name, "fire" | "soul_fire" | "lava" | "magma_block" | "lava_cauldron") || lit_campfire {
        return StateRule::after_speleothems(PathType::Fire);
    }
    if name.ends_with("_door") {
        let door = if has("open", "true") {
            PathType::DoorOpen
        } else if name == "iron_door" {
            PathType::DoorIronClosed
        } else {
            PathType::DoorWoodClosed
        };
        return StateRule::after_speleothems(door);
    }
    if name.ends_with("rail") {
        return StateRule::after_speleothems(PathType::Rail);
    }
    if name.ends_with("_leaves") {
        return StateRule::after_speleothems(PathType::Leaves);
    }
    // A closed fence gate is a fence whatever the fence and wall tags say.
    if name.ends_with("_fence_gate") && !has("open", "true") {
        return StateRule::after_speleothems(PathType::Fence);
    }
    // The fence and wall tags come here, before walkability.
    let verdict = if !land_ok(st) {
        PathType::Blocked
    } else if fluid == 1 {
        PathType::Water
    } else {
        PathType::Open
    };
    StateRule::after_all_tags(verdict)
}

/// The rules of every state, one byte each (≈ 30 KB), built once with the version data.
#[derive(Default)]
pub(crate) struct PathRules {
    bytes: Box<[u8]>,
}

impl PathRules {
    pub(crate) fn new(d: &VersionData, fluid_mob: &[u8], land_ok: impl Fn(u32) -> bool) -> PathRules {
        let bytes = (0..d.states.len() as u32).map(|st| state_rule(d, fluid_mob, st, &land_ok).to_byte()).collect();
        PathRules { bytes }
    }

    /// Rule of state `st`, `None` outside the table.
    pub(crate) fn get(&self, st: u32) -> Option<StateRule> {
        self.bytes.get(st as usize).map(|&b| StateRule::from_byte(b))
    }
}
