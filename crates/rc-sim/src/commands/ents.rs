//! Uniform access to singleplayer entities for commands: player, simulated mobs (S4),
//! dropped items and orbs (S2), other entities read from the world (frozen, NBT only).

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_text::{Component, HoverEvent};

use crate::nbtio;
use crate::server::Server;

#[cfg(test)]
thread_local! {
    /// Tests: entity display names built by this thread (`ent_display`), to prove that a
    /// silent function names nobody.
    pub(crate) static DISPLAY_NAMES_BUILT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Designated entity (network identifier, except the player).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum E {
    /// The installed player: the host, or the LAN guest whose state is installed.
    Player,
    Mob(i32),
    Item(i32),
    Orb(i32),
    Other(i32),
    /// Another player of the world open to the local network, by its entity identifier
    /// (`other_players.rs`).
    OtherPlayer(i32),
}

impl E {
    /// A player of the list, installed or not.
    pub fn is_player(self) -> bool {
        matches!(self, E::Player | E::OtherPlayer(_))
    }
}

/// Active effect: identifier (`minecraft:speed`), amplifier, duration (−1 infinite), flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub id: String,
    pub amplifier: i32,
    pub duration: i32,
    pub ambient: bool,
    pub particles: bool,
    pub icon: bool,
}

impl Effect {
    pub fn to_nbt(&self) -> Nbt {
        Nbt::Compound(vec![
            ("id".into(), Nbt::String(self.id.clone())),
            ("amplifier".into(), Nbt::Byte(self.amplifier as i8)),
            ("duration".into(), Nbt::Int(self.duration)),
            ("ambient".into(), Nbt::Byte(i8::from(self.ambient))),
            ("show_particles".into(), Nbt::Byte(i8::from(self.particles))),
            ("show_icon".into(), Nbt::Byte(i8::from(self.icon))),
        ])
    }

    pub fn from_nbt(v: &Nbt) -> Option<Effect> {
        let b = |k: &str, d: bool| nbtio::int(v, k).map_or(d, |x| x != 0);
        Some(Effect {
            id: v.get("id")?.as_str()?.to_owned(),
            amplifier: nbtio::int(v, "amplifier").unwrap_or(0) as i32,
            duration: nbtio::int(v, "duration").unwrap_or(0) as i32,
            ambient: b("ambient", false),
            particles: b("show_particles", true),
            icon: b("show_icon", true),
        })
    }
}

impl Server {
    /// All living entities of the open world, in order of addition (identifiers).
    pub fn all_ents(&self) -> Vec<E> {
        let mut out = Vec::new();
        if !self.life.vit.dead {
            out.push(E::Player);
        }
        let mut rest: Vec<E> = Vec::new();
        rest.extend(self.mobs.list.iter().filter(|m| m.alive()).map(|m| E::Mob(m.id)));
        rest.extend(self.life.drops.iter().map(|d| E::Item(d.id)));
        rest.extend(self.life.orbs.iter().map(|o| E::Orb(o.id)));
        for l in self.entities.values() {
            rest.extend(l.iter().map(|(id, _)| E::Other(*id)));
        }
        // F15: crystals, clouds, dragon balls, shulker bullets.
        rest.extend(self.mobs.end.snapshots.iter().map(|(id, _)| E::Other(*id)));
        // G5-F06: minecarts.
        rest.extend(self.carts.list.iter().flatten().filter(|c| !c.removed).map(|c| E::Other(c.id)));
        rest.sort_by_key(|e| self.ent_id(*e));
        out.extend(rest);
        out
    }

    pub fn ent_id(&self, e: E) -> i32 {
        match e {
            E::Player => self.player_id,
            E::Mob(i) | E::Item(i) | E::Orb(i) | E::Other(i) | E::OtherPlayer(i) => i,
        }
    }

    pub(crate) fn other(&self, id: i32) -> Option<&Nbt> {
        self.kept_entity_slot(id)
            .map(|(c, k)| &self.entities[&c][k].1)
            .or_else(|| self.end_other(id)) // F15
            .or_else(|| self.cart_ref(id).map(|c| &c.nbt)) // G5-F06: fields kept (name, tags)
            // L10-COMBAT: the entity exploding right now, out of its list.
            .or_else(|| self.proj.blast_ent.as_ref().filter(|(i, _)| *i == id).map(|(_, n)| n))
    }

    /// Compound of `id` for a change. A kept compound's chunk is marked so the next save
    /// rewrites it (otherwise `/data`, `/tag` or `/tp` on it would be lost at reload).
    fn other_mut(&mut self, id: i32) -> Option<&mut Nbt> {
        if self.cart_ref(id).is_some() {
            return self.cart_mut(id).map(|c| &mut c.nbt); // G5-F06: saved with the live minecarts
        }
        let (chunk, k) = self.kept_entity_slot(id)?;
        self.life.drop_chunks.insert(chunk);
        self.entities.get_mut(&chunk).map(|l| &mut l[k].1)
    }

    /// Chunk whose list holds the kept compound `id`, and its index there.
    pub(crate) fn kept_entity_slot(&self, id: i32) -> Option<((i32, i32), usize)> {
        self.kept_index.find(&self.entities, id)
    }

