//! G3-F10: `data modify … compute`, mob equipment slots (`execute if items`),
//! `data merge entity` that keeps the UUID, typed arrays in paths. Test world
//! `RUSTCRAFT_SOLO_WORLD`.

use rc_proto::nbt::Nbt;

use super::ents::E;
use super::w11_tests::{big_stack, chats, open, run};

#[test]
fn data_compute_equipment_and_uuid() {
    big_stack(|| {
        let Some((mut s, session, _)) = open("g3f10") else { return };
        s.loaded = true;
        let storage = |s: &crate::server::Server, k: &str| s.cmd.storage.get("t:c").and_then(|n| n.get(k).cloned());

        // `compute`: inline providers, default context or an entity's.
        assert!(
            run(&mut s, "data modify storage t:c i set compute default integer {type:\"minecraft:constant\",value:7}")
                .0
        );
        assert_eq!(storage(&s, "i"), Some(Nbt::Int(7)));
        assert!(
            run(&mut s, "data modify storage t:c f set compute default float {type:\"minecraft:constant\",value:2.5}")
                .0
        );
        assert_eq!(storage(&s, "f"), Some(Nbt::Float(2.5)));
        assert!(
            run(
                &mut s,
                "data modify storage t:c e set compute entity @s integer {type:\"minecraft:constant\",value:3}"
            )
            .0
        );
        assert_eq!(storage(&s, "e"), Some(Nbt::Int(3)));
        assert!(
            run(
                &mut s,
                "data modify storage t:c ef set compute entity @s float {type:\"minecraft:constant\",value:1.5}"
            )
            .0
        );
        assert_eq!(storage(&s, "ef"), Some(Nbt::Float(1.5)));
        // Context of a block: loaded position required (the test world may not be loaded here).
        chats(&session);
        if run(
            &mut s,
            "data modify storage t:c b set compute block ~ ~ ~ float {type:\"minecraft:constant\",value:0.25}",
        )
        .0
        {
            assert_eq!(storage(&s, "b"), Some(Nbt::Float(0.25)));
        } else {
            let c = chats(&session);
            assert!(c.iter().any(|m| m.contains("argument.pos.unloaded")), "{c:?}");
        }
        assert!(run(&mut s, "data modify storage t:c l set value []").0);
        assert!(
            run(
                &mut s,
                "data modify storage t:c l append compute default integer {type:\"minecraft:constant\",value:4}"
            )
            .0
        );
        assert_eq!(storage(&s, "l"), Some(Nbt::List(vec![Nbt::Int(4)])));
        chats(&session);

        // Equipment of a mob seen by `execute if items entity`.
        assert!(run(&mut s, "summon minecraft:zombie ~ ~ ~ {Tags:[\"g3\"],NoAI:1b,equipment:{mainhand:{id:\"minecraft:diamond_sword\",count:1},head:{id:\"minecraft:stone\",count:3}}}").0);
        let z = s
            .all_ents()
            .into_iter()
            .find(|e| matches!(e, E::Mob(_)) && s.ent_type(*e) == "minecraft:zombie")
            .expect("zombie");
        let (ok, n) = run(&mut s, "execute if items entity @e[tag=g3,limit=1] weapon.mainhand minecraft:diamond_sword");
        assert!(ok && n == 1, "{:?}", chats(&session));
        let (ok, n) = run(&mut s, "execute if items entity @e[tag=g3,limit=1] armor.* *");
        assert!(ok && n == 3, "{n} {:?}", chats(&session));

        // `data merge entity`: the provided UUID is ignored.
        let before = s.ent_uuid(z);
        assert!(run(&mut s, "data merge entity @e[tag=g3,limit=1] {UUID:[I;1,2,3,4],CustomName:\"x\"}").0);
        assert_eq!(s.ent_uuid(z), before);

        // Typed array in a path: UUID element read, `nothing_found` with the prefix.
        let (ok, _) = run(&mut s, "data get entity @e[tag=g3,limit=1] UUID[0]");
        assert!(ok, "{:?}", chats(&session));
        chats(&session);
        assert!(!run(&mut s, "data get storage t:c i.a.b").0);
        let c = chats(&session);
        assert!(c.iter().any(|m| m.contains("arguments.nbtpath.nothing_found") && m.contains("\"i.a\"")), "{c:?}");
    });
}

