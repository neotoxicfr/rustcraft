//! L9-TICK: chunk tickets other than the players' (a player's ender pearl, the dragon fight),
//! state that changes dimension with its world (evoker fangs, sign editors, the bobber left
//! behind). Fresh flat world (`tests_g3f13::open`: 26.3 version data mandatory, no test world).

use crate::blocks::probe::Probe;
use crate::dims::{NETHER, OVERWORLD, Transition, rel};
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::tests_g3f13::{finish, open};

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

fn block(p: &Probe, at: [i32; 3]) -> String {
    p.s.state_name(p.s.get(at)).to_owned()
}

/// A player's pearl entering chunk 3 (simulation distance 2: outside the player's zone) places
/// its ticket there at the end of its tick; the ticket counts from the next tick and lasts 40
/// ticks after its last renewal, even with the pearl gone: a zombie of chunk 3 ticks exactly 40
/// times, water in chunk 4 (one chunk away: "ticking blocks") spreads, water in chunk 5 (two
/// away: full chunk only) does not. Before: only the pearl itself ticked there, never its
/// neighbours, and nothing once it was gone.
#[test]
fn pearl_ticket_simulates_its_chunks_for_40_ticks() {
    let (mut p, dir) = open("l9-pearl");
    p.s.sim_dist = 2;
    cmd(&mut p, "setblock 72 -60 8 minecraft:water");
    cmd(&mut p, "setblock 88 -60 8 minecraft:water");
    cmd(&mut p, "summon minecraft:ender_pearl 47.5 -50 8.5 {NoGravity:1b,Motion:[1.0d,0.0d,0.0d]}");
    let uuid = p.s.uuid;
    p.s.proj.list.last_mut().expect("pearl").owner = Some(uuid);
    let mut z = p.s.new_mob(Kind::Zombie, [56.5, -50.0, 8.5], 0.0);
    z.persistent = true;
    let zid = z.id;
    p.s.add_mob(z);
    let ticks = |p: &Probe| p.s.mob_ref(zid).expect("zombie").tick_count;
    // Tick 1: the pearl (chunk 2, simulated) flies into chunk 3 and places its ticket there.
    game_ticks(&mut p, 1);
    let pearl = p.s.proj.list.last().expect("pearl");
    assert_eq!((pearl.pos[0].floor() as i32) >> 4, 3, "pearl in chunk 3");
    assert_eq!(p.s.ticket_level((3, 0)), i32::MAX, "ticket applied in the tick that placed it");
    assert_eq!(ticks(&p), 0, "chunk 3 outside the simulation");
    // The pearl disappears: its last ticket stays until it expires.
    p.s.proj.list.last_mut().expect("pearl").removed = true;
    game_ticks(&mut p, 1);
    assert_eq!(p.s.ticket_level((3, 0)), 31, "pearl ticket at its chunk");
    assert_eq!(p.s.ticket_level((4, 0)), 32, "one chunk around: ticking blocks");
    assert_eq!(ticks(&p), 1, "the zombie of the pearl's chunk ticks");
    game_ticks(&mut p, 39);
    assert_eq!(ticks(&p), 40, "40 ticks of the ticket");
    game_ticks(&mut p, 5);
    assert_eq!(ticks(&p), 40, "ticket expired: the zombie waits");
    assert_eq!(p.s.ticket_level((3, 0)), i32::MAX, "ticket removed");
    assert!(p.s.proj.tickets.is_empty());
    assert_eq!(block(&p, [73, -60, 8]), "minecraft:water", "water of chunk 4 spread");
    assert_eq!(block(&p, [89, -60, 8]), "minecraft:air", "chunk 5 is not ticking");
    finish(p, dir);
}

/// A pearl renews its ticket every 40 ticks where it stays, and on entering a chunk; the old
/// chunk keeps its own until it expires. Only a player's pearl carries one.
#[test]
fn pearl_ticket_renewal() {
    let (mut p, dir) = open("l9-renew");
    cmd(&mut p, "summon minecraft:ender_pearl 8.5 -50 8.5 {NoGravity:1b}");
    let uuid = p.s.uuid;
    p.s.proj.list.last_mut().expect("pearl").owner = Some(uuid);
    cmd(&mut p, "summon minecraft:ender_pearl 24.5 -50 8.5 {NoGravity:1b}");
    let left = |p: &Probe, c: (i32, i32)| p.s.proj.tickets.iter().find(|t| t.chunk == c).map(|t| t.left);
    game_ticks(&mut p, 1);
    assert_eq!(left(&p, (0, 0)), Some(40), "placed at its first tick");
    assert_eq!(left(&p, (1, 0)), None, "a pearl without a player carries no ticket");
    game_ticks(&mut p, 38);
    assert_eq!(left(&p, (0, 0)), Some(2), "counted down at each purge");
    game_ticks(&mut p, 1);
    assert_eq!(left(&p, (0, 0)), Some(40), "renewed after 39 more ticks");
    // Into chunk 1: a new ticket there, the old one runs out.
    let q = p.s.proj.list.iter_mut().find(|q| q.owner == Some(uuid)).expect("pearl");
    q.vel = [9.0, 0.0, 0.0];
    game_ticks(&mut p, 1);
    assert_eq!((left(&p, (0, 0)), left(&p, (1, 0))), (Some(39), Some(40)));
    finish(p, dir);
}