    /// After a kept compound's position changed: when it now lies in another chunk, its entry
    /// moves to that chunk's list, as the game files an entity under the chunk of its position
    /// (saved there, shown and forgotten with it). Both chunks are rewritten at the next save,
    /// and clients see it leave or enter the chunks they were sent. A destination never read
    /// keeps nothing (its file would be overwritten without its own entities): the entry stays.
    pub(crate) fn refile_kept_entity(&mut self, id: i32) -> KeptEntityMove {
        let Some((from, k)) = self.kept_entity_slot(id) else { return KeptEntityMove::SameChunk };
        let n = &self.entities[&from][k].1;
        let to = crate::decoration::chunk_of(nbtio::f64s::<3>(n.get("Pos")).unwrap_or_default());
        if to == from || !self.chunks.contains_key(&to) {
            return KeptEntityMove::SameChunk;
        }
        let (_, n) = self.entities.get_mut(&from).expect("entry found above").remove(k);
        let visible = crate::decoration::client_visible(&n);
        let list = self.entities.entry(to).or_default();
        list.push((id, n));
        self.kept_index.placed(id, to, list.len() - 1);
        self.life.drop_chunks.extend([from, to]);
        self.deco_moved_to_chunk(id, to);
        let (seen_before, seen_now) = (self.sent.contains(&from), self.sent.contains(&to));
        if !visible || seen_before == seen_now {
            return if seen_before && seen_now { KeptEntityMove::InView } else { KeptEntityMove::OutOfView };
        }
        if seen_before {
            let _ = self.send(Event::Entity(EntityEvent::Remove(vec![id])));
        } else if let Some(n) = self.other(id).cloned() {
            self.deco_announce_tracked(id, &n);
        }
        KeptEntityMove::ViewChanged
    }

    /// G5-F06: designated minecart.
    pub(crate) fn cart_of(&self, e: E) -> Option<&crate::minecart::Cart> {
        match e {
            E::Other(i) => self.cart_ref(i),
            _ => None,
        }
    }

    /// Whether `e` stands in the world the server works in (the player always does); a parked
    /// world answers through the same lists test (`entity_worlds.rs`).
    pub fn ent_exists(&self, e: E) -> bool {
        match e {
            E::Player => true,
            // Like the installed player, another player in game stands wherever a command looks.
            E::OtherPlayer(id) => self.other_player(id).is_some(),
            _ => self.entity_lists().holds(e),
        }
    }

    pub fn ent_type(&self, e: E) -> String {
        if let Some(c) = self.cart_of(e) {
            return c.kind.name().to_owned();
        }
        match e {
            E::Player | E::OtherPlayer(_) => "minecraft:player".into(),
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or_else(String::new, |m| m.kind.info().name.to_owned()),
            E::Item(_) => "minecraft:item".into(),
            E::Orb(_) => "minecraft:experience_orb".into(),
            E::Other(i) => self.other(i).and_then(|n| n.get("id")?.as_str().map(str::to_owned)).unwrap_or_default(),
        }
    }

    pub fn e_pos(&self, e: E) -> [f64; 3] {
        if let Some(c) = self.cart_of(e) {
            return c.pos;
        }
        match e {
            E::Player => self.pos,
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or([0.0; 3], |m| m.pos()),
            E::Item(i) => self.life.drops.iter().find(|d| d.id == i).map_or([0.0; 3], |d| d.pos),
            E::Orb(i) => self.life.orbs.iter().find(|d| d.id == i).map_or([0.0; 3], |d| d.pos),
            E::Other(i) => self.other(i).and_then(|n| nbtio::f64s::<3>(n.get("Pos"))).unwrap_or_default(),
            E::OtherPlayer(id) => self.other_player(id).map_or([0.0; 3], |c| c.pos),
        }
    }

    /// Yaw, pitch.
    pub fn ent_rot(&self, e: E) -> [f32; 2] {
        if let Some(c) = self.cart_of(e) {
            return [c.yaw, c.pitch];
        }
        match e {
            E::Player => [self.yaw, self.pitch],
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or([0.0; 2], |m| [m.yaw, m.pitch]),
            E::Item(_) | E::Orb(_) => [0.0; 2],
            E::Other(i) => self.other(i).and_then(|n| nbtio::f32s::<2>(n.get("Rotation"))).unwrap_or_default(),
            E::OtherPlayer(id) => self.other_player(id).map_or([0.0; 2], |c| [c.yaw, c.pitch]),
        }
    }

    pub fn ent_uuid(&self, e: E) -> u128 {
        if let Some(c) = self.cart_of(e) {
            return c.uuid;
        }
        match e {
            E::Player => self.uuid,
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or(0, |m| m.uuid),
            E::Item(i) | E::Orb(i) => crate::drops::ground_uuid(i),
            E::Other(i) => self.other(i).and_then(|n| nbtio::uuid(n.get("UUID"))).unwrap_or(0),
            E::OtherPlayer(id) => self.other_player(id).map_or(0, |c| c.uuid),
        }
    }

    /// Width, height, eye height (version data, mobs: babies included).
    pub fn ent_dims(&self, e: E) -> [f32; 3] {
        match e {
            E::Player => {
                let (w, h, eye) = self.player_dims();
                [w as f32, h as f32, eye as f32]
            }
            E::OtherPlayer(id) => {
                let (w, h, eye) = self.other_player(id).map_or((0.6, 1.8, 1.62), crate::mobs::guest_dims);
                [w as f32, h as f32, eye as f32]
            }
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or([0.6, 1.8, 1.62], |m| {
                let (w, h, eye) = crate::mobs::species::dims(m.kind, m.is_baby());
                [w, h, eye]
            }),
            _ => {
                // Armor stand, mannequin: marker, small stand, pose and scale (L10-COMBAT).
                if let E::Other(i) = e
                    && let Some(n) = self.other(i)
                    && let Some(k @ (crate::decoration::DKind::Stand | crate::decoration::DKind::Mannequin)) =
                        crate::decoration::kind_of(n)
                {
                    let (w, h, eye) = self.living_size(n, k == crate::decoration::DKind::Stand);
                    return [w as f32, h as f32, eye as f32];
                }
                self.type_dims(&self.ent_type(e)).unwrap_or([0.25, 0.25, 0.2])
            }
        }
    }

