//! W8c: totem, shield, buckets and worn equipment of the player in singleplayer, on a copy of the test
//! world (`RUSTCRAFT_SOLO_WORLD`). Absent: tests ignored.

use crate::blocks::probe::{Probe, state_string};
use crate::mobs::species::Kind;
use crate::player::Stack;

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    p.s.pos = [110.5, -59.0, 110.5];
    p.s.yaw = 0.0;
    p.s.pitch = 0.0;
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    p.s.life.vit.cooldown = 0;
    for i in 0..p.s.slots.len() {
        p.s.slots[i] = None;
    }
    Some((p, dir))
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

fn st(id: &str, count: i32) -> Option<Stack> {
    Some(Stack { id: id.to_owned(), count, components: None })
}

fn entity_event(p: &Probe, ev: i8) -> bool {
    p.take_events()
        .iter()
        .any(|e| matches!(e, rc_client::Event::Entity(rc_client::EntityEvent::Event { event, .. }) if i32::from(*event) == i32::from(ev)))
}

/// Totem in main hand: the lethal hit leaves 1 health point, consumes the totem, applies
/// regeneration II, absorption II, fire resistance, and sends event 35.
#[test]
fn totem_saves_player() {
    let Some((mut p, dir)) = probe("w8c-totem") else { return };
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = st("minecraft:totem_of_undying", 1);
    p.take_events();
    assert!(p.s.hurt("minecraft:generic", 100.0));
    assert!(!p.s.life.vit.dead, "the player died despite the totem");
    assert_eq!(p.s.life.vit.health, 1.0);
    assert!(p.s.slots[held].is_none(), "totem not consumed");
    let fx = p.s.fx();
    assert!(fx.has("minecraft:regeneration") && fx.has("minecraft:absorption") && fx.has("minecraft:fire_resistance"));
    assert!(entity_event(&p, 35), "event 35 missing");
    // Off hand too; `/kill` (bypasses invulnerability) overrides.
    p.s.slots[45] = st("minecraft:totem_of_undying", 1);
    p.s.life.vit.cooldown = 0;
    p.s.hurt("minecraft:generic_kill", f32::MAX);
    assert!(p.s.life.vit.dead, "the totem does not protect from /kill");
    assert!(p.s.slots[45].is_none() || p.s.slots[45].as_ref().is_some_and(|s| s.id == "minecraft:totem_of_undying"));
    done(p, dir);
}

/// Shield raised for 5 ticks, frontal hit: nothing gets through, shield worn (⌊1 + 5⌋), shield
/// sound; hit from behind: damage taken.
#[test]
fn shield_blocks_front_only() {
    let Some((mut p, dir)) = probe("w8c-shield") else { return };
    p.s.slots[45] = st("minecraft:shield", 1);
    assert!(p.s.w8c_use_item(1), "shield not raised");
    // Raise delay (0.25 s): not active yet.
    let front = p.s.new_mob(Kind::Zombie, [110.5, -59.0, 112.5], 180.0);
    assert!(p.s.blocking_item().is_none(), "shield active without delay");
    if let Some((h, left)) = p.s.life.vit.using {
        p.s.life.vit.using = Some((h, left - 5));
    }
    assert!(p.s.blocking_item().is_some(), "shield inactive after 5 ticks");
    let hp = p.s.life.vit.health;
    let ok = p.s.player_hurt_by_mob(&front, "minecraft:mob_attack", 5.0, Some((0.0, 2.0)));
    assert!(!ok, "parried hit counted as landed");
    assert_eq!(p.s.life.vit.health, hp, "damage got through the shield");
    let dmg =
        p.s.slots[45].as_ref().and_then(|s| crate::items::component(s, "minecraft:damage")).and_then(|n| n.as_i64());
    // Hit scaled to the test world's difficulty, then ⌊1 + hit⌋.
    let dealt: f32 = match p.s.level.difficulty_id() {
        1 => 3.5,
        3 => 7.5,
        _ => 5.0,
    };
    assert_eq!(dmg, Some((1.0 + dealt).floor() as i64), "shield wear");
    p.s.life.vit.cooldown = 0;
    let back = p.s.new_mob(Kind::Zombie, [110.5, -59.0, 108.5], 0.0);
    assert!(p.s.player_hurt_by_mob(&back, "minecraft:mob_attack", 5.0, Some((0.0, -2.0))));
    assert!(p.s.life.vit.health < hp, "hit from behind parried");
    done(p, dir);
}

