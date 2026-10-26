//! Species sheets of the simulated species (`server-mobs.md` § 4): dimensions, base attributes,
//! spawn category, sounds, experience. Values of game 26.3.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Zombie,
    Skeleton,
    Creeper,
    Spider,
    Enderman,
    Pig,
    Cow,
    Sheep,
    Chicken,
    Villager,
    /// F05: wandering trader (`villager/trader.rs`).
    WanderingTrader,
    Wolf,
    Cat,
    // F01 family: variants of the base hostiles and small pests.
    Husk,
    Stray,
    Bogged,
    Parched,
    ZombieVillager,
    Giant,
    CaveSpider,
    Silverfish,
    Endermite,
    Phantom,
    // Simple surface animals (F02, `surface/`).
    Rabbit,
    Mooshroom,
    Bat,
    Ocelot,
    PolarBear,
    Turtle,
    Parrot,
    // F03: cube family (`cubes.rs`).
    Slime,
    MagmaCube,
    SulfurCube,
    // F04: mounts (`mounts/`).
    Horse,
    Donkey,
    Mule,
    Llama,
    TraderLlama,
    SkeletonHorse,
    ZombieHorse,
    Camel,
    CamelHusk,
    // F06: golems (`golems/`).
    IronGolem,
    SnowGolem,
    CopperGolem,
    // F07: illagers, vex, ravager, witch (`illagers/`).
    Pillager,
    Vindicator,
    Evoker,
    Vex,
    Illusioner,
    Ravager,
    Witch,
    // F08: aquatic hostiles and monument (`aquatic/`).
    Drowned,
    Guardian,
    ElderGuardian,
    // F09: aquatic fauna and nautiluses (`sealife/`).
    Cod,
    Salmon,
    Pufferfish,
    TropicalFish,
    Squid,
    GlowSquid,
    Dolphin,
    Nautilus,
    ZombieNautilus,
    // F10: animals with a brain (`brainy/`).
    Goat,
    Frog,
    Tadpole,
    Armadillo,
    Sniffer,
    Axolotl,
    Allay,
    // F11: bee, fox, panda (`wild/`).
    Bee,
    Fox,
    Panda,
    // F12: Nether hostiles (`nether/`).
    Blaze,
    Ghast,
    WitherSkeleton,
    Strider,
    // F13: piglins and hoglins (`piglins/`).
    Piglin,
    PiglinBrute,
    ZombifiedPiglin,
    Hoglin,
    Zoglin,
    // F14: recent mobs (`recent/`).
    Warden,
    Breeze,
    Creaking,
    HappyGhast,
    // F15: Ender dragon, shulker (`end/`).
    EnderDragon,
    Shulker,
    // F16: wither (`wither/`).
    Wither,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Monster,
    Creature,
    Misc,
    Ambient,
    // F09: aquatic categories (game order: axolotls, underground aquatic,
    // aquatic, aquatic ambient).
    Axolotls,
    UndergroundWaterCreature,
    WaterCreature,
    WaterAmbient,
}

impl Category {
    /// Immediate despawn distance (§ 2.1); 64 for aquatic ambient.
    pub fn despawn_distance(self) -> i32 {
        if self == Category::WaterAmbient { 64 } else { 128 }
    }

    pub fn no_despawn_distance(self) -> i32 {
        32
    }

    pub fn friendly(self) -> bool {
        self != Category::Monster
    }

    pub fn persistent(self) -> bool {
        self == Category::Creature || self == Category::Misc
    }

    pub fn max(self) -> i32 {
        match self {
            Category::Monster => 70,
            Category::Creature => 10,
            Category::Misc => -1,
            Category::Ambient => 15,
            Category::Axolotls | Category::UndergroundWaterCreature | Category::WaterCreature => 5,
            Category::WaterAmbient => 20,
        }
    }

    /// Categories that spawn, in the order of the game's enumeration (without "misc").
    pub const SPAWNING: [Category; 7] = [
        Category::Monster,
        Category::Creature,
        Category::Ambient,
        Category::Axolotls,
        Category::UndergroundWaterCreature,
        Category::WaterCreature,
        Category::WaterAmbient,
    ];