    /// Width, height and eye height of entity type `ty` (version data).
    pub(crate) fn type_dims(&self, ty: &str) -> Option<[f32; 3]> {
        // Through the registry's name index (`RUSTCRAFT_OPT_NAMEINDEX`), not a walk of its entries.
        let i = self.data.entity_types()?.id(ty)?;
        self.data.entity_dims.get(i as usize).map(|d| [d[0], d[1], d[2]])
    }

    pub fn ent_eye_height(&self, e: E) -> f64 {
        f64::from(self.ent_dims(e)[2])
    }

    pub fn e_bbox(&self, e: E) -> rc_physics::Aabb {
        // Painting and frames: box according to direction and size; small stand or marker.
        if let E::Other(i) = e
            && let Some(b) = self.other(i).and_then(|n| self.deco_box(n))
        {
            return b;
        }
        // Another player: the box of its pose, at its scale.
        if let E::OtherPlayer(id) = e
            && let Some(c) = self.other_player(id)
        {
            return crate::mobs::guest_box(c);
        }
        let p = self.e_pos(e);
        let [w, h, _] = self.ent_dims(e);
        let hw = f64::from(w) / 2.0;
        rc_physics::Aabb { min: [p[0] - hw, p[1], p[2] - hw], max: [p[0] + hw, p[1] + f64::from(h), p[2] + hw] }
    }

    /// Full NBT of the entity (the writing without identifier + `id`, except the player).
    pub fn ent_nbt(&self, e: E) -> Nbt {
        if let Some(c) = self.cart_of(e) {
            return self.cart_to_nbt(c);
        }
        match e {
            E::Player => {
                let mut n = self.player_nbt();
                nbtio::remove(&mut n, "DataVersion");
                n
            }
            E::Mob(i) => self.mobs.mob_ref_any(i).map_or(Nbt::Compound(Vec::new()), |m| self.mob_nbt(m)),
            // L10-ENT: the compound the save writes (one writer for both).
            E::Item(i) => {
                self.life.drops.iter().find(|d| d.id == i).map_or(Nbt::Compound(Vec::new()), |d| self.item_ent_nbt(d))
            }
            E::Orb(i) => self.life.orbs.iter().find(|o| o.id == i).map_or(Nbt::Compound(Vec::new()), |o| self.orb_ent_nbt(o)),
            E::Other(i) => self.other(i).cloned().unwrap_or(Nbt::Compound(Vec::new())),
            // Written from the server's own fields: read with that player installed
            // (`Server::readable_nbt`); here only what its kept data holds.
            E::OtherPlayer(id) => self.other_player(id).map_or(Nbt::Compound(Vec::new()), |c| c.pdata.root.clone()),
        }
    }

    /// What the `nbt` predicate and the text components read of entity `e`; another player is
    /// read with it installed (its data is written from the server's own fields).
    pub(crate) fn readable_nbt(&mut self, e: E) -> Nbt {
        match e {
            E::OtherPlayer(_) => {
                self.on_player(e, |sv| sv.w_nbt(E::Player.into())).unwrap_or_else(|| Nbt::Compound(Vec::new()))
            }
            _ => self.w_nbt(e.into()),
        }
    }

    /// Replaces the NBT of an entity (`data merge`); the player is refused by the caller.
    pub fn ent_set_nbt(&mut self, e: E, n: &Nbt) {
        // G5-F06: minecart read back (`load`), identifier and UUID kept.
        if let E::Other(i) = e
            && let Some((k, mut c)) = self.cart_take(i)
        {
            let mut full = n.clone();
            nbtio::set(&mut full, "UUID", nbtio::uuid_nbt(c.uuid));
            nbtio::remove(&mut full, "Passengers");
            c.load(self, &full);
            self.cart_put(k, c);
            return;
        }
        match e {
            E::Player => {
                // Only the player's tags and effects can be modified.
                if let Some(t) = n.get("Tags") {
                    nbtio::set(&mut self.pdata.root, "Tags", t.clone());
                }
            }
            E::OtherPlayer(_) => {
                let _ = self.on_player(e, |sv| sv.ent_set_nbt(E::Player, n));
            }
            E::Mob(i) => {
                if let Some(idx) = self.mobs.index_of(i)
                    && let Some(mut m) = self.mobs.list.take(idx)
                {
                    let before = m.pos();
                    // Reading back keeps the entity's UUID (a provided `UUID` is ignored).
                    let uuid = m.uuid;
                    self.load_mob(&mut m, n);
                    m.uuid = uuid;
                    if m.pos() != before {
                        m.needs_sync = true;
                    }
                    m.sync_data();
                    self.mobs.list.put(idx, m);
                }
            }
            E::Item(i) => {
                // L10-ENT: read back like the game's load (common part included).
                if let Some(k) = self.life.drops.iter().position(|d| d.id == i) {
                    self.item_read_nbt(k, n);
                    let d = &mut self.life.drops[k];
                    if let Some(s) = n.get("Item").and_then(crate::player::Stack::from_nbt) {
                        d.stack = s;
                    }
                    if let Some(p) = nbtio::f64s::<3>(n.get("Pos")) {
                        d.pos = p;
                    }
                }
            }
            E::Orb(i) => {
                if let Some(k) = self.life.orbs.iter().position(|o| o.id == i) {
                    self.orb_read_nbt(k, n); // L10-ENT
                }
            }
            E::Other(i) => {
                let mut new = n.clone();
                let mut old = None;
                if let Some(o) = self.other_mut(i) {
                    // Identifier and UUID kept (`load` does not change them).
                    for k in ["id", "UUID"] {
                        if let Some(v) = o.get(k).cloned() {
                            nbtio::set(&mut new, k, v);
                        }
                    }
                    old = Some(std::mem::replace(o, new.clone()));
                }
                // A compound without a UUID takes the one provided.
                if let Some(o) = &old
                    && nbtio::uuid(o.get("UUID")) != nbtio::uuid(new.get("UUID"))
                {
                    self.kept_index.uuid_changed();
                }
                // G5-F05: the reloaded text display resolves its text again (previous one kept
                // on failure); own data of displays and interactions sent when changed.
                if let Some(o) = &old
                    && crate::display::kind_of(o).is_some()
                {
                    let at = nbtio::f64s::<3>(new.get("Pos")).unwrap_or_default();
                    self.display_resolve_text(i, at, o.get("text").cloned());
                    new = self.other(i).cloned().unwrap_or(new);
                }
                // The game's reading sets the position: a new `Pos` files it under its new chunk.
                // After the text resolution, so that an entity entering the view is announced
                // with the resolved text, as the game reads the whole compound before showing it.
                let moved = self.refile_kept_entity(i);
                // Left the view (removed) or entered it (announced with its data): nothing more.
                if matches!(moved, KeptEntityMove::SameChunk | KeptEntityMove::InView) {
                    self.send_entity_meta_after(i, &new, old.as_ref());
                }
            }
        }
    }

