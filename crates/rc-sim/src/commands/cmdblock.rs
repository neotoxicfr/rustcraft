//! Command blocks (impulse, repeating, chain) and command block minecarts:
//! redstone power, scheduled tick, condition (the block behind succeeded),
//! execution with the block's source, output (`LastOutput`), success count
//! (comparator output), settings received from the screen (`set_command_block`).

use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;
use rc_proto::{BlockPos, Decode, Reader, read_str};
use rc_text::Component;

use super::src::{Origin, Src};
use super::text::{GRAY, lit};
use crate::blocks::Dir;
use crate::nbtio::{int, set};
use crate::server::Server;
use crate::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sequence,
    Auto,
    Redstone,
}

impl Server {
    fn cb_name(&self, s: u32) -> Option<&str> {
        let n = self.data.block_of(s)?.name.as_str();
        matches!(n, "minecraft:command_block" | "minecraft:chain_command_block" | "minecraft:repeating_command_block")
            .then_some(n)
    }

    fn cb_mode(&self, s: u32) -> Mode {
        match self.cb_name(s) {
            Some("minecraft:repeating_command_block") => Mode::Auto,
            Some("minecraft:chain_command_block") => Mode::Sequence,
            _ => Mode::Redstone,
        }
    }

    fn cb_flag(&self, p: [i32; 3], k: &str) -> bool {
        self.be_get(p).and_then(|b| int(b, k)).is_some_and(|v| v != 0)
    }

    fn cb_set(&mut self, p: [i32; 3], k: &str, v: Nbt) {
        if let Some(b) = self.be_get_mut(p) {
            set(b, k, v);
        }
    }

    /// True, or (conditional) success of the command block behind.
    fn cb_mark_condition(&mut self, p: [i32; 3]) -> bool {
        let s = self.get(p);
        let mut met = true;
        if self.v().get_bool(s, "conditional") {
            let back = crate::blocks::rel(p, self.v().facing(s).opposite());
            met = self.cb_name(self.get(back)).is_some() && self.be_int(back, "SuccessCount") > 0;
        }
        self.cb_set(p, "conditionMet", Nbt::Byte(i8::from(met)));
        met
    }

    /// Neighbour changed: power; rising edge of a non-automatic impulse block.
    pub(crate) fn cmdblock_neighbor_changed(&mut self, s: u32, p: [i32; 3]) {
        if self.cb_name(s).is_none() || self.be_get(p).is_none() {
            return;
        }
        let powered = self.has_neighbor_signal(p);
        self.cb_set_powered(p, s, powered);
    }

    fn cb_set_powered(&mut self, p: [i32; 3], s: u32, powered: bool) {
        let was = self.cb_flag(p, "powered");
        if powered == was {
            return;
        }
        self.cb_set(p, "powered", Nbt::Byte(i8::from(powered)));
        if powered {
            if self.cb_flag(p, "auto") || self.cb_mode(s) == Mode::Sequence {
                return;
            }
            self.cb_mark_condition(p);
            let b = self.v().block(s);
            self.schedule_tick(p, b, 1);
        }
    }

    /// Scheduled tick of a command block; false if it is not one.
    pub(crate) fn cmdblock_tick(&mut self, s: u32, p: [i32; 3]) -> bool {
        if self.cb_name(s).is_none() {
            return false;
        }
        if self.be_get(p).is_none() {
            return true;
        }
        let mode = self.cb_mode(s);
        let was_met = self.cb_flag(p, "conditionMet");
        let conditional = self.v().get_bool(s, "conditional");
        let b = self.v().block(s);
        match mode {
            Mode::Auto => {
                self.cb_mark_condition(p);
                if was_met {
                    self.cb_execute(s, p);
                } else if conditional {
                    self.cb_set(p, "SuccessCount", Nbt::Int(0));
                }
                if self.cb_flag(p, "powered") || self.cb_flag(p, "auto") {
                    self.schedule_tick(p, b, 1);
                }
            }
            Mode::Redstone => {
                if was_met {
                    self.cb_execute(s, p);
                } else if conditional {
                    self.cb_set(p, "SuccessCount", Nbt::Int(0));
                }
            }
            Mode::Sequence => {}
        }
        self.update_neighbour_for_output_signal(p, b);
        true
    }