/// The dragon fight's ticket (radius 9 at chunk (0, 0)) also simulates the blocks of the arena
/// beyond the simulation distance: random ticks within 7 chunks, scheduled ticks within 8 (water
/// of chunk 8 spreads). Before: only the entities. `fight`: a fight with players in its bar.
pub(crate) fn fight_ticket_ticks_the_blocks_of_the_arena(fight: impl FnOnce() -> crate::mobs::end::fight::Fight) {
    let (mut p, dir) = open("l9-fight");
    p.s.sim_dist = 2;
    let f = fight();
    p.s.mobs.end.fight = Some(Box::new(f));
    let centers = p.s.sim_centers();
    assert!(p.s.chunk_entity_ticking((7, 0), &centers), "random ticks within 7");
    assert!(!p.s.chunk_entity_ticking((8, 0), &centers));
    assert!(p.s.chunk_simulated((8, 0), &centers), "scheduled ticks within 8");
    assert!(!p.s.chunk_simulated((9, 0), &centers));
    assert!(p.s.chunk_simulated_now((8, -2)));
    // Chunk 8 (neighbours loaded up to chunk 9): its water spreads; without the fight, it waits.
    cmd(&mut p, "setblock 136 -60 8 minecraft:water");
    for _ in 0..12 {
        p.s.tick_level_blocks(false);
        p.s.time += 1;
    }
    assert_eq!(block(&p, [137, -60, 8]), "minecraft:water");
    if let Some(f) = p.s.mobs.end.fight.as_mut() {
        f.bar_player = false;
    }
    cmd(&mut p, "setblock 136 -60 9 minecraft:air");
    for _ in 0..12 {
        p.s.tick_level_blocks(false);
        p.s.time += 1;
    }
    assert_eq!(block(&p, [136, -60, 9]), "minecraft:air", "no players in the fight: no ticket");
    finish(p, dir);
}

/// Evoker fangs and sign editors stay in their dimension; the bobber left behind is gone
/// when the player comes back. Before: the fangs and the editors followed the player (fangs
/// biting at the same coordinates in the Nether, a Nether sign at the same place accepting
/// the text) and the old bobber reappeared on return.
#[test]
fn dimension_state_stays_with_its_world() {
    let (mut p, dir) = open("l9-swap");
    p.s.mobs.ill.fangs.push(crate::mobs::illagers::spells::Fangs {
        id: 9001,
        pos: [2.5, -60.0, 2.5],
        yaw: 0.0,
        warmup: 5,
        life: 22,
        sent_spike: false,
        owner: None,
        removed: false,
        born: 0,
    });
    let uuid = p.s.uuid;
    p.s.life.sign_editors.insert([3, -60, 3], uuid);
    p.s.slots[36] = Some(Stack { id: "minecraft:fishing_rod".into(), count: 1, components: None });
    p.s.held = 0;
    assert!(p.s.fishing_rod_use(0) && p.s.proj.hook.is_some(), "rod cast");
    let to = |dim: &str| Transition {
        dim: dim.into(),
        pos: [0.5, 70.0, 0.5],
        yaw: 0.0,
        pitch: 0.0,
        relative: rel::ROTATION,
        sound: false,
        ticket: false,
        missing_respawn: false,
    };
    p.s.change_dimension(to(NETHER));
    assert!(p.s.mobs.ill.fangs.is_empty(), "fangs followed the player");
    assert!(p.s.life.sign_editors.is_empty(), "sign editor followed the player");
    let left = p.s.dims.parked.iter().find(|w| w.dim.name == OVERWORLD).expect("parked overworld");
    assert!(left.proj.hook.is_none(), "bobber kept in the world left");
    p.s.change_dimension(to(OVERWORLD));
    assert_eq!(p.s.mobs.ill.fangs.iter().map(|f| f.id).collect::<Vec<_>>(), vec![9001], "fangs kept");
    assert_eq!(p.s.life.sign_editors.get(&[3, -60, 3]), Some(&uuid), "editor kept with its sign");
    assert!(p.s.proj.hook.is_none());
    finish(p, dir);
}

