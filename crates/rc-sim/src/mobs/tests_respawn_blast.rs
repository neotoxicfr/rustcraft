//! Explosion of a respawn anchor outside the Nether through the common explosion: a nearby mob
//! is hurt and pushed back (the old own explosion only touched the player).

use std::time::Duration;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

use super::tests::{drain_until, kind_id, prep};
use crate::tests::{start_test, test_world};

#[test]
fn anchor_blast_hurts_mobs() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "anchorblast", 1, "normal", &[]);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    std::thread::sleep(Duration::from_millis(2500));
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    let cmd = |c: &str| {
        s.link.send_command(c).unwrap();
        std::thread::sleep(Duration::from_millis(200));
    };
    cmd(&format!("setblock {} {} {} minecraft:respawn_anchor[charges=1]", x + 3, y, z));
    cmd(&format!("summon minecraft:zombie {} {} {} {{NoAI:1b,PersistenceRequired:1b}}", x + 5, y, z));
    let zk = kind_id(&data, "minecraft:zombie");
    let mut zombie = None;
    drain_until(&s, 10, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == zk
        {
            zombie = Some(a.id);
            return true;
        }
        false
    });
    let zombie = zombie.expect("zombie did not appear");
    s.link.send_use_item_on([x + 3, y, z], 1, [0.5, 1.0, 0.5], false, 9).unwrap();
    // Horizontal knockback toward +x (the mob is east of the anchor): only the common explosion
    // pushes mobs; the fire placed around can also hurt them, hence this criterion.
    let (mut boom, mut pushed) = (false, false);
    drain_until(&s, 4, |e| {
        match e {
            Event::Explode(_) => boom = true,
            Event::Entity(EntityEvent::Motion { id, velocity }) if *id == zombie && boom && velocity[0] > 0.05 => {
                pushed = true
            }
            _ => {}
        }
        boom && pushed
    });
    assert!(boom, "the anchor did not explode");
    assert!(pushed, "the mob was not pushed back by the anchor's explosion");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