/// Axe in the attacker's hand: shield lowered, 5 s cooldown.
#[test]
fn axe_disables_shield() {
    let Some((mut p, dir)) = probe("w8c-axe") else { return };
    p.s.slots[45] = st("minecraft:shield", 1);
    p.s.w8c_use_item(1);
    if let Some((h, left)) = p.s.life.vit.using {
        p.s.life.vit.using = Some((h, left - 10));
    }
    let mut m = p.s.new_mob(Kind::Vindicator, [110.5, -59.0, 112.5], 180.0);
    m.equipment[0] = st("minecraft:iron_axe", 1);
    p.s.player_hurt_by_mob(&m, "minecraft:mob_attack", 5.0, Some((0.0, 2.0)));
    assert!(p.s.life.vit.using.is_none(), "shield still raised");
    let shield = p.s.slots[45].clone().expect("shield");
    assert!(p.s.on_cooldown(&shield), "no cooldown");
    assert!(p.s.w8c_use_item(1) && p.s.life.vit.using.is_none(), "raised again during the cooldown");
    done(p, dir);
}

/// The server stops the use of the disarmed shield: the player's data 8 goes back without bit 1,
/// otherwise the client keeps the shield raised.
#[test]
fn disabled_shield_clears_client_using_flag() {
    let Some((mut p, dir)) = probe("w8c-shield-flag") else { return };
    p.s.slots[45] = st("minecraft:shield", 1);
    p.s.w8c_use_item(1);
    p.s.sync_living_flags();
    let flags = |p: &Probe| {
        p.take_events()
            .iter()
            .filter_map(|e| match e {
                rc_client::Event::Entity(rc_client::EntityEvent::Data { values, .. }) => {
                    values.iter().find(|(k, _)| *k == 8).map(|(_, v)| format!("{v:?}"))
                }
                _ => None,
            })
            .next_back()
    };
    assert!(flags(&p).is_some_and(|f| f.contains('3')), "bit 1 and off hand expected");
    if let Some((h, left)) = p.s.life.vit.using {
        p.s.life.vit.using = Some((h, left - 10));
    }
    let mut m = p.s.new_mob(Kind::Vindicator, [110.5, -59.0, 112.5], 180.0);
    m.equipment[0] = st("minecraft:iron_axe", 1);
    p.s.player_hurt_by_mob(&m, "minecraft:mob_attack", 5.0, Some((0.0, 2.0)));
    p.s.sync_living_flags();
    assert!(flags(&p).is_some_and(|f| f.contains('0')), "use stopped without new data 8");
    done(p, dir);
}

/// Adult hoglin against a shield that parries everything: the player is still thrown.
#[test]
fn hoglin_throws_through_full_block() {
    let Some((mut p, dir)) = probe("w8c-hoglin-block") else { return };
    p.s.slots[45] = st("minecraft:shield", 1);
    p.s.w8c_use_item(1);
    if let Some((h, left)) = p.s.life.vit.using {
        p.s.life.vit.using = Some((h, left - 10));
    }
    let m = p.s.new_mob(Kind::Hoglin, [110.5, -59.0, 112.5], 180.0);
    p.s.mobs.player_vel = [0.0; 3];
    let hurt = p.s.player_hurt_by_mob(&m, "minecraft:mob_attack", 5.0, Some((0.0, 2.0)));
    assert!(!hurt, "hit got through despite the shield");
    let v = p.s.mobs.player_vel;
    assert!(v[1] > 0.0 || v[0] != 0.0 || v[2] != 0.0, "no knockback on total block");
    done(p, dir);
}

/// Ravager: a parried hit stuns it one time in two, otherwise knocks the player back hard.
#[test]
fn ravager_reacts_to_parry() {
    let Some((mut p, dir)) = probe("w8c-ravager-block") else { return };
    p.s.slots[45] = st("minecraft:shield", 1);
    p.s.w8c_use_item(1);
    if let Some((h, left)) = p.s.life.vit.using {
        p.s.life.vit.using = Some((h, left - 10));
    }
    let (mut stunned, mut pushed) = (false, false);
    for _ in 0..40 {
        p.s.life.vit.cooldown = 0;
        let mut m = p.s.new_mob(Kind::Ravager, [110.5, -59.0, 112.5], 180.0);
        p.s.mobs.player_vel = [0.0; 3];
        p.s.player_hurt_by_mob(&m, "minecraft:mob_attack", 5.0, Some((0.0, 2.0)));
        p.s.take_riposte(&mut m);
        if m.ill.stun_tick == 40 {
            stunned = true;
        } else if p.s.mobs.player_vel[2].abs() > 1.0 {
            pushed = true;
        }
        if let Some(s) = p.s.slots[45].as_mut() {
            s.components = None; // shield intact for the rest
        }
    }
    assert!(stunned && pushed, "stunned: {stunned}, pushed: {pushed}");
    done(p, dir);
}

