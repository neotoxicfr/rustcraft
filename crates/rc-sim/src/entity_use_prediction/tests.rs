//! The client's reaction to a right click on an entity against the game's: the outcome (swing,
//! no swing, pass to the item) and the local effects of each family, and the facts read from
//! the synced values that the integrated server itself writes (one layout for both).

use rc_proto_v777::entity::Meta;

use super::*;
use crate::mobs::jrand::JRand;
use crate::mobs::mob::Mob;

const ME: u128 = 0x00c0_ffee;
const SWING: Outcome = Outcome::Success { swing: true };
const NO_SWING: Outcome = Outcome::Success { swing: false };

/// A clicked entity: its type, synced values and surroundings as a client holds them.
struct Fake {
    kind: String,
    data: Vec<(u8, Meta)>,
    worn: [Option<&'static str>; 8],
    shearable: [bool; 8],
    max_health: f32,
    passengers: usize,
    on_ground: bool,
    leash: LeashHolder,
    holds_leashes: bool,
    distance_sqr: f64,
    cooldown: i32,
}

impl Fake {
    /// A non-mob entity (`minecraft:` added).
    fn thing(kind: &str) -> Fake {
        Fake {
            kind: format!("minecraft:{kind}"),
            data: Vec::new(),
            worn: [None; 8],
            shearable: [false; 8],
            max_health: 20.0,
            passengers: 0,
            on_ground: true,
            leash: LeashHolder::Nobody,
            holds_leashes: false,
            distance_sqr: 4.0,
            cooldown: 0,
        }
    }

    /// A mob of `kind` with the synced values the server writes for it after `setup`.
    fn mob(kind: Kind, setup: impl FnOnce(&mut Mob)) -> Fake {
        let mut m = Mob::new(kind, 7, 99, [0.0; 3], 0.0, JRand::new(1));
        m.define_data();
        m.health = m.max_health();
        setup(&mut m);
        m.sync_data();
        let data = (0..40).filter_map(|i| m.get(i).map(|v| (i, v.clone()))).collect();
        Fake { data, max_health: m.max_health(), ..Fake::thing(kind.info().name.trim_start_matches("minecraft:")) }
    }

    fn wearing(mut self, slot: usize, item: &'static str, shearable: bool) -> Fake {
        self.worn[slot] = Some(item);
        self.shearable[slot] = shearable;
        self
    }

    fn with(mut self, index: u8, value: Meta) -> Fake {
        self.data.retain(|(i, _)| *i != index);
        self.data.push((index, value));
        self
    }
}

impl SeenEntity for Fake {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn data(&self, index: u8) -> Option<&Meta> {
        self.data.iter().find(|(i, _)| *i == index).map(|(_, v)| v)
    }

    fn worn(&self, slot: usize) -> Option<&str> {
        self.worn[slot]
    }

    fn worn_comes_off_with_shears(&self, slot: usize) -> bool {
        self.shearable[slot]
    }

    fn max_health(&self) -> f32 {
        self.max_health
    }

    fn passengers(&self) -> usize {
        self.passengers
    }

    fn on_ground(&self) -> bool {
        self.on_ground
    }

    fn leash_holder(&self) -> LeashHolder {
        self.leash
    }

    fn holds_leashes(&self) -> bool {
        self.holds_leashes
    }

    fn distance_sqr(&self) -> f64 {
        self.distance_sqr
    }

    fn age_lock_cooldown(&self) -> i32 {
        self.cooldown
    }
}

fn held(name: &'static str) -> HeldItem<'static> {
    HeldItem { name, custom_name: false, dye: None, equippable: None }
}

fn bare_hand() -> HeldItem<'static> {
    held("")
}

fn player() -> ActingPlayer<'static> {
    ActingPlayer {
        uuid: ME,
        hand: 0,
        sneaking: false,
        infinite_materials: false,
        other_hand_item: "",
        thundering: false,
    }
}

fn sneaking() -> ActingPlayer<'static> {
    ActingPlayer { sneaking: true, ..player() }
}