    /// Rank in `SPAWNING` (census counters).
    pub fn slot(self) -> usize {
        Category::SPAWNING.iter().position(|c| *c == self).unwrap_or(1)
    }

    /// Key of the biome spawn lists.
    pub fn key(self) -> Option<&'static str> {
        Some(match self {
            Category::Monster => "monster",
            Category::Creature => "creature",
            Category::Ambient => "ambient",
            Category::Axolotls => "axolotls",
            Category::UndergroundWaterCreature => "underground_water_creature",
            Category::WaterCreature => "water_creature",
            Category::WaterAmbient => "water_ambient",
            Category::Misc => return None,
        })
    }
}

pub struct Info {
    pub name: &'static str,
    pub category: Category,
    pub width: f32,
    pub height: f32,
    pub eye: f32,
    pub health: f64,
    pub speed: f64,
    pub attack: f64,
    pub follow: f64,
    pub armor: f64,
    pub step: f64,
    /// Base experience, animals: 1 + next_int(3) drawn at death.
    pub xp: i32,
    /// Tracking range (chunks) on the client side.
    pub tracking: i32,
    pub ambient: Option<&'static str>,
    pub hurt: &'static str,
    pub death: &'static str,
    /// Own step noise (volume 0.15), otherwise that of the block.
    pub step_sound: Option<&'static str>,
    pub volume: f32,
    pub ambient_interval: i32,
    pub burns: bool,
    pub fall_immune: bool,
    pub not_in_peaceful: bool,
    /// Burns in water (enderman).
    pub water_sensitive: bool,
    /// Scale of the spawn box.
    pub spawn_scale: f32,
    /// Fire immune.
    pub fire_immune: bool,
}

pub(crate) const fn info(name: &'static str, category: Category, width: f32, height: f32, eye: f32) -> Info {
    Info {
        name,
        category,
        width,
        height,
        eye,
        health: 20.0,
        speed: 0.7,
        attack: 2.0,
        follow: 16.0,
        armor: 0.0,
        step: 0.6,
        xp: 0,
        tracking: 10,
        ambient: None,
        hurt: "minecraft:entity.generic.hurt",
        death: "minecraft:entity.generic.death",
        step_sound: None,
        volume: 1.0,
        ambient_interval: 80,
        burns: false,
        fall_immune: false,
        not_in_peaceful: false,
        water_sensitive: false,
        spawn_scale: 1.0,
        fire_immune: false,
    }
}

const fn monster(name: &'static str, width: f32, height: f32, eye: f32) -> Info {
    let mut i = info(name, Category::Monster, width, height, eye);
    i.xp = 5;
    i.tracking = 8;
    i.not_in_peaceful = true;
    i.hurt = "minecraft:entity.hostile.hurt";
    i.death = "minecraft:entity.hostile.death";
    i
}

pub(crate) const fn animal(name: &'static str, width: f32, height: f32, eye: f32, health: f64, speed: f64) -> Info {
    let mut i = info(name, Category::Creature, width, height, eye);
    i.health = health;
    i.speed = speed;
    i.ambient_interval = 120;
    i
}

static ZOMBIE: Info = {
    let mut i = monster("minecraft:zombie", 0.6, 1.95, 1.74);
    i.speed = 0.23;
    i.attack = 3.0;
    i.follow = 35.0;
    i.armor = 2.0;
    i.ambient = Some("minecraft:entity.zombie.ambient");
    i.hurt = "minecraft:entity.zombie.hurt";
    i.death = "minecraft:entity.zombie.death";
    i.step_sound = Some("minecraft:entity.zombie.step");
    i.burns = true;
    i
};

static SKELETON: Info = {
    let mut i = monster("minecraft:skeleton", 0.6, 1.99, 1.74);
    i.speed = 0.25;
    i.ambient = Some("minecraft:entity.skeleton.ambient");
    i.hurt = "minecraft:entity.skeleton.hurt";
    i.death = "minecraft:entity.skeleton.death";
    i.step_sound = Some("minecraft:entity.skeleton.step");
    i.burns = true;
    i
};