/// Creative: a non-empty Shulker box broken drops with its contents, an empty one does not.
#[test]
fn creative_shulker_drops_contents() {
    use rc_proto::nbt::Nbt;
    let Some((mut p, dir)) = probe("w8c-shulker-creative") else { return };
    p.s.game_mode = 1;
    p.run(
        "setblock 112 -59 112 minecraft:shulker_box
setblock 114 -59 112 minecraft:shulker_box
",
    );
    let be = p.s.block_entity_mut([112, -59, 112], "minecraft:shulker_box").expect("block entity");
    let dirt = Stack { id: "minecraft:dirt".into(), count: 7, components: None }.to_nbt(Some(3));
    crate::nbtio::set(be, "Items", Nbt::List(vec![dirt]));
    p.s.try_break([112, -59, 112]);
    p.s.try_break([114, -59, 112]);
    p.run(
        "tick 2
",
    );
    let boxes: Vec<_> =
        p.s.life
            .drops
            .iter()
            .filter(|d| !d.is_removed() && d.stack.id == "minecraft:shulker_box")
            .map(|d| d.stack.clone())
            .collect();
    assert_eq!(boxes.len(), 1, "only the full box should drop");
    assert!(crate::items::component(&boxes[0], "minecraft:container").is_some(), "contents lost");
    done(p, dir);
}

/// Water bucket aimed at the ground: source placed above, empty bucket given back; the empty bucket
/// takes it back; lava too.
#[test]
fn buckets_place_and_pick_up() {
    let Some((mut p, dir)) = probe("w8c-bucket") else { return };
    let held = 36 + p.s.held as usize;
    p.s.pitch = 90.0;
    p.s.slots[held] = st("minecraft:water_bucket", 1);
    assert!(p.s.w8c_use_item(0));
    assert_eq!(state_string(&p.s, p.s.get([110, -59, 110])), "minecraft:water[level=0]");
    assert_eq!(p.s.slots[held].as_ref().map(|s| s.id.as_str()), Some("minecraft:bucket"));
    assert!(p.s.w8c_use_item(0));
    assert_eq!(state_string(&p.s, p.s.get([110, -59, 110])), "minecraft:air");
    assert_eq!(p.s.slots[held].as_ref().map(|s| s.id.as_str()), Some("minecraft:water_bucket"));
    p.s.slots[held] = st("minecraft:lava_bucket", 1);
    assert!(p.s.w8c_use_item(0));
    assert_eq!(state_string(&p.s, p.s.get([110, -59, 110])), "minecraft:lava[level=0]");
    p.s.slots[held] = st("minecraft:bucket", 3);
    assert!(p.s.w8c_use_item(0));
    assert_eq!(state_string(&p.s, p.s.get([110, -59, 110])), "minecraft:air");
    assert_eq!(p.s.slots[held].as_ref().map(|s| s.count), Some(2), "stack of buckets −1");
    assert!(p.s.slots.iter().flatten().any(|s| s.id == "minecraft:lava_bucket"), "lava bucket given back");
    done(p, dir);
}

/// Chestplate on right click: worn, empty hand; second chestplate: swap.
#[test]
fn armor_equips_on_use() {
    let Some((mut p, dir)) = probe("w8c-armor") else { return };
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = st("minecraft:iron_chestplate", 1);
    assert!(p.s.w8c_use_item(0));
    assert_eq!(p.s.slots[6].as_ref().map(|s| s.id.as_str()), Some("minecraft:iron_chestplate"));
    assert!(p.s.slots[held].is_none());
    p.s.slots[held] = st("minecraft:diamond_chestplate", 1);
    assert!(p.s.w8c_use_item(0));
    assert_eq!(p.s.slots[6].as_ref().map(|s| s.id.as_str()), Some("minecraft:diamond_chestplate"));
    assert_eq!(p.s.slots[held].as_ref().map(|s| s.id.as_str()), Some("minecraft:iron_chestplate"));
    done(p, dir);
}