/// The item tags these tests use, with the game's members that matter here.
fn tags(item: &str, tag: &str) -> bool {
    let item = item.trim_start_matches("minecraft:");
    match tag.trim_start_matches("minecraft:") {
        "pig_food" => matches!(item, "carrot" | "potato" | "beetroot"),
        "cow_food" | "sheep_food" | "goat_food" => item == "wheat",
        "horse_food" => matches!(item, "wheat" | "sugar" | "hay_block" | "apple" | "carrot" | "golden_carrot"),
        "llama_food" => matches!(item, "wheat" | "hay_block"),
        "wolf_food" => matches!(item, "beef" | "bone"),
        "creeper_igniters" => matches!(item, "flint_and_steel" | "fire_charge"),
        "fishes" => matches!(item, "cod" | "salmon"),
        "axolotl_food" => item == "tropical_fish_bucket",
        "duplicates_allays" => item == "amethyst_shard",
        "parrot_food" => item == "wheat_seeds",
        "wolf_collar_dyes" => item.ends_with("_dye"),
        _ => false,
    }
}

fn predict_with(seen: &Fake, item: &HeldItem, p: &ActingPlayer) -> Prediction {
    super::predict(seen, item, p, &tags)
}

fn outcome(seen: &Fake, item: &HeldItem) -> Outcome {
    predict_with(seen, item, &player()).outcome
}

fn baby(m: &mut Mob) {
    m.age = -24000;
    m.baby = true;
}

fn adult(_: &mut Mob) {}

/// A carrot to a grown pig: the click stops there without a swing (the old client went on to
/// eat the carrot); a young pig eats it with a swing.
#[test]
fn food_to_a_grown_animal_stops_the_click_without_a_swing() {
    let pig = Fake::mob(Kind::Pig, adult);
    assert_eq!(
        predict_with(&pig, &held("minecraft:carrot"), &player()),
        Prediction { outcome: NO_SWING, effects: vec![] }
    );
    let piglet = Fake::mob(Kind::Pig, baby);
    let fed = predict_with(&piglet, &held("minecraft:carrot"), &player());
    assert_eq!(fed, Prediction { outcome: SWING, effects: vec![LocalEffect::ConsumeHeld] });
    let locked = Fake::mob(Kind::Pig, |m| {
        baby(m);
        m.mount.age_locked = true;
    });
    assert_eq!(outcome(&locked, &held("minecraft:carrot")), NO_SWING, "age-locked young: no growth");
    assert_eq!(outcome(&pig, &held("minecraft:stick")), Outcome::Pass);
}

/// A bucket on a grown cow: milking heard by the actor and the milk bucket in hand at once; a
/// calf ignores the bucket (it falls to the item).
#[test]
fn bucket_on_a_cow_milks_it_locally() {
    let cow = Fake::mob(Kind::Cow, adult);
    let milked = predict_with(&cow, &held("minecraft:bucket"), &player());
    let effects = vec![
        LocalEffect::PlayerSound { name: "minecraft:entity.cow.milk", volume: 1.0, pitch: 1.0 },
        LocalEffect::FillHeld { result: "minecraft:milk_bucket", creative_keeps: true },
    ];
    assert_eq!(milked, Prediction { outcome: SWING, effects });
    assert_eq!(outcome(&Fake::mob(Kind::Cow, baby), &held("minecraft:bucket")), Outcome::Pass);
    let mooshroom = Fake::mob(Kind::Mooshroom, adult);
    let milked = predict_with(&mooshroom, &held("minecraft:bucket"), &player());
    assert_eq!(milked.outcome, SWING, "a mooshroom is milked like a cow");
    let stew = predict_with(&mooshroom, &held("minecraft:bowl"), &player());
    assert_eq!(stew.effects, vec![LocalEffect::FillHeld { result: "minecraft:mushroom_stew", creative_keeps: false }]);
    let screaming = Fake::mob(Kind::Goat, adult).with(18, Meta::Bool(true));
    let sound = predict_with(&screaming, &held("minecraft:bucket"), &player()).effects[0];
    assert!(matches!(sound, LocalEffect::PlayerSound { name: "minecraft:entity.goat.screaming.milk", .. }));
}

