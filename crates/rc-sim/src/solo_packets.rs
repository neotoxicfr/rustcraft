//! Client packets specific to the integrated server (W8a): difficulty and its lock, game
//! rules (read and write), signs (editor on placement, on use, received text, applicators),
//! books (saving, signing, reading a written book), item pick on an entity,
//! spectator teleport and data requests (F3+I).

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;
use rc_proto::{BlockPos, Decode, Reader};

use crate::commands::ents::E;
use crate::items::{component, remove_component, set_component};
use crate::player::Stack;
use crate::server::Server;

/// Game rule: short identifier, integer (bounds) or boolean.
struct Rule {
    name: &'static str,
    int: Option<(i32, i32)>,
}

const fn b(name: &'static str) -> Rule {
    Rule { name, int: None }
}

const fn i(name: &'static str, min: i32, max: i32) -> Rule {
    Rule { name, int: Some((min, max)) }
}

/// Rules available in a world without experimentation (26.3 registry).
const RULES: &[Rule] = &[
    b("advance_time"),
    b("advance_weather"),
    b("allow_entering_nether_using_portals"),
    b("block_drops"),
    b("block_explosion_drop_decay"),
    b("command_blocks_work"),
    b("command_block_output"),
    b("drowning_damage"),
    b("elytra_movement_check"),
    b("ender_pearls_vanish_on_death"),
    b("entity_drops"),
    b("fall_damage"),
    b("fire_damage"),
    i("fire_spread_radius_around_player", -1, i32::MAX),
    b("forgive_dead_players"),
    b("freeze_damage"),
    b("global_sound_events"),
    b("immediate_respawn"),
    b("keep_inventory"),
    b("lava_source_conversion"),
    b("limited_crafting"),
    b("locator_bar"),
    b("log_admin_commands"),
    i("max_block_modifications", 1, i32::MAX),
    i("max_command_forks", 0, i32::MAX),
    i("max_command_sequence_length", 0, i32::MAX),
    i("max_entity_cramming", 0, i32::MAX),
    i("max_snow_accumulation_height", 0, 8),
    b("mob_drops"),
    b("mob_explosion_drop_decay"),
    b("mob_griefing"),
    b("natural_health_regeneration"),
    b("player_movement_check"),
    i("players_nether_portal_creative_delay", 0, i32::MAX),
    i("players_nether_portal_default_delay", 0, i32::MAX),
    i("players_sleeping_percentage", 0, i32::MAX),
    b("projectiles_can_break_blocks"),
    b("pvp"),
    b("raids"),
    i("random_tick_speed", 0, i32::MAX),
    b("reduced_debug_info"),
    i("respawn_radius", 0, i32::MAX),
    b("send_command_feedback"),
    b("show_advancement_messages"),
    b("show_death_messages"),
    b("spawner_blocks_work"),
    b("spawn_mobs"),
    b("spawn_monsters"),
    b("spawn_patrols"),
    b("spawn_phantoms"),
    b("spawn_wandering_traders"),
    b("spawn_wardens"),
    b("spectators_generate_chunks"),
    b("spread_vines"),
    b("tnt_explodes"),
    b("tnt_explosion_drop_decay"),
    b("universal_anger"),
    b("water_source_conversion"),
];

/// Reading a rule value like the game's command argument: exact `true`/`false`,
/// integer within its bounds, nothing after.
fn parse_rule(r: &Rule, v: &str) -> Option<Nbt> {
    match r.int {
        None => match v {
            "true" => Some(Nbt::Byte(1)),
            "false" => Some(Nbt::Byte(0)),
            _ => None,
        },
        Some((min, max)) => {
            if v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit() || c == b'-' || c == b'.') {
                return None;
            }
            let n: i32 = v.parse().ok()?;
            (min..=max).contains(&n).then_some(Nbt::Int(n))
        }
    }
}