static CREEPER: Info = {
    let mut i = monster("minecraft:creeper", 0.6, 1.7, 1.7 * 0.85);
    i.speed = 0.25;
    i.hurt = "minecraft:entity.creeper.hurt";
    i.death = "minecraft:entity.creeper.death";
    i
};

static SPIDER: Info = {
    let mut i = monster("minecraft:spider", 1.4, 0.9, 0.65);
    i.health = 16.0;
    i.speed = 0.3;
    i.ambient = Some("minecraft:entity.spider.ambient");
    i.hurt = "minecraft:entity.spider.hurt";
    i.death = "minecraft:entity.spider.death";
    i.step_sound = Some("minecraft:entity.spider.step");
    i
};

static ENDERMAN: Info = {
    let mut i = monster("minecraft:enderman", 0.6, 2.9, 2.55);
    i.health = 40.0;
    i.speed = 0.3;
    i.attack = 7.0;
    i.follow = 64.0;
    i.step = 1.0;
    i.ambient = Some("minecraft:entity.enderman.ambient");
    i.hurt = "minecraft:entity.enderman.hurt";
    i.death = "minecraft:entity.enderman.death";
    i.water_sensitive = true;
    i
};

static PIG: Info = {
    let mut i = animal("minecraft:pig", 0.9, 0.9, 0.9 * 0.85, 10.0, 0.25);
    i.ambient = Some("minecraft:entity.pig.ambient");
    i.hurt = "minecraft:entity.pig.hurt";
    i.death = "minecraft:entity.pig.death";
    i.step_sound = Some("minecraft:entity.pig.step");
    i
};

static COW: Info = {
    let mut i = animal("minecraft:cow", 0.9, 1.4, 1.3, 10.0, 0.2f32 as f64);
    i.ambient = Some("minecraft:entity.cow.ambient");
    i.hurt = "minecraft:entity.cow.hurt";
    i.death = "minecraft:entity.cow.death";
    i.step_sound = Some("minecraft:entity.cow.step");
    i.volume = 0.4;
    i
};

static SHEEP: Info = {
    let mut i = animal("minecraft:sheep", 0.9, 1.3, 1.235, 8.0, 0.23f32 as f64);
    i.ambient = Some("minecraft:entity.sheep.ambient");
    i.hurt = "minecraft:entity.sheep.hurt";
    i.death = "minecraft:entity.sheep.death";
    i.step_sound = Some("minecraft:entity.sheep.step");
    i
};

static CHICKEN: Info = {
    let mut i = animal("minecraft:chicken", 0.4, 0.7, 0.644, 4.0, 0.25);
    i.ambient = Some("minecraft:entity.chicken.ambient");
    i.hurt = "minecraft:entity.chicken.hurt";
    i.death = "minecraft:entity.chicken.death";
    i.step_sound = Some("minecraft:entity.chicken.step");
    i.fall_immune = true;
    i
};

static VILLAGER: Info = {
    let mut i = info("minecraft:villager", Category::Misc, 0.6, 1.95, 1.62);
    i.speed = 0.5;
    i.ambient = Some("minecraft:entity.villager.ambient");
    i.hurt = "minecraft:entity.villager.hurt";
    i.death = "minecraft:entity.villager.death";
    i
};

/// Wandering trader: default mob attributes (speed 0.7, HP 20).
static WANDERING_TRADER: Info = {
    let mut i = info("minecraft:wandering_trader", Category::Creature, 0.6, 1.95, 1.62);
    i.ambient = Some("minecraft:entity.wandering_trader.ambient");
    i.hurt = "minecraft:entity.wandering_trader.hurt";
    i.death = "minecraft:entity.wandering_trader.death";
    i
};

static WOLF: Info = {
    let mut i = animal("minecraft:wolf", 0.6, 0.85, 0.68, 8.0, 0.3f32 as f64);
    i.attack = 4.0;
    i.ambient = Some("minecraft:entity.wolf.ambient");
    i.hurt = "minecraft:entity.wolf.hurt";
    i.death = "minecraft:entity.wolf.death";
    i.step_sound = Some("minecraft:entity.wolf.step");
    i.volume = 0.4;
    i
};