    /// Tags (`Tags`).
    pub fn ent_tags(&self, e: E) -> Vec<String> {
        let n = match e {
            E::Player => self.pdata.root.get("Tags").cloned(),
            E::OtherPlayer(id) => self.other_player(id).and_then(|c| c.pdata.root.get("Tags").cloned()),
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| m.nbt.as_ref()?.get("Tags").cloned()),
            E::Other(i) => self.other(i).and_then(|n| n.get("Tags").cloned()),
            _ => None,
        };
        nbtio::list(n.as_ref()).iter().filter_map(|t| t.as_str().map(str::to_owned)).collect()
    }

    pub fn ent_set_tags(&mut self, e: E, tags: &[String]) {
        let v = Nbt::List(tags.iter().map(|t| Nbt::String(t.clone())).collect());
        match e {
            E::Player => nbtio::set(&mut self.pdata.root, "Tags", v),
            E::Mob(i) => {
                if let Some(m) = self.mobs.get_mut(i) {
                    let n = m.nbt.get_or_insert_with(|| Nbt::Compound(Vec::new()));
                    nbtio::set(n, "Tags", v);
                }
            }
            E::Other(i) => {
                if let Some(o) = self.other_mut(i) {
                    nbtio::set(o, "Tags", v);
                }
            }
            E::OtherPlayer(_) => {
                let _ = self.on_player(e, |sv| sv.ent_set_tags(E::Player, tags));
            }
            _ => {}
        }
    }

    /// Profile name of a player of the list.
    fn player_name_of(&self, e: E) -> Option<&str> {
        match e {
            E::Player => Some(&self.name),
            E::OtherPlayer(id) => self.other_player(id).map(|c| c.name.as_str()),
            _ => None,
        }
    }

    /// Score holder name: the player's name, otherwise the UUID as text.
    pub fn ent_score_name(&self, e: E) -> String {
        match self.player_name_of(e) {
            Some(name) => name.to_owned(),
            None => nbtio::uuid_text(self.ent_uuid(e)),
        }
    }

    /// Team of an entity, like the team of its [`Server::ent_score_name`], without formatting
    /// its UUID (found by UUID).
    pub fn ent_team(&self, e: E) -> Option<&super::scores::Team> {
        match self.player_name_of(e) {
            Some(name) => self.cmd.scores.team_of(name),
            None => self.cmd.scores.team_of_uuid(self.ent_uuid(e)),
        }
    }

    /// Raw name (without style): custom name or translated name of the type.
    pub fn ent_custom_name(&self, e: E) -> Option<Component> {
        let n = match e {
            E::Player | E::OtherPlayer(_) => return None,
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| m.custom_name.clone()),
            E::Other(i) => self.other(i).and_then(|n| n.get("CustomName").cloned()),
            E::Item(_) | E::Orb(_) => None,
        };
        n.as_ref().and_then(rc_text::from_nbt)
    }

    /// Simple name: player, custom name, carried item, or translated type.
    pub fn ent_name(&self, e: E) -> Component {
        if let Some(name) = self.player_name_of(e) {
            return Component::text(name.to_owned());
        }
        if let Some(c) = self.ent_custom_name(e) {
            return c;
        }
        if let E::Item(i) = e
            && let Some(d) = self.life.drops.iter().find(|d| d.id == i)
        {
            return self.stack_name(&d.stack);
        }
        let ty = self.ent_type(e);
        Component::translate(format!("entity.{}", ty.replace(':', ".")), Vec::new())
    }

    /// Displayed name: team, "entity" hover, insertion (UUID or name),
    /// "/tell" click for a player.
    pub fn ent_display(&self, e: E) -> Component {
        #[cfg(test)]
        DISPLAY_NAMES_BUILT.with(|n| n.set(n.get() + 1));
        let base = self.ent_name(e);
        let score_name = self.ent_score_name(e);
        let mut c = self.cmd.scores.decorate(&score_name, base.clone());
        c.style.hover = Some(std::sync::Arc::new(HoverEvent::Entity {
            id: self.ent_type(e),
            uuid: self.ent_uuid(e),
            name: Some(base),
        }));
        if e.is_player() {
            super::text::suggest_click(&mut c, format!("/tell {score_name} "));
        }
        // The scoreboard name is the insertion text (player name, or the UUID's text formatted once).
        c.style.insertion = Some(std::sync::Arc::from(score_name));
        c
    }

    pub(crate) fn player_display_name(&self) -> Component {
        self.ent_display(E::Player)
    }

    /// Teleportation of an entity (position, rotation).
    pub fn ent_teleport(&mut self, e: E, pos: [f64; 3], rot: Option<[f32; 2]>) {
        if let E::Other(i) = e
            && let Some(c) = self.cart_mut(i)
        {
            c.pos = pos;
            c.old_pos = pos;
            if let Some([y, p]) = rot {
                c.yaw = y;
                c.pitch = p;
            }
            c.needs_sync = true;
            return;
        }
        match e {
            E::Player => {
                if let Some([y, p]) = rot {
                    self.yaw = y;
                    self.pitch = p;
                }
                self.teleport_to(pos);
                self.update_tracking();
            }
            E::OtherPlayer(_) => {
                let _ = self.on_player(e, |sv| sv.ent_teleport(E::Player, pos, rot));
            }
            E::Mob(i) => {
                if let Some(m) = self.mobs.get_mut(i) {
                    m.set_pos(pos);
                    m.old = pos;
                    m.inside.clear(); // like the game: nothing is swept until arrival
                    if let Some([y, p]) = rot {
                        m.yaw = y;
                        m.pitch = p;
                        m.head_yaw = y;
                        m.body_yaw = y;
                    }
                    m.needs_sync = true;
                }
            }
            E::Item(i) => {
                if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == i) {
                    d.pos = pos;
                }
            }
            E::Orb(i) => {
                if let Some(d) = self.life.orbs.iter_mut().find(|d| d.id == i) {
                    d.pos = pos;
                }
            }
            E::Other(i) => {
                if let Some(o) = self.other_mut(i) {
                    nbtio::set(o, "Pos", Nbt::List(pos.iter().map(|&x| Nbt::Double(x)).collect()));
                    if let Some([y, p]) = rot {
                        nbtio::set(o, "Rotation", Nbt::List(vec![Nbt::Float(y), Nbt::Float(p)]));
                    }
                }
                let moved = self.refile_kept_entity(i);
                if matches!(moved, KeptEntityMove::OutOfView | KeptEntityMove::ViewChanged) {
                    return;
                }
                // A marker is unknown to the clients (G5-F05).
                if self.other(i).is_some_and(|o| !crate::decoration::client_visible(o)) {
                    return;
                }
                let [yaw, pitch] = rot.unwrap_or([0.0, 0.0]);
                let _ = self.send(Event::Entity(EntityEvent::Teleport(rc_proto_v777::entity::TeleportEntity {
                    id: i,
                    pos,
                    velocity: [0.0; 3],
                    yaw,
                    pitch,
                    relative: 0,
                    on_ground: false,
                })));
            }
        }
    }

    /// Teleportation by a command (`tp`), like the game: `~`/`^` axes and absent or
    /// relative rotation sent to the player as relative flags (its momentum is kept on those axes, its view
    /// is not realigned), then vertical speed zero and on the ground (except when gliding),
    /// navigation stopped. `rot`: rotation resolved against the source, `None` = unchanged.
    pub fn command_teleport(&mut self, e: E, pos: [f64; 3], rel: [bool; 3], rot: Option<[f32; 2]>, rot_rel: [bool; 2]) {
        use crate::mobs::mth::wrap;
        use rc_proto_v777::play::relative as r;
        if let E::OtherPlayer(_) = e {
            let _ = self.on_player(e, |sv| sv.command_teleport(E::Player, pos, rel, rot, rot_rel));
            return;
        }
        if e == E::Player {
            self.player_remove_vehicle(); // No longer a passenger
        }
        let cur = self.e_pos(e);
        let [cy, cp] = self.ent_rot(e);
        let [ty, tp] = rot.unwrap_or([cy, cp]);
        // Values sent: difference for a relative axis, absolute otherwise (rotations wrapped).
        let sent: [f64; 3] = std::array::from_fn(|k| if rel[k] { pos[k] - cur[k] } else { pos[k] });
        let ry = wrap(if rot_rel[0] { ty - cy } else { ty });
        let rp = wrap(if rot_rel[1] { tp - cp } else { tp });
        let abs: [f64; 3] = std::array::from_fn(|k| if rel[k] { cur[k] + sent[k] } else { sent[k] });
        let fy = if rot_rel[0] { cy + ry } else { ry };
        let fp = (if rot_rel[1] { cp + rp } else { rp }).clamp(-90.0, 90.0);
        if e == E::Player {
            let mut flags = 0;
            for (k, (p, d)) in [(r::X, r::DELTA_X), (r::Y, r::DELTA_Y), (r::Z, r::DELTA_Z)].into_iter().enumerate() {
                if rel[k] {
                    flags |= p | d;
                }
            }
            if rot_rel[0] {
                flags |= r::YAW;
            }
            if rot_rel[1] {
                flags |= r::PITCH;
            }
            self.yaw = fy;
            self.pitch = fp;
            self.begin_teleport(abs);
            let _ = self.send(Event::Teleport(rc_proto_v777::play::PlayerPosition {
                id: self.teleport_id,
                pos: sent,
                delta: [0.0; 3],
                yaw: ry,
                pitch: rp,
                relative: flags,
            }));
            self.update_tracking();
        } else {
            self.ent_teleport(e, abs, Some([fy, fp]));
        }
    }

    /// `rotate <target> <rotation>`: rotation resolved against the source, applied relative or
    /// absolute per component (pitch clamped to ±90); the player only receives a rotation
    /// packet (its speed and position do not move).
    pub fn command_rotate(&mut self, e: E, rot: [f32; 2], rel: [bool; 2]) {
        if let E::OtherPlayer(_) = e {
            let _ = self.on_player(e, |sv| sv.command_rotate(E::Player, rot, rel));
            return;
        }
        let [cy, cp] = self.ent_rot(e);
        let sy = if rel[0] { rot[0] - cy } else { rot[0] };
        let sp = if rel[1] { rot[1] - cp } else { rot[1] };
        let fy = if rel[0] { cy + sy } else { sy };
        let fp = (if rel[1] { cp + sp } else { sp }).clamp(-90.0, 90.0);
        if e == E::Player {
            self.yaw = fy;
            self.pitch = fp;
            let _ =
                self.send(Event::PlayerRotation { yaw: sy, yaw_relative: rel[0], pitch: sp, pitch_relative: rel[1] });
        } else if let E::Mob(i) = e {
            self.mob_rotate(i, fy, fp, false);
        } else {
            let pos = self.e_pos(e);
            self.ent_teleport(e, pos, Some([fy, fp]));
        }
    }

    /// Rotation alone of a mob (`rotate`, directed look), like the game: yaw, head, pitch and
    /// previous values; the look also aligns the body on the head. Neither the position, nor the
    /// old position, nor the tick's movements (read back by the crossed-block pass), nor the path
    /// move, and nothing is forced at send time: the rotation goes out with the tracking.
    fn mob_rotate(&mut self, i: i32, yaw: f32, pitch: f32, look: bool) {
        let Some(m) = self.mobs.get_mut(i) else { return };
        m.yaw = yaw;
        m.head_yaw = yaw;
        m.pitch = pitch;
        m.pitch_o = pitch;
        if look {
            m.head_yaw_o = yaw;
            m.body_yaw = yaw;
        }
    }

    /// After a command teleportation: the game restarts from a zero speed and keeps that
    /// of the relative axes (`rel`) only on those, then cancels the vertical and puts the entity on
    /// the ground, except for a living entity gliding; the path of a mob is dropped
    /// (`tp` only: `spreadplayers` keeps it).
    pub fn after_command_teleport(&mut self, e: E, rel: [bool; 3]) {
        if let E::Other(i) = e
            && let Some(c) = self.cart_mut(i)
        {
            c.vel[1] = 0.0;
            zero_absolute(&mut c.vel, rel);
            c.on_ground = true;
            return;
        }
        match e {
            E::Player => {
                if !self.proj_player.fall_flying {
                    self.on_ground = true;
                }
            }
            E::OtherPlayer(_) => {
                let _ = self.on_player(e, |sv| sv.after_command_teleport(E::Player, rel));
            }
            E::Mob(i) => {
                if let Some(m) = self.mobs.get_mut(i) {
                    m.vel[1] = 0.0;
                    zero_absolute(&mut m.vel, rel);
                    m.on_ground = true;
                    m.nav.stop();
                }
            }
            E::Item(i) => {
                if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == i) {
                    d.vel[1] = 0.0;
                    zero_absolute(&mut d.vel, rel);
                    d.on_ground = true;
                }
            }
            E::Orb(i) => {
                if let Some(d) = self.life.orbs.iter_mut().find(|d| d.id == i) {
                    d.vel[1] = 0.0;
                    zero_absolute(&mut d.vel, rel);
                }
            }
            E::Other(i) => {
                if let Some(o) = self.other_mut(i) {
                    if let Some(Nbt::List(m)) = o.get("Motion").cloned()
                        && m.len() == 3
                    {
                        let keep = |k: usize| if rel[k] { m[k].clone() } else { Nbt::Double(0.0) };
                        nbtio::set(o, "Motion", Nbt::List(vec![keep(0), Nbt::Double(0.0), keep(2)]));
                    }
                    nbtio::set(o, "OnGround", Nbt::Byte(1));
                }
            }
        }
    }

    /// Look of an entity towards a point from its feet or its eyes (anchor of the
    /// source); the player receives the look packet (point, or entity and its anchor).
    pub fn command_look_at(&mut self, e: E, from_eyes: bool, t: [f64; 3], entity: Option<(E, bool)>) {
        if let E::OtherPlayer(_) = e {
            let entity = entity.map(|(x, eyes)| (self.seen_from_player(x, e), eyes));
            let _ = self.on_player(e, |sv| sv.command_look_at(E::Player, from_eyes, t, entity));
            return;
        }
        let mut from = self.e_pos(e);
        if from_eyes {
            from[1] += self.ent_eye_height(e);
        }
        let (dx, dy, dz) = (t[0] - from[0], t[1] - from[1], t[2] - from[2]);
        let sd = (dx * dx + dz * dz).sqrt();
        // Degrees like the game: fast arctangent (table), `× 180 / (float)π`, rounded in
        // float.
        use crate::mobs::mth::atan2;
        let deg = |a: f64| (a * 180.0 / f64::from(std::f32::consts::PI)) as f32;
        let pitch = crate::mobs::mth::wrap(-deg(atan2(dy, sd)));
        let yaw = crate::mobs::mth::wrap(deg(atan2(dz, dx)) - 90.0);
        if e == E::Player {
            let entity = entity.map(|(x, eyes)| (self.ent_id(x), eyes));
            let _ = self.send(Event::LookAt { eyes: from_eyes, target: t, entity });
            self.yaw = yaw;
            self.pitch = pitch;
        } else if let E::Mob(i) = e {
            self.mob_rotate(i, yaw, pitch, true);
        } else {
            let pos = self.e_pos(e);
            self.ent_teleport(e, pos, Some([yaw, pitch]));
        }
    }

    /// `kill`: infinite "generic kill" damage for living entities, removal for the rest.
    pub fn ent_kill(&mut self, e: E) {
        match e {
            // F15: the dragon is removed on the spot (victory of the fight).
            E::Mob(i) if self.end_kill_mob(i) => {}
            E::Player => {
                self.hurt_player_from_command(|sv| sv.hurt("minecraft:generic_kill", f32::MAX));
            }
            E::OtherPlayer(_) => {
                let _ = self.on_player(e, |sv| sv.ent_kill(E::Player));
            }
            E::Mob(i) => {
                if let Some(idx) = self.mobs.index_of(i)
                    && let Some(mut m) = self.mobs.list.take(idx)
                {
                    self.mob_hurt(&mut m, "minecraft:generic_kill", f32::MAX, None, None);
                    self.mobs.list.put(idx, m);
                }
            }
            // During the entity walk (a command block minecart's command) the lists keep their
            // slots: the item or orb is marked removed and leaves with the walk's removals.
            E::Item(i) if self.walk.on() => {
                if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == i && !d.is_removed()) {
                    d.mark_removed();
                }
            }
            E::Orb(i) if self.walk.on() => {
                if let Some(o) = self.life.orbs.iter_mut().find(|o| o.id == i && !o.is_removed()) {
                    o.mark_removed();
                }
            }
            E::Item(i) => {
                self.entity_scores_removed(self.ent_uuid(e));
                self.life.drops.retain(|d| d.id != i);
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![i])));
            }
            E::Orb(i) => {
                self.entity_scores_removed(self.ent_uuid(e));
                self.life.orbs.retain(|d| d.id != i);
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![i])));
            }
            // F15: crystal (fight notified), cloud, ball, bullet.
            E::Other(i) if self.end_kill_other(i) => {}
            // G5-F06: minecart killed (the contents drop), removed on the next minecart tick.
            E::Other(i) if self.cart_ref(i).is_some() => {
                if let Some((k, mut c)) = self.cart_take(i) {
                    self.cart_kill(&mut c);
                    self.cart_put(k, c);
                }
            }
            E::Other(i) => {
                let uuid = self.ent_uuid(e);
                self.entity_scores_removed(uuid);
                // A marker was never announced: no removal either (G5-F05).
                let visible = self.other(i).is_none_or(crate::decoration::client_visible);
                let held_by = self.kept_entity_slot(i);
                // Only the chunk that held it changes (one counter numbers every entity, so no
                // other list holds the identifier): rewriting every entity chunk of the dimension
                // at the next save wrote the same contents again.
                if kill_rewrites_one_chunk() {
                    if let Some((c, k)) = held_by {
                        self.entities.get_mut(&c).expect("slot found above").remove(k);
                        self.life.drop_chunks.insert(c);
                    }
                    #[cfg(test)]
                    assert!(
                        super::kept_entity_index::scan(&self.entities, i).is_none(),
                        "kept compound {i} listed twice"
                    );
                } else {
                    for l in self.entities.values_mut() {
                        l.retain(|(x, _)| *x != i);
                    }
                    self.life.drop_chunks.extend(self.entities.keys().copied().collect::<Vec<_>>());
                }
                self.kept_index.forget(i, uuid);
                self.deco_forget(i);
                if visible {
                    let _ = self.send(Event::Entity(EntityEvent::Remove(vec![i])));
                }
            }
        }
    }

    /// Alive in the sense of the `@e` selectors.
    pub fn e_alive(&self, e: E) -> bool {
        match e {
            E::Player => !self.life.vit.dead,
            E::OtherPlayer(id) => self.other_player(id).is_some_and(|c| !c.vit.dead),
            _ => self.ent_exists(e),
        }
    }

    /// Living entity: player, mobs, others according to their NBT.
    pub fn ent_is_living(&self, e: E) -> bool {
        match e {
            E::Player | E::OtherPlayer(_) | E::Mob(_) => true,
            E::Item(_) | E::Orb(_) => false,
            // Living by its type (the game's rule; an armor stand read from an old compound may
            // lack its living fields), otherwise by its health.
            E::Other(i) => {
                matches!(self.ent_type(e).as_str(), "minecraft:armor_stand" | "minecraft:mannequin")
                    || self.other(i).is_some_and(|n| n.get("Health").is_some())
            }
        }
    }

    /// Entity from its UUID: the first of [`Server::all_ents`] carrying it (the player, then the
    /// smallest identifier). Looked up family by family through the indexes, without listing and
    /// sorting every entity (`RUSTCRAFT_OPT_MOBINDEX=0`: [`Server::ent_by_uuid_reference`]).
    pub fn ent_by_uuid(&self, u: u128) -> Option<E> {
        if !crate::mobs::index::on() {
            return self.ent_by_uuid_reference(u);
        }
        let found = self.ent_by_uuid_direct(u);
        #[cfg(test)]
        if crate::mobs::index::verify() {
            assert_eq!(found, self.ent_by_uuid_reference(u), "entity by UUID {u:x}");
        }
        found
    }

    /// Old path (and test reference): every living entity listed and sorted, then the first
    /// carrying `u`.
    pub(crate) fn ent_by_uuid_reference(&self, u: u128) -> Option<E> {
        self.all_ents().into_iter().find(|e| self.ent_uuid(*e) == u).or_else(|| self.living_other_player_with_uuid(u))
    }

    /// Another living player of the list whose profile is `u` (a LAN player stands in the world
    /// like the installed one).
    fn living_other_player_with_uuid(&self, u: u128) -> Option<E> {
        let e = self.player_with_uuid(u).filter(|e| *e != E::Player)?;
        self.e_alive(e).then_some(e)
    }

    fn ent_by_uuid_direct(&self, u: u128) -> Option<E> {
        // Many kept compounds have no UUID (read as 0) and the mob index never records 0.
        if u == 0 {
            return self.ent_by_uuid_reference(u);
        }
        if !self.life.vit.dead && self.uuid == u {
            return Some(E::Player);
        }
        if let Some(e) = self.living_other_player_with_uuid(u) {
            return Some(e);
        }
        // Smallest identifier over the families; on a tie the earlier family, like the stable
        // sort of `all_ents` (families pushed in this order).
        let mut best: Option<(i32, E)> = None;
        let mut offer = |id: i32, e: E| {
            if best.is_none_or(|(b, _)| id < b) {
                best = Some((id, e));
            }
        };
        if let Some(m) = self.mobs.alive_with_uuid(u) {
            offer(m.id, E::Mob(m.id));
        }
        // Ground items and orbs carry a UUID made from their identifier.
        if let Some(id) = crate::drops::ground_id_of_uuid(u) {
            if self.life.drops.iter().any(|d| d.id == id) {
                offer(id, E::Item(id));
            }
            if self.life.orbs.iter().any(|o| o.id == id) {
                offer(id, E::Orb(id));
            }
        }
        if let Some(id) = self.kept_index.find_uuid(&self.entities, u) {
            offer(id, E::Other(id));
        }
        for (id, n) in &self.mobs.end.snapshots {
            if nbtio::uuid(n.get("UUID")) == Some(u) {
                offer(*id, E::Other(*id));
            }
        }
        for c in self.carts.list.iter().flatten().filter(|c| !c.removed && c.uuid == u) {
            offer(c.id, E::Other(c.id));
        }
        best.map(|(_, e)| e)
    }

    /// Name of a stack (custom name or item name).
    pub fn stack_name(&self, s: &crate::player::Stack) -> Component {
        if let Some(n) = crate::items::component(s, "minecraft:custom_name").and_then(rc_text::from_nbt) {
            return n;
        }
        if let Some(n) = crate::items::component(s, "minecraft:item_name").and_then(rc_text::from_nbt) {
            return n;
        }
        // The item's first registry entry through the name index; an index past the item table
        // finds nothing, as the former walk of both lists side by side did.
        let key = self
            .data
            .items()
            .and_then(|r| r.id(&s.id))
            .and_then(|i| self.data.items.get(i as usize))
            .map(|i| i.name_key.clone())
            .unwrap_or_else(|| format!("item.{}", s.id.replace(':', ".")));
        Component::translate(key, Vec::new())
    }
}

