//! What the local player's physics must know about its equipment and neighboring entities,
//! gathered before every tick: worn glider (elytra), leather boots (powder snow),
//! attached rockets (thrust while gliding), neighbors that push it (`docs/specs/movement.md`).

use rc_data::VersionData;
use rc_physics::{Aabb, Nearby, Pusher};
use rc_proto_v777::entity::Meta;
use rc_proto_v777::item::ItemStack;

use crate::entities::{Entities, Kind, kind_id_filters_on};
use crate::scoreboard::Scoreboard;

/// Ids of the components read here (`minecraft:data_component_type` registry).
struct GearIds {
    glider: Option<u16>,
    damage: Option<u16>,
    max_damage: Option<u16>,
    unbreakable: Option<u16>,
}

fn gear_ids(data: &VersionData) -> &'static GearIds {
    static IDS: std::sync::OnceLock<GearIds> = std::sync::OnceLock::new();
    IDS.get_or_init(|| {
        let names = data.registry("minecraft:data_component_type").map(|r| r.entries.as_slice()).unwrap_or(&[]);
        let id = |n: &str| names.iter().position(|e| e == n).and_then(|i| u16::try_from(i).ok());
        GearIds {
            glider: id("minecraft:glider"),
            damage: id("minecraft:damage"),
            max_damage: id("minecraft:max_damage"),
            unbreakable: id("minecraft:unbreakable"),
        }
    })
}

/// Component present on the stack: added, or default and not removed.
fn has(s: &ItemStack, defaults: &[u16], id: Option<u16>) -> bool {
    let Some(id) = id else { return false };
    s.component(id).is_some() || (defaults.contains(&id) && !s.removed.contains(&id))
}

fn varint_of(s: &ItemStack, id: Option<u16>) -> Option<i32> {
    let b = s.component(id?)?;
    rc_proto::varint::read_varint(&mut rc_proto::Reader::new(b)).ok()
}

/// (usable worn glider, leather boots on the feet), from the armor slots of the
/// inventory menu (5 head, 6 chest, 7 legs, 8 feet). Glider: `glider` component, item made
/// for this slot, and the next wear point does not break it.
pub(crate) fn gear(menu: &[Option<ItemStack>], data: &VersionData, names: &[String]) -> (bool, bool) {
    let ids = gear_ids(data);
    // Slot code of `ItemInfo::equip_slot` for each slot: head, chest, legs, feet.
    let mut glider = false;
    for (slot, equip) in [(5usize, 5u8), (6, 4), (7, 3), (8, 2)] {
        let Some(Some(s)) = menu.get(slot) else { continue };
        let Some(info) = usize::try_from(s.item).ok().and_then(|i| data.items.get(i)) else { continue };
        if info.equip_slot != equip || !has(s, &info.components, ids.glider) {
            continue;
        }
        let max = varint_of(s, ids.max_damage).unwrap_or(info.max_damage as i32);
        let damageable = has(s, &info.components, ids.max_damage)
            && !has(s, &info.components, ids.unbreakable)
            && has(s, &info.components, ids.damage);
        let damage = varint_of(s, ids.damage).unwrap_or(0);
        if !(damageable && damage >= max - 1) {
            glider = true;
        }
    }
    let boots = matches!(menu.get(8), Some(Some(s))
        if usize::try_from(s.item).ok().and_then(|i| names.get(i)).is_some_and(|n| n == "minecraft:leather_boots"));
    (glider, boots)
}

/// Firework rockets attached to the local player (`player_id`).
pub(crate) fn rockets(ents: &Entities, player_id: i32) -> u32 {
    let attached =
        |e: &crate::entities::Entity| matches!(e.meta.get(&9), Some(Meta::OptionalInt(Some(id))) if *id == player_id);
    let rockets = ents.of_rare_kind(&ents.subsets.rockets, crate::entities::EntitySubsets::is_rocket);
    rockets.filter(|e| attached(e)).count() as u32
}

/// Does the neighbor with team rule `own` (its team `own_team`) push the local player with rule
/// `their` (team `their_team`)? Rules: 0 always, 1 never, 2 other teams, 3 own
/// team (game names: `pushOtherTeams`, `pushOwnTeam`).
fn team_allows(own: i32, own_team: Option<&str>, their: i32, their_team: Option<&str>) -> bool {
    if own == 1 || their == 1 {
        return false;
    }
    let same = own_team.is_some() && own_team == their_team;
    if (own == 3 || their == 3) && same {
        return false;
    }
    (own != 2 && their != 2) || same
}

/// What decides how a neighbour pushes the local player, by type.
struct PushKind {
    /// Remote player (skipped in spectator mode).
    player: bool,
    /// Player or mannequin: standing width 0.6 whatever the table says.
    avatar: bool,
    /// Pushes as a boat.
    boat: bool,
    /// Never pushes: bat, armor stand, parrot, vex (passes through).
    quiet: bool,
}