static CAT: Info = {
    let mut i = animal("minecraft:cat", 0.6, 0.7, 0.35, 10.0, 0.3f32 as f64);
    i.attack = 3.0;
    i.tracking = 8;
    i.ambient = Some("minecraft:entity.cat.stray_ambient");
    i.hurt = "minecraft:entity.cat.hurt";
    i.death = "minecraft:entity.cat.death";
    i.fall_immune = true;
    i
};

static HUSK: Info = {
    let mut i = monster("minecraft:husk", 0.6, 1.95, 1.74);
    i.speed = 0.23;
    i.attack = 3.0;
    i.follow = 35.0;
    i.armor = 2.0;
    i.ambient = Some("minecraft:entity.husk.ambient");
    i.hurt = "minecraft:entity.husk.hurt";
    i.death = "minecraft:entity.husk.death";
    i.step_sound = Some("minecraft:entity.husk.step");
    i
};

static ZOMBIE_VILLAGER: Info = {
    let mut i = monster("minecraft:zombie_villager", 0.6, 1.95, 1.74);
    i.speed = 0.23;
    i.attack = 3.0;
    i.follow = 35.0;
    i.armor = 2.0;
    i.ambient = Some("minecraft:entity.zombie_villager.ambient");
    i.hurt = "minecraft:entity.zombie_villager.hurt";
    i.death = "minecraft:entity.zombie_villager.death";
    i.step_sound = Some("minecraft:entity.zombie_villager.step");
    i.burns = true;
    i
};

const fn skeleton_like(name: &'static str, health: f64, sounds: [&'static str; 4], burns: bool) -> Info {
    let mut i = monster(name, 0.6, 1.99, 1.74);
    i.health = health;
    i.speed = 0.25;
    i.ambient = Some(sounds[0]);
    i.hurt = sounds[1];
    i.death = sounds[2];
    i.step_sound = Some(sounds[3]);
    i.burns = burns;
    i
}

static STRAY: Info = skeleton_like(
    "minecraft:stray",
    20.0,
    [
        "minecraft:entity.stray.ambient",
        "minecraft:entity.stray.hurt",
        "minecraft:entity.stray.death",
        "minecraft:entity.stray.step",
    ],
    true,
);

static BOGGED: Info = skeleton_like(
    "minecraft:bogged",
    16.0,
    [
        "minecraft:entity.bogged.ambient",
        "minecraft:entity.bogged.hurt",
        "minecraft:entity.bogged.death",
        "minecraft:entity.bogged.step",
    ],
    true,
);

static PARCHED: Info = skeleton_like(
    "minecraft:parched",
    16.0,
    [
        "minecraft:entity.parched.ambient",
        "minecraft:entity.parched.hurt",
        "minecraft:entity.parched.death",
        "minecraft:entity.parched.step",
    ],
    false,
);

static GIANT: Info = {
    let mut i = monster("minecraft:giant", 3.6, 12.0, 10.44);
    i.health = 100.0;
    i.speed = 0.5;
    i.attack = 50.0;
    i.tracking = 10;
    i
};

static CAVE_SPIDER: Info = {
    let mut i = monster("minecraft:cave_spider", 0.7, 0.5, 0.45);
    i.health = 12.0;
    i.speed = 0.3;
    i.ambient = Some("minecraft:entity.spider.ambient");
    i.hurt = "minecraft:entity.spider.hurt";
    i.death = "minecraft:entity.spider.death";
    i.step_sound = Some("minecraft:entity.spider.step");
    i
};

/// Silverfish and endermite: no sound emission for steps (events only).
static SILVERFISH: Info = {
    let mut i = monster("minecraft:silverfish", 0.4, 0.3, 0.13);
    i.health = 8.0;
    i.speed = 0.25;
    i.attack = 1.0;
    i.ambient = Some("minecraft:entity.silverfish.ambient");
    i.hurt = "minecraft:entity.silverfish.hurt";
    i.death = "minecraft:entity.silverfish.death";
    i
};

static ENDERMITE: Info = {
    let mut i = monster("minecraft:endermite", 0.4, 0.3, 0.13);
    i.health = 8.0;
    i.speed = 0.25;
    i.xp = 3;
    i.ambient = Some("minecraft:entity.endermite.ambient");
    i.hurt = "minecraft:entity.endermite.hurt";
    i.death = "minecraft:entity.endermite.death";
    i
};

