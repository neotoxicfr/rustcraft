//! Contacts des entités avec les blocs qu'elles traversent (G5-F02e) et enchantements des
//! bottes liés au bloc foulé.
//!
//! Blocs traversés, après le déplacement du tick : la boîte de l'entité (rétrécie de 1e-5) est
//! balayée de sa position de début de tick à sa position finale, axe par axe (Y, puis le plus
//! grand de X et Z) ; chaque case n'est vue qu'une fois. Effets :
//! - plaques de pression et fil : toute entité (créatures, objets, orbes, projectiles, blocs
//!   qui tombent, TNT) teste la plaque à 0 ou le fil détendu, comme le joueur ;
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
const PLATE: u8 = 1;
const TRIPWIRE: u8 = 2;
const BERRY: u8 = 3;
const WEB: u8 = 4;
const WITHER_ROSE: u8 = 5;
const LAVA_CAULDRON: u8 = 6;
const WATER_CAULDRON: u8 = 7;
const SNOW_CAULDRON: u8 = 8;

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
    cells: Vec<P>,
    /// Créature en cours de tick, sortie de la liste : identifiant, boîte, ignore les blocs
    /// déclencheurs (chauve-souris). Comptée par les plaques qu'elle enfonce.
    pub(crate) ticking: Option<(i32, Aabb, bool)>,
    /// Parcours différés (objets, orbes) le temps que leur liste soit remise en place.
    pending: Vec<([f64; 3], [f64; 3], f64, f64)>,
    /// Bottes du joueur.
    pub(crate) player: Boots,
}

/// Effets différés d'un parcours.
#[derive(Default)]
struct Deferred {
    lava: u8,
    quench: Vec<(P, u32)>,
}