/// Unreadable inline provider refused without writing anything; error order of the game (argument
/// decoded at reading, before the target); slot table common to `execute if items`, `item`
/// and `loot`: armor stand, held item of the player, slot name `horse.saddle` unknown to the game.
#[test]
fn compute_invalid_and_common_slots() {
    big_stack(|| {
        let Some((mut s, session, _)) = open("g3f10b") else { return };
        s.loaded = true;
        let v = |s: &crate::server::Server| s.cmd.storage.get("t:d").and_then(|n| n.get("v").cloned());
        chats(&session);
        for p in [
            "integer {type:\"minecraft:nope\"}",
            "integer {}",
            "float {type:\"minecraft:nope\"}",
            "integer [1]",
            "integer {type:\"minecraft:uniform\",min:1}",
            "float {type:\"minecraft:abs\",input:\"minecraft:absent\"}",
        ] {
            assert!(!run(&mut s, &format!("data modify storage t:d v set compute default {p}")).0, "{p}");
            let c = chats(&session);
            assert!(c.iter().any(|m| m.contains("argument.resource_or_id.failed_to_parse")), "{p} {c:?}");
            assert_eq!(v(&s), None, "{p}");
        }
        assert!(
            run(
                &mut s,
                "data modify storage t:d v set compute default integer {type:\"minecraft:uniform\",min:2,max:2}"
            )
            .0
        );
        assert_eq!(v(&s), Some(Nbt::Int(2)));
        assert!(
            !run(
                &mut s,
                "data modify storage t:d w set compute entity @e[tag=absent,limit=1] integer minecraft:nope"
            )
            .0
        );
        let c = chats(&session);
        assert!(c.iter().any(|m| m.contains("argument.resource_or_id.no_such_element")), "{c:?}");

        // Armor stand: equipment read and copied like a mob's.
        assert!(run(&mut s, "summon minecraft:armor_stand ~ ~ ~ {Tags:[\"g3s\"],equipment:{head:{id:\"minecraft:diamond_helmet\",count:1}}}").0);
        let (ok, n) = run(&mut s, "execute if items entity @e[tag=g3s,limit=1] armor.head minecraft:diamond_helmet");
        assert!(ok && n == 1, "{n} {:?}", chats(&session));
        s.held = 3;
        assert!(
            run(&mut s, "item replace entity @s weapon.mainhand from entity @e[tag=g3s,limit=1] armor.head").0,
            "{:?}",
            chats(&session)
        );
        assert_eq!(s.slots[39].as_ref().map(|st| st.id.as_str()), Some("minecraft:diamond_helmet"));
        assert!(
            run(&mut s, "item replace entity @e[tag=g3s,limit=1] weapon.mainhand with minecraft:stone 2").0,
            "{:?}",
            chats(&session)
        );
        let (ok, n) = run(&mut s, "execute if items entity @e[tag=g3s,limit=1] weapon.mainhand minecraft:stone");
        assert!(ok && n == 2, "{n} {:?}", chats(&session));

        // Player: `weapon.mainhand` = the held item (hotbar slot 3 here), not slot 0.
        let (ok, n) = run(&mut s, "execute if items entity @s weapon.mainhand minecraft:diamond_helmet");
        assert!(ok && n == 1, "{n} {:?}", chats(&session));

        // `horse.saddle` does not exist in the game's table.
        assert!(run(&mut s, "summon minecraft:horse ~ ~ ~ {Tags:[\"g3h\"],NoAI:1b}").0);
        chats(&session);
        assert!(!run(&mut s, "item replace entity @e[tag=g3h,limit=1] horse.saddle with minecraft:saddle").0);
        assert!(
            run(&mut s, "item replace entity @e[tag=g3h,limit=1] saddle with minecraft:saddle").0,
            "{:?}",
            chats(&session)
        );
    });
}