    fn cb_execute(&mut self, s: u32, p: [i32; 3]) {
        let has_cmd = self.be_get(p).and_then(|b| b.get("Command")?.as_str().map(|c| !c.is_empty())).unwrap_or(false);
        if has_cmd {
            self.cb_perform(p);
        } else {
            self.cb_set(p, "SuccessCount", Nbt::Int(0));
        }
        self.cb_chain(p, self.v().facing(s));
    }

    /// Execution of a chain behind a block.
    fn cb_chain(&mut self, start: [i32; 3], mut dir: Dir) {
        let mut pos = start;
        let mut left = self.rules.get("minecraft:max_command_sequence_length").and_then(Nbt::as_i64).unwrap_or(65536);
        while left > 0 {
            left -= 1;
            pos = crate::blocks::rel(pos, dir);
            let st = self.get(pos);
            if self.cb_name(st) != Some("minecraft:chain_command_block") || self.be_get(pos).is_none() {
                break;
            }
            if self.cb_flag(pos, "powered") || self.cb_flag(pos, "auto") {
                if self.cb_mark_condition(pos) {
                    if !self.cb_perform(pos) {
                        break;
                    }
                    let b = self.v().block(st);
                    self.update_neighbour_for_output_signal(pos, b);
                } else if self.v().get_bool(st, "conditional") {
                    self.cb_set(pos, "SuccessCount", Nbt::Int(0));
                }
            }
            dir = self.v().facing(st);
        }
    }

    /// False if already executed this tick.
    fn cb_perform(&mut self, p: [i32; 3]) -> bool {
        let Some(be) = self.be_get(p).cloned() else { return false };
        let last = be.get("LastExecution").and_then(Nbt::as_i64).unwrap_or(-1);
        if last == self.time {
            return false;
        }
        let cmd = be.get("Command").and_then(Nbt::as_str).unwrap_or("").to_owned();
        if cmd.eq_ignore_ascii_case("Searge") {
            self.cb_set(p, "LastOutput", crate::commands::text::to_nbt(&lit("#itzlipofutzli")));
            self.cb_set(p, "SuccessCount", Nbt::Int(1));
            return true;
        }
        self.cb_set(p, "SuccessCount", Nbt::Int(0));
        let mut successes = 0;
        if self.rule("minecraft:command_blocks_work", true) && !cmd.is_empty() {
            crate::nbtio::remove(self.be_get_mut(p).unwrap_or(&mut Nbt::Compound(Vec::new())), "LastOutput");
            let s = self.get(p);
            let yaw = match self.v().facing(s) {
                Dir::South => 0.0,
                Dir::West => 90.0,
                Dir::North => 180.0,
                Dir::East => -90.0,
                _ => 0.0,
            };
            let name = be.get("CustomName").and_then(rc_text::from_nbt).unwrap_or_else(|| lit("@"));
            let src = Src {
                origin: Origin::Block(p),
                entity: None,
                pos: [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5],
                rot: [0.0, yaw],
                dim: self.dim.name.clone(),
                level: 2,
                name: crate::commands::cmds_ents::plain(&name),
                display: name,
                eyes: false,
                silent: false,
                stores: Vec::new(),
            };
            self.cmd.depth += 1;
            let (ok, _) =
                if self.cmd.depth < 64 { self.perform_counted(src, &cmd, &mut successes) } else { (false, 0) };
            self.cmd.depth -= 1;
            let _ = ok;
        }
        self.cb_set(p, "SuccessCount", Nbt::Int(successes));
        let update = be.get("UpdateLastExecution").and_then(Nbt::as_i64).is_none_or(|v| v != 0);
        self.cb_set(p, "LastExecution", Nbt::Long(if update { self.time } else { -1 }));
        if !update && let Some(b) = self.be_get_mut(p) {
            crate::nbtio::remove(b, "LastExecution");
        }
        true
    }

    /// Execution counting the successes (adds a callback).
    fn perform_counted(&mut self, src: Src, cmd: &str, successes: &mut i32) -> (bool, i32) {
        let op = src.level;
        let Some(lvl) = self.cmd.level(op) else { return (false, 0) };
        let text = cmd.strip_prefix('/').unwrap_or(cmd);
        match super::analyse(lvl, &self.cmd.data, text, op) {
            Ok(parsed) => {
                let before = self.cmd.successes;
                self.cmd.successes = 0;
                let r = self.run_parsed(src, &parsed);
                *successes = self.cmd.successes;
                self.cmd.successes = before;
                r
            }
            Err(msgs) => {
                for m in msgs {
                    self.fail(&src, m);
                }
                (false, 0)
            }
        }
    }

