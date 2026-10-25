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

/// Projectile ajouté au monde, lancé à `vel`.
fn shoot(w: &mut W, kind: crate::projectiles::PKind, pos: [f64; 3], vel: [f64; 3]) -> i32 {
    let id = w.p.s.proj_new_id();
    let item = Stack { id: kind.default_item().into(), count: 1, components: None };
    let mut p = crate::projectiles::Proj::new(kind, id, pos, item);
    p.vel = vel;
    p.has_been_shot = true;
    w.p.s.proj_add(p);
    id
}

/// Fil : seule la boîte qui recoupe sa forme le tend (1 à 2,5 px accroché, 0 à 8 px sinon).
/// Joueur et créature qui passent au-dessus d'un fil accroché, flèche à 0,6 au-dessus d'un fil
/// libre : rien ; joueur au sol et boule de neige à 0,3 : tendu.
#[test]
fn fil_tendu_seulement_par_sa_forme() {
    use crate::projectiles::PKind;
    let mut w = world("fil-forme");
    w.p.run(&format!(
        "setblock 10 {Y} 10 minecraft:tripwire[attached=true]\nsetblock 14 {Y} 10 minecraft:tripwire[attached=true]\nsetblock 10 {Y} 14 minecraft:tripwire[attached=true]\nsetblock 6 {Y} 14 minecraft:tripwire\nsetblock 2 {Y} 14 minecraft:tripwire\n"
    ));
    let y = f64::from(Y);
    // Joueur qui saute par-dessus (pieds à 0,3, au-dessus des 2,5 px).
    w.p.s.pos = [10.5, y + 0.3, 10.5];
    w.p.s.player_touch_blocks([9.9, y + 0.3, 10.5]);
    assert!(w.block([10, Y, 10]).contains("powered=false"), "joueur au-dessus : {}", w.block([10, Y, 10]));
    // Joueur qui marche dessus : tendu.
    w.p.s.pos = [14.5, y, 10.5];
    w.p.s.player_touch_blocks([13.9, y, 10.5]);
    assert!(w.block([14, Y, 10]).contains("powered=true"), "joueur au sol : {}", w.block([14, Y, 10]));
    w.p.s.pos = [40.5, y, 40.5];
    // Créature qui passe au-dessus du fil accroché.
    let mut z = w.mob(Kind::Zombie, [10.5, y + 0.3, 14.5], true);
    z.on_ground = false;
    z.vel = [0.2, 0.0, 0.0];
    w.p.s.tick_mob(&mut z);
    assert!(z.pos[1] > y + 2.5 / 16.0, "zombie resté au-dessus : {}", z.pos[1]);
    assert!(w.block([10, Y, 14]).contains("powered=false"), "créature au-dessus : {}", w.block([10, Y, 14]));
    // Flèche à hauteur de poitrine (boîte de 0,6 à 1,1) à travers un fil libre (0 à 0,5).
    shoot(&mut w, PKind::Arrow, [5.6, y + 0.6, 14.5], [0.8, 0.0, 0.0]);
    w.p.s.projectiles_tick();
    assert!(w.block([6, Y, 14]).contains("powered=false"), "flèche haute : {}", w.block([6, Y, 14]));
    // Boule de neige basse (objet lancé) à travers un fil libre : tendu.
    shoot(&mut w, PKind::Snowball, [1.6, y + 0.3, 14.5], [0.8, 0.0, 0.0]);
    w.p.s.projectiles_tick();
    assert!(w.block([2, Y, 14]).contains("powered=true"), "boule de neige basse : {}", w.block([2, Y, 14]));
}

/// Boule de neige sur une plaque en bois : elle l'enfonce au tick de son impact, avant d'être
/// retirée (effets des blocs avant l'impact, comme le jeu).
#[test]
fn boule_de_neige_enfonce_la_plaque() {
    use crate::projectiles::PKind;
    let mut w = world("neige-plaque");
    w.p.run(&format!("setblock 2 {Y} 10 minecraft:oak_pressure_plate\n"));
    let id = shoot(&mut w, PKind::Snowball, [2.5, f64::from(Y) + 0.5, 10.5], [0.0, -0.8, 0.0]);
    w.p.s.projectiles_tick();
    assert!(!w.p.s.proj.list.iter().any(|p| p.id == id && !p.removed), "boule de neige retirée à l'impact");
    assert_eq!(w.block([2, Y, 10]), "minecraft:oak_pressure_plate[powered=true]");
}