/// Bundle in hand, held use: one stack at the first tick, nothing for the next 10, then
/// one stack every two ticks.
#[test]
fn bundle_use_drops_contents() {
    let Some((mut p, dir)) = probe("w8c-bundle") else { return };
    let held = 36 + p.s.held as usize;
    let mut b = Stack { id: "minecraft:bundle".into(), count: 1, components: None };
    let list = ["dirt", "stone", "sand", "gravel"]
        .iter()
        .map(|n| Stack { id: format!("minecraft:{n}"), count: 2, components: None }.to_nbt(None))
        .collect();
    crate::items::set_component(&mut b, "minecraft:bundle_contents", rc_proto::nbt::Nbt::List(list));
    p.s.slots[held] = Some(b);
    let before = p.s.life.drops.len();
    assert!(p.s.w8c_use_item(0));
    let mut per_tick = Vec::new();
    for _ in 0..14 {
        let (hand, left) = p.s.life.vit.using.expect("use");
        p.s.w8c_use_tick(hand, left);
        p.s.life.vit.using = Some((hand, left - 1));
        per_tick.push(p.s.life.drops.len() - before);
    }
    assert_eq!(per_tick, vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2]);
    done(p, dir);
}

fn items_named(p: &Probe, id: &str) -> i32 {
    p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == id).map(|d| d.stack.count).sum()
}

/// Tall grass broken with shears (upper half): two short grasses — the offset location
/// condition (`offsetY`) sees the other half.
#[test]
fn tall_grass_drops_with_shears() {
    let Some((mut p, dir)) = probe("w8c-grass") else { return };
    p.run("setblock 110 -60 112 minecraft:grass_block\nsetblock 110 -59 112 minecraft:tall_grass[half=lower]\nsetblock 110 -58 112 minecraft:tall_grass[half=upper]\n");
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = st("minecraft:shears", 1);
    p.s.try_break([110, -58, 112]);
    p.run("tick 2\n");
    assert_eq!(state_string(&p.s, p.s.get([110, -59, 112])), "minecraft:air");
    assert_eq!(items_named(&p, "minecraft:short_grass"), 2, "short grasses with shears");
    done(p, dir);
}

fn enchanted(id: &str, ench: &str) -> Stack {
    let mut s = Stack { id: id.into(), count: 1, components: None };
    let f = vec![(ench.to_owned(), rc_proto::nbt::Nbt::Int(1))];
    crate::items::set_component(&mut s, "minecraft:enchantments", rc_proto::nbt::Nbt::Compound(f));
    s
}

/// Cow killed with a Fire Aspect sword: cooked meat (direct attacker and its equipment);
/// with a bow (direct entity = arrow): raw meat.
#[test]
fn fire_aspect_cooks_loot() {
    let Some((mut p, dir)) = probe("w8c-cow") else { return };
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = Some(enchanted("minecraft:diamond_sword", "minecraft:fire_aspect"));
    let cow = p.s.new_mob(Kind::Cow, [112.5, -59.0, 112.5], 0.0);
    let by = Some(crate::mobs::mob::Ent::Player);
    for _ in 0..5 {
        let d = p.s.entity_loot(&cow, "minecraft:player_attack", by, true, "minecraft:entities/cow");
        assert!(d.iter().any(|s| s.id == "minecraft:cooked_beef"), "raw meat despite Fire Aspect: {d:?}");
        let d = p.s.entity_loot(&cow, "minecraft:arrow", by, true, "minecraft:entities/cow");
        assert!(d.iter().all(|s| s.id != "minecraft:cooked_beef"), "cooked meat from an arrow: {d:?}");
    }
    done(p, dir);
}

/// Ghast killed by its fireball sent back by the player: "Tears" disc.
#[test]
fn ghast_fireball_drops_tears() {
    let Some((mut p, dir)) = probe("w8c-ghast") else { return };
    let ghast = p.s.new_mob(Kind::Ghast, [112.5, -50.0, 112.5], 0.0);
    let by = Some(crate::mobs::mob::Ent::Player);
    let d = p.s.entity_loot(&ghast, "minecraft:fireball", by, true, "minecraft:entities/ghast");
    assert!(d.iter().any(|s| s.id == "minecraft:music_disc_tears"), "no disc: {d:?}");
    let d = p.s.entity_loot(&ghast, "minecraft:player_attack", by, true, "minecraft:entities/ghast");
    assert!(d.iter().all(|s| s.id != "minecraft:music_disc_tears"));
    done(p, dir);
}

