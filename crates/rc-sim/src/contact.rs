//! Contacts des entités avec les blocs qu'elles traversent (G5-F02e) et enchantements des
//! bottes liés au bloc foulé.
//!
//! Blocs traversés, après le déplacement du tick : la boîte de l'entité (rétrécie de 1e-5) est
//! balayée de sa position de début de tick à sa position finale, axe par axe (Y, puis le plus
//! grand de X et Z) pour ce qui se déplace avec collisions, en une seule ligne droite pour les
//! projectiles qui prennent leur position directement (tous sauf la fusée) ; au plus 16
//! itérations, puis la seule boîte d'arrivée ; un effet par case. Effets :
//! - plaques de pression et fil : toute entité (créatures, objets, orbes, projectiles hors œil
//!   de l'Ender, blocs qui tombent, TNT) teste la plaque à 0 ou le fil détendu, comme le joueur,
//!   juste après son propre déplacement ; le fil seulement si la boîte balayée recoupe sa forme
//!   (1 à 2,5 px s'il est accroché, 0 à 8 px sinon) ;
//! - buisson à baies (vivants hors renard et abeille) : enlisement 0,8 × 0,75 × 0,8 et, dès
//!   l'âge 1, 1 dégât `sweet_berry_bush` si le déplacement horizontal du tick dépasse 0,003
//!   sur un axe ;
//! - toile : enlisement 0,25 × 0,05 × 0,25, ou 0,5 × 0,25 × 0,5 sous l'effet Tissage ;
//! - rose de Wither (vivants, hors paisible, entité non invulnérable) : Wither 40 ticks ;
//! - chaudrons, seulement si la boîte touche le contenu (colonne de 12 px de large depuis 4 px
//!   jusqu'au niveau) : lave = dégel, allumage 15 s puis dégâts de lave ; eau et neige
//!   poudreuse = l'entité en feu baisse le niveau (la neige devient de l'eau) puis s'éteint.
//!   Ces effets différés passent après le parcours, dans l'ordre du jeu : dégel, allumage par
//!   la lave (puis ses dégâts), extinction.
//!
//! Bottes : Semelles givrantes (disque de glace givrée sur l'eau, immunité aux dégâts de sol
//! brûlant) et Agilité des âmes (vitesse et efficacité de mouvement sur le sable et la terre
//! des âmes, usure, particules et son), d'après les fichiers d'enchantement du jeu.

use rc_physics::Aabb;

use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;
use crate::server::Server;

type P = [i32; 3];

const NONE: u8 = 0;
pub(crate) const PLATE: u8 = 1;
pub(crate) const TRIPWIRE: u8 = 2;
const BERRY: u8 = 3;
const WEB: u8 = 4;
const WITHER_ROSE: u8 = 5;
const LAVA_CAULDRON: u8 = 6;
const WATER_CAULDRON: u8 = 7;
const SNOW_CAULDRON: u8 = 8;
/// Feu, lave et bouton en bois (enfoncé par les flèches) : effets des projectiles seulement (les
/// vivants passent ailleurs).
pub(crate) const FIRE: u8 = 9;
pub(crate) const LAVA: u8 = 10;
pub(crate) const BUTTON: u8 = 11;

/// Famille de contact d'un bloc (par nom).
fn kind_of(name: &str) -> u8 {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    match n {
        "tripwire" => TRIPWIRE,
        "sweet_berry_bush" => BERRY,
        "cobweb" => WEB,
        "wither_rose" => WITHER_ROSE,
        "lava_cauldron" => LAVA_CAULDRON,
        "water_cauldron" => WATER_CAULDRON,
        "powder_snow_cauldron" => SNOW_CAULDRON,
        "fire" | "soul_fire" => FIRE,
        "lava" => LAVA,
        _ if n.ends_with("pressure_plate") => PLATE,
        _ => NONE,
    }
}

/// Identifiant des modificateurs d'Agilité des âmes (emplacement des pieds).
const SOUL_ID: &str = "minecraft:enchantment.soul_speed/feet";

/// État des enchantements de bottes liés au bloc (joueur ou créature).
#[derive(Debug, Clone, Default)]
pub struct Boots {
    /// Bloc de la dernière évaluation (changement de bloc = nouvelle évaluation).
    last: Option<P>,
    /// Au sol au tick précédent (un atterrissage relance l'évaluation, comme le jeu).
    grounded: bool,
    /// Niveaux lus sur les bottes portées lors de la dernière évaluation.
    soul: i32,
    frost: i32,
    /// Modificateurs d'Agilité des âmes posés.
    pub(crate) soul_active: bool,
    /// Joueur spectateur au tick précédent.
    spectator: bool,
}

/// État des contacts tenu par le serveur.
#[derive(Debug, Default)]
pub struct Contact {
    /// Famille de contact par bloc (construite au premier usage).
    table: Vec<u8>,
    /// Cases du parcours en cours (réutilisées d'un appel à l'autre).
    pub(crate) cells: Vec<Seen>,
    /// Cases déjà entrées pendant ce parcours (un effet par case).
    entered: Vec<P>,
    /// Créature en cours de tick, sortie de la liste : identifiant, boîte, ignore les blocs
    /// déclencheurs (chauve-souris). Comptée par les plaques qu'elle enfonce.
    pub(crate) ticking: Option<(i32, Aabb, bool)>,
    /// Projectile en cours de tick (copie hors de sa case) : identifiant, boîte à sa position
    /// courante ; sa case, encore à l'ancienne position, n'est pas comptée par les plaques.
    pub(crate) ticking_proj: Option<(i32, Aabb)>,
    /// Bloc qui tombe ou TNT en cours de tick, même principe que `ticking_proj`.
    pub(crate) ticking_sim: Option<(i32, Aabb)>,
    /// Pendant le tick de la liste des objets au sol (sortie du serveur) : position et retrait
    /// de chacun, tenus à jour au fil du tick ; vide hors de ce tick. Une plaque touchée compte
    /// ainsi les objets déjà déplacés à leur nouvelle place, les suivants à l'ancienne.
    pub(crate) live_items: Vec<([f64; 3], bool)>,
    pub(crate) items_ticking: bool,
    /// Même chose pour les orbes d'expérience.
    pub(crate) live_orbs: Vec<([f64; 3], bool)>,
    pub(crate) orbs_ticking: bool,
    /// Bottes du joueur.
    pub(crate) player: Boots,
}