/// A name tag that names, a spawn egg, a lead: the game's common reactions.
#[test]
fn name_tag_spawn_egg_and_lead() {
    let zombie = Fake::mob(Kind::Zombie, adult);
    let named = HeldItem { custom_name: true, ..held("minecraft:name_tag") };
    assert_eq!(outcome(&zombie, &named), SWING);
    assert_eq!(outcome(&zombie, &held("minecraft:name_tag")), Outcome::Pass, "a tag without a name does nothing");
    assert_eq!(outcome(&zombie, &held("minecraft:pig_spawn_egg")), NO_SWING);
    assert_eq!(outcome(&zombie, &held("minecraft:lead")), NO_SWING, "the server ties the lead");
    let led = Fake { leash: LeashHolder::ActingPlayer, ..Fake::mob(Kind::Cow, adult) };
    assert_eq!(outcome(&led, &bare_hand()), SWING, "the player unties their own lead");
    let other = Fake { leash: LeashHolder::OtherPlayer, ..Fake::mob(Kind::Cow, adult) };
    assert_eq!(outcome(&other, &held("minecraft:lead")), Outcome::Pass, "held by someone else");
    assert_eq!(outcome(&led, &held("minecraft:shears")), SWING, "shears cut the lead");
    let stand = Fake::thing("armor_stand");
    assert_eq!(outcome(&stand, &bare_hand()), NO_SWING, "an armor stand's equipment is the server's");
    assert_eq!(outcome(&stand, &named), SWING, "a name tag names an armor stand");
}

/// Dye on a sheep of another colour: the dye sound for the actor; the same colour or a
/// sheared sheep let the click pass.
#[test]
fn dye_on_a_sheep() {
    let white = Fake::mob(Kind::Sheep, adult);
    let red = HeldItem { dye: Some(14), ..held("minecraft:red_dye") };
    let dyed = predict_with(&white, &red, &player());
    let sound = LocalEffect::EntitySound {
        name: "minecraft:item.dye.use",
        category: SoundCategory::Players,
        volume: 1.0,
        pitch_low: 1.0,
        pitch_spread: 0.0,
    };
    assert_eq!(dyed, Prediction { outcome: SWING, effects: vec![sound] });
    let red_sheep = Fake::mob(Kind::Sheep, |m| m.wool = 14);
    assert_eq!(outcome(&red_sheep, &red), Outcome::Pass);
    let sheared = Fake::mob(Kind::Sheep, |m| m.sheared = true);
    assert_eq!(outcome(&sheared, &red), Outcome::Pass);
    assert_eq!(outcome(&white, &held("minecraft:shears")), NO_SWING, "shearing is the server's");
}

/// A water bucket takes a fish away at once; the axolotl's food leaves a water bucket.
#[test]
fn fish_into_a_bucket() {
    let cod = Fake::mob(Kind::Cod, adult);
    let picked = predict_with(&cod, &held("minecraft:water_bucket"), &player());
    let effects = vec![
        LocalEffect::FillHeld { result: "minecraft:cod_bucket", creative_keeps: false },
        LocalEffect::RemoveEntity,
    ];
    assert_eq!(picked, Prediction { outcome: SWING, effects });
    assert_eq!(outcome(&cod, &held("minecraft:bucket")), Outcome::Pass);
    let young_axolotl = Fake::mob(Kind::Axolotl, baby);
    let fed = predict_with(&young_axolotl, &held("minecraft:tropical_fish_bucket"), &player());
    assert_eq!(fed.effects, vec![LocalEffect::FillHeld { result: "minecraft:water_bucket", creative_keeps: true }]);
}