/// Projectiles : blocs traversés sur une seule ligne droite, du début du tick à la position prise
/// (pas de découpage par axe, propre aux déplacements avec collisions). Fil accroché (1 à 2,5 px).
/// Flèche qui descend après le fil (bas de sa boîte encore à 0,22 au-dessus de lui) : détendu ;
/// boule de neige qui monte en travers : tendu ; flèche en biais à travers un fil à hauteur de
/// tête (case jamais vue par un découpage Y puis X) : tendu.
#[test]
fn fil_tendu_par_les_projectiles_en_ligne_droite() {
    use crate::projectiles::PKind;
    let mut w = world("fil-droit");
    w.p.run(&format!(
        "setblock 3 {Y} 2 minecraft:tripwire[attached=true]\nsetblock 3 {Y} 6 minecraft:tripwire[attached=true]\nsetblock 3 {} 10 minecraft:tripwire[attached=true]\n",
        Y + 1
    ));
    let y = f64::from(Y);
    shoot(&mut w, PKind::Arrow, [2.5, y + 0.5, 2.5], [2.5, -0.4, 0.0]);
    w.p.s.projectiles_tick();
    // Boule de neige : gravité puis inertie avant le pas, qui va de (2,6 ; 0,01) à (4,1 ; 0,41).
    let k = f64::from(0.99f32);
    shoot(&mut w, PKind::Snowball, [2.6, y + 0.01, 6.5], [1.5 / k, 0.4 / k + PKind::Snowball.gravity(), 0.0]);
    w.p.s.projectiles_tick();
    shoot(&mut w, PKind::Arrow, [2.5, y + 1.6, 10.5], [2.5, -1.5, 0.0]);
    w.p.s.projectiles_tick();
    let got = [w.block([3, Y, 2]), w.block([3, Y, 6]), w.block([3, Y + 1, 10])];
    // Flèche qui descend après le fil, boule de neige qui monte, flèche en biais.
    assert_eq!(got.clone().map(|s| s.contains("powered=true")), [false, true, true], "{got:?}");
}

/// Plafond du parcours des blocs traversés (16 itérations pour tout le trajet), puis la seule
/// boîte d'arrivée, comme le jeu ; valeurs calculées à part (hors dépôt) d'après le parcours du
/// jeu, pas d'après ce code. Flèche à 20 blocs par tick au ras du sol, fils libres sur sa
/// ligne en x = 15, 16 et 20 : tendus en 15 et 20. TNT qui file de 9,9 en X puis 9,9 en Z
/// (tronçons par axe), fils libres sur la branche Z de z = 21 à 30 : tendus de 21 à 24 puis en
/// 29 et 30 (boîte d'arrivée).
#[test]
fn plafond_d_iterations_des_blocs_traverses() {
    use crate::projectiles::PKind;
    let mut w = world("plafond");
    let wires = |w: &mut W, cells: &[[i32; 3]]| {
        let cmds: String =
            cells.iter().map(|c| format!("setblock {} {} {} minecraft:tripwire\n", c[0], c[1], c[2])).collect();
        w.p.run(&cmds);
    };
    let powered = |w: &W, cells: &[[i32; 3]]| -> Vec<bool> {
        cells.iter().map(|&c| w.block(c).contains("powered=true")).collect()
    };
    let y = f64::from(Y);
    let line: Vec<[i32; 3]> = [15, 16, 20].iter().map(|&x| [x, Y, 34]).collect();
    wires(&mut w, &line);
    shoot(&mut w, PKind::Arrow, [0.5, y + 0.05, 34.5], [20.0, 0.0, 0.0]);
    w.p.s.projectiles_tick();
    assert_eq!(powered(&w, &line), [true, false, true], "flèche en ligne droite, x = 15, 16, 20");
    let leg: Vec<[i32; 3]> = (21..=30).map(|z| [12, Y, z]).collect();
    wires(&mut w, &leg);
    // Gravité (0,04) avant le déplacement : pas de (9,9 ; 0 ; 9,9).
    w.cmd(&format!("summon minecraft:tnt 2.5 {y} 20.5 {{Motion:[9.9d,0.04d,9.9d]}}"));
    w.p.s.tick_sim_entities();
    let want = [true, true, true, true, false, false, false, false, true, true];
    assert_eq!(powered(&w, &leg), want, "TNT par axe, z = 21 à 30");
}