/// Effets différés d'un parcours.
#[derive(Default)]
struct Deferred {
    lava: u8,
    quench: Vec<(P, u32)>,
}

/// Une case vue par le parcours d'un déplacement et le tronçon (début, fin) qui l'a vue : le
/// test de forme suit ce tronçon. Une case peut revenir (tronçon suivant, boîte d'arrivée après
/// le plafond) : un effet qui ne se rejoue pas sans dommage se garde des cases déjà entrées.
pub(crate) type Seen = (P, [f64; 3], [f64; 3]);

/// Plafond d'itérations du parcours d'un déplacement, comme le jeu.
const MAX_STEPS: i32 = 16;

/// Trajet par axe dont la somme des trois axes reste sous 7 blocs : un tronçon d'un axe consomme
/// au plus sa longueur + 3 itérations, le plafond est hors d'atteinte.
const SHORT: f64 = 7.0;

/// Ordre des tronçons d'un trajet par axe : Y, puis le plus grand de X et Z (X à égalité).
fn axis_order(d: [f64; 3]) -> [usize; 3] {
    if d[0].abs() < d[2].abs() { [1, 2, 0] } else { [1, 0, 2] }
}

/// Cases touchées par la boîte (demi-largeur `half`, hauteur `h`, rétrécie de 1e-5) déplacée de
/// `from` à `to`, comme le parcours des blocs traversés du jeu : en une ligne droite
/// (`straight`, projectile qui prend sa position sans collision) ou en un tronçon par axe
/// (déplacement avec collisions). Au plus 16 itérations pour tout le trajet ; plafond atteint :
/// la boîte d'arrivée en plus. Trajet par axe court : chaque tronçon couvre exactement son
/// enveloppe, chaque case une fois, testée sur tout le trajet (même résultat, sans parcours).
pub(crate) fn path_cells(from: [f64; 3], to: [f64; 3], half: f64, h: f64, straight: bool, out: &mut Vec<Seen>) {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let budget = if straight {
        MAX_STEPS - walk(from, to, half, h, MAX_STEPS, out)
    } else if d[0].abs() + d[1].abs() + d[2].abs() < SHORT {
        envelope_cells(from, to, half, h, out);
        return;
    } else {
        axis_walks(from, to, half, h, out)
    };
    if budget <= 0 {
        walk(to, to, half, h, 1, out);
    }
}

/// Tronçons d'un axe parcourus un à un sous le plafond commun ; rend le budget restant. Trajet
/// nul : la boîte d'arrivée seule, comme le jeu (ligne droite de longueur nulle).
fn axis_walks(from: [f64; 3], to: [f64; 3], half: f64, h: f64, out: &mut Vec<Seen>) -> i32 {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut budget = MAX_STEPS;
    if d == [0.0; 3] {
        return budget - walk(from, to, half, h, budget, out);
    }
    let mut p = from;
    for a in axis_order(d) {
        if d[a] != 0.0 {
            let mut q = p;
            q[a] = to[a];
            budget -= walk(p, q, half, h, budget, out);
            p = q;
        }
    }
    budget
}

/// Enveloppes des tronçons d'un axe d'un trajet court (un tronçon d'un axe balaie exactement son
/// enveloppe), chaque case une fois, avec le trajet entier comme tronçon de test.
fn envelope_cells(from: [f64; 3], to: [f64; 3], half: f64, h: f64, out: &mut Vec<Seen>) {
    let e = f64::from(1.0e-5f32);
    let push_env = |a: [f64; 3], b: [f64; 3], out: &mut Vec<Seen>| {
        let lo = |i: usize, off: f64| (a[i].min(b[i]) + off + e).floor() as i32;
        let hi = |i: usize, off: f64| (a[i].max(b[i]) + off - e).floor() as i32;
        let (x0, x1) = (lo(0, -half), hi(0, half));
        let (y0, y1) = (lo(1, 0.0), hi(1, h));
        let (z0, z1) = (lo(2, -half), hi(2, half));
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    if !out.iter().any(|s| s.0 == [x, y, z]) {
                        out.push(([x, y, z], from, to));
                    }
                }
            }
        }
    };
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut p = from;
    let mut moved = false;
    for a in axis_order(d) {
        if d[a] != 0.0 {
            let mut q = p;
            q[a] = to[a];
            push_env(p, q, out);
            p = q;
            moved = true;
        }
    }
    if !moved {
        push_env(from, from, out);
    }
}

/// Piste du dédoublonnage par grille de bits dans le parcours (`RUSTCRAFT_OPT_WALKGRID=1`) : sans
/// gain mesuré machine chargée (G5-F02e, docs/perf.md), gardée pour le banc machine calme.
fn walk_grid() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_WALKGRID") == Some("1"))
}