/// Copy of the flat chunk (9, 0) placed at `at` with the block entities `bes`.
fn crafted_chunk(p: &Probe, at: (i32, i32), bes: Vec<rc_proto::nbt::Nbt>) -> Box<crate::chunk::Chunk> {
    let mut c = p.s.chunks.get(&(9, 0)).cloned().flatten().expect("chunk (9, 0)");
    (c.x, c.z) = at;
    c.block_entities = bes;
    c
}

fn hopper_nbt(at: [i32; 3], packed: bool) -> rc_proto::nbt::Nbt {
    use rc_proto::nbt::Nbt;
    Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:hopper".into())),
        ("x".into(), Nbt::Int(at[0])),
        ("y".into(), Nbt::Int(at[1])),
        ("z".into(), Nbt::Int(at[2])),
        ("keepPacked".into(), Nbt::Byte(i8::from(packed))),
    ])
}

/// A chunk missing from the disk and generated afterwards resumes its ticks and registers its
/// block entities' tickers when it arrives. Before: it was marked as done when put into
/// generation, so a generated chunk's hopper (a dungeon's spawner, a village's bell) never ticked
/// during the session.
#[test]
fn generated_chunk_registers_its_tickers() {
    let (mut p, dir) = open("l9-gen");
    let at = (12, 4);
    let pos = [196, -60, 68];
    // Read miss: put into generation.
    p.s.chunks.insert(at, None);
    p.s.import_chunk_ticks(at);
    // Generation done.
    let c = crafted_chunk(&p, at, vec![hopper_nbt(pos, false)]);
    p.s.chunks.insert(at, Some(c));
    p.s.gen_arrived(vec![(at, Vec::new())], Vec::new());
    assert!(p.s.bs.hoppers.contains_key(&pos), "ticker of the generated chunk's hopper");
    finish(p, dir);
}

/// The promotion order of packed block entities is that of the game's chained hash table
/// (values from Java's standard hash map run on these positions: growth past 3/4, growth on a ninth
/// entry in a bucket below 64 buckets, negative heights).
#[test]
fn packed_order_is_the_game_hash_table_walk() {
    let at: [[i32; 3]; 20] = [
        [0, 0, 0],
        [1, 0, 1],
        [2, 0, 2],
        [3, 0, 3],
        [4, 0, 4],
        [5, 0, 5],
        [6, 0, 6],
        [7, 0, 7],
        [8, 0, 8],
        [9, 0, 9],
        [3, -60, 7],
        [15, -1, 0],
        [12, 70, 4],
        [2, 200, 9],
        [7, -64, 15],
        [11, 5, 3],
        [4, -30, 2],
        [0, 319, 15],
        [9, 64, 9],
        [5, -12, 13],
    ];
    let want = [0, 1, 13, 2, 16, 3, 10, 4, 15, 5, 12, 6, 7, 11, 8, 17, 9, 18, 14, 19];
    assert_eq!(crate::blocks::entities::java_hash_order(&at), want);
    let b: [[i32; 3]; 13] = [
        [1, 2, 3],
        [4, 5, 6],
        [7, 8, 9],
        [10, 11, 12],
        [13, 14, 15],
        [0, -5, 0],
        [15, -64, 15],
        [8, 100, 1],
        [2, 33, 14],
        [6, -7, 6],
        [14, 1, 1],
        [3, 3, 3],
        [9, 250, 0],
    ];
    assert_eq!(crate::blocks::entities::java_hash_order(&b), [0, 11, 1, 7, 2, 3, 4, 10, 8, 12, 9, 5, 6]);
    // Ten positions in one bucket up to 32 buckets: the ninth grows the table to 32, the tenth
    // to 64, which splits them.
    let c: [[i32; 3]; 13] = [
        [0, 0, 0],
        [1, 1, 0],
        [2, 2, 0],
        [3, 3, 0],
        [4, 4, 0],
        [5, 5, 0],
        [6, 6, 0],
        [7, 7, 0],
        [8, 8, 0],
        [9, 9, 0],
        [3, -60, 7],
        [15, 64, 2],
        [11, 5, 3],
    ];
    assert_eq!(crate::blocks::entities::java_hash_order(&c), [0, 2, 4, 6, 8, 10, 11, 1, 3, 5, 7, 9, 12]);
    assert_eq!(crate::blocks::entities::java_hash_order(&c[..9]), [0, 1, 2, 3, 4, 5, 6, 7, 8]);
}