/// Projectiles : feu et lave traversés en vol, sur la ligne droite du pas (pas seulement à
/// l'arrivée) ; la lave seulement si la boîte touche le fluide (8/9 de bloc pour une source).
/// Flèches à 3 blocs par tick à travers une source de lave puis un feu : enflammées ; flèche qui
/// passe dans la case de lave au-dessus du fluide : non. Le feu est seul dans son tronçon (le
/// relevé des palettes doit le voir sans l'aide de la lave).
#[test]
fn fleches_enflammees_par_la_lave_et_le_feu_traverses() {
    use crate::projectiles::PKind;
    let mut w = world("fleche-feu");
    w.p.run(&format!(
        "setblock 3 {Y} 2 minecraft:lava\nsetblock 3 {Y} 22 minecraft:fire\nsetblock 3 {Y} 10 minecraft:lava\n"
    ));
    let y = f64::from(Y);
    let ids = [
        shoot(&mut w, PKind::Arrow, [2.5, y + 0.2, 2.5], [3.0, 0.0, 0.0]),
        shoot(&mut w, PKind::Arrow, [2.5, y + 0.2, 22.5], [3.0, 0.0, 0.0]),
        shoot(&mut w, PKind::Arrow, [2.5, y + 0.9, 10.5], [3.0, 0.0, 0.0]),
    ];
    w.p.s.projectiles_tick();
    let lit = ids.map(|id| w.p.s.proj.list.iter().find(|p| p.id == id).map(|p| p.fire > 0));
    assert_eq!(lit, [Some(true), Some(true), Some(false)], "lave traversée, feu traversé, au-dessus de la lave");
}

/// Bloc qui tombe et TNT : portails touchés par axe (Y, puis X), comme leur déplacement avec
/// collisions, pas en ligne droite. TNT qui file en biais de (2,5 ; 2) à (3,6 ; 0,9) : la case
/// (3, 2) d'un portail n'est vue qu'en ligne droite, donc pas marquée ; TNT qui tombe dans un
/// portail : marquée.
#[test]
fn portails_touches_par_axe_pour_le_bloc_qui_tombe() {
    let mut w = world("portail-axe");
    let top = Y + 2;
    w.p.run(&format!("setblock 3 {top} 2 minecraft:nether_portal\nsetblock 7 {top} 2 minecraft:nether_portal\n"));
    assert!(w.block([3, Y + 2, 2]).starts_with("minecraft:nether_portal"), "portail posé : {}", w.block([3, Y + 2, 2]));
    let y = f64::from(Y);
    let (from, to) = ([2.5, y + 2.0, 2.5], [3.6, y + 0.9, 2.5]);
    let n = |w: &W, straight: bool| w.p.s.portals_touched_swept(from, to, 0.98, 0.98, straight).len();
    assert_eq!((n(&w, true), n(&w, false)), (1, 0), "ligne droite, par axe");
    // Gravité (0,04) avant le déplacement : pas de (1,1 ; −1,1).
    w.cmd(&format!("summon minecraft:tnt 2.5 {} 2.5 {{Motion:[1.1d,-1.06d,0.0d]}}", y + 2.0));
    w.cmd(&format!("summon minecraft:tnt 7.5 {} 2.5", y + 2.2));
    let ids: Vec<i32> = w.p.s.bs.entities.list.iter().map(|e| e.id).collect();
    w.p.s.tick_sim_entities();
    let marked: Vec<bool> = ids.iter().map(|&id| w.p.s.entity_inside_portal_now(id)).collect();
    assert_eq!(marked, [false, true], "TNT en biais, TNT qui tombe dans le portail");
}

/// Bloc qui tombe et TNT : plaques touchées juste après le déplacement, avant la pose ou le bris
/// et avant la mèche. Sable sans objet lâché (`DropItem:0b`) retiré en arrivant sur une plaque en
/// chêne, TNT éteinte à son dernier tick (`tnt_explodes` faux) : la plaque s'enfonce à ce tick.
#[test]
fn sable_et_tnt_enfoncent_la_plaque_avant_de_disparaitre() {
    let mut w = world("sable-plaque");
    w.p.run(&format!(
        "setblock 10 {Y} 2 minecraft:oak_pressure_plate\nsetblock 14 {Y} 2 minecraft:oak_pressure_plate\n"
    ));
    w.cmd("gamerule tnt_explodes false");
    let y = f64::from(Y);
    w.cmd(&format!(
        "summon minecraft:falling_block 10.5 {} 2.5 {{BlockState:{{Name:\"minecraft:sand\"}},DropItem:0b,Motion:[0.0d,-0.6d,0.0d]}}",
        y + 0.5
    ));
    w.cmd(&format!("summon minecraft:tnt 14.5 {y} 2.5 {{fuse:1}}"));
    w.p.s.tick_sim_entities();
    assert!(w.p.s.bs.entities.list.is_empty(), "sable et TNT retirés à ce tick");
    let pressed = "minecraft:oak_pressure_plate[powered=true]";
    assert_eq!([w.block([10, Y, 2]), w.block([14, Y, 2])], [pressed, pressed], "sable, TNT");
}