/// Golden dandelion on a young mob: locked once, then its 40 ticks on this client.
#[test]
fn golden_dandelion_waits_its_cooldown() {
    let calf = Fake::mob(Kind::Cow, baby);
    let locked = predict_with(&calf, &held("minecraft:golden_dandelion"), &player());
    let effects = vec![LocalEffect::ConsumeHeld, LocalEffect::StartAgeLockCooldown];
    assert_eq!(locked, Prediction { outcome: SWING, effects });
    let waiting = Fake { cooldown: 12, ..Fake::mob(Kind::Cow, baby) };
    assert_eq!(outcome(&waiting, &held("minecraft:golden_dandelion")), Outcome::Pass);
    let villager = Fake::mob(Kind::Villager, baby);
    assert_eq!(outcome(&villager, &held("minecraft:golden_dandelion")), SWING, "a young villager shakes its head");
    let zombie_foal = Fake::mob(Kind::ZombieHorse, baby);
    assert_eq!(outcome(&zombie_foal, &held("minecraft:golden_dandelion")), SWING, "an untamed zombie foal rears");
}

/// Traders: a villager takes the click sneaking or not, with a swing; a wandering trader
/// without one (the server swings).
#[test]
fn traders_take_the_click() {
    let villager = Fake::mob(Kind::Villager, adult);
    assert_eq!(predict_with(&villager, &bare_hand(), &sneaking()).outcome, SWING, "sneaking does not stop trading");
    let trader = Fake::mob(Kind::WanderingTrader, adult);
    assert_eq!(outcome(&trader, &bare_hand()), NO_SWING);
    let asleep = Fake::mob(Kind::Villager, adult).with(6, Meta::Pose(2));
    assert_eq!(outcome(&asleep, &bare_hand()), Outcome::Pass);
    assert_eq!(outcome(&Fake::mob(Kind::ZombieVillager, adult), &held("minecraft:golden_apple")), NO_SWING);
}

/// Item frames: an empty frame and an empty hand do nothing; the rest is the server's.
#[test]
fn item_frames_and_other_entities() {
    let frame = Fake::thing("item_frame");
    assert_eq!(outcome(&frame, &bare_hand()), Outcome::Pass);
    assert_eq!(outcome(&frame, &held("minecraft:apple")), SWING);
    let full = Fake::thing("glow_item_frame").with(8, Meta::Item(None));
    assert_eq!(outcome(&full, &bare_hand()), Outcome::Pass, "an empty item value is an empty frame");
    assert_eq!(outcome(&Fake::thing("interaction").with(10, Meta::Bool(true)), &bare_hand()), SWING);
    assert_eq!(outcome(&Fake::thing("interaction"), &bare_hand()), NO_SWING);
    assert_eq!(outcome(&Fake::thing("cushion"), &bare_hand()), NO_SWING);
    assert_eq!(predict_with(&Fake::thing("cushion"), &bare_hand(), &sneaking()).outcome, Outcome::Pass);
    assert_eq!(outcome(&Fake::thing("oak_boat"), &bare_hand()), SWING);
    assert_eq!(predict_with(&Fake::thing("oak_boat"), &bare_hand(), &sneaking()).outcome, Outcome::Pass);
    assert_eq!(predict_with(&Fake::thing("oak_chest_boat"), &bare_hand(), &sneaking()).outcome, SWING);
    assert_eq!(outcome(&Fake::thing("oak_boat"), &held("minecraft:lead")), NO_SWING, "a boat takes a lead");
    assert_eq!(outcome(&Fake::thing("leash_knot"), &bare_hand()), SWING);
    assert_eq!(outcome(&Fake::thing("painting"), &bare_hand()), Outcome::Pass);
}

fn saddle() -> HeldItem<'static> {
    let equippable = HeldEquippable { slot: slot::SADDLE, on_interact: true, allows_clicked: true };
    HeldItem { equippable: Some(equippable), ..held("minecraft:saddle") }
}

/// Saddles: put on a grown pig, a saddled pig is mounted, shears take the saddle off.
#[test]
fn saddles_on_pigs() {
    let pig = Fake::mob(Kind::Pig, adult);
    assert_eq!(outcome(&pig, &saddle()), SWING);
    assert_eq!(outcome(&Fake::mob(Kind::Pig, baby), &saddle()), Outcome::Pass, "a piglet has no saddle slot");
    let saddled = Fake::mob(Kind::Pig, adult).wearing(slot::SADDLE, "minecraft:saddle", true);
    assert_eq!(outcome(&saddled, &bare_hand()), SWING, "mounted");
    assert_eq!(outcome(&saddled, &saddle()), SWING, "mounted, the saddle stays in hand");
    assert_eq!(predict_with(&saddled, &bare_hand(), &sneaking()).outcome, Outcome::Pass);
    assert_eq!(outcome(&saddled, &held("minecraft:shears")), SWING, "saddle sheared off");
    let ridden = Fake { passengers: 1, ..Fake::mob(Kind::Pig, adult).wearing(slot::SADDLE, "minecraft:saddle", true) };
    assert_eq!(outcome(&ridden, &held("minecraft:shears")), Outcome::Pass, "not while carrying someone");
}