/// Cases touchées par la boîte (demi-largeur `half`, hauteur `h`, rétrécie de 1e-5) déplacée de
/// `from` à `to`, axe par axe : Y, puis le plus grand de X et Z. Chaque segment suit un seul axe,
/// son enveloppe est exactement la zone balayée. Un saut de plus de 8 blocs (téléportation) ne
/// garde que l'arrivée, comme le plafond d'itérations du jeu.
pub(crate) fn swept_cells(from: [f64; 3], to: [f64; 3], half: f64, h: f64, out: &mut Vec<P>) {
    let e = f64::from(1.0e-5f32);
    let push_env = |a: [f64; 3], b: [f64; 3], out: &mut Vec<P>| {
        let lo = |i: usize, off: f64| (a[i].min(b[i]) + off + e).floor() as i32;
        let hi = |i: usize, off: f64| (a[i].max(b[i]) + off - e).floor() as i32;
        let (x0, x1) = (lo(0, -half), hi(0, half));
        let (y0, y1) = (lo(1, 0.0), hi(1, h));
        let (z0, z1) = (lo(2, -half), hi(2, half));
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    if !out.contains(&[x, y, z]) {
                        out.push([x, y, z]);
                    }
                }
            }
        }
    };
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    if d.iter().any(|v| v.abs() > 8.0) {
        push_env(to, to, out);
        return;
    }
    let order = if d[0].abs() < d[2].abs() { [1, 2, 0] } else { [1, 0, 2] };
    let mut p = from;
    let mut moved = false;
    for a in order {
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

/// Hauteur du contenu d'un chaudron (en blocs) : lave 15 px, niveaux 6 + 3n px.
fn content_top(kind: u8, level: i32) -> f64 {
    if kind == LAVA_CAULDRON { 15.0 / 16.0 } else { (6.0 + 3.0 * f64::from(level)) / 16.0 }
}

/// La boîte balayée touche le contenu du chaudron en `p` (colonne de 12 px depuis 4 px).
fn touches_content(p: P, top: f64, from: [f64; 3], to: [f64; 3], half: f64, h: f64) -> bool {
    let [x, y, z] = p.map(f64::from);
    let c = Aabb { min: [x + 0.125, y + 0.25, z + 0.125], max: [x + 0.875, y + top, z + 0.875] };
    let b = Aabb {
        min: [from[0].min(to[0]) - half, from[1].min(to[1]), from[2].min(to[2]) - half],
        max: [from[0].max(to[0]) + half, from[1].max(to[1]) + h, from[2].max(to[2]) + half],
    };
    (0..3).all(|i| b.max[i] > c.min[i] && b.min[i] < c.max[i])
}

impl Server {
    fn contact_kind(&mut self, s: u32) -> u8 {
        if self.contact.table.is_empty() {
            self.contact.table = self.data.blocks.iter().map(|b| kind_of(&b.name)).collect();
        }
        let b = self.v().block(s);
        self.contact.table.get(usize::from(b)).copied().unwrap_or(NONE)
    }

    /// Parcours des cases de la boîte déplacée (cases gardées dans `self.contact.cells`).
    fn contact_cells(&mut self, from: [f64; 3], to: [f64; 3], half: f64, h: f64) -> Vec<P> {
        let mut cells = std::mem::take(&mut self.contact.cells);
        cells.clear();
        swept_cells(from, to, half, h, &mut cells);
        cells
    }

    /// Plaques et fil touchés par une entité qui n'est pas vivante (objet, orbe, projectile,
    /// bloc qui tombe, TNT) déjà remise dans sa liste.
    pub(crate) fn contact_triggers(&mut self, from: [f64; 3], to: [f64; 3], half: f64, h: f64) {
        let cells = self.contact_cells(from, to, half, h);
        for &p in &cells {
            match self.contact_kind(self.get(p)) {
                PLATE => self.plate_entity_inside(p),
                TRIPWIRE => self.tripwire_entity_inside(p, false),
                _ => {}
            }
        }
        self.contact.cells = cells;
    }

    /// Même chose, différé jusqu'à `flush_contact_triggers` (liste de l'entité sortie).
    pub(crate) fn defer_contact_triggers(&mut self, from: [f64; 3], to: [f64; 3], half: f64, h: f64) {
        self.contact.pending.push((from, to, half, h));
    }

    pub(crate) fn flush_contact_triggers(&mut self) {
        if self.contact.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.contact.pending);
        for &(from, to, half, h) in &pending {
            self.contact_triggers(from, to, half, h);
        }
        let mut pending = pending;
        pending.clear();
        self.contact.pending = pending;
    }

    /// Blocs traversés par une créature pendant son tick (après le déplacement).
    pub(crate) fn mob_contacts(&mut self, m: &mut Mob) {
        if m.removed || !m.alive() || crate::mobs::illagers::no_physics(m) {
            return;
        }
        let (half, h) = (f64::from(m.width()) / 2.0, f64::from(m.height()));
        let (from, to) = (m.old, m.pos);
        let cells = self.contact_cells(from, to, half, h);
        let ignores = m.kind == Kind::Bat;
        self.contact.ticking = Some((m.id, m.bbox(), ignores));
        let mut fx = Deferred::default();
        for &p in &cells {
            if !m.alive() {
                break;
            }
            let s = self.get(p);
            match self.contact_kind(s) {
                PLATE => self.plate_entity_inside(p),
                TRIPWIRE => self.tripwire_entity_inside(p, ignores),
                BERRY if !matches!(m.kind, Kind::Fox | Kind::Bee) => {
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
                    self.give_mob_effect(m, "minecraft:wither", 40, 0);
                }
                k @ (LAVA_CAULDRON | WATER_CAULDRON | SNOW_CAULDRON) => {
                    let top = content_top(k, self.v().get_int(s, "level"));
                    if touches_content(p, top, from, to, half, h) {
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
        let cells = self.contact_cells(from, to, half, h);
        let mut fx = Deferred::default();
        for &p in &cells {
            if self.life.vit.dead {
                break;
            }
            let s = self.get(p);
            match self.contact_kind(s) {
                BERRY => {
                    self.life.vit.fall = 0.0;
                    if self.v().get_int(s, "age") != 0 && berry_moved([k[0], k[2]]) {
                        self.hurt("minecraft:sweet_berry_bush", 1.0);
                    }
                }
                WEB => self.life.vit.fall = 0.0,
                WITHER_ROSE if self.level.difficulty_id() != 0 && !self.abilities().invulnerable => {
                    self.give_player_effect("minecraft:wither", 40, 0);
                }
                k @ (LAVA_CAULDRON | WATER_CAULDRON | SNOW_CAULDRON) => {
                    let top = content_top(k, self.v().get_int(s, "level"));
                    if touches_content(p, top, from, to, half, h) {
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
        self.contact.cells = cells;
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