/// Plaque pondérée légère : chaque objet la teste juste après son propre déplacement. Le
/// premier (déjà posé) voit le second encore en l'air : signal 1, que le second, arrivé au même
/// tick, ne recalcule pas.
#[test]
fn plaque_ponderee_comptee_objet_par_objet() {
    let mut w = world("ponderee");
    w.p.run(&format!("setblock 6 {Y} 10 minecraft:light_weighted_pressure_plate\n"));
    let y = f64::from(Y);
    for (id, h, vy) in [("minecraft:stick", 0.0, 0.0), ("minecraft:dirt", 0.6, -0.7)] {
        let s = Stack { id: id.into(), count: 1, components: None };
        w.p.s.spawn_item([6.5, y + h, 10.5], [0.0; 3], s, 40);
        let d = w.p.s.life.drops.last_mut().expect("objet");
        d.pos = [6.5, y + h, 10.5];
        d.vel = [0.0, vy, 0.0];
        d.on_ground = h == 0.0;
    }
    w.p.s.drops_tick();
    let n = w.p.s.life.drops.iter().filter(|d| d.pos[1] < y + 0.25).count();
    assert_eq!(n, 2, "les deux objets sur la plaque");
    assert_eq!(w.block([6, Y, 10]), "minecraft:light_weighted_pressure_plate[power=1]");
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

/// Bancs : section riche du tronçon (0, 1) (une quarantaine d'états, aucun bloc de contact).
fn rich_section(w: &mut W) {
    let colors = ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray"];
    let colors = colors.iter().chain(&["light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black"]);
    let flowers = ["dandelion", "poppy", "blue_orchid", "allium", "azure_bluet", "red_tulip", "orange_tulip"];
    let flowers =
        flowers.iter().chain(&["white_tulip", "pink_tulip", "oxeye_daisy", "cornflower", "lily_of_the_valley"]);
    let leaves = ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "azalea", "pale_oak"];
    let mut cmds = String::new();
    for (i, c) in colors.enumerate() {
        cmds += &format!("setblock {} {} 28 minecraft:{c}_wool\n", i % 16, Y + 2);
    }
    for (i, f) in flowers.enumerate() {
        cmds += &format!("setblock {} {Y} 30 minecraft:{f}\n", i);
    }
    for (i, l) in leaves.iter().enumerate() {
        cmds += &format!("setblock {} {} 30 minecraft:{l}_leaves[persistent=true]\n", i, Y + 3);
    }
    w.p.run(&cmds);
}

/// Banc (lancé à la main, `--ignored`) : coût par tick du parcours des blocs traversés d'une
/// créature qui marche (0,2 bloc par tick) : section vide (palette du monde plat), section
/// riche, plein air dans la section d'une toile, puis dans la toile.
#[test]
#[ignore]
fn banc_contacts_creature() {
    let mut w = world("banc");
    w.p.run(&format!("setblock 12 {Y} 12 minecraft:cobweb\n"));
    rich_section(&mut w);
    for (label, pos) in [("vide", at(4, 36)), ("riche", at(4, 20)), ("air", at(4, 4)), ("toile", at(12, 12))] {
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

/// Banc (lancé à la main, `--ignored`) : coût d'un pas des effets des blocs d'une flèche qui
/// file en biais (2,5 blocs par tick, 0,4 de descente) : section vide (palette du monde plat),
/// section riche (une quarantaine d'états, aucun bloc de contact), plein air dans la section
/// d'une rangée de plaques, puis au ras des plaques (boîte au-dessus de leur zone de contact :
/// rien ne change).
#[test]
#[ignore]
fn banc_contacts_projectile() {
    use crate::projectiles::{PKind, Proj};
    let mut w = world("banc-proj");
    w.p.run(&format!("fill 0 {Y} 10 15 {Y} 10 minecraft:oak_pressure_plate\n"));
    rich_section(&mut w);
    let y = f64::from(Y);
    for (label, z, h) in [("vide", 36.5, 1.6), ("riche", 20.5, 1.6), ("air", 4.5, 1.6), ("plaques", 10.5, 0.9)] {
        let item = Stack { id: PKind::Arrow.default_item().into(), count: 1, components: None };
        let mut p = Proj::new(PKind::Arrow, w.p.s.proj_new_id(), [5.5, y + h - 0.4, z], item);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            for _ in 0..100_000 {
                p.pos = [5.5, y + h - 0.4, z];
                w.p.s.proj_blocks_effects_from(&mut p, [3.0, y + h, z]);
            }
            best = best.min(t.elapsed().as_secs_f64() * 1.0e9 / 100_000.0);
        }
        assert!(w.block([5, Y, 10]).contains("powered=false"), "banc : plaque restée haute");
        println!("banc projectile {label} : {best:.1} ns par pas");
    }
}