/// Full Shulker box broken by hand: the item keeps its contents and its name; placed again,
/// the block entity gets them back.
#[test]
fn shulker_box_keeps_contents() {
    use rc_proto::nbt::Nbt;
    let Some((mut p, dir)) = probe("w8c-shulker") else { return };
    p.run("setblock 112 -59 112 minecraft:shulker_box\n");
    let at = [112, -59, 112];
    let be = p.s.block_entity_mut(at, "minecraft:shulker_box").expect("block entity");
    let dirt = Stack { id: "minecraft:dirt".into(), count: 7, components: None }.to_nbt(Some(3));
    crate::nbtio::set(be, "Items", Nbt::List(vec![dirt]));
    crate::nbtio::set(be, "CustomName", Nbt::String("Chest".into()));
    p.s.try_break(at);
    p.run("tick 2\n");
    let item =
        p.s.life
            .drops
            .iter()
            .find(|d| !d.is_removed() && d.stack.id == "minecraft:shulker_box")
            .map(|d| d.stack.clone());
    let item = item.expect("box dropped");
    let c = crate::items::component(&item, "minecraft:container").expect("contents lost");
    let first = &crate::nbtio::list(Some(c))[0];
    assert_eq!(first.get("slot").and_then(Nbt::as_i64), Some(3));
    assert_eq!(first.get("item").and_then(|i| i.get("count")).and_then(Nbt::as_i64), Some(7));
    assert!(crate::items::component(&item, "minecraft:custom_name").is_some(), "name lost");
    // Placed again on the stone: contents and name given back to the block entity.
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = Some(item);
    p.s.use_item_on_block([112, -60, 112], crate::blocks::Dir::Up, [0.5, 1.0, 0.5]);
    let be = p.s.block_entity(at).expect("placed block entity");
    let items = crate::nbtio::list(be.get("Items"));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].get("Slot").and_then(Nbt::as_i64), Some(3));
    assert!(be.get("CustomName").is_some());
    done(p, dir);
}

fn open_table(p: &mut Probe) -> i32 {
    p.run("setblock 112 -59 110 minecraft:crafting_table\n");
    assert!(p.s.use_block_or_bed([112, -59, 110]), "workbench not opened");
    p.s.current_window()
}

fn recipe(p: &mut Probe, id: &str) -> i32 {
    p.s.life.known.insert(id.to_owned());
    let i = p.s.life.recipes.list.iter().position(|r| r.id == id).expect("recipe");
    p.s.life.recipes.displays(i).start
}

fn grid(p: &Probe) -> Vec<i32> {
    let m = p.s.life.menu.as_ref().expect("menu");
    (1..10).map(|i| m.get(&p.s.slots, i).map_or(0, |s| s.count)).collect()
}

/// Recipe book in the workbench: sticks centered (middle column), one more batch
/// on each re-click, taken from the hotbar before the inventory.
#[test]
fn recipe_book_centers_and_adds_batches() {
    let Some((mut p, dir)) = probe("w8c-book") else { return };
    p.s.slots[36] = st("minecraft:oak_planks", 1);
    p.s.slots[9] = st("minecraft:oak_planks", 5);
    let w = open_table(&mut p);
    let r = recipe(&mut p, "minecraft:stick");
    p.s.place_recipe(w, r, false);
    assert_eq!(grid(&p), vec![0, 1, 0, 0, 1, 0, 0, 0, 0], "sticks not centered");
    assert!(p.s.slots[36].is_none(), "the hotbar is not taken first");
    assert_eq!(p.s.slots[9].as_ref().map(|s| s.count), Some(4));
    p.s.place_recipe(w, r, false);
    assert_eq!(grid(&p), vec![0, 2, 0, 0, 2, 0, 0, 0, 0], "re-click without one more batch");
    done(p, dir);
}