/// Un parcours du jeu de `a` à `b` autour de la boîte d'arrivée rétrécie de 1e-5 : chaque case
/// une fois dans ce parcours, arrêt à la première case nouvelle d'itération `max` ou plus. Rend
/// les itérations consommées (dernière itération vue + 1).
fn walk(a: [f64; 3], b: [f64; 3], half: f64, h: f64, max: i32, out: &mut Vec<Seen>) -> i32 {
    let e = f64::from(1.0e-5f32);
    let at = Aabb {
        min: [b[0] - half + e, b[1] + e, b[2] - half + e],
        max: [b[0] + half - e, b[1] + h - e, b[2] + half - e],
    };
    // Cases déjà vues de ce parcours : recherche dans la liste, ou (`RUSTCRAFT_OPT_WALKGRID=1`) un
    // bit par case de l'enveloppe du trajet quand elle tient en 256 cases.
    let (grid, lo, n) = if walk_grid() {
        let lo = [(a[0].min(b[0]) - half).floor(), a[1].min(b[1]).floor(), (a[2].min(b[2]) - half).floor()];
        let hi = [(a[0].max(b[0]) + half).floor(), (a[1].max(b[1]) + h).floor(), (a[2].max(b[2]) + half).floor()];
        let n = [hi[0] - lo[0] + 1.0, hi[1] - lo[1] + 1.0, hi[2] - lo[2] + 1.0];
        (n[0] * n[1] * n[2] <= 256.0, lo.map(|v| v as i32), n.map(|v| v as i32))
    } else {
        (false, [0; 3], [0; 3])
    };
    let slot = |p: P| -> Option<usize> {
        let r = [p[0] - lo[0], p[1] - lo[1], p[2] - lo[2]];
        (grid && (0..3).all(|i| (0..n[i]).contains(&r[i]))).then(|| ((r[0] * n[1] + r[1]) * n[2] + r[2]) as usize)
    };
    let mut bits = [0u64; 4];
    let start = out.len();
    let mut last = 0;
    crate::mobs::nether::ghast::each_cell_step(a, b, &at, &mut |p, it| {
        let i = slot(p);
        let seen = match i {
            Some(i) => bits[i / 64] >> (i % 64) & 1 == 1,
            None => out[start..].iter().any(|s| s.0 == p),
        };
        if seen {
            return true;
        }
        if it >= max {
            return false;
        }
        if let Some(i) = i {
            bits[i / 64] |= 1 << (i % 64);
        }
        last = it;
        out.push((p, a, b));
        true
    });
    last + 1
}

/// Hauteur du contenu d'un chaudron (en blocs) : lave 15 px, niveaux 6 + 3n px.
fn content_top(kind: u8, level: i32) -> f64 {
    if kind == LAVA_CAULDRON { 15.0 / 16.0 } else { (6.0 + 3.0 * f64::from(level)) / 16.0 }
}

/// La boîte (demi-largeur `half`, hauteur `h`, pieds en `from`) déplacée par axe jusqu'à `to`
/// recoupe la forme `c`, tronçon par tronçon dans l'ordre de `path_cells` (un tronçon suit un
/// seul axe : son enveloppe est la zone balayée), marge de 1e-7 comme le jeu.
pub(crate) fn swept_touches(c: &Aabb, from: [f64; 3], to: [f64; 3], half: f64, h: f64) -> bool {
    let e = 1.0e-7;
    let hits = |a: [f64; 3], b: [f64; 3]| {
        let lo = [a[0].min(b[0]) - half, a[1].min(b[1]), a[2].min(b[2]) - half];
        let hi = [a[0].max(b[0]) + half, a[1].max(b[1]) + h, a[2].max(b[2]) + half];
        (0..3).all(|i| hi[i] > c.min[i] + e && lo[i] < c.max[i] - e)
    };
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut p = from;
    let mut moved = false;
    for a in axis_order(d) {
        if d[a] != 0.0 {
            let mut q = p;
            q[a] = to[a];
            if hits(p, q) {
                return true;
            }
            p = q;
            moved = true;
        }
    }
    !moved && hits(from, from)
}

/// La boîte déplacée de `a` à `b` recoupe la forme `c` : en ligne droite (boîte de départ, non
/// rétrécie, glissée le long du trajet, comme le jeu) ou par tronçons d'un axe.
pub(crate) fn touches(c: &Aabb, a: [f64; 3], b: [f64; 3], half: f64, h: f64, straight: bool) -> bool {
    if !straight {
        return swept_touches(c, a, b, half, h);
    }
    let start = Aabb { min: [a[0] - half, a[1], a[2] - half], max: [a[0] + half, a[1] + h, a[2] + half] };
    let travel = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    crate::mobs::nether::ghast::collided_along(&start, travel, std::slice::from_ref(c))
}

/// Piste « sauter le vide » des projectiles (`RUSTCRAFT_OPT_CONTACTPAL=0` la coupe) : aucun
/// parcours quand les palettes des sections traversées n'ont aucun bloc qui agit sur eux.
pub(crate) fn contact_pal() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_CONTACTPAL") != Some("0"))
}

/// Même relevé pour les créatures (`RUSTCRAFT_OPT_CONTACTPAL_MOBS=1`).
fn contact_pal_mobs() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_CONTACTPAL_MOBS") == Some("1"))
}

/// Familles qui agissent sur une créature.
const MOB_KINDS: u32 = 1 << PLATE
    | 1 << TRIPWIRE
    | 1 << BERRY
    | 1 << WEB
    | 1 << WITHER_ROSE
    | 1 << LAVA_CAULDRON
    | 1 << WATER_CAULDRON
    | 1 << SNOW_CAULDRON;