    /// Output of a command block: `LastOutput` (tracking enabled) and broadcast to the operators.
    pub(crate) fn cb_output(&mut self, p: [i32; 3], msg: Component, success: bool) {
        if success && !self.rule("minecraft:command_block_output", true) {
            return;
        }
        let track = self.be_get(p).and_then(|b| int(b, "TrackOutput")).is_none_or(|v| v != 0);
        if track {
            let now = chrono_hms();
            let mut c = lit(format!("[{now}] "));
            c.extra.push(msg.clone());
            self.cb_set(p, "LastOutput", crate::commands::text::to_nbt(&c));
        }
        if success && self.rule("minecraft:send_command_feedback", true) && self.cmd.op >= 2 {
            let name = self
                .be_get(p)
                .and_then(|b| b.get("CustomName").and_then(rc_text::from_nbt))
                .unwrap_or_else(|| lit("@"));
            let mut c = tr!("chat.type.admin", name, msg);
            c.style.color = Some(GRAY);
            c.style.italic = Some(true);
            self.system(c);
        }
    }

    /// Operator (level 2) in creative: placement and data of the game master blocks.
    pub(crate) fn can_use_game_master_blocks(&self) -> bool {
        self.game_mode == 1 && self.cmd.op >= 2
    }

    /// Command block used by an operator in creative: data sent (the screen is
    /// opened by the client); true if it was one.
    pub(crate) fn cmdblock_use(&mut self, p: [i32; 3]) -> bool {
        if self.cb_name(self.get(p)).is_none() || self.be_get(p).is_none() {
            return false;
        }
        if self.game_mode != 1 || self.cmd.op < 2 {
            return false;
        }
        self.send_block_entity(p);
        true
    }

    /// Settings received from the screen (`set_command_block`).
    pub(crate) fn on_set_command_block(&mut self, r: &mut Reader<'_>) {
        let Ok(bp) = BlockPos::decode(r) else { return };
        let (Ok(cmd), Ok(mode), Ok(flags)) = (read_str(r, 32767).map(str::to_owned), read_varint(r), r.u8()) else {
            return;
        };
        if self.cmd.op < 2 || self.game_mode != 1 {
            self.system(tr!("advMode.notAllowed"));
            return;
        }
        let p = [bp.x, bp.y, bp.z];
        if self.be_get(p).is_none() || self.cb_name(self.get(p)).is_none() {
            return;
        }
        let (track, conditional, auto) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
        let old = self.get(p);
        let facing = self.v().facing(old);
        let block = match mode {
            0 => "chain_command_block",
            1 => "repeating_command_block",
            _ => "command_block",
        };
        let b = self.tb().block_id(block);
        let v = self.v();
        let mut st = v.set_dir(v.default_state(b), "facing", facing);
        st = v.set_bool(st, "conditional", conditional);
        if st != old {
            let be = self.be_get(p).cloned();
            self.set_block(p, st, crate::blocks::flag::ALL);
            if let (Some(be), Some(now)) = (be, self.be_get_mut(p)) {
                *now = be;
            }
        }
        self.cb_set(p, "Command", Nbt::String(cmd.clone()));
        self.cb_set(p, "TrackOutput", Nbt::Byte(i8::from(track)));
        if !track && let Some(b) = self.be_get_mut(p) {
            crate::nbtio::remove(b, "LastOutput");
        }
        let was_auto = self.cb_flag(p, "auto");
        self.cb_set(p, "auto", Nbt::Byte(i8::from(auto)));
        let powered = self.cb_flag(p, "powered");
        let new_mode = self.cb_mode(st);
        if !was_auto && auto && !powered && new_mode != Mode::Sequence {
            self.cb_mark_condition(p);
            self.schedule_tick(p, b, 1);
        }
        if new_mode == Mode::Auto && (powered || auto) {
            self.cb_mark_condition(p);
            self.schedule_tick(p, b, 1);
        }
        self.send_block_entity(p);
        if !cmd.is_empty() {
            self.system(tr!("advMode.setCommand.success", cmd));
        }
    }