/// Creeper and flint and steel: the ignition sound for the actor, pitch 0.8 to 1.2.
#[test]
fn creeper_ignited() {
    let creeper = Fake::mob(Kind::Creeper, adult);
    let lit = predict_with(&creeper, &held("minecraft:flint_and_steel"), &player());
    let sound = LocalEffect::EntitySound {
        name: "minecraft:item.flintandsteel.use",
        category: SoundCategory::Hostile,
        volume: 1.0,
        pitch_low: 0.8,
        pitch_spread: 0.4,
    };
    assert_eq!(lit, Prediction { outcome: SWING, effects: vec![sound] });
}

/// Horses: an untamed one rears at any item that is not its food, eats without a swing (the
/// stack used only when it heals), is mounted by hand; its owner sneaking opens its inventory.
#[test]
fn horses() {
    let wild = Fake::mob(Kind::Horse, adult);
    assert_eq!(outcome(&wild, &held("minecraft:stick")), SWING, "rears");
    assert_eq!(
        predict_with(&wild, &held("minecraft:wheat"), &player()),
        Prediction { outcome: NO_SWING, effects: vec![] }
    );
    let hurt = Fake::mob(Kind::Horse, |m| m.health = 5.0);
    let healed = predict_with(&hurt, &held("minecraft:apple"), &player());
    assert_eq!(healed, Prediction { outcome: NO_SWING, effects: vec![LocalEffect::ConsumeHeld] });
    assert_eq!(outcome(&wild, &bare_hand()), SWING, "mounted");
    let tamed = Fake::mob(Kind::Horse, |m| m.mount.flags |= crate::mobs::mounts::FLAG_TAME);
    assert_eq!(predict_with(&tamed, &bare_hand(), &sneaking()).outcome, SWING);
    let foal = Fake::mob(Kind::Horse, baby);
    let fed = predict_with(&foal, &held("minecraft:sugar"), &player());
    assert_eq!(fed.effects, vec![LocalEffect::HappyVillagerParticle], "a growing foal sparks");
    let donkey = Fake::mob(Kind::Donkey, |m| m.mount.flags |= crate::mobs::mounts::FLAG_TAME);
    assert_eq!(predict_with(&donkey, &held("minecraft:chest"), &player()).effects, vec![LocalEffect::ConsumeHeld]);
    let chested = Fake::mob(Kind::Donkey, |m| {
        m.mount.flags |= crate::mobs::mounts::FLAG_TAME;
        m.mount.chest = true;
    });
    assert_eq!(predict_with(&chested, &held("minecraft:chest"), &player()).effects, vec![], "a second chest: mounted");
    assert_eq!(outcome(&Fake::mob(Kind::SkeletonHorse, adult), &bare_hand()), Outcome::Pass, "untamed skeleton horse");
    let camel = Fake::mob(Kind::Camel, adult);
    assert_eq!(outcome(&camel, &bare_hand()), NO_SWING, "a camel's ride does not swing");
}

