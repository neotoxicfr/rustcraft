//! G5-F02e : contacts des entités avec les blocs traversés (buisson à baies, toile et Tissage,
//! rose de Wither, chaudrons, plaques et fil pour toutes les entités) et bottes enchantées
//! (Semelles givrantes, Agilité des âmes). Monde plat neuf : aucune donnée externe autre que le
//! cache 26.3 et le jar client, dont l'absence fait ÉCHOUER le test. Valeurs attendues tirées
//! des blocs et des fichiers d'enchantement du jeu 26.3.

use std::path::PathBuf;
use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::probe::{Probe, state_string};
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::player::Stack;

struct W {
    p: Probe,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Première couche d'air du monde plat (herbe en y = −61).
const Y: i32 = -60;

fn world(tag: &str) -> W {
    let data = rc_testdata::version_data().expect("G5-F02e : cache des données 26.3 absent");
    let data = Arc::new(data);
    assert!(crate::registries::client_jar(&data).is_some(), "G5-F02e : jar client 26.3 absent");
    let dir = std::env::temp_dir().join(format!("rc-g5f02e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    w.preset = crate::newworld::Preset::Flat;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("création du monde");
    let mut p = Probe::open(&dir, data);
    let want: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 120, "G5-F02e : génération du monde plat trop longue");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.run(&format!("fill 0 {Y} 0 15 {} 15 minecraft:air\n", Y + 3));
    // Joueur loin des essais (en x = 40), au sol.
    p.s.pos = [40.5, f64::from(Y), 40.5];
    p.s.on_ground = true;
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    let mut w = W { p, dir };
    w.p.s.set_difficulty(2, true);
    w.cmd("time set midnight"); // pas de zombie qui brûle au soleil
    w
}

impl W {
    fn cmd(&mut self, c: &str) {
        let src = self.p.s.player_source();
        assert!(self.p.s.perform(src, c).0, "commande refusée : {c}");
    }

    fn block(&self, p: [i32; 3]) -> String {
        state_string(&self.p.s, self.p.s.get(p))
    }

    /// Créature sans IA posée en `pos` (elle ne bouge que par sa vitesse quand `ai`).
    fn mob(&mut self, k: Kind, pos: [f64; 3], ai: bool) -> Mob {
        let mut m = self.p.s.new_mob(k, pos, 0.0);
        m.no_ai = !ai;
        m.on_ground = true;
        m
    }

    /// Le joueur posé en `pos`, au sol, mouvement connu `mv`, puis un tick de vie.
    fn player_tick_at(&mut self, pos: [f64; 3], mv: [f64; 3]) {
        self.p.s.pos = pos;
        self.p.s.on_ground = true;
        self.p.s.proj_player.known_move = mv;
        self.p.s.vitals_tick();
    }
}

fn boots(ench: &str, level: i32) -> Stack {
    let mut s = Stack { id: "minecraft:diamond_boots".into(), count: 1, components: None };
    crate::items::set_component(&mut s, "minecraft:enchantments", Nbt::Compound(vec![(ench.into(), Nbt::Int(level))]));
    s
}

fn at(x: i32, z: i32) -> [f64; 3] {
    [f64::from(x) + 0.5, f64::from(Y), f64::from(z) + 0.5]
}

/// Buisson à baies : 1 dégât `sweet_berry_bush` à qui bouge dedans (âge ≥ 1), sauf renard et
/// abeille ; rien à l'arrêt ni dans un buisson d'âge 0.
#[test]
fn baies_piquent_qui_bouge_dedans() {
    let mut w = world("baies");
    w.p.run(&format!(
        "setblock 2 {Y} 2 minecraft:sweet_berry_bush[age=1]\nsetblock 6 {Y} 2 minecraft:sweet_berry_bush[age=0]\nsetblock 10 {Y} 2 minecraft:sweet_berry_bush[age=3]\nsetblock 2 {Y} 6 minecraft:sweet_berry_bush[age=2]\n"
    ));
    // Cochon qui traverse (vitesse donnée) : piqué.
    let mut z = w.mob(Kind::Pig, at(2, 2), true);
    z.vel = [0.08, 0.0, 0.0];
    w.p.s.tick_mob(&mut z);
    assert_eq!(z.health, 9.0, "cochon qui bouge dans le buisson d'âge 1");
    // Zombie immobile (sans IA) : pas de dégât.
    let mut still = w.mob(Kind::Zombie, at(10, 2), false);
    w.p.s.tick_mob(&mut still);
    assert_eq!(still.health, 20.0, "zombie immobile");
    // Âge 0 : pas de dégât même en bougeant.
    let mut young = w.mob(Kind::Zombie, at(6, 2), true);
    young.vel = [0.08, 0.0, 0.0];
    w.p.s.tick_mob(&mut young);
    assert_eq!(young.health, 20.0, "buisson d'âge 0");
    // Renard : jamais piqué.
    let mut fox = w.mob(Kind::Fox, at(2, 6), true);
    fox.vel = [0.08, 0.0, 0.0];
    let full = fox.health;
    w.p.s.tick_mob(&mut fox);
    assert_eq!(fox.health, full, "renard");
    // Joueur qui bouge dans le buisson d'âge 3 (mouvement connu du client).
    w.player_tick_at(at(10, 2), [0.1, 0.0, 0.0]);
    assert_eq!(w.p.s.life.vit.health, 19.0, "joueur qui bouge");
    // Sous le seuil (0,002 par axe) : rien.
    w.p.s.life.vit.health = 20.0;
    w.p.s.life.vit.cooldown = 0;
    w.player_tick_at(at(10, 2), [0.002, 0.0, 0.002]);
    assert_eq!(w.p.s.life.vit.health, 20.0, "joueur presque immobile");
}

/// Toile : la créature y est enlisée (0,25 à l'horizontale), deux fois moins sous Tissage.
#[test]
fn toile_enlise_les_creatures_et_tissage() {
    let mut w = world("toile");
    w.p.run(&format!("setblock 2 {Y} 2 minecraft:cobweb\nsetblock 2 {Y} 8 minecraft:cobweb\n"));
    let mut plain = w.mob(Kind::Zombie, at(2, 2), true);
    let mut woven = w.mob(Kind::Zombie, at(2, 8), true);
    assert!(w.p.s.give_mob_effect(&mut woven, "minecraft:weaving", 100_000, 0));
    let mut free = w.mob(Kind::Zombie, at(8, 14), true);
    let mut dx = Vec::new();
    for m in [&mut plain, &mut woven, &mut free] {
        m.vel = [0.2, 0.0, 0.0];
        w.p.s.tick_mob(m); // contact posé
        m.vel = [0.2, 0.0, 0.0];
        let x0 = m.pos[0];
        w.p.s.tick_mob(m); // déplacement enlisé
        dx.push(m.pos[0] - x0);
    }
    assert!(dx[0] > 0.0 && dx[0] < 0.06, "toile : {dx:?}");
    assert!((dx[1] / dx[0] - 2.0).abs() < 1.0e-6, "Tissage = deux fois moins d'enlisement : {dx:?}");
    assert!(dx[2] > 0.15, "hors toile : {dx:?}");
}

/// Rose de Wither : effet Wither 40 ticks aux vivants, sauf en paisible et pour l'invulnérable.
#[test]
fn rose_de_wither_donne_l_effet() {
    let mut w = world("rose");
    w.p.run(&format!("setblock 2 {Y} 2 minecraft:wither_rose\nsetblock 10 {Y} 10 minecraft:wither_rose\n"));
    let wither = |m: &Mob| m.x.effects.iter().find(|e| e.id == "minecraft:wither").map(|e| e.dur);
    let mut z = w.mob(Kind::Zombie, at(2, 2), false);
    w.p.s.tick_mob(&mut z);
    assert!(wither(&z).is_some_and(|d| (38..=40).contains(&d)), "zombie : {:?}", wither(&z));
    let mut inv = w.mob(Kind::Zombie, at(2, 2), false);
    inv.invulnerable = true;
    w.p.s.tick_mob(&mut inv);
    assert_eq!(wither(&inv), None, "créature invulnérable");
    // Joueur en survie.
    w.player_tick_at(at(10, 10), [0.0; 3]);
    assert!(w.p.s.fx().has("minecraft:wither"), "joueur en survie");
    // Paisible : rien.
    w.cmd("effect clear @s");
    w.p.s.set_difficulty(0, true);
    let mut z = w.mob(Kind::Pig, at(2, 2), false);
    w.p.s.tick_mob(&mut z);
    assert_eq!(wither(&z), None, "paisible (créature)");
    w.player_tick_at(at(10, 10), [0.0; 3]);
    assert!(!w.p.s.fx().has("minecraft:wither"), "paisible (joueur)");
}

/// Chaudrons : l'entité en feu dans l'eau baisse le niveau (la neige poudreuse devient de l'eau)
/// et s'éteint, seulement si elle touche le contenu ; la lave allume (15 s) et blesse.
#[test]
fn chaudrons_eteignent_et_brulent() {
    let mut w = world("chaudrons");
    w.p.run(&format!(
        "setblock 2 {Y} 2 minecraft:water_cauldron[level=3]\nsetblock 6 {Y} 2 minecraft:powder_snow_cauldron[level=1]\nsetblock 10 {Y} 2 minecraft:lava_cauldron\nsetblock 2 {Y} 6 minecraft:water_cauldron[level=1]\nsetblock 6 {Y} 6 minecraft:water_cauldron[level=2]\nsetblock 10 {Y} 6 minecraft:lava_cauldron\n"
    ));
    let floor = |x: i32, z: i32| [f64::from(x) + 0.5, f64::from(Y) + 0.25, f64::from(z) + 0.5];
    // Zombie en feu au fond d'un chaudron plein.
    let mut z = w.mob(Kind::Zombie, floor(2, 2), false);
    z.fire = 100;
    w.p.s.tick_mob(&mut z);
    assert_eq!(w.block([2, Y, 2]), "minecraft:water_cauldron[level=2]");
    assert!(z.fire <= 0, "éteint : {}", z.fire);
    // Neige poudreuse niveau 1 : eau niveau 1 puis un de moins = chaudron vide.
    let mut z = w.mob(Kind::Zombie, floor(6, 2), false);
    z.fire = 100;
    w.p.s.tick_mob(&mut z);
    assert_eq!(w.block([6, Y, 2]), "minecraft:cauldron");
    assert!(z.fire <= 0);
    // Au-dessus du contenu (niveau 1 = 9 px) : rien.
    let mut z = w.mob(Kind::Zombie, [2.5, f64::from(Y) + 0.6, 6.5], false);
    z.fire = 100;
    w.p.s.tick_mob(&mut z);
    assert_eq!(w.block([2, Y, 6]), "minecraft:water_cauldron[level=1]");
    assert!(z.fire > 0, "hors du contenu, toujours en feu");
    // Lave : allumage 15 s, 4 dégâts, dégel.
    let mut z = w.mob(Kind::Pig, floor(10, 2), false);
    z.ticks_frozen = 50;
    w.p.s.tick_mob(&mut z);
    assert!(z.fire >= 299, "allumé : {}", z.fire);
    assert_eq!(z.health, 6.0, "dégâts de lave");
    assert_eq!(z.ticks_frozen, 0, "dégel");
    // Joueur en feu dans l'eau, puis dans la lave.
    w.p.s.life.vit.fire = 100;
    w.player_tick_at(floor(6, 6), [0.0; 3]);
    assert_eq!(w.block([6, Y, 6]), "minecraft:water_cauldron[level=1]");
    assert!(w.p.s.life.vit.fire <= 0, "joueur éteint : {}", w.p.s.life.vit.fire);
    w.player_tick_at(floor(10, 6), [0.0; 3]);
    assert!(w.p.s.life.vit.fire >= 299, "joueur allumé : {}", w.p.s.life.vit.fire);
    assert!(w.p.s.life.vit.health <= 16.0, "joueur blessé : {}", w.p.s.life.vit.health);
}

/// Plaques et fil : une créature, un objet au sol enfoncent la plaque ; une créature tend le
/// fil, pas une chauve-souris.
#[test]
fn plaques_et_fil_pour_toutes_les_entites() {
    let mut w = world("plaques");
    w.p.run(&format!(
        "setblock 2 {Y} 2 minecraft:stone_pressure_plate\nsetblock 6 {Y} 2 minecraft:oak_pressure_plate\nsetblock 10 {Y} 2 minecraft:tripwire\nsetblock 10 {Y} 6 minecraft:tripwire\n"
    ));
    let mut z = w.mob(Kind::Zombie, [2.5, f64::from(Y) + 0.0625, 2.5], false);
    w.p.s.tick_mob(&mut z);
    assert_eq!(w.block([2, Y, 2]), "minecraft:stone_pressure_plate[powered=true]", "créature sur la plaque");
    // Objet lâché sur la plaque en bois.
    let s = Stack { id: "minecraft:stick".into(), count: 1, components: None };
    w.p.s.spawn_item([6.5, f64::from(Y) + 0.07, 6.5 - 4.0], [0.0; 3], s, 40);
    if let Some(d) = w.p.s.life.drops.last_mut() {
        d.pos = [6.5, f64::from(Y) + 0.0625, 2.5];
    }
    w.p.s.drops_tick();
    assert_eq!(w.block([6, Y, 2]), "minecraft:oak_pressure_plate[powered=true]", "objet sur la plaque");
    // Fil : la chauve-souris ne compte pas, le zombie si.
    let mut bat = w.mob(Kind::Bat, [10.5, f64::from(Y) + 0.1, 6.5], false);
    w.p.s.tick_mob(&mut bat);
    assert!(w.block([10, Y, 6]).contains("powered=false"), "chauve-souris : {}", w.block([10, Y, 6]));
    let mut z = w.mob(Kind::Zombie, [10.5, f64::from(Y), 2.5], false);
    w.p.s.tick_mob(&mut z);
    assert!(w.block([10, Y, 2]).contains("powered=true"), "zombie : {}", w.block([10, Y, 2]));
}

/// Semelles givrantes : disque de glace givrée (rayon 3 au niveau 1) sur l'eau de source sous
/// le joueur au sol ; immunité aux dégâts `hot_floor`.
#[test]
fn semelles_givrantes_gelent_l_eau() {
    let mut w = world("givre");
    w.p.run(&format!("fill 0 {} 0 15 {} 15 minecraft:water\n", Y - 1, Y - 1));
    w.p.s.slots[8] = Some(boots("minecraft:frost_walker", 1));
    w.player_tick_at(at(8, 8), [0.0; 3]);
    let mut n = 0;
    for x in 0..16 {
        for z in 0..16 {
            let ice = w.block([x, Y - 1, z]).starts_with("minecraft:frosted_ice");
            let (dx, dz) = (f64::from(x) - 8.0, f64::from(z) - 8.0);
            assert_eq!(ice, dx * dx + dz * dz < 9.0, "case {x} {z}");
            n += usize::from(ice);
        }
    }
    assert_eq!(n, 25, "disque de rayon 3");
    // Immunité au sol brûlant avec les bottes, pas sans.
    assert!(!w.p.s.hurt("minecraft:hot_floor", 1.0), "immunisé avec les Semelles givrantes");
    w.p.s.slots[8] = None;
    assert!(w.p.s.hurt("minecraft:hot_floor", 1.0), "dégâts sans les bottes");
}

/// Agilité des âmes : sur le sable des âmes, +0,0405 + 0,0105 par niveau au-delà du premier à la
/// vitesse et efficacité de mouvement 1 ; retirées sur un autre bloc au sol.
#[test]
fn agilite_des_ames_sur_le_sable_des_ames() {
    let mut w = world("ames");
    w.p.run(&format!("setblock 8 {} 8 minecraft:soul_sand\nsetblock 9 {} 8 minecraft:stone\n", Y - 1, Y - 1));
    let base = w.p.s.player_attribute("minecraft:movement_speed");
    w.p.s.slots[8] = Some(boots("minecraft:soul_speed", 3));
    w.player_tick_at(at(8, 8), [0.0; 3]);
    let bonus = f64::from(0.0405f32 + 0.0105f32 * 2.0);
    assert_eq!(w.p.s.player_attribute("minecraft:movement_speed"), base + bonus);
    assert_eq!(w.p.s.player_attribute("minecraft:movement_efficiency"), 1.0);
    w.player_tick_at(at(9, 8), [0.0; 3]);
    assert_eq!(w.p.s.player_attribute("minecraft:movement_speed"), base, "retiré hors du sable des âmes");
    assert_eq!(w.p.s.player_attribute("minecraft:movement_efficiency"), 0.0);
    // Créature chaussée : vitesse augmentée sur le sable des âmes.
    w.p.run(&format!("setblock 2 {} 2 minecraft:soul_sand\n", Y - 1));
    let mut z = w.mob(Kind::Zombie, at(2, 2), false);
    let before = z.attrs.speed.value();
    z.equipment[2] = Some(boots("minecraft:soul_speed", 1));
    w.p.s.tick_mob(&mut z);
    assert_eq!(z.attrs.speed.value(), before + f64::from(0.0405f32));
}

/// Banc (lancé à la main, `--ignored`) : coût par tick du parcours des blocs traversés d'une
/// créature qui marche (0,2 bloc par tick), sans bloc de contact puis dans une toile.
#[test]
#[ignore]
fn banc_contacts_creature() {
    let mut w = world("banc");
    w.p.run(&format!("setblock 12 {Y} 12 minecraft:cobweb\n"));
    for (label, pos) in [("air", at(4, 4)), ("toile", at(12, 12))] {
        let mut z = w.mob(Kind::Zombie, pos, false);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            for _ in 0..100_000 {
                z.old = [pos[0] - 0.2, pos[1], pos[2] - 0.1];
                z.pos = pos;
                w.p.s.mob_contacts(&mut z);
            }
            best = best.min(t.elapsed().as_secs_f64() * 1.0e9 / 100_000.0);
        }
        println!("banc contacts {label} : {best:.1} ns par tick et par créature");
    }
}