/// Phantom: flying mob (not "Monster" in the game's sense but monster category).
static PHANTOM: Info = {
    let mut i = monster("minecraft:phantom", 0.9, 0.5, 0.175);
    i.attack = 6.0;
    i.hurt = "minecraft:entity.phantom.hurt";
    i.death = "minecraft:entity.phantom.death";
    i.ambient = Some("minecraft:entity.phantom.ambient");
    i.burns = true;
    i.fall_immune = true;
    i
};

static SLIME: Info = {
    let mut i = monster("minecraft:slime", 0.52, 0.52, 0.325);
    i.tracking = 10;
    i.xp = 0;
    i.hurt = "minecraft:entity.slime.hurt";
    i.death = "minecraft:entity.slime.death";
    i.spawn_scale = 4.0;
    i
};

static MAGMA_CUBE: Info = {
    let mut i = monster("minecraft:magma_cube", 0.52, 0.52, 0.325);
    i.speed = 0.2f32 as f64;
    i.xp = 0;
    i.hurt = "minecraft:entity.magma_cube.hurt";
    i.death = "minecraft:entity.magma_cube.death";
    i.spawn_scale = 4.0;
    i.fire_immune = true;
    // `#fall_damage_immune`.
    i.fall_immune = true;
    i
};

static SULFUR_CUBE: Info = {
    let mut i = info("minecraft:sulfur_cube", Category::Monster, 0.49, 0.49, 0.175);
    i.hurt = "minecraft:entity.sulfur_cube.hurt";
    i.death = "minecraft:entity.sulfur_cube.death";
    i.spawn_scale = 2.0;
    i
};

impl Kind {
    pub const ALL: [Kind; 90] = [
        Kind::Warden,
        Kind::Breeze,
        Kind::Creaking,
        Kind::HappyGhast,
        Kind::Bee,
        Kind::Fox,
        Kind::Panda,
        Kind::Cod,
        Kind::Salmon,
        Kind::Pufferfish,
        Kind::TropicalFish,
        Kind::Squid,
        Kind::GlowSquid,
        Kind::Dolphin,
        Kind::Nautilus,
        Kind::ZombieNautilus,
        Kind::Goat,
        Kind::Frog,
        Kind::Tadpole,
        Kind::Armadillo,
        Kind::Sniffer,
        Kind::Axolotl,
        Kind::Allay,
        Kind::WanderingTrader,
        Kind::Rabbit,
        Kind::Mooshroom,
        Kind::Bat,
        Kind::Ocelot,
        Kind::PolarBear,
        Kind::Turtle,
        Kind::Parrot,
        Kind::Zombie,
        Kind::Skeleton,
        Kind::Creeper,
        Kind::Spider,
        Kind::Enderman,
        Kind::Pig,
        Kind::Cow,
        Kind::Sheep,
        Kind::Chicken,
        Kind::Villager,
        Kind::Wolf,
        Kind::Cat,
        Kind::Husk,
        Kind::Stray,
        Kind::Bogged,
        Kind::Parched,
        Kind::ZombieVillager,
        Kind::Giant,
        Kind::CaveSpider,
        Kind::Silverfish,
        Kind::Endermite,
        Kind::Phantom,
        Kind::Slime,
        Kind::MagmaCube,
        Kind::SulfurCube,
        Kind::Horse,
        Kind::Donkey,
        Kind::Mule,
        Kind::Llama,
        Kind::TraderLlama,
        Kind::SkeletonHorse,
        Kind::ZombieHorse,
        Kind::Camel,
        Kind::CamelHusk,
        Kind::IronGolem,
        Kind::SnowGolem,
        Kind::CopperGolem,
        Kind::Pillager,
        Kind::Vindicator,
        Kind::Evoker,
        Kind::Vex,
        Kind::Illusioner,
        Kind::Ravager,
        Kind::Witch,
        Kind::Drowned,
        Kind::Guardian,
        Kind::ElderGuardian,
        Kind::Blaze,
        Kind::Ghast,
        Kind::WitherSkeleton,
        Kind::Strider,
        Kind::Piglin,
        Kind::PiglinBrute,
        Kind::ZombifiedPiglin,
        Kind::Hoglin,
        Kind::Zoglin,
        Kind::EnderDragon,
        Kind::Shulker,
        Kind::Wither,
    ];