/// Shift: as many batches as possible, capped at the stack size of each retained item
/// (eggs: 16).
#[test]
fn recipe_book_max_respects_stack_size() {
    let Some((mut p, dir)) = probe("w8c-book-max") else { return };
    p.s.slots[9] = st("minecraft:pumpkin", 64);
    p.s.slots[10] = st("minecraft:sugar", 64);
    for k in 11..14 {
        p.s.slots[k] = st("minecraft:egg", 16);
    }
    let w = open_table(&mut p);
    let r = recipe(&mut p, "minecraft:pumpkin_pie");
    p.s.place_recipe(w, r, true);
    let g = grid(&p);
    assert_eq!(g.iter().filter(|&&n| n > 0).count(), 3);
    assert!(g.iter().all(|&n| n == 0 || n == 16), "stacks over the cap: {g:?}");
    done(p, dir);
}

fn adv(p: &Probe, id: &str) -> bool {
    let i = *p.s.prog.tree.index.get(id).unwrap_or_else(|| panic!("advancement {id} missing"));
    p.s.prog.done(i)
}

fn mob_mut(p: &mut Probe, id: i32) -> &mut crate::mobs::mob::Mob {
    p.s.mobs.get_mut(id).expect("mob")
}

/// Shears on a tamed wolf wearing armor: the armor drops, the shears
/// wear, "remove_wolf_armor" is obtained; another's wolf: nothing.
#[test]
fn shears_remove_wolf_armor() {
    let Some((mut p, dir)) = probe("w8c-wolf") else { return };
    let held = 36 + p.s.held as usize;
    let mut stranger = p.s.new_mob(Kind::Wolf, [108.5, -59.0, 110.5], 0.0);
    stranger.owner = Some(1234);
    stranger.equipment[6] = st("minecraft:wolf_armor", 1);
    let sid = stranger.id;
    p.s.add_mob(stranger);
    p.s.slots[held] = st("minecraft:shears", 1);
    p.s.player_interact(sid, 0);
    assert!(mob_mut(&mut p, sid).equipment[6].is_some(), "armor removed from another's wolf");
    let mut w = p.s.new_mob(Kind::Wolf, [111.5, -59.0, 110.5], 0.0);
    w.owner = Some(p.s.uuid);
    w.equipment[6] = st("minecraft:wolf_armor", 1);
    let id = w.id;
    p.s.add_mob(w);
    assert!(!adv(&p, "minecraft:husbandry/remove_wolf_armor"));
    assert!(p.s.player_interact(id, 0), "interaction refused");
    assert!(mob_mut(&mut p, id).equipment[6].is_none(), "armor still worn");
    assert_eq!(items_named(&p, "minecraft:wolf_armor"), 1, "armor not dropped");
    let s = p.s.slots[held].clone().expect("shears");
    assert_eq!(p.s.life.items.damage(&s), 1, "shears not worn");
    assert!(adv(&p, "minecraft:husbandry/remove_wolf_armor"), "trigger missing");
    done(p, dir);
}

/// Zombie mounted on a zombie horse: red mushroom possible (vehicle seen by the loot);
/// on foot: never.
#[test]
fn zombie_jockey_loot_sees_vehicle() {
    let Some((mut p, dir)) = probe("w8c-jockey") else { return };
    let by = Some(crate::mobs::mob::Ent::Player);
    let walker = p.s.new_mob(Kind::Zombie, [106.5, -59.0, 106.5], 0.0);
    let mut z = p.s.new_mob(Kind::Zombie, [112.5, -59.0, 112.5], 0.0);
    let mut h = p.s.new_mob(Kind::ZombieHorse, [112.5, -59.0, 112.5], 0.0);
    assert!(p.s.mob_start_riding(&mut z, &mut h, true));
    let zid = z.id;
    p.s.add_mob(h);
    p.s.add_mob(z);
    let rider = p.s.mob_ref(zid).cloned().expect("zombie");
    let (mut on, mut off) = (0, 0);
    for _ in 0..24 {
        let d = p.s.entity_loot(&rider, "minecraft:player_attack", by, true, "minecraft:entities/zombie");
        on += d.iter().filter(|s| s.id == "minecraft:red_mushroom").count();
        let d = p.s.entity_loot(&walker, "minecraft:player_attack", by, true, "minecraft:entities/zombie");
        off += d.iter().filter(|s| s.id == "minecraft:red_mushroom").count();
    }
    assert!(on > 0, "no mushroom for the mounted zombie");
    assert_eq!(off, 0, "mushroom for a zombie on foot");
    done(p, dir);
}