    /// Settings of a command block minecart (`set_command_minecart`): not simulated, read.
    pub(crate) fn on_set_command_minecart(&mut self, r: &mut Reader<'_>) {
        let (Ok(id), Ok(cmd), Ok(track)) = (read_varint(r), read_str(r, 32767).map(str::to_owned), bool::decode(r))
        else {
            return;
        };
        if self.cmd.op < 2 || self.game_mode != 1 {
            self.system(tr!("advMode.notAllowed"));
            return;
        }
        let e = super::ents::E::Other(id);
        if self.ent_type(e) != "minecraft:command_block_minecart" {
            return;
        }
        let mut n = self.ent_nbt(e);
        set(&mut n, "Command", Nbt::String(cmd.clone()));
        set(&mut n, "TrackOutput", Nbt::Byte(i8::from(track)));
        self.ent_set_nbt(e, &n);
        if !cmd.is_empty() {
            self.system(tr!("advMode.setCommand.success", cmd));
        }
    }

    /// Block entity created: default values, automatic "chain".
    pub(crate) fn cmdblock_created(&mut self, p: [i32; 3], s: u32) {
        let Some(name) = self.cb_name(s) else { return };
        let auto = name == "minecraft:chain_command_block";
        for (k, v) in [
            ("Command", Nbt::String(String::new())),
            ("SuccessCount", Nbt::Int(0)),
            ("TrackOutput", Nbt::Byte(1)),
            ("UpdateLastExecution", Nbt::Byte(1)),
            ("powered", Nbt::Byte(0)),
            ("conditionMet", Nbt::Byte(0)),
            ("auto", Nbt::Byte(i8::from(auto))),
        ] {
            if self.be_get(p).is_some_and(|b| b.get(k).is_none()) {
                self.cb_set(p, k, v);
            }
        }
    }

    /// Loaded data (`setblock {…}`, `data`, `clone`, `execute store`, template): missing
    /// fields at their load value (tracked output and last execution recorded by
    /// default, automatic not), output and date forgotten if not tracked, then switch to
    /// automatic: a block neither powered nor in a chain that becomes automatic is scheduled
    /// for the next tick (condition recorded first).
    pub(crate) fn cmdblock_loaded(&mut self, p: [i32; 3], was_auto: bool) {
        let s = self.get(p);
        if self.cb_name(s).is_none() {
            return;
        }
        let Some(b) = self.be_get_mut(p) else { return };
        for (k, v) in [
            ("Command", Nbt::String(String::new())),
            ("SuccessCount", Nbt::Int(0)),
            ("TrackOutput", Nbt::Byte(1)),
            ("UpdateLastExecution", Nbt::Byte(1)),
            ("powered", Nbt::Byte(0)),
            ("conditionMet", Nbt::Byte(0)),
            ("auto", Nbt::Byte(0)),
        ] {
            if b.get(k).is_none() {
                set(b, k, v);
            }
        }
        if int(b, "TrackOutput") == Some(0) {
            crate::nbtio::remove(b, "LastOutput");
        }
        if int(b, "UpdateLastExecution") == Some(0) {
            crate::nbtio::remove(b, "LastExecution");
        }
        let auto = self.cb_flag(p, "auto");
        if !was_auto && auto && !self.cb_flag(p, "powered") && self.cb_mode(s) != Mode::Sequence {
            self.cb_mark_condition(p);
            let blk = self.v().block(s);
            self.schedule_tick(p, blk, 1);
        }
    }

    /// Placed by the player: tracked output according to the rule, power.
    pub(crate) fn cmdblock_placed(&mut self, p: [i32; 3], s: u32) {
        let Some(name) = self.cb_name(s) else { return };
        let auto = name == "minecraft:chain_command_block";
        let t = self.rule("minecraft:send_command_feedback", true);
        self.cb_set(p, "TrackOutput", Nbt::Byte(i8::from(t)));
        self.cb_set(p, "auto", Nbt::Byte(i8::from(auto)));
        let powered = self.has_neighbor_signal(p);
        self.cb_set_powered(p, s, powered);
    }
}

/// Local time `HH:mm:ss`.
fn chrono_hms() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let off = local_offset_secs();
    let t = (secs as i64 + off).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
}

/// Offset of the local time (system time zone, daylight saving time included), in seconds;
/// `RUSTCRAFT_TZ_OFFSET` forces it (tests).
fn local_offset_secs() -> i64 {
    rc_data::env_str!("RUSTCRAFT_TZ_OFFSET")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(crate::storage::local_offset_secs)
}