    /// Base species whose behavior the variant inherits (zombie, skeleton, spider).
    pub fn base(self) -> Kind {
        match self {
            Kind::Husk | Kind::ZombieVillager | Kind::Drowned => Kind::Zombie,
            Kind::Stray | Kind::Bogged | Kind::Parched | Kind::WitherSkeleton => Kind::Skeleton,
            Kind::CaveSpider => Kind::Spider,
            k => k,
        }
    }

    /// Tag `#minecraft:zombies` limited to the simulated species.
    pub fn zombie_like(self) -> bool {
        self.base() == Kind::Zombie || self == Kind::ZombifiedPiglin
    }

    /// Tag `#minecraft:skeletons` limited to the simulated species.
    pub fn skeleton_like(self) -> bool {
        self.base() == Kind::Skeleton
    }

    pub fn info(self) -> &'static Info {
        match self {
            Kind::Zombie => &ZOMBIE,
            Kind::Skeleton => &SKELETON,
            Kind::Creeper => &CREEPER,
            Kind::Spider => &SPIDER,
            Kind::Enderman => &ENDERMAN,
            Kind::Pig => &PIG,
            Kind::Cow => &COW,
            Kind::Sheep => &SHEEP,
            Kind::Chicken => &CHICKEN,
            Kind::Villager => &VILLAGER,
            Kind::WanderingTrader => &WANDERING_TRADER,
            Kind::Wolf => &WOLF,
            Kind::Cat => &CAT,
            Kind::Husk => &HUSK,
            Kind::Stray => &STRAY,
            Kind::Bogged => &BOGGED,
            Kind::Parched => &PARCHED,
            Kind::ZombieVillager => &ZOMBIE_VILLAGER,
            Kind::Giant => &GIANT,
            Kind::CaveSpider => &CAVE_SPIDER,
            Kind::Silverfish => &SILVERFISH,
            Kind::Endermite => &ENDERMITE,
            Kind::Phantom => &PHANTOM,
            Kind::Slime => &SLIME,
            Kind::MagmaCube => &MAGMA_CUBE,
            Kind::SulfurCube => &SULFUR_CUBE,
            k if super::mounts::handles(k) => super::mounts::info(k),
            k if super::golems::handles(k) => super::golems::info(k),
            k if super::illagers::handles(k) => super::illagers::info(k),
            k if super::aquatic::handles(k) => super::aquatic::info(k),
            k if super::sealife::handles(k) => super::sealife::info(k),
            k if super::brainy::handles(k) => super::brainy::info(k),
            k if super::wild::handles(k) => super::wild::info(k),
            k if super::nether::handles(k) => super::nether::info(k),
            k if super::piglins::handles(k) => super::piglins::info(k),
            k if super::recent::handles(k) => super::recent::info(k),
            k if super::end::handles(k) => super::end::info(k),
            Kind::Wither => &super::wither::WITHER,
            k => super::surface::info(k),
        }
    }

    /// Cube family (variable size, jumps, splitting).
    pub fn is_cube(self) -> bool {
        matches!(self, Kind::Slime | Kind::MagmaCube | Kind::SulfurCube)
    }

    pub fn from_name(n: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.info().name == n)
    }

    pub fn is_animal(self) -> bool {
        matches!(self, Kind::Pig | Kind::Cow | Kind::Sheep | Kind::Chicken | Kind::Wolf | Kind::Cat)
            || super::surface::is_animal(self)
            || super::mounts::handles(self)
            || super::sealife::is_nautilus(self)
            || super::brainy::is_animal(self)
            || super::wild::handles(self)
            || super::nether::is_animal(self)
            || self == Kind::Hoglin
            || super::recent::is_animal(self)
    }

    pub fn is_monster(self) -> bool {
        self.info().category == Category::Monster
    }

    /// Age (babies) handled: animals, villagers; zombie: own baby flag.
    pub fn ageable(self) -> bool {
        (self.is_animal() && self != Kind::ZombieNautilus)
            || self == Kind::Villager
            || self == Kind::WanderingTrader
            || self == Kind::SulfurCube
            || super::sealife::ageable_water(self)
    }

    /// Undead (`#minecraft:undead`): water impassable for pathing, etc.
    pub fn undead(self) -> bool {
        self.zombie_like()
            || self.skeleton_like()
            || self == Kind::Phantom
            || super::mounts::undead(self)
            || self == Kind::ZombieNautilus
            || self == Kind::Zoglin
            || self == Kind::Wither
    }

    // Entity type tags frozen into lists (checked species by species against the
    // jar's tags by `tag_tests`): every added species must be filed there.

    /// `#can_breathe_under_water`: undead, aquatic fauna except the dolphin, frog,
    /// copper golem.
    pub fn breathes_underwater(self) -> bool {
        self.undead()
            || super::aquatic::breathes_underwater(self)
            || super::sealife::breathes_underwater(self)
            || super::brainy::breathes_underwater(self)
            || matches!(self, Kind::CopperGolem | Kind::Turtle)
    }

    /// `#arthropod` (= `#sensitive_to_bane_of_arthropods`).
    pub fn arthropod(self) -> bool {
        matches!(self, Kind::Spider | Kind::CaveSpider | Kind::Silverfish | Kind::Endermite | Kind::Bee)
    }

    /// `#aquatic` (= `#sensitive_to_impaling`).
    pub fn aquatic(self) -> bool {
        super::sealife::handles(self)
            || matches!(self, Kind::Guardian | Kind::ElderGuardian | Kind::Axolotl | Kind::Tadpole | Kind::Turtle)
    }

    /// `#no_anger_from_wind_charge`: hit by a wind charge, they do not designate the
    /// thrower.
    pub fn no_anger_from_wind_charge(self) -> bool {
        matches!(
            self,
            Kind::Breeze
                | Kind::Skeleton
                | Kind::Bogged
                | Kind::Stray
                | Kind::Zombie
                | Kind::Husk
                | Kind::Spider
                | Kind::CaveSpider
                | Kind::Slime
        )
    }

    /// `#non_controlling_rider`: passenger that never steers its mount.
    pub fn non_controlling_rider(self) -> bool {
        self.is_cube()
    }

    /// `#can_float_while_ridden`: floats in a deep fluid while it carries a passenger.
    pub fn floats_while_ridden(self) -> bool {
        matches!(self, Kind::Horse | Kind::ZombieHorse | Kind::Mule | Kind::Donkey | Kind::Camel | Kind::CamelHusk)
    }

    /// `#cannot_be_age_locked`.
    pub fn cannot_be_age_locked(self) -> bool {
        matches!(self, Kind::ZombieHorse | Kind::SkeletonHorse | Kind::Villager)
    }

    /// `#powder_snow_walkable_mobs`.
    pub fn powder_snow_walkable(self) -> bool {
        matches!(self, Kind::Rabbit | Kind::Fox | Kind::Silverfish | Kind::Endermite)
    }

    /// `#freeze_immune_entity_types`.
    pub fn freeze_immune(self) -> bool {
        matches!(self, Kind::Stray | Kind::PolarBear | Kind::SnowGolem | Kind::Wither)
    }

    /// `#freeze_hurts_extra_types`: freeze damage ×5.
    pub fn freeze_hurts_extra(self) -> bool {
        matches!(self, Kind::Strider | Kind::Blaze | Kind::MagmaCube)
    }

    /// `#skeletons`.
    pub fn skeletons_tag(self) -> bool {
        self.skeleton_like() || self == Kind::SkeletonHorse
    }

    pub fn loot_table(self) -> String {
        format!("minecraft:entities/{}", self.info().name.trim_start_matches("minecraft:"))
    }

    /// Food that attracts and makes breed (item tag).
    pub fn food_tag(self) -> Option<&'static str> {
        Some(match self {
            Kind::Pig => "minecraft:pig_food",
            Kind::Cow => "minecraft:cow_food",
            Kind::Sheep => "minecraft:sheep_food",
            Kind::Chicken => "minecraft:chicken_food",
            Kind::Wolf => "minecraft:wolf_food",
            Kind::Cat => "minecraft:cat_food",
            k if super::brainy::handles(k) => return super::brainy::food_tag(k),
            k if super::wild::handles(k) => return super::wild::food_tag(k),
            Kind::Hoglin => "minecraft:hoglin_food",
            k if super::recent::handles(k) => return super::recent::food_tag(k),
            k => return super::surface::food_tag(k).or_else(|| super::nether::food_tag(k)),
        })
    }
}