/// Cases (bornes incluses) de l'enveloppe de la boîte (demi-largeur `half`, hauteur `h`) déplacée
/// de `from` à `to` : tout parcours de ce trajet reste dedans.
pub(crate) fn envelope(from: [f64; 3], to: [f64; 3], half: f64, h: f64) -> (P, P) {
    let lo = [from[0].min(to[0]) - half, from[1].min(to[1]), from[2].min(to[2]) - half];
    let hi = [from[0].max(to[0]) + half, from[1].max(to[1]) + h, from[2].max(to[2]) + half];
    (lo.map(|v| v.floor() as i32), hi.map(|v| v.floor() as i32))
}

/// La boîte balayée touche le contenu du chaudron en `p` (colonne de 12 px depuis 4 px).
fn touches_content(p: P, top: f64, from: [f64; 3], to: [f64; 3], half: f64, h: f64) -> bool {
    let [x, y, z] = p.map(f64::from);
    let c = Aabb { min: [x + 0.125, y + 0.25, z + 0.125], max: [x + 0.875, y + top, z + 0.875] };
    swept_touches(&c, from, to, half, h)
}

impl Server {
    /// Famille de contact par bloc, construite au premier usage.
    fn contact_table(&mut self) -> &[u8] {
        if self.contact.table.is_empty() {
            let kinds = &self.bs.t.kind;
            self.contact.table = (self.data.blocks.iter().enumerate())
                .map(|(i, b)| match kinds.get(i) {
                    Some(crate::blocks::Kind::Button { arrows: true, .. }) => BUTTON,
                    _ => kind_of(&b.name),
                })
                .collect();
        }
        &self.contact.table
    }

    pub(crate) fn contact_kind(&mut self, s: u32) -> u8 {
        let b = self.state_block(s);
        self.contact_table().get(usize::from(b)).copied().unwrap_or(NONE)
    }