/// Pets: a tamed wolf's owner makes it sit (swing), heals it; a stranger's wolf ignores the
/// hand; a parrot is offered seeds.
#[test]
fn pets() {
    let mine = |m: &mut Mob| {
        m.tame = true;
        m.owner = Some(ME);
    };
    let wolf = Fake::mob(Kind::Wolf, mine);
    assert_eq!(outcome(&wolf, &bare_hand()), SWING, "sits or stands");
    let hurt = Fake::mob(Kind::Wolf, |m| {
        mine(m);
        m.health = 2.0;
    });
    assert_eq!(predict_with(&hurt, &held("minecraft:beef"), &player()).effects, vec![LocalEffect::ConsumeHeld]);
    let theirs = Fake::mob(Kind::Wolf, |m| {
        m.tame = true;
        m.owner = Some(ME + 1);
    });
    assert_eq!(outcome(&theirs, &bare_hand()), Outcome::Pass);
    let blue = HeldItem { dye: Some(11), ..held("minecraft:blue_dye") };
    assert_eq!(predict_with(&wolf, &blue, &player()).effects, vec![LocalEffect::ConsumeHeld], "collar dyed");
    let cat = Fake::mob(Kind::Cat, mine);
    assert_eq!(outcome(&cat, &bare_hand()), SWING);
    let parrot = Fake::mob(Kind::Parrot, adult);
    let fed = predict_with(&parrot, &held("minecraft:wheat_seeds"), &player());
    assert_eq!(fed, Prediction { outcome: SWING, effects: vec![LocalEffect::ConsumeHeld] }, "seeds offered");
    let flying = Fake { on_ground: false, ..Fake::mob(Kind::Parrot, mine) };
    assert_eq!(outcome(&flying, &bare_hand()), Outcome::Pass, "a flying parrot does not sit");
    assert_eq!(outcome(&Fake::mob(Kind::Parrot, mine), &bare_hand()), SWING, "a perched one sits");
}

/// Others with their own rule: iron golem repaired only when hurt; allay given an item; piglin
/// offered gold; armadillo rolled up.
#[test]
fn golems_allays_piglins_armadillos() {
    let golem = Fake::mob(Kind::IronGolem, adult);
    assert_eq!(outcome(&golem, &held("minecraft:iron_ingot")), Outcome::Pass, "unhurt");
    let hurt = Fake::mob(Kind::IronGolem, |m| m.health = 30.0);
    assert_eq!(predict_with(&hurt, &held("minecraft:iron_ingot"), &player()).effects, vec![LocalEffect::ConsumeHeld]);
    let allay = Fake::mob(Kind::Allay, adult);
    let given = predict_with(&allay, &held("minecraft:apple"), &player());
    assert_eq!(given.outcome, SWING);
    assert_eq!(given.effects[0], LocalEffect::ConsumeHeld);
    let holding = Fake::mob(Kind::Allay, adult).wearing(slot::MAIN_HAND, "minecraft:apple", false);
    assert_eq!(outcome(&holding, &bare_hand()), SWING, "takes the item back");
    let piglin = Fake::mob(Kind::Piglin, adult);
    assert_eq!(outcome(&piglin, &held("minecraft:gold_ingot")), SWING);
    assert_eq!(outcome(&Fake::mob(Kind::Piglin, |m| m.baby = true), &held("minecraft:gold_ingot")), Outcome::Pass);
    let rolled = Fake::mob(Kind::Armadillo, adult).with(18, Meta::Variant(1, 36));
    assert_eq!(outcome(&rolled, &held("minecraft:wheat")), Outcome::Pass, "rolled up refuses");
    assert_eq!(outcome(&Fake::mob(Kind::Armadillo, adult), &held("minecraft:brush")), SWING);
}

/// Cost of one click's prediction (a grown pig fed, a cow milked, a saddled horse mounted, an
/// item frame). A figure to read (`--ignored --nocapture`), not a check.
#[test]
#[ignore]
fn bench_entity_click_prediction() {
    let clicks = [
        (Fake::mob(Kind::Pig, adult), held("minecraft:carrot")),
        (Fake::mob(Kind::Cow, adult), held("minecraft:bucket")),
        (Fake::mob(Kind::Horse, adult).wearing(slot::SADDLE, "minecraft:saddle", true), bare_hand()),
        (Fake::thing("item_frame"), held("minecraft:apple")),
    ];
    let p = player();
    for (seen, item) in &clicks {
        const ROUNDS: u32 = 100_000;
        let start = std::time::Instant::now();
        for _ in 0..ROUNDS {
            std::hint::black_box(predict_with(std::hint::black_box(seen), item, &p));
        }
        let ns = start.elapsed().as_secs_f64() * 1e9 / f64::from(ROUNDS);
        println!("entity click prediction {} with {:?}: {ns:.0} ns", seen.kind, item.name);
    }
}