/// Static bound of a species' box around its position: (half-width, height)
/// maximum over adult, baby and poses (sitting, jumping, digging: all smaller than
/// the adult). `None` for variable boxes: cubes (size), shulker (opening), phantom
/// (size), pufferfish (inflation), salmon (size). Tests check the bound at
/// each use (`Mob::may_touch`).
pub fn reach(kind: Kind) -> Option<(f64, f64)> {
    static T: std::sync::OnceLock<Vec<Option<(f64, f64)>>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let n = Kind::ALL.iter().map(|k| *k as usize + 1).max().unwrap_or(0);
        let mut t = vec![None; n];
        for k in Kind::ALL {
            if k.is_cube() || matches!(k, Kind::Shulker | Kind::Phantom | Kind::Pufferfish | Kind::Salmon) {
                continue;
            }
            let (a, b) = (dims(k, false), dims(k, true));
            t[k as usize] = Some((f64::from(a.0.max(b.0)) / 2.0, f64::from(a.1.max(b.1))));
        }
        t
    })
    .get(kind as usize)
    .copied()
    .flatten()
}

/// Dimensions of a baby (scale 0.5 except exceptions).
pub fn dims(kind: Kind, baby: bool) -> (f32, f32, f32) {
    let i = kind.info();
    if !baby {
        return (i.width, i.height, i.eye);
    }
    match kind {
        Kind::Villager => (0.49, 0.98, 0.63),
        Kind::Zombie | Kind::Drowned => (0.49, 0.98, 0.775),
        Kind::Husk => (0.49, 0.98, 0.825),
        Kind::ZombieVillager => (0.49, 0.98, 0.67),
        Kind::Cat => (0.3, 0.35, 0.343_75),
        Kind::Wolf => (0.3, 0.425, 0.343_75),
        Kind::Chicken => (0.3, 0.4, 0.281_25),
        Kind::Cow => (0.45, 0.7, 0.69),
        Kind::Pig => (0.45, 0.45, 0.406_25),
        Kind::Sheep => (0.45, 0.65, 0.656_25),
        k if super::surface::baby_dims(k).is_some() => super::surface::baby_dims(k).unwrap_or_default(),
        k if super::mounts::handles(k) => super::mounts::baby_dims(k),
        k if super::sealife::handles(k) => super::sealife::baby_dims(k),
        k if super::brainy::handles(k) => super::brainy::baby_dims(k),
        k if super::wild::handles(k) => super::wild::baby_dims(k),
        k if super::nether::baby_dims(k).is_some() => super::nether::baby_dims(k).unwrap_or_default(),
        k if super::piglins::handles(k) => super::piglins::baby_dims(k),
        k if super::recent::baby_dims(k).is_some() => super::recent::baby_dims(k).unwrap_or_default(),
        _ => (i.width * 0.5, i.height * 0.5, i.eye * 0.5),
    }
}

#[cfg(test)]
mod peaceful_tests {
    use super::Kind;

    /// Simulated species follow the shared list of types forbidden in peaceful
    /// (egg tooltip on the client side, summoning and spawning on the server side).
    #[test]
    fn simulated_kinds_follow_the_shared_peaceful_list() {
        let wrong: Vec<&str> = Kind::ALL
            .iter()
            .map(|k| k.info())
            .filter(|i| i.not_in_peaceful == rc_data::rules::allowed_in_peaceful(i.name))
            .map(|i| i.name)
            .collect();
        assert!(wrong.is_empty(), "different peaceful rule: {wrong:?}");
    }
}