impl PushKind {
    fn from_facts(e: &crate::entities::Entity) -> PushKind {
        let f = &e.facts;
        PushKind { player: f.player, avatar: f.avatar, boat: f.boat_or_raft, quiet: f.never_pushes }
    }

    /// By the type name (old path, test reference).
    fn by_name(k: &str) -> PushKind {
        PushKind {
            player: k == "player",
            avatar: k == "player" || k == "mannequin",
            boat: k.ends_with("boat") || k.ends_with("raft"),
            quiet: matches!(k, "bat" | "armor_stand" | "parrot" | "vex"),
        }
    }
}

/// Entities within 4 blocks of the local player at `me` (boxes, alive, way of pushing).
/// `spectator`: remote player in spectator (it does not push).
pub(crate) fn nearby(
    ents: &Entities,
    me: [f64; 3],
    scores: &Scoreboard,
    players: &crate::players::PlayerList,
    local_name: &str,
    spectator: &dyn Fn(u128) -> bool,
    out: &mut Vec<Nearby>,
) {
    nearby_with(ents, me, (scores, players, local_name), spectator, out, kind_id_filters_on());
}

/// `nearby` with the kind test chosen (`by_facts`: facts of the type, otherwise type names).
fn nearby_with(
    ents: &Entities,
    me: [f64; 3],
    (scores, players, local_name): (&Scoreboard, &crate::players::PlayerList, &str),
    spectator: &dyn Fn(u128) -> bool,
    out: &mut Vec<Nearby>,
    by_facts: bool,
) {
    out.clear();
    let teams = !scores.members.is_empty();
    let mine = teams.then(|| scores.team_named(local_name)).flatten();
    let their_rule = mine.map_or(0, |(_, t)| t.params.collision);
    for e in ents.map.values() {
        if (0..3).any(|a| (e.pos[a] - me[a]).abs() > 4.0) {
            continue;
        }
        let PushKind { player, avatar, boat, quiet } =
            if by_facts { PushKind::from_facts(e) } else { PushKind::by_name(&e.kind) };
        if player && spectator(e.uuid) {
            continue;
        }
        let living = e.class != Kind::Other;
        let d =
            usize::try_from(e.kind_id).ok().and_then(|i| ents.dims.get(i)).copied().unwrap_or([0.6, 1.8, 1.62, 1.8]);
        let h = f64::from(e.dims(&ents.dims)[0]);
        let width = if avatar {
            if e.pose() == 2 { 0.2 } else { 0.6 }
        } else if living && e.pose() == 2 {
            0.2
        } else {
            f64::from(d[0]) * f64::from(e.size_factor().unwrap_or_else(|| if e.baby() { 0.5 } else { 1.0 }))
        };
        let r = width / 2.0;
        let bb = Aabb { min: [e.pos[0] - r, e.pos[1], e.pos[2] - r], max: [e.pos[0] + r, e.pos[1] + h, e.pos[2] + r] };
        let mut pusher = if boat {
            Pusher::Boat
        } else if living && !quiet {
            Pusher::Living
        } else {
            Pusher::None
        };
        if pusher != Pusher::None && teams {
            let holder = crate::holder_name(e, players, true);
            let own = holder.as_deref().and_then(|h| scores.team_named(h));
            let own_rule = own.map_or(0, |(_, t)| t.params.collision);
            if !team_allows(own_rule, own.map(|o| o.0), their_rule, mine.map(|m| m.0)) {
                pusher = Pusher::None;
            }
        }
        out.push(Nearby { bb, pos: e.pos, living, pusher });
    }
}

impl crate::App {
    /// Before the physics tick: worn glider and boots, attached rockets, neighbors.
    pub(crate) fn player_env_tick(&mut self) {
        let (glider, boots) = gear(&self.inventory.menu, &self.data, &self.item_names);
        self.player.glider = glider;
        self.player.leather_boots = boots;
        self.player.rocket_boosts = rockets(&self.entities, self.player_id);
        let players = &self.players;
        let spectator = |u: u128| players.map.get(&u).is_some_and(|p| p.game_mode == 3);
        let mut list = std::mem::take(&mut self.player.nearby);
        nearby(&self.entities, self.player.pos, &self.scores, players, &self.local_name, &spectator, &mut list);
        self.player.nearby = list;
    }
}