/// The game's gear as the version data compiles it: the saddle, a harness and the nautilus
/// armour go on at a right click; horse and wolf armour do not; all come off with shears; the
/// allowed entities are the game's.
#[test]
fn equippable_rules_of_the_game_items() {
    let data = rc_testdata::version_data().expect("26.3 version data missing");
    let items = data.items().expect("item registry");
    let info = |name: &str| &data.items[items.id(name).unwrap_or_else(|| panic!("{name}")) as usize];
    let saddle = info("minecraft:saddle");
    assert!(saddle.equip_on_interact && saddle.equip_can_be_sheared);
    assert_eq!(saddle.equip_allowed, vec!["#minecraft:can_equip_saddle".to_owned()]);
    assert!(info("minecraft:white_harness").equip_on_interact);
    assert!(info("minecraft:iron_nautilus_armor").equip_on_interact);
    let horse_armor = info("minecraft:iron_horse_armor");
    assert!(!horse_armor.equip_on_interact && horse_armor.equip_can_be_sheared);
    let wolf_armor = info("minecraft:wolf_armor");
    assert_eq!(wolf_armor.equip_allowed, vec!["minecraft:wolf".to_owned()]);
    assert!(!info("minecraft:diamond_helmet").equip_on_interact);
}

/// The facts read from the synced values are the server's own layout: young and age-locked,
/// tamed and owner, a horse's tame flag and chest, a sheep's colour and shearing.
#[test]
fn facts_read_from_the_server_synced_values() {
    let item = bare_hand();
    let p = player();
    let read = |seen: &Fake, check: &dyn Fn(&Click) -> bool| {
        let click =
            Click { seen, item: &item, player: &p, tags: &tags, kind: Kind::from_name(seen.kind()), effects: vec![] };
        check(&click)
    };
    for kind in [Kind::Pig, Kind::Cow, Kind::Horse, Kind::Goat, Kind::Bee, Kind::Strider, Kind::Dolphin, Kind::Villager]
    {
        let young = Fake::mob(kind, |m| {
            baby(m);
            m.mount.age_locked = true;
        });
        assert!(read(&young, &|c| c.baby() && c.age_locked()), "{kind:?} young and locked");
        assert!(read(&Fake::mob(kind, adult), &|c| !c.baby() && !c.age_locked()), "{kind:?} grown");
    }
    assert!(read(&Fake::mob(Kind::Piglin, |m| m.baby = true), &|c| c.baby()), "piglin young flag");
    for kind in [Kind::Wolf, Kind::Cat, Kind::Parrot, Kind::Nautilus] {
        let pet = Fake::mob(kind, |m| {
            m.tame = true;
            m.owner = Some(ME);
        });
        assert!(read(&pet, &|c| c.tame() && c.owned_by_player()), "{kind:?} tamed by the player");
        assert!(read(&Fake::mob(kind, adult), &|c| !c.tame() && !c.owned_by_player()), "{kind:?} wild");
    }
    let horse = Fake::mob(Kind::Llama, |m| {
        m.mount.flags |= crate::mobs::mounts::FLAG_TAME;
        m.mount.chest = true;
    });
    assert!(read(&horse, &|c| c.horse_tamed() && c.bool_at(19)), "llama tamed with a chest");
    let sheep = Fake::mob(Kind::Sheep, |m| {
        m.wool = 5;
        m.sheared = true;
    });
    assert!(read(&sheep, &|c| c.sheep_sheared() && c.sheep_colour() == 5), "sheep colour and shearing");
    let hurt = Fake::mob(Kind::IronGolem, |m| m.health = 1.0);
    assert!(read(&hurt, &|c| c.hurt() && c.alive()), "health synced at 9");
    let dead = Fake::mob(Kind::IronGolem, |m| m.health = 0.0);
    assert!(read(&dead, &|c| !c.alive()), "dead at zero health");
}