impl crate::mobs::Mobs {
    /// Mob from its network identifier: the first slot carrying it, dead mobs included, like the
    /// former scan of the list, through the identifier index (`RUSTCRAFT_OPT_MOBINDEX=0`: the scan).
    pub fn mob_ref_any(&self, id: i32) -> Option<&crate::mobs::mob::Mob> {
        self.get(id)
    }

    /// Living mob carrying UUID `u` with the smallest identifier: the first one `all_ents` lists.
    /// Through the UUID index while no UUID repeats in the list (at most one slot then carries
    /// `u`); otherwise every mob is looked at.
    fn alive_with_uuid(&self, u: u128) -> Option<&crate::mobs::mob::Mob> {
        if self.index.unique_uuids() {
            return self.find_uuid_where(u, crate::mobs::mob::Mob::alive);
        }
        self.list.iter().filter(|m| m.uuid == u && m.alive()).min_by_key(|m| m.id)
    }
}

/// `kill` of a kept compound marks only the chunk that held it for the next save
/// (`RUSTCRAFT_OPT_KILL_REWRITES_ONE_CHUNK=0`: every entity chunk of the dimension, the former way).
fn kill_rewrites_one_chunk() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_KILL_REWRITES_ONE_CHUNK.with(std::cell::Cell::get) {
        return on;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::generation::opt_switch(&ON, "RUSTCRAFT_OPT_KILL_REWRITES_ONE_CHUNK", true)
}

#[cfg(test)]
thread_local! {
    /// Tests: [`kill_rewrites_one_chunk`] forced on this thread (both paths compared).
    pub(crate) static TEST_KILL_REWRITES_ONE_CHUNK: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

/// What [`Server::refile_kept_entity`] did, for a caller that sends the move itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeptEntityMove {
    /// Still filed under the same chunk (or a destination never read).
    SameChunk,
    /// Moved between two chunks the clients were sent: they follow the move.
    InView,
    /// Moved between chunks the clients do not see, or a marker (never shown) leaving or
    /// entering their view.
    OutOfView,
    /// Left or entered the clients' view: removal or announcement already sent.
    ViewChanged,
}

/// Resets the horizontal speed of the absolute axes of a command teleportation.
fn zero_absolute(vel: &mut [f64; 3], rel: [bool; 3]) {
    for k in [0, 2] {
        if !rel[k] {
            vel[k] = 0.0;
        }
    }
}