/// Strider in surface lava, player on top: start noted, then 50 blocks further
/// "ride_strider_in_overworld_lava"; vehicle out of the lava: start forgotten.
#[test]
fn strider_lava_ride_distance() {
    let Some((mut p, dir)) = probe("w8c-strider") else { return };
    let m = p.s.new_mob(Kind::Strider, [110.5, -59.0, 110.5], 0.0);
    let id = m.id;
    p.s.add_mob(m);
    p.s.mobs.mounts.riding.vehicle = Some(id);
    mob_mut(&mut p, id).lava_height = 0.5;
    p.s.trig_tick();
    assert_eq!(p.s.prog.lava_ride, Some(p.s.pos));
    p.s.pos[0] += 51.0;
    p.s.trig_tick();
    assert!(adv(&p, "minecraft:nether/ride_strider_in_overworld_lava"), "trigger missing");
    mob_mut(&mut p, id).lava_height = 0.0;
    p.s.trig_tick();
    assert!(p.s.prog.lava_ride.is_none());
    done(p, dir);
}

/// Gust of a wind charge: impact noted; fall started 8 blocks higher →
/// "who_needs_rockets"; impact of a TNT: nothing (cause), impact forgotten.
#[test]
fn wind_charge_fall_after_explosion() {
    let Some((mut p, dir)) = probe("w8c-rockets") else { return };
    let start = p.s.pos;
    p.s.prog.expl_cause_next = Some("minecraft:tnt");
    p.s.wind_burst(None, [110.5, -59.5, 110.5], 1.2, 1.22, "minecraft:entity.wind_charge.wind_burst");
    p.s.prog.expl_cause_next = None;
    assert!(p.s.prog.expl_impact.is_some(), "impact not noted");
    p.s.pos[1] += 8.0;
    p.s.life.vit.fall = 0.5;
    p.s.trig_tick();
    assert!(!adv(&p, "minecraft:adventure/who_needs_rockets"), "cause ignored");
    assert!(p.s.prog.expl_impact.is_none(), "impact kept");
    p.s.pos = start;
    p.s.life.vit.fall = 0.0;
    p.s.prog.fall_start = None;
    p.s.prog.expl_cause_next = Some("minecraft:wind_charge");
    p.s.wind_burst(None, [110.5, -59.5, 110.5], 1.2, 1.22, "minecraft:entity.wind_charge.wind_burst");
    p.s.prog.expl_cause_next = None;
    p.s.pos[1] += 8.0;
    p.s.life.vit.fall = 0.5;
    p.s.trig_tick();
    assert!(adv(&p, "minecraft:adventure/who_needs_rockets"), "trigger missing");
    done(p, dir);
}

/// Player mounting: `started_riding` signaled then emitted at the next tick.
#[test]
fn player_mount_signals_started_riding() {
    let Some((mut p, dir)) = probe("w8c-ride") else { return };
    let mut h = p.s.new_mob(Kind::Horse, [111.5, -59.0, 110.5], 0.0);
    h.tame = true;
    assert!(p.s.player_start_riding(&mut h));
    assert!(p.s.prog.ride_started);
    p.s.add_mob(h);
    p.s.trig_tick();
    assert!(!p.s.prog.ride_started);
    done(p, dir);
}

/// Number providers: `add`/`mul` read `inputs`, `from_float` reads `input`, division
/// by zero and overflow brought back to 0.
#[test]
fn loot_number_providers() {
    let Some((p, dir)) = probe("w8c-providers") else { return };
    let tag = |_: &str, _: &str| false;
    let loot = crate::loot::Loot { pack: &p.s.life.pack, items: &p.s.life.items, block_tag: &tag };
    let ctx = crate::loot::Ctx::default();
    let mut rng = crate::rng::Rng::new(1);
    let mut int = |v: serde_json::Value| loot.int(&v, &ctx, &mut rng);
    assert_eq!(
        int(serde_json::json!({"type": "minecraft:add", "inputs": [1, 2, {"type": "minecraft:constant", "value": 3}]})),
        6
    );
    assert_eq!(int(serde_json::json!({"type": "minecraft:mul", "inputs": [2, 3, 4]})), 24);
    assert_eq!(int(serde_json::json!({"type": "minecraft:mul", "inputs": [65536, 65536]})), 0);
    assert_eq!(int(serde_json::json!({"type": "minecraft:div", "left": 7, "right": 0})), 0);
    assert_eq!(int(serde_json::json!({"type": "minecraft:from_float", "input": 2.7})), 2);
    assert_eq!(int(serde_json::json!({"type": "minecraft:min", "inputs": [5, -3, 9]})), -3);
    done(p, dir);
}