/// A packed block entity (saved before its chunk ever ticked) is promoted once the chunk's
/// neighbours are loaded, after every ticker registered so far, and is then saved unpacked; the
/// unpacked one of the same chunk registers at once. Before: both at once, in table order.
#[test]
fn packed_block_entities_promoted_when_the_chunk_can_tick() {
    let (mut p, dir) = open("l9-packed");
    let at = (12, 0);
    let (packed, plain) = ([200, -60, 3], [201, -60, 3]);
    let c = crafted_chunk(&p, at, vec![hopper_nbt(packed, true), hopper_nbt(plain, false)]);
    p.s.chunks.insert(at, Some(c));
    p.s.import_chunk_ticks(at);
    assert!(p.s.bs.hoppers.contains_key(&plain), "unpacked: at once");
    assert!(!p.s.bs.hoppers.contains_key(&packed), "packed: waits for the neighbours");
    // Hopper in a neighbour, registered before the promotion.
    let nb = (11, 1);
    let other = [180, -60, 20];
    p.s.chunks.insert(nb, Some(crafted_chunk(&p, nb, vec![hopper_nbt(other, false)])));
    p.s.import_chunk_ticks(nb);
    assert!(!p.s.bs.hoppers.contains_key(&packed), "neighbours still missing");
    let around: Vec<(i32, i32)> = (11..=13).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    let id = |q: [i32; 3]| p.s.bs.hoppers.get(&q).map(|h| h.0).expect("hopper ticker");
    assert!(id(packed) > id(plain) && id(packed) > id(other), "promoted after the others");
    let flag = p.s.chunks.get(&at).cloned().flatten().expect("chunk").block_entities[0].get("keepPacked").cloned();
    assert_eq!(flag, Some(rc_proto::nbt::Nbt::Byte(0)), "saved unpacked once promoted");
    finish(p, dir);
}

/// L9-TICK, scenario of the entity order (docs/gaps-audit2.md, "Entity tick order"): the game
/// ticks every entity in one list, in the order they were added. An item (a nether star: the
/// blast does not destroy it) lying in the air before a TNT is primed therefore ticks before the
/// TNT explodes in the same tick: it does not move that tick (its velocity was zero) and only
/// flies off on the next one. Our walk ticks family by family (falling blocks and TNT first,
/// items after): the TNT explodes first and the item moves with the blast in the same tick, one
/// tick early. Fails until the entity walk follows the single insertion order.
#[test]
#[ignore = "L9-TICK: the entity walk is still family by family (single list not done)"]
fn item_before_tnt_moves_on_the_next_tick() {
    let (mut p, dir) = open("l9-order");
    cmd(&mut p, "summon minecraft:item 4.5 -50 4.5 {NoGravity:1b,Item:{id:\"minecraft:nether_star\",count:1}}");
    cmd(&mut p, "summon minecraft:tnt 2.5 -50.5 4.5 {fuse:1s}");
    game_ticks(&mut p, 1);
    let item = p.s.life.drops.iter().find(|d| !d.is_removed()).expect("nether star");
    assert!(item.vel[0] > 0.0, "blast received");
    assert_eq!(item.pos, [4.5, -50.0, 4.5], "the item ticked before the explosion");
    finish(p, dir);
}

/// A parked world stays active while it holds a pearl ticket, whoever threw the pearl: its
/// ticket is counted down there and expires.
#[test]
fn parked_world_kept_active_by_a_pearl_ticket() {
    let (mut p, dir) = open("l9-parked");
    p.s.enter_world(NETHER);
    p.s.proj.pearl_ticket((0, 0), false);
    p.s.enter_world(OVERWORLD);
    let left = |p: &Probe| {
        let w = p.s.dims.parked.iter().find(|w| w.dim.name == NETHER).expect("parked Nether");
        w.proj.tickets.first().map(|t| (t.left, t.live))
    };
    assert_eq!(left(&p), Some((40, false)));
    game_ticks(&mut p, 1);
    assert_eq!(left(&p), Some((39, true)), "parked world ticked");
    game_ticks(&mut p, 40);
    assert_eq!(left(&p), None, "expired in the parked world");
    // A world parked during a tick (player through a portal) is not purged a second time in
    // that tick by the parked worlds' pass.
    p.s.proj.pearl_ticket((0, 0), true);
    p.s.time += 1;
    p.s.tick_level_blocks(false);
    p.s.enter_world(NETHER);
    p.s.dims_tick();
    let w = p.s.dims.parked.iter().find(|w| w.dim.name == OVERWORLD).expect("parked overworld");
    assert_eq!(w.proj.tickets.first().map(|t| t.left), Some(39), "purged once in the tick");
    finish(p, dir);
}