    /// Une des sections couvrant les cases `lo..=hi` peut-elle contenir un bloc d'une famille de
    /// `mask` (bit `1 << famille`) ? Lu sur leurs palettes (valeur unique ou liste d'états), sans
    /// lire leurs blocs ; section sans palette (identifiants directs) : peut-être ; tronçon ou
    /// section absents : non (leurs cases se lisent comme de l'air du vide).
    pub(crate) fn contact_maybe(&mut self, lo: P, hi: P, mask: u32) -> bool {
        self.contact_table();
        let (table, states) = (&self.contact.table, &self.data.states);
        let hit = |s: u32| {
            let b = states.get(s as usize).map_or(u16::MAX, |i| i.block);
            table.get(usize::from(b)).is_some_and(|&k| mask >> k & 1 == 1)
        };
        let min_y = self.dim.height.min_y;
        let (sy0, sy1) = ((lo[1] - min_y).max(0) >> 4, (hi[1] - min_y) >> 4);
        for cx in lo[0] >> 4..=hi[0] >> 4 {
            for cz in lo[2] >> 4..=hi[2] >> 4 {
                let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { continue };
                for sy in sy0..=sy1 {
                    let Some(sec) = usize::try_from(sy).ok().and_then(|i| c.sections.get(i)) else { continue };
                    let maybe = match &sec.blocks {
                        rc_world::Container::Single(v) => hit(*v),
                        rc_world::Container::Packed { palette: Some(p), .. } => p.iter().any(|&v| hit(v)),
                        rc_world::Container::Packed { palette: None, .. } => true,
                    };
                    if maybe {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Parcours des cases de la boîte déplacée (cases gardées dans `self.contact.cells`, à y
    /// remettre après usage).
    pub(crate) fn contact_cells(
        &mut self,
        from: [f64; 3],
        to: [f64; 3],
        half: f64,
        h: f64,
        straight: bool,
    ) -> Vec<Seen> {
        let mut cells = std::mem::take(&mut self.contact.cells);
        cells.clear();
        path_cells(from, to, half, h, straight, &mut cells);
        cells
    }

    /// Plaques et fil touchés par une entité qui n'est pas vivante et se déplace avec collisions
    /// (objet, orbe, bloc qui tombe, TNT), juste après son déplacement. Sa position courante doit
    /// être visible du compte des plaques (liste à jour, ou `ticking_sim` / `live_items` /
    /// `live_orbs`). Plaque et fil se rejouent sans effet : pas de garde des cases entrées.
    pub(crate) fn contact_triggers(&mut self, from: [f64; 3], to: [f64; 3], half: f64, h: f64) {
        let cells = self.contact_cells(from, to, half, h, false);
        for &(p, a, b) in &cells {
            match self.contact_kind(self.get(p)) {
                PLATE => self.plate_entity_inside(p),
                TRIPWIRE => self.tripwire_entity_inside(p, false, a, b, half, h, false),
                _ => {}
            }
        }
        self.contact.cells = cells;
    }

    /// Lave en `p` touchée par la boîte déplacée de `a` à `b` : boîte du fluide (hauteur propre,
    /// bloc entier sous de la lave), comme le jeu.
    pub(crate) fn lava_touched(&self, p: P, a: [f64; 3], b: [f64; 3], half: f64, h: f64, straight: bool) -> bool {
        let f = self.fluid_at(p);
        if f.kind != 2 {
            return false;
        }
        let top = if self.fluid_at([p[0], p[1] + 1, p[2]]).kind == 2 { 1.0 } else { f64::from(f.own_height()) };
        let [x, y, z] = p.map(f64::from);
        let c = Aabb { min: [x, y, z], max: [x + 1.0, y + top, z + 1.0] };
        touches(&c, a, b, half, h, straight)
    }

    /// Blocs traversés par une créature pendant son tick (après le déplacement).
    pub(crate) fn mob_contacts(&mut self, m: &mut Mob) {
        if m.removed || !m.alive() || crate::mobs::illagers::no_physics(m) {
            return;
        }
        let (half, h) = (f64::from(m.width()) / 2.0, f64::from(m.height()));
        let (from, to) = (m.old, m.pos);
        if contact_pal_mobs() {
            let (lo, hi) = envelope(from, to, half, h);
            if !self.contact_maybe(lo, hi, MOB_KINDS) {
                return;
            }
        }
        let cells = self.contact_cells(from, to, half, h, false);
        let mut entered = std::mem::take(&mut self.contact.entered);
        entered.clear();
        let ignores = m.kind == Kind::Bat;
        self.contact.ticking = Some((m.id, m.bbox(), ignores));
        let mut fx = Deferred::default();
        for &(p, a, b) in &cells {
            if !m.alive() {
                break;
            }
            let s = self.get(p);
            let k = self.contact_kind(s);
            if k == NONE || entered.contains(&p) {
                continue;
            }
            match k {
                PLATE => self.plate_entity_inside(p),
                TRIPWIRE => self.tripwire_entity_inside(p, ignores, a, b, half, h, false),
                BERRY if !matches!(m.kind, Kind::Fox | Kind::Bee) => {
                    entered.push(p);
                    m.fall_distance = 0.0;
                    m.stuck = [f64::from(0.8f32), 0.75, f64::from(0.8f32)];
                    if self.v().get_int(s, "age") != 0 && berry_moved([from[0] - to[0], from[2] - to[2]]) {
                        self.mob_hurt(m, "minecraft:sweet_berry_bush", 1.0, None, None);
                    }
                }
                WEB => {
                    m.fall_distance = 0.0;
                    let weaving = m.x.effects.iter().any(|e| e.id == "minecraft:weaving");
                    m.stuck = if weaving { [0.5, 0.25, 0.5] } else { [0.25, f64::from(0.05f32), 0.25] };
                }
                WITHER_ROSE if self.level.difficulty_id() != 0 && !m.invulnerable => {
                    entered.push(p);
                    self.give_mob_effect(m, "minecraft:wither", 40, 0);
                }
                LAVA_CAULDRON | WATER_CAULDRON | SNOW_CAULDRON => {
                    let top = content_top(k, self.v().get_int(s, "level"));
                    if touches_content(p, top, a, b, half, h) {
                        entered.push(p);
                        if k == LAVA_CAULDRON {
                            fx.lava += 1;
                        } else {
                            fx.quench.push((p, s));
                        }
                    }
                }
                _ => {}
            }
        }
        self.contact.ticking = None;
        self.contact.cells = cells;
        self.contact.entered = entered;
        if fx.lava > 0 {
            m.ticks_frozen = 0; // dégel
            let immune = m.kind.info().fire_immune || crate::mobs::recent::fire_immune(m);
            if !immune {
                m.fire = m.fire.max(300);
                for _ in 0..fx.lava {
                    if self.mob_hurt(m, "minecraft:lava", 4.0, None, None) && !m.x.silent {
                        let pitch = 2.0 + m.rng.next_float() * 0.4;
                        self.mob_sound(m, "minecraft:entity.generic.burn", 0.4, pitch);
                    }
                }
            }
        }
        if !fx.quench.is_empty() {
            let immune = m.kind.info().fire_immune || crate::mobs::recent::fire_immune(m);
            for &(p, s) in &fx.quench {
                if m.fire > 0 && !immune {
                    self.cauldron_quench(p, s);
                }
            }
            m.fire = m.fire.min(0);
        }
    }

    /// Blocs traversés par le joueur au tick (son mouvement connu ; plaques et fil restent au
    /// mouvement reçu, `player_touch_blocks`).
    pub(crate) fn player_contacts(&mut self) {
        if self.game_mode == 3 || self.life.vit.dead {
            return;
        }
        let (w, h, _) = self.player_dims();
        let k = self.proj_player.known_move;
        let to = self.pos;
        let from = [to[0] - k[0], to[1] - k[1], to[2] - k[2]];
        let half = w / 2.0;
        let cells = self.contact_cells(from, to, half, h, false);
        let mut entered = std::mem::take(&mut self.contact.entered);
        entered.clear();
        let mut fx = Deferred::default();
        for &(p, a, b) in &cells {
            if self.life.vit.dead {
                break;
            }
            let s = self.get(p);
            let kind = self.contact_kind(s);
            if kind == NONE || entered.contains(&p) {
                continue;
            }
            match kind {
                BERRY => {
                    entered.push(p);
                    self.life.vit.fall = 0.0;
                    if self.v().get_int(s, "age") != 0 && berry_moved([k[0], k[2]]) {
                        self.hurt("minecraft:sweet_berry_bush", 1.0);
                    }
                }
                WEB => self.life.vit.fall = 0.0,
                WITHER_ROSE if self.level.difficulty_id() != 0 && !self.abilities().invulnerable => {
                    entered.push(p);
                    self.give_player_effect("minecraft:wither", 40, 0);
                }
                LAVA_CAULDRON | WATER_CAULDRON | SNOW_CAULDRON => {
                    let top = content_top(kind, self.v().get_int(s, "level"));
                    if touches_content(p, top, a, b, half, h) {
                        entered.push(p);
                        if kind == LAVA_CAULDRON {
                            fx.lava += 1;
                        } else {
                            fx.quench.push((p, s));
                        }
                    }
                }
                _ => {}
            }
        }
        self.contact.cells = cells;
        self.contact.entered = entered;
        if fx.lava > 0 {
            self.life.vit.ticks_frozen = 0; // dégel
            self.player_ignite(300);
            for _ in 0..fx.lava {
                if self.hurt("minecraft:lava", 4.0) {
                    let pitch = 2.0 + self.life.rng.next_float() * 0.4;
                    self.play_sound("minecraft:entity.generic.burn", 7, self.pos, 0.4, pitch);
                }
            }
        }
        if !fx.quench.is_empty() {
            for &(p, s) in &fx.quench {
                if self.life.vit.fire > 0 && self.may_interact(p) {
                    self.cauldron_quench(p, s);
                }
            }
            self.life.vit.fire = self.life.vit.fire.min(0);
        }
    }

    /// Entité en feu dans un chaudron d'eau ou de neige poudreuse : un niveau en moins (la neige
    /// devient de l'eau au même niveau avant de baisser).
    fn cauldron_quench(&mut self, p: P, s: u32) {
        let s = if self.contact_kind(s) == SNOW_CAULDRON {
            let w = self.default_state("minecraft:water_cauldron");
            self.v().set_int(w, "level", self.v().get_int(s, "level"))
        } else {
            s
        };
        self.cauldron_lower(p, s);
    }

    // --- Bottes : Semelles givrantes, Agilité des âmes. ---

    /// Immunité des Semelles givrantes : dégâts `burn_from_stepping` qui ne passent pas
    /// l'invulnérabilité.
    pub(crate) fn frost_walker_immune(&self, boots: Option<&crate::player::Stack>, ty: &str) -> bool {
        boots.is_some()
            && self.dmg_tag(ty, "minecraft:burn_from_stepping")
            && !self.dmg_tag(ty, "minecraft:bypasses_invulnerability")
            && boots.is_some_and(|b| self.life.items.enchantment(b, "minecraft:frost_walker") > 0)
    }

    /// Bloc sous les pieds qui règle le mouvement (0,500001 sous les pieds) : sable ou terre
    /// des âmes.
    fn soul_block_below(&self, pos: [f64; 3], support: Option<P>) -> bool {
        let p = self.on_pos(pos, support, 0.500_001);
        self.btag_id(p, crate::tag_bits::TagId::SoulSpeedBlocks)
    }

    /// Disque de glace givrée des Semelles givrantes (rayon 3 + niveau − 1, borné à 16) sous
    /// l'entité : eau de source sous de l'air, sans entité qui gêne.
    fn frost_disk(&mut self, pos: [f64; 3], level: i32, src: Ent) {
        let dist = (3 + level - 1).clamp(0, 16);
        let c = [pos[0].floor() as i32, pos[1].floor() as i32 - 1, pos[2].floor() as i32];
        let ice = self.default_state("minecraft:frosted_ice");
        let r2 = f64::from(dist * dist);
        for z in c[2] - dist..=c[2] + dist {
            for x in c[0] - dist..=c[0] + dist {
                let p = [x, c[1], z];
                let (dx, dz) = (f64::from(x) + 0.5 - pos[0], f64::from(z) + 0.5 - pos[2]);
                if dx * dx + dz * dz >= r2 {
                    continue;
                }
                let s = self.get(p);
                if self.state_name(s) != "minecraft:water" || self.v().get_int(s, "level") != 0 {
                    continue;
                }
                if !matches!(
                    self.bname([x, c[1] + 1, z]),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ) {
                    continue;
                }
                if !self.frost_unobstructed(p) {
                    continue;
                }
                if self.set_update(p, ice) {
                    self.game_event_block("minecraft:block_place", p, Some(src), None);
                }
            }
        }
    }

    /// Aucune entité qui gêne la pose (vivants hors spectateur, blocs qui tombent, TNT,
    /// wagonnets) ne recoupe le cube `p`.
    fn frost_unobstructed(&self, p: P) -> bool {
        let [x, y, z] = p.map(f64::from);
        let b = Aabb { min: [x, y, z], max: [x + 1.0, y + 1.0, z + 1.0] };
        let hit = |o: &Aabb| (0..3).all(|i| o.max[i] > b.min[i] && o.min[i] < b.max[i]);
        if self.entities_in(&b, true, None, false) > 0 {
            return false;
        }
        if self.contact.ticking.as_ref().is_some_and(|t| hit(&t.1)) {
            return false;
        }
        !self.bs.entities.list.iter().any(|e| !e.removed && hit(&Self::entity_box(e)))
            && !self.carts.list.iter().flatten().any(|c| !c.removed && hit(&c.bbox()))
    }

    /// Bottes du joueur, au début de son tick : particules et son d'Agilité des âmes, puis
    /// effets liés au bloc quand il change (ou à l'atterrissage, au changement de bottes, à la
    /// sortie du mode spectateur).
    pub(crate) fn player_boots_tick(&mut self) {
        let spectator = self.game_mode == 3;
        let (soul, frost) = self.slots[8].as_ref().map_or((0, 0), |b| {
            let it = &self.life.items;
            (it.enchantment(b, "minecraft:soul_speed"), it.enchantment(b, "minecraft:frost_walker"))
        });
        let mut st = std::mem::take(&mut self.contact.player);
        // Sans bottes concernées ni effet en cours : rien à évaluer (cas courant, sans requête).
        if soul == 0 && frost == 0 && !st.soul_active && st.soul == 0 && st.frost == 0 {
            st.last = None;
            st.grounded = self.on_ground;
            st.spectator = spectator;
            self.contact.player = st;
            return;
        }
        let flying = self.flying || self.proj_player.fall_flying;
        let support = self.player_support();
        let on_soul = self.soul_block_below(self.pos, support);
        let ground = self.on_ground;
        // Effets de tick (périodiques, toutes les 5 ticks).
        if soul > 0 && !spectator && !self.life.vit.dead {
            let k = self.proj_player.known_move;
            let moving = (k[0] * 20.0).powi(2) + (k[2] * 20.0).powi(2) >= MIN_SPEED * MIN_SPEED;
            let due = !flying && ground && moving && on_soul && self.life.vit.age.is_multiple_of(5);
            if due {
                let w = self.player_dims().0;
                let rx = self.life.rng.next_double();
                let rz = self.life.rng.next_double();
                let at = [self.pos[0] + (rx - 0.5) * w, self.pos[1] + 0.1, self.pos[2] + (rz - 0.5) * w];
                self.particles("minecraft:soul", at, [(k[0] * -0.2) as f32, 0.1, (k[2] * -0.2) as f32], 1.0, 0);
            }
            let chance = self.life.rng.next_float() < 0.35;
            if chance && due {
                let pitch = 0.6 + self.life.rng.next_float() * 0.4;
                self.play_sound("minecraft:particle.soul_escape", 7, self.pos, 0.6, pitch);
            }
        }
        // Évaluation liée au bloc.
        let pos = self.pos.map(|v| v.floor() as i32);
        let changed_boots = soul != st.soul || frost != st.frost;
        if changed_boots && st.soul_active {
            self.player_soul_mods(false, st.soul);
            st.soul_active = false;
        }
        if spectator {
            if st.soul_active {
                self.player_soul_mods(false, st.soul);
                st.soul_active = false;
            }
        } else if !self.life.vit.dead {
            let landed = ground && !st.grounded;
            if st.last != Some(pos) || landed || changed_boots || st.spectator {
                let vehicle = self.player_riding_any();
                if frost > 0 && ground && !vehicle {
                    self.frost_disk(self.pos, frost, Ent::Player);
                }
                if soul > 0 {
                    let want = soul_wanted(st.soul_active, vehicle, flying, on_soul, ground);
                    if want != st.soul_active {
                        self.player_soul_mods(want, soul);
                        st.soul_active = want;
                    }
                    let wear = self.life.rng.next_float() < 0.04 * soul as f32;
                    if wear && ground && on_soul {
                        self.hurt_item(8, 1);
                    }
                }
            }
        }
        st.last = Some(pos);
        st.grounded = ground;
        st.soul = soul;
        st.frost = frost;
        st.spectator = spectator;
        self.contact.player = st;
    }

    /// Modificateurs d'Agilité des âmes du joueur posés ou retirés.
    fn player_soul_mods(&mut self, on: bool, level: i32) {
        let mods = &mut self.life.attrs.mods;
        mods.retain(|m| m.id != SOUL_ID);
        if on {
            for (attr, amount) in
                [("minecraft:movement_speed", soul_speed_bonus(level)), ("minecraft:movement_efficiency", 1.0)]
            {
                mods.push(crate::attrs::EquipMod { attr: attr.to_owned(), id: SOUL_ID.to_owned(), amount, op: 0 });
            }
        }
    }

    /// Effets de tick des bottes d'une créature (début du tick de base).
    pub(crate) fn mob_boots_tick_effects(&mut self, m: &mut Mob) {
        let Some(b) = m.equipment[2].as_ref() else { return };
        let soul = self.life.items.enchantment(b, "minecraft:soul_speed");
        if soul <= 0 {
            return;
        }
        let v = m.vel;
        let moving = (v[0] * 20.0).powi(2) + (v[2] * 20.0).powi(2) >= MIN_SPEED * MIN_SPEED;
        let due = m.on_ground && moving && m.tick_count % 5 == 0 && self.soul_block_below(m.pos, m.support);
        if due {
            let w = f64::from(m.width());
            let rx = m.rng.next_double();
            let rz = m.rng.next_double();
            let at = [m.pos[0] + (rx - 0.5) * w, m.pos[1] + 0.1, m.pos[2] + (rz - 0.5) * w];
            self.particles("minecraft:soul", at, [(v[0] * -0.2) as f32, 0.1, (v[2] * -0.2) as f32], 1.0, 0);
        }
        let chance = self.life.rng.next_float() < 0.35;
        if chance && due && !m.x.silent {
            let pitch = 0.6 + m.rng.next_float() * 0.4;
            self.mob_sound(m, "minecraft:particle.soul_escape", 0.6, pitch);
        }
    }

    /// Effets des bottes d'une créature liés au bloc (après l'air du tick de base).
    pub(crate) fn mob_boots_location(&mut self, m: &mut Mob) {
        let (soul, frost) = m.equipment[2].as_ref().map_or((0, 0), |b| {
            let it = &self.life.items;
            (it.enchantment(b, "minecraft:soul_speed"), it.enchantment(b, "minecraft:frost_walker"))
        });
        let st = m.boots.clone();
        if soul == 0 && frost == 0 && !st.soul_active && st.soul == 0 && st.frost == 0 {
            m.boots.last = None;
            m.boots.grounded = m.on_ground;
            return;
        }
        let pos = m.block_pos();
        let changed_boots = soul != st.soul || frost != st.frost;
        let mut active = st.soul_active;
        if changed_boots && active {
            m.attrs.speed.remove_mod(SOUL_ID);
            active = false;
        }
        let landed = m.on_ground && !st.grounded;
        if m.alive() && (st.last != Some(pos) || landed || changed_boots) {
            let vehicle = m.mount.vehicle.is_some();
            if frost > 0 && m.on_ground && !vehicle {
                self.frost_disk(m.pos, frost, Ent::Mob(m.id));
            }
            if soul > 0 {
                // Le vol plané des créatures n'est pas simulé : jamais « en vol ».
                let flying = false;
                let on_soul = self.soul_block_below(m.pos, m.support);
                let want = soul_wanted(active, vehicle, flying, on_soul, m.on_ground);
                if want && !active {
                    m.attrs.speed.set_mod(SOUL_ID, soul_speed_bonus(soul), 0);
                } else if !want && active {
                    m.attrs.speed.remove_mod(SOUL_ID);
                }
                active = want;
                let wear = self.life.rng.next_float() < 0.04 * soul as f32;
                if wear && m.on_ground && on_soul {
                    self.mob_hurt_boots(m, 1);
                }
            }
        }
        m.boots.soul_active = active;
        m.boots.last = Some(pos);
        m.boots.grounded = m.on_ground;
        m.boots.soul = soul;
        m.boots.frost = frost;
    }

    /// Usure des bottes d'une créature (Solidité des armures) ; cassées : retirées, événement 52.
    fn mob_hurt_boots(&mut self, m: &mut Mob, n: i32) {
        let Some(s) = m.equipment[2].clone() else { return };
        if !self.life.items.damageable(&s) {
            return;
        }
        let n = self.unbreaking_kept(&s, n, true);
        if n <= 0 {
            return;
        }
        let dmg = self.life.items.damage(&s) + n;
        let mut s = s;
        if dmg >= self.life.items.max_damage(&s) {
            s.count -= 1;
            crate::items::remove_component(&mut s, "minecraft:damage");
            m.equipment[2] = (s.count > 0).then_some(s);
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Event { id: m.id, event: 52 }));
        } else {
            crate::items::set_component(&mut s, "minecraft:damage", rc_proto::nbt::Nbt::Int(dmg));
            m.equipment[2] = Some(s);
        }
    }
}

/// Vitesse minimale (blocs par seconde) des effets de tick d'Agilité des âmes.
const MIN_SPEED: f64 = 9.999_999_747_378_752e-6;

/// Bonus de vitesse d'Agilité des âmes : 0,0405 + 0,0105 par niveau au-delà du premier (calcul
/// en simple précision, comme les valeurs par niveau du jeu).
fn soul_speed_bonus(level: i32) -> f64 {
    f64::from(0.0405f32 + 0.0105f32 * (level - 1) as f32)
}

/// Condition des modificateurs d'Agilité des âmes : jamais en véhicule ni en vol ; actifs, ils
/// restent tant que l'entité est sur un bloc des âmes ou en l'air ; inactifs, ils se posent sur
/// un bloc des âmes.
fn soul_wanted(active: bool, vehicle: bool, flying: bool, on_soul: bool, ground: bool) -> bool {
    !vehicle && !flying && (on_soul || (active && !ground))
}

/// Déplacement horizontal du tick assez grand pour que le buisson à baies pique (0,003 sur un
/// axe, valeur simple précision).
fn berry_moved(d: [f64; 2]) -> bool {
    let t = f64::from(0.003f32);
    (d[0] * d[0] + d[1] * d[1]) > 0.0 && (d[0].abs() >= t || d[1].abs() >= t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trajet par axe court (somme des axes sous 7) : les enveloppes donnent exactement les cases
    /// des tronçons parcourus un à un, sans atteindre le plafond, et le même verdict de forme pour
    /// une boîte quelconque dans chaque case. Positions sur la grille du 1/16 une fois sur deux
    /// (entités posées, au sol), un axe immobile une fois sur trois.
    #[test]
    fn enveloppes_egales_au_parcours_pour_un_trajet_court() {
        let mut r = crate::mobs::jrand::JRand::new(crate::rng::test_seed());
        let (mut fast, mut exact) = (Vec::new(), Vec::new());
        let sizes = [(0.125, 0.25), (0.25, 0.5), (0.3, 1.8), (0.49, 0.98), (0.7, 1.95), (1.0, 2.9)];
        for i in 0..20_000 {
            let snap = |v: f64| if i % 2 == 0 { (v * 16.0).round() / 16.0 } else { v };
            let from: [f64; 3] = std::array::from_fn(|_| snap(r.next_double() * 8.0 - 4.0));
            let still = i % 3;
            let d: [f64; 3] = std::array::from_fn(|a| if a == still { 0.0 } else { snap(r.next_double() * 6.8 - 3.4) });
            let to = [from[0] + d[0], from[1] + d[1], from[2] + d[2]];
            let (half, h) = sizes[i % sizes.len()];
            fast.clear();
            exact.clear();
            envelope_cells(from, to, half, h, &mut fast);
            assert!(axis_walks(from, to, half, h, &mut exact) > 0, "plafond atteint : {from:?} -> {to:?}");
            let set = |v: &Vec<Seen>| {
                let mut c: Vec<P> = v.iter().map(|s| s.0).collect();
                c.sort_unstable();
                c.dedup();
                c
            };
            let cells = set(&fast);
            assert_eq!(cells, set(&exact), "{from:?} -> {to:?}, demi-largeur {half}, hauteur {h}");
            for &p in &cells {
                let [x, y, z] = p.map(f64::from);
                let (a, b) = (r.next_double(), r.next_double());
                let c =
                    Aabb { min: [x, y + a * 0.5, z + b * 0.5], max: [x + 1.0, y + a * 0.5 + 0.1, z + 0.5 + b * 0.5] };
                let quick = swept_touches(&c, from, to, half, h);
                let slow = exact.iter().filter(|s| s.0 == p).any(|s| swept_touches(&c, s.1, s.2, half, h));
                assert_eq!(quick, slow, "forme {c:?}, {from:?} -> {to:?}, demi-largeur {half}, hauteur {h}");
            }
        }
    }
}