/// Removes the formatting codes `§x` (colors, styles, reset).
pub(crate) fn strip_formatting(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '§'
            && let Some(&n) = it.peek()
            && matches!(n.to_ascii_lowercase(), '0'..='9' | 'a'..='f' | 'k'..='o' | 'r')
        {
            it.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// Text of a sign face: raw message, with no content other than plain text?
fn plain_message(m: &Nbt) -> bool {
    match m {
        Nbt::String(_) => true,
        Nbt::Compound(f) => !f
            .iter()
            .any(|(k, _)| matches!(k.as_str(), "translate" | "score" | "selector" | "keybind" | "nbt" | "object")),
        Nbt::List(_) => false,
        _ => true,
    }
}

fn message_is_empty(m: &Nbt) -> bool {
    match m {
        Nbt::String(s) => s.is_empty(),
        Nbt::Compound(f) => f.iter().all(|(k, v)| match k.as_str() {
            "text" => v.as_str() == Some(""),
            "type" => v.as_str() == Some("text"),
            "extra" => matches!(v, Nbt::List(l) if l.iter().all(message_is_empty)),
            _ => false,
        }),
        _ => false,
    }
}

fn messages(text: Option<&Nbt>) -> Vec<Nbt> {
    let mut v: Vec<Nbt> =
        text.and_then(|t| t.get("messages")).map(|m| crate::nbtio::list(Some(m)).to_vec()).unwrap_or_default();
    v.resize(4, Nbt::String(String::new()));
    v
}

/// Editable text: each line empty or plain text.
fn editable(text: Option<&Nbt>) -> bool {
    messages(text).iter().all(|m| message_is_empty(m) || plain_message(m))
}

/// Non-empty text.
fn has_message(text: Option<&Nbt>) -> bool {
    messages(text).iter().any(|m| !message_is_empty(m))
}

/// New line keeping the style of the old one (literal text `l` with the style copied).
fn restyle(old: &Nbt, line: &str) -> Nbt {
    match old {
        Nbt::Compound(f) => {
            let mut keep: Vec<(String, Nbt)> =
                f.iter().filter(|(k, _)| !matches!(k.as_str(), "text" | "type" | "extra")).cloned().collect();
            if keep.is_empty() {
                return Nbt::String(line.to_owned());
            }
            keep.insert(0, ("text".to_owned(), Nbt::String(line.to_owned())));
            Nbt::Compound(keep)
        }
        _ => Nbt::String(line.to_owned()),
    }
}

/// Complete face (fields always present: messages, color, glow).
fn full_text(text: Option<&Nbt>, msgs: Vec<Nbt>) -> Nbt {
    let color = text.and_then(|t| t.get("color")).and_then(Nbt::as_str).unwrap_or("black").to_owned();
    let glow = text.and_then(|t| t.get("has_glowing_text")).and_then(Nbt::as_i64).unwrap_or(0) != 0;
    Nbt::Compound(vec![
        ("messages".to_owned(), Nbt::List(msgs)),
        ("color".to_owned(), Nbt::String(color)),
        ("has_glowing_text".to_owned(), Nbt::Byte(i8::from(glow))),
    ])
}

fn face_key(front: bool) -> &'static str {
    if front { "front_text" } else { "back_text" }
}

/// Angle of a horizontal direction.
fn facing_yrot(f: &str) -> f32 {
    match f {
        "south" => 0.0,
        "west" => 90.0,
        "north" => 180.0,
        _ => 270.0,
    }
}

impl Server {
    /// The current player is the world's owner (not a LAN guest).
    fn is_owner(&self) -> bool {
        self.lan.as_ref().is_none_or(|l| l.active.is_none())
    }

    /// "Game master" permission (level 2) of the current player (host or guest).
    pub(crate) fn gamemaster(&self) -> bool {
        self.cmd.op >= 2
    }

    /// Operator blocks and tools (debug stick, light block, command blocks):
    /// creative mode and level 2 of the current player.
    pub(crate) fn gamemaster_blocks(&self) -> bool {
        self.game_mode == 1 && self.gamemaster()
    }

    /// W8a packets; `true` if handled.
    pub(crate) fn solo_packet(&mut self, id: i32, r: &mut Reader<'_>) -> bool {
        let ids = self.peer.ids();
        if id == ids.change_difficulty_out {
            if let Ok(d) = read_varint(r)
                && (0..4).contains(&d)
                && (self.gamemaster() || self.is_owner())
            {
                self.set_difficulty(d, false);
            }
        } else if id == ids.lock_difficulty_out {
            if let Ok(locked) = bool::decode(r)
                && (self.gamemaster() || self.is_owner())
            {
                self.set_difficulty_locked(locked);
            }
        } else if id == ids.set_game_rule_out {
            let _ = self.read_set_game_rules(r);
        } else if id == ids.sign_update {
            let _ = self.read_sign_update(r);
        } else if id == ids.edit_book {
            let _ = self.read_edit_book(r);
        } else if id == ids.pick_item_from_entity {
            if let (Ok(eid), Ok(_data)) = (read_varint(r), bool::decode(r)) {
                self.pick_from_entity(eid);
            }
        } else if id == ids.teleport_to_entity {
            if let Ok(b) = r.take(16) {
                let mut a = [0u8; 16];
                a.copy_from_slice(b);
                self.spectator_teleport(u128::from_be_bytes(a));
            }
        } else if id == ids.block_entity_tag_query {
            if let (Ok(tx), Ok(p)) = (read_varint(r), BlockPos::decode(r))
                && self.gamemaster()
            {
                let nbt = self.be_get([p.x, p.y, p.z]).cloned().map(|mut n| {
                    for k in ["id", "x", "y", "z"] {
                        crate::nbtio::remove(&mut n, k);
                    }
                    n
                });
                let _ = self.send(Event::TagQuery { id: tx, nbt });
            }
        } else if id == ids.entity_tag_query {
            if let (Ok(tx), Ok(eid)) = (read_varint(r), read_varint(r))
                && self.gamemaster()
                && let Some(e) = self.e_of_net(eid)
            {
                let mut n = self.ent_nbt(e);
                crate::nbtio::remove(&mut n, "id");
                let _ = self.send(Event::TagQuery { id: tx, nbt: Some(n) });
            }
        } else {
            return false;
        }
        true
    }

    // ---------- Difficulty and rules ----------

    /// Difficulty: refused if locked (except `ignore_lock`), hard in
    /// hardcore; sent back to the client even unchanged.
    pub(crate) fn set_difficulty(&mut self, id: i32, ignore_lock: bool) {
        if !ignore_lock && self.level.difficulty_locked {
            return;
        }
        let id = if self.level.hardcore { 3 } else { id };
        let name = ["peaceful", "easy", "normal", "hard"][id.clamp(0, 3) as usize];
        self.level.difficulty = name.to_owned();
        self.difficulty_settings_set("difficulty", Nbt::String(name.to_owned()));
        let _ = self.send(Event::Difficulty { id: self.level.difficulty_id(), locked: self.level.difficulty_locked });
    }

    /// Difficulty lock, saved and sent back.
    pub(crate) fn set_difficulty_locked(&mut self, locked: bool) {
        self.level.difficulty_locked = locked;
        self.difficulty_settings_set("locked", Nbt::Byte(i8::from(locked)));
        let _ = self.send(Event::Difficulty { id: self.level.difficulty_id(), locked });
    }

    fn difficulty_settings_set(&mut self, key: &str, v: Nbt) {
        if let Some(d) = crate::nbtio::get_mut(&mut self.level.root, "Data") {
            let mut ds = d.get("difficulty_settings").cloned().unwrap_or(Nbt::Compound(Vec::new()));
            crate::nbtio::set(&mut ds, key, v);
            crate::nbtio::set(d, "difficulty_settings", ds);
        }
    }

    /// `client_command` 2: values of all rules (game master only).
    pub(crate) fn send_game_rule_values(&mut self) {
        if !self.gamemaster() {
            return;
        }
        let values = RULES
            .iter()
            .map(|r| {
                let key = format!("minecraft:{}", r.name);
                let v = self.rule_value(&key, r.int.is_none());
                (key, v)
            })
            .collect();
        let _ = self.send(Event::GameRuleValues(values));
    }

    fn read_set_game_rules(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let n = rc_proto::read_count(r, 2)?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let k = rc_proto::read_str(r, 32767)?.to_owned();
            let v = rc_proto::read_str(r, 32767)?.to_owned();
            entries.push((k, v));
        }
        if !self.gamemaster() {
            return Ok(());
        }
        for (k, v) in entries {
            let short = k.strip_prefix("minecraft:").unwrap_or(&k);
            let Some(rule) = RULES.iter().find(|x| x.name == short) else { continue };
            let Some(val) = parse_rule(rule, &v) else { continue };
            let key = format!("minecraft:{}", rule.name);
            let shown = match &val {
                Nbt::Byte(b) => (*b != 0).to_string(),
                o => o.as_i64().unwrap_or(0).to_string(),
            };
            crate::nbtio::set(&mut self.rules, &key, val);
            self.cmd.rules_dirty = true;
            self.after_rule_change(&key);
            // Announcement to operators (the owner is one if commands are allowed).
            if self.cmd.op >= 1 || (self.is_owner() && self.level.allow_commands) {
                let c = rc_text::Component::translate(
                    "commands.gamerule.set",
                    vec![rc_text::Arg::Text(rule.name.to_owned()), rc_text::Arg::Text(shown)],
                );
                let _ = self.send(Event::Chat { component: c, tag: rc_client::ChatTag::System });
            }
        }
        Ok(())
    }

    // ---------- Signs ----------

    /// Sign (block entity) at this position: its NBT.
    fn sign_be(&self, p: [i32; 3]) -> Option<&Nbt> {
        let be = self.be_get(p)?;
        matches!(be.get("id").and_then(Nbt::as_str), Some("minecraft:sign" | "minecraft:hanging_sign")).then_some(be)
    }

    /// Face the player is looking at: true = front.
    fn sign_facing_front(&self, p: [i32; 3]) -> bool {
        let s = self.get(p);
        let Some(name) = self.data.block_of(s).map(|b| b.name.clone()) else { return true };
        let prop = |k: &str| self.data.state_properties(s).find(|(n, _)| *n == k).map(|(_, v)| v.to_owned());
        let wall = name.ends_with("_wall_sign") || name.ends_with("_wall_hanging_sign");
        let yrot = if wall {
            facing_yrot(prop("facing").as_deref().unwrap_or("north"))
        } else {
            prop("rotation").and_then(|r| r.parse::<i32>().ok()).unwrap_or(0) as f32 * 22.5
        };
        // Center of the box: wall sign stuck to the wall opposite its direction, otherwise the middle.
        let mut c = [0.5f64, 0.5];
        if name.ends_with("_wall_sign") && !name.ends_with("_wall_hanging_sign") {
            match prop("facing").as_deref() {
                Some("north") => c[1] = 0.9375,
                Some("south") => c[1] = 0.0625,
                Some("west") => c[0] = 0.9375,
                _ => c[0] = 0.0625,
            }
        }
        let xd = self.pos[0] - (f64::from(p[0]) + c[0]);
        let zd = self.pos[2] - (f64::from(p[2]) + c[1]);
        let player = (zd.atan2(xd) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32 - 90.0;
        crate::mobs::mth::wrap(player - yrot).abs() <= 90.0
    }

    /// Opening the editor: player allowed, block state then editor.
    fn open_sign_editor(&mut self, p: [i32; 3], front: bool) {
        self.life.sign_editors.insert(p, self.uuid);
        if let Some(state) = self.state_at(p) {
            let _ = self.send(Event::BlockUpdate { pos: BlockPos::new(p[0], p[1], p[2]), state });
        }
        let _ = self.send(Event::OpenSignEditor { pos: p, front });
    }

    /// Another player is editing this sign.
    fn other_editing(&self, p: [i32; 3]) -> bool {
        self.life.sign_editors.get(&p).is_some_and(|u| *u != self.uuid)
    }

    /// Placement of a sign by the player: editor of the front face if the text
    /// is editable and the sign not waxed.
    pub(crate) fn sign_placed_by(&mut self, p: [i32; 3]) {
        let Some(be) = self.sign_be(p) else { return };
        let waxed = be.get("is_waxed").and_then(Nbt::as_i64).unwrap_or(0) != 0;
        if !waxed && editable(be.get("front_text")) {
            self.open_sign_editor(p, true);
        }
    }

    /// Click of a hanging sign on a hanging sign (underside of the ceiling sign, side off the
    /// wall sign's axis): the block lets it through to place the next one,
    /// except for a targeted face that is waxed with click commands.
    pub(crate) fn hanging_sign_chains(&self, p: [i32; 3], face: crate::blocks::Dir, item: &str) -> bool {
        if !item.ends_with("_hanging_sign") {
            return false;
        }
        let name = self.bname(p);
        if !name.ends_with("_hanging_sign") {
            return false;
        }
        let wall = name.ends_with("_wall_hanging_sign");
        let hits = if wall { face.axis() != self.v().facing(self.get(p)).axis() } else { face == crate::blocks::Dir::Down };
        if !hits {
            return false;
        }
        let Some(be) = self.sign_be(p) else { return false };
        let waxed = be.get("is_waxed").and_then(Nbt::as_i64).unwrap_or(0) != 0;
        let commands = messages(be.get(face_key(self.sign_facing_front(p)))).iter().any(|m| {
            m.get("click_event").and_then(|e| e.get("action")).and_then(Nbt::as_str) == Some("run_command")
        });
        !(waxed && commands)
    }

    /// Use of a sign (applicator item first, then empty hand); true if consumed.
    pub(crate) fn sign_use(&mut self, p: [i32; 3]) -> bool {
        let Some(be) = self.sign_be(p).cloned() else { return false };
        let may_build = self.game_mode != 2 && self.game_mode != 3;
        let waxed = be.get("is_waxed").and_then(Nbt::as_i64).unwrap_or(0) != 0;
        let front = self.sign_facing_front(p);
        let key = face_key(front);
        let slot = 36 + self.held as usize;
        // Applicators: dye, ink (glowing), honeycomb.
        if let Some(st) = self.slots[slot].clone()
            && may_build
            && let Some(kind) = self.sign_applicator(&st)
        {
            if waxed || self.other_editing(p) {
                // Empty-hand attempt next.
            } else {
                let text = be.get(key);
                let can = kind == Applicator::Wax || has_message(text);
                if can && self.apply_to_sign(p, front, &kind, &be) {
                    let be = self.sign_be(p).cloned().unwrap_or(be);
                    self.sign_click_actions(p, front, &be);
                    self.award_stat("minecraft:used", &st.id, 1);
                    let st_state = self.get(p);
                    self.game_event_block(
                        "minecraft:block_change",
                        p,
                        Some(crate::mobs::mob::Ent::Player),
                        Some(st_state),
                    );
                    if self.game_mode != 1
                        && let Some(s) = &mut self.slots[slot]
                    {
                        s.count -= 1;
                        if s.count <= 0 {
                            self.slots[slot] = None;
                        }
                        self.send_slot(slot);
                    }
                    return true;
                }
            }
        }
        // Empty hand.
        let be = self.sign_be(p).cloned().unwrap_or(be);
        // Click events of the targeted face first (waxed or not).
        let executed = self.sign_click_actions(p, front, &be);
        let waxed = be.get("is_waxed").and_then(Nbt::as_i64).unwrap_or(0) != 0;
        if waxed {
            let hanging = self.bname(p).ends_with("_hanging_sign");
            let fail = if hanging {
                "minecraft:block.hanging_sign.waxed_interact_fail"
            } else {
                "minecraft:block.sign.waxed_interact_fail"
            };
            self.block_sound(p, fail, 1.0, 1.0);
            return true;
        }
        if executed {
            return true;
        }
        if !self.other_editing(p) && may_build && editable(be.get(key)) {
            self.open_sign_editor(p, front);
            return true;
        }
        false
    }

    /// Click events of the lines of a face (style of the line, not of its children):
    /// command launched by the player (position at the center of the sign, null gaze, permission 2,
    /// no feedback), dialog opened; only if the sign allows operator features
    /// (`allow_op_features`), otherwise red message. True if there was one.
    fn sign_click_actions(&mut self, p: [i32; 3], front: bool, be: &Nbt) -> bool {
        let allow = be.get("allow_op_features").and_then(Nbt::as_i64).unwrap_or(0) != 0;
        let mut any = false;
        for m in messages(be.get(face_key(front))) {
            let Some(ev) = m.get("click_event") else { continue };
            match ev.get("action").and_then(Nbt::as_str) {
                Some("run_command") => {
                    if allow && let Some(cmd) = ev.get("command").and_then(Nbt::as_str) {
                        let src = self.sign_command_source(p);
                        let _ = self.perform(src, cmd);
                    }
                    any = true;
                }
                Some("show_dialog") => {
                    if allow && let Some(d) = ev.get("dialog") {
                        let r = match d {
                            Nbt::String(id) => {
                                let id = crate::commands::args::id(id);
                                let i = self.regs.entries("minecraft:dialog").iter().position(|e| *e == id);
                                i.map(|i| rc_client::extra::DialogRef::Registry(i as i32))
                            }
                            other => Some(rc_client::extra::DialogRef::Inline(other.clone())),
                        };
                        if let Some(r) = r {
                            let _ = self.send(Event::ShowDialog(r));
                        }
                    }
                    any = true;
                }
                // Server-specific action: no effect here, but counted.
                Some("custom") => any = true,
                _ => {}
            }
        }
        if !allow && any {
            let mut c = rc_text::Component::translate("sign.click_actions_disabled", Vec::new());
            c.style.color = Some(0xFF5555);
            let _ = self.send(Event::ActionBar(c));
        }
        any
    }

    /// Source of the commands of a clicked sign: the player, without message feedback.
    fn sign_command_source(&self, p: [i32; 3]) -> crate::commands::src::Src {
        crate::commands::src::Src {
            origin: crate::commands::src::Origin::Server,
            entity: Some(E::Player),
            pos: [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5],
            rot: [0.0, 0.0],
            dim: self.dim.name.clone(),
            level: 2,
            name: self.name.clone(),
            display: self.player_display_name(),
            eyes: false,
            silent: true,
            stores: Vec::new(),
        }
    }

    fn sign_applicator(&self, s: &Stack) -> Option<Applicator> {
        match s.id.as_str() {
            "minecraft:honeycomb" => Some(Applicator::Wax),
            "minecraft:glow_ink_sac" => Some(Applicator::Glow(true)),
            "minecraft:ink_sac" => Some(Applicator::Glow(false)),
            _ => self.life.items.dye(s).map(Applicator::Dye),
        }
    }

    /// Application to a face; false if nothing changes.
    fn apply_to_sign(&mut self, p: [i32; 3], front: bool, kind: &Applicator, be: &Nbt) -> bool {
        let key = face_key(front);
        let text = be.get(key);
        let msgs = messages(text);
        let mut face = full_text(text, msgs);
        let (sound, changed) = match kind {
            Applicator::Wax => {
                if be.get("is_waxed").and_then(Nbt::as_i64).unwrap_or(0) != 0 {
                    return false;
                }
                if let Some(v) = self.be_get_mut(p) {
                    crate::nbtio::set(v, "is_waxed", Nbt::Byte(1));
                }
                self.level_event(3003, p, 0);
                ("minecraft:item.honeycomb.wax_on", true)
            }
            Applicator::Glow(g) => {
                let cur = face.get("has_glowing_text").and_then(Nbt::as_i64).unwrap_or(0) != 0;
                if cur == *g {
                    return false;
                }
                crate::nbtio::set(&mut face, "has_glowing_text", Nbt::Byte(i8::from(*g)));
                (if *g { "minecraft:item.glow_ink_sac.use" } else { "minecraft:item.ink_sac.use" }, true)
            }
            Applicator::Dye(c) => {
                if face.get("color").and_then(Nbt::as_str) == Some(c.as_str()) {
                    return false;
                }
                crate::nbtio::set(&mut face, "color", Nbt::String(c.clone()));
                ("minecraft:item.dye.use", true)
            }
        };
        if !matches!(kind, Applicator::Wax)
            && let Some(v) = self.be_get_mut(p)
        {
            crate::nbtio::set(v, key, face);
        }
        self.send_block_entity(p);
        self.block_sound(p, sound, 1.0, 1.0);
        changed
    }

    fn read_sign_update(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let pos = BlockPos::decode(r)?;
        let mut lines = Vec::with_capacity(4);
        for _ in 0..4 {
            lines.push(strip_formatting(rc_proto::read_str(r, 384)?));
        }
        let front = read_varint(r)? == 1;
        let p = [pos.x, pos.y, pos.z];
        let Some(be) = self.sign_be(p).cloned() else { return Ok(()) };
        let waxed = be.get("is_waxed").and_then(Nbt::as_i64).unwrap_or(0) != 0;
        if waxed || self.life.sign_editors.get(&p) != Some(&self.uuid) {
            return Ok(());
        }
        let key = face_key(front);
        let text = be.get(key);
        let old = messages(text);
        let msgs: Vec<Nbt> = old.iter().zip(&lines).map(|(o, l)| restyle(o, l)).collect();
        let face = full_text(text, msgs);
        if let Some(v) = self.be_get_mut(p) {
            crate::nbtio::set(v, key, face);
        }
        self.life.sign_editors.remove(&p);
        self.send_block_entity(p);
        Ok(())
    }

    /// Editors too far forgotten (block range + 4), disappeared signs too.
    pub(crate) fn sign_editors_tick(&mut self) {
        if self.life.sign_editors.is_empty() {
            return;
        }
        let me = self.uuid;
        let far: Vec<[i32; 3]> = self
            .life
            .sign_editors
            .iter()
            .filter(|(p, u)| **u == me && (!self.within_block_range(**p, 4.0) || self.sign_be(**p).is_none()))
            .map(|(p, _)| *p)
            .collect();
        for p in far {
            self.life.sign_editors.remove(&p);
        }
    }

    /// Block in range: block box within range +
    /// `buffer` of the eyes.
    pub(crate) fn within_block_range(&self, p: [i32; 3], buffer: f64) -> bool {
        let range = self.player_attribute("minecraft:block_interaction_range") + buffer;
        let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(E::Player), self.pos[2]];
        let d2: f64 = (0..3)
            .map(|a| {
                let (lo, hi) = (f64::from(p[a]), f64::from(p[a]) + 1.0);
                let c = eye[a].clamp(lo, hi);
                (eye[a] - c) * (eye[a] - c)
            })
            .sum();
        d2 < range * range
    }

    // ---------- Books ----------

    fn read_edit_book(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let slot = read_varint(r)?;
        let n = rc_proto::read_count(r, 2)?.min(100);
        let mut pages = Vec::with_capacity(n);
        for _ in 0..n {
            pages.push(rc_proto::read_str(r, 1024)?.to_owned());
        }
        let title = if bool::decode(r)? { Some(rc_proto::read_str(r, 32)?.to_owned()) } else { None };
        let menu_slot = match slot {
            0..=8 => 36 + slot as usize,
            40 => 45,
            _ => return Ok(()),
        };
        let Some(mut st) = self.slots[menu_slot].clone() else { return Ok(()) };
        let writable = component(&st, "minecraft:writable_book_content").is_some()
            || self.life.items.default_component(&st, "minecraft:writable_book_content").is_some();
        if !writable {
            return Ok(());
        }
        let raw = |s: &str| Nbt::Compound(vec![("raw".to_owned(), Nbt::String(s.to_owned()))]);
        match title {
            None => {
                let list = Nbt::List(pages.iter().map(|p| raw(p)).collect());
                set_component(
                    &mut st,
                    "minecraft:writable_book_content",
                    Nbt::Compound(vec![("pages".to_owned(), list)]),
                );
            }
            Some(t) => {
                // Written book: same stack changed item, content replaced.
                st.id = "minecraft:written_book".to_owned();
                remove_component(&mut st, "minecraft:writable_book_content");
                remove_component(&mut st, "!minecraft:writable_book_content");
                let mut f = vec![("title".to_owned(), raw(&t)), ("author".to_owned(), Nbt::String(self.name.clone()))];
                if !pages.is_empty() {
                    f.push(("pages".to_owned(), Nbt::List(pages.iter().map(|p| raw(p)).collect())));
                }
                f.push(("resolved".to_owned(), Nbt::Byte(1)));
                set_component(&mut st, "minecraft:written_book_content", Nbt::Compound(f));
            }
        }
        self.slots[menu_slot] = Some(st);
        self.send_slot(menu_slot);
        Ok(())
    }

    /// Use of a book: written book resolved then opened on the client; book
    /// and quill opened by the client alone. True if the item is a book.
    pub(crate) fn book_use_item(&mut self, hand: i32) -> bool {
        if self.game_mode == 3 || self.life.vit.dead {
            return false;
        }
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        let Some(st) = self.slots[slot].clone() else { return false };
        if st.id != "minecraft:written_book" && st.id != "minecraft:writable_book" {
            return false;
        }
        if let Some(c) = component(&st, "minecraft:written_book_content").cloned() {
            if c.get("resolved").and_then(Nbt::as_i64).unwrap_or(0) == 0 {
                let mut c = c;
                crate::nbtio::set(&mut c, "resolved", Nbt::Byte(1));
                let mut st = st.clone();
                set_component(&mut st, "minecraft:written_book_content", c);
                self.slots[slot] = Some(st);
                self.send_slot(slot);
            }
            let _ = self.send(Event::OpenBook(hand));
        }
        self.award_stat("minecraft:used", &st.id, 1);
        true
    }

    // ---------- Entities ----------

    /// Entity from its network identifier.
    pub(crate) fn e_of_net(&self, id: i32) -> Option<E> {
        if id == self.player_id {
            return Some(E::Player);
        }
        self.all_ents().into_iter().find(|e| match e {
            E::Mob(i) | E::Item(i) | E::Orb(i) | E::Other(i) => *i == id,
            E::Player => false,
        })
    }

    /// Item of an entity chosen by middle click.
    fn entity_pick_stack(&self, e: E) -> Option<Stack> {
        let ty = self.ent_type(e);
        let short = ty.trim_start_matches("minecraft:");
        let item = match e {
            E::Mob(_) => format!("minecraft:{short}_spawn_egg"),
            // G5-F06: minecart item (spawner minecart: plain minecart).
            E::Other(i) if self.cart_ref(i).is_some() => self.cart_ref(i).map(|c| c.kind.pick_item().to_owned())?,
            E::Other(i) => {
                let n = self.ent_nbt(E::Other(i));
                match short {
                    "item_frame" | "glow_item_frame" => {
                        if let Some(it) = n.get("Item").and_then(Stack::from_nbt) {
                            return Some(Stack { count: 1, ..it });
                        }
                        ty.clone()
                    }
                    "leash_knot" => "minecraft:lead".to_owned(),
                    // Cushion: the item of its color.
                    "cushion" => format!(
                        "minecraft:{}_cushion",
                        n.get("color").and_then(Nbt::as_str).filter(|c| crate::decoration::DYES.contains(c)).unwrap_or("white")
                    ),
                    _ => ty.clone(),
                }
            }
            _ => return None,
        };
        self.life.items.id(&item)?;
        Some(Stack { id: item, count: 1, components: None })
    }

    fn pick_from_entity(&mut self, id: i32) {
        let Some(e) = self.e_of_net(id) else { return };
        if e == E::Player {
            return;
        }
        // Entity range + 3 from the eyes to the box.
        let range = self.player_attribute("minecraft:entity_interaction_range") + 3.0;
        let bb = self.e_bbox(e);
        let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(E::Player), self.pos[2]];
        let d2: f64 = (0..3)
            .map(|a| {
                let c = eye[a].clamp(bb.min[a], bb.max[a]);
                (eye[a] - c) * (eye[a] - c)
            })
            .sum();
        if d2 >= range * range {
            return;
        }
        if let Some(st) = self.entity_pick_stack(e) {
            self.try_pick_item(st);
        }
    }

    /// Choice of an item: selected hotbar slot if the item is there, swap
    /// with the inventory otherwise, add in creative; held slot returned.
    pub(crate) fn try_pick_item(&mut self, st: Stack) {
        // Game inventory: 0–8 hotbar (menu 36–44), 9–35 storage (menu 9–35).
        let menu = |i: usize| if i < 9 { 36 + i } else { i };
        let found = (0..36).find(|&i| self.slots[menu(i)].as_ref().is_some_and(|s| crate::items::same(s, &st)));
        match found {
            Some(i) if i < 9 => self.held = i as i32,
            Some(i) => {
                self.held = self.suitable_hotbar_slot() as i32;
                let h = menu(self.held as usize);
                self.slots.swap(h, i);
            }
            None if self.game_mode == 1 => {
                self.held = self.suitable_hotbar_slot() as i32;
                let h = menu(self.held as usize);
                if self.slots[h].is_some()
                    && let Some(free) = (0..36).find(|&i| self.slots[menu(i)].is_none())
                {
                    let cur = self.slots[h].take();
                    self.slots[menu(free)] = cur;
                }
                self.slots[h] = Some(st);
            }
            None => {}
        }
        let _ = self.send(Event::HeldSlot(self.held));
        self.send_slot(36 + self.held as usize);
    }

    /// Suitable hotbar slot: empty, otherwise unenchanted, from
    /// the held slot.
    fn suitable_hotbar_slot(&self) -> usize {
        let held = self.held as usize;
        if let Some(i) = (0..9).map(|k| (held + k) % 9).find(|&i| self.slots[36 + i].is_none()) {
            return i;
        }
        (0..9)
            .map(|k| (held + k) % 9)
            .find(|&i| {
                self.slots[36 + i].as_ref().is_none_or(|s| {
                    crate::enchanting::read_comp(&self.life.items, s, "minecraft:enchantments").is_empty()
                })
            })
            .unwrap_or(held)
    }

    /// Spectator teleport to an entity (spectator menu).
    fn spectator_teleport(&mut self, uuid: u128) {
        if self.game_mode != 3 {
            return;
        }
        let Some(e) = self.ent_by_uuid(uuid) else { return };
        let pos = self.e_pos(e);
        let rot = self.ent_rot(e);
        self.ent_teleport(E::Player, pos, Some(rot));
    }
}

/// Item that applies to a sign.
#[derive(Debug, Clone, PartialEq)]
enum Applicator {
    Wax,
    Glow(bool),
    Dye(String),
}