/// Flags of the local player's entity received from the server: gliding (index 0, bit 0x80) and
/// spin attack (index 8, bit 4).
pub(crate) fn local_flags(values: &[(u8, Meta)], player: &mut rc_physics::Player) {
    for (i, v) in values {
        match (i, v) {
            (0, Meta::Byte(b)) => player.fall_flying = *b as u8 & 0x80 != 0,
            (8, Meta::Byte(b)) => player.spin_attack = *b & 4 != 0,
            _ => {}
        }
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::{AddEntity, Meta};

    fn data() -> rc_data::VersionData {
        rc_testdata::version_data().expect("version data missing (extraction)")
    }

    /// Push rule of every type of the 26.3 registry: facts and names agree.
    #[test]
    fn push_kind_of_every_registry_type_matches_the_names() {
        let data = data();
        let names = data.registry("minecraft:entity_type").expect("type registry").entries.clone();
        assert!(names.len() > 100, "type registry too short: {}", names.len());
        let mut ents = Entities { type_names: names.clone(), ..Default::default() };
        for i in 0..names.len() {
            let add =
                AddEntity { id: i as i32, uuid: 0, kind: i as i32, pos: [0.0; 3], velocity: [0.0; 3], pitch: 0.0, yaw: 0.0, head_yaw: 0.0, data: 0 };
            ents.apply(EntityEvent::Add(Box::new(add)));
        }
        let key = |p: PushKind| (p.player, p.avatar, p.boat, p.quiet);
        let differ: Vec<&str> = ents
            .map
            .values()
            .filter(|e| key(PushKind::from_facts(e)) != key(PushKind::by_name(&e.kind)))
            .map(|e| e.kind.as_str())
            .collect();
        assert!(differ.is_empty(), "{differ:?}");
    }

    /// 200 ticks of a crowd of 1,000 mixed entities moving around the player (teams on): the
    /// neighbours found by facts equal those found by names, bit for bit; a sabotaged fact (cows
    /// that never push) is caught.
    #[test]
    fn neighbours_by_facts_equal_neighbours_by_names() {
        let data = data();
        let names = data.registry("minecraft:entity_type").expect("type registry").entries.clone();
        let kinds = ["cow", "player", "mannequin", "bat", "armor_stand", "parrot", "vex", "oak_boat", "bamboo_raft", "minecart", "item", "zombie", "slime", "shulker"];
        let id_of = |k: &str| names.iter().position(|n| n.strip_prefix("minecraft:") == Some(k)).expect(k) as i32;
        let mut ents = Entities { type_names: names.clone(), dims: data.entity_dims.clone(), ..Default::default() };
        let mut seed = 0x1234_5678_9abc_def1u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for id in 0..1_000 {
            let kind = kinds[(next() % kinds.len() as u64) as usize];
            let pos = [(next() % 900) as f64 / 100.0 - 4.5, 64.0, (next() % 900) as f64 / 100.0 - 4.5];
            let add = AddEntity { id, uuid: id as u128, kind: id_of(kind), pos, velocity: [0.0; 3], pitch: 0.0, yaw: 0.0, head_yaw: 0.0, data: 0 };
            ents.apply(EntityEvent::Add(Box::new(add)));
            let pose = Meta::Pose([0, 0, 2][(next() % 3) as usize]);
            ents.apply(EntityEvent::Data { id, values: vec![(6, pose), (16, Meta::Bool(next().is_multiple_of(4)))] });
        }
        let mut scores = Scoreboard::default();
        let params = rc_proto_v777::scores::TeamParams { display: None, prefix: None, suffix: None, nametag: 0, collision: 2, color: None, options: 0 };
        let members = (0..500).map(|i: u128| crate::app_entity_sets::UuidText::new(i).as_str().to_owned()).chain(["Me".to_owned()]).collect();
        scores.apply(rc_proto_v777::scores::ScoreEvent::Team { name: "half".into(), mode: 0, params: Some(params), members });
        let players = crate::players::PlayerList::default();
        let spectator = |u: u128| u.is_multiple_of(7);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let mut run = |ents: &Entities| {
            let mut differ = 0;
            for t in 0..200 {
                let me = [f64::from(t % 20) * 0.1 - 1.0, 64.0, 0.3];
                nearby_with(ents, me, (&scores, &players, "Me"), &spectator, &mut a, false);
                nearby_with(ents, me, (&scores, &players, "Me"), &spectator, &mut b, true);
                differ += usize::from(a != b);
            }
            (differ, a.len())
        };
        let (differ, found) = run(&ents);
        assert_eq!(differ, 0);
        assert!(found > 50, "too few neighbours: {found}");
        // Sabotage: cows that never push.
        for e in ents.map.values_mut().filter(|e| e.kind == "cow") {
            e.facts.never_pushes = true;
        }
        assert!(run(&ents).0 > 0, "sabotage not caught");
    }
}

#[cfg(test)]
mod tests {
    use super::team_allows;

    #[test]
    fn team_collision_rules() {
        assert!(team_allows(0, None, 0, None));
        assert!(!team_allows(1, Some("a"), 0, None));
        assert!(!team_allows(0, None, 1, Some("b")));
        // Rule 2: only teammates push each other (game code, despite the name).
        assert!(team_allows(2, Some("a"), 0, Some("a")));
        assert!(!team_allows(2, Some("a"), 0, Some("b")));
        // Rule 3: never between teammates, yes with the others.
        assert!(!team_allows(3, Some("a"), 0, Some("a")));
        assert!(team_allows(3, Some("a"), 0, Some("b")));
    }
}
