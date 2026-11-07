//! Command blocks (impulse, repeating, chain) and command block minecarts:
//! redstone power, scheduled tick, condition (the block behind succeeded),
//! execution with the block's source, output (`LastOutput`), success count
//! (comparator output), settings received from the screen (`set_command_block`).

use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;
use rc_proto::{BlockPos, Decode, Reader, read_str};
use rc_text::Component;

use super::src::{Origin, Src};
use super::text::lit;
use crate::blocks::Dir;
use crate::nbtio::{int, set};
use crate::server::Server;
use crate::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Chain,
    Repeating,
    Impulse,
}

/// How a command block step reaches its block entity: looked up again at every field (the
/// former path), or found once and then read by reference (`COMMANDBLOCKBORROW`: a lookup scans
/// the chunk's block entities, a dozen of them per block and per tick before). A slot is only
/// valid until a command runs: a command can add or remove block entities of the chunk.
#[derive(Debug, Clone, Copy)]
enum BlockEntityAt {
    Lookup([i32; 3]),
    Slot([i32; 3], usize),
}

/// What a run reads from the block entity before its command runs.
struct RunFacts {
    last_execution: i64,
    command: String,
    /// Read before the command runs (the game reads it after: separate fidelity point).
    update_last_execution: bool,
    custom_name: Option<Component>,
}

impl RunFacts {
    fn read(be: &Nbt) -> RunFacts {
        RunFacts {
            last_execution: be.get("LastExecution").and_then(Nbt::as_i64).unwrap_or(-1),
            command: be.get("Command").and_then(Nbt::as_str).unwrap_or("").to_owned(),
            update_last_execution: be.get("UpdateLastExecution").and_then(Nbt::as_i64).is_none_or(|v| v != 0),
            custom_name: be.get("CustomName").and_then(rc_text::from_nbt),
        }
    }
}

impl Server {
    /// Kind of command block of state `s`, `None` for another block.
    fn command_block_mode(&self, s: u32) -> Option<Mode> {
        if self.cmd.paths.command_block_borrow {
            let (ids, b) = (&self.table().id, self.view().block(s));
            return if b == ids.command_block {
                Some(Mode::Impulse)
            } else if b == ids.repeating_command_block {
                Some(Mode::Repeating)
            } else if b == ids.chain_command_block {
                Some(Mode::Chain)
            } else {
                None
            };
        }
        match self.data.block_of(s)?.name.as_str() {
            "minecraft:command_block" => Some(Mode::Impulse),
            "minecraft:repeating_command_block" => Some(Mode::Repeating),
            "minecraft:chain_command_block" => Some(Mode::Chain),
            _ => None,
        }
    }

    /// The block entity at `p` for one step, `None` if there is none.
    fn command_block_entity(&self, p: [i32; 3]) -> Option<BlockEntityAt> {
        if self.cmd.paths.command_block_borrow {
            return self.be_index(p).map(|i| BlockEntityAt::Slot(p, i));
        }
        self.be_get(p).map(|_| BlockEntityAt::Lookup(p))
    }

    fn entity(&self, at: BlockEntityAt) -> Option<&Nbt> {
        match at {
            BlockEntityAt::Lookup(p) => self.be_get(p),
            BlockEntityAt::Slot(p, i) => self.chunks.get(&(p[0] >> 4, p[2] >> 4))?.as_ref()?.block_entities.get(i),
        }
    }

    fn entity_mut(&mut self, at: BlockEntityAt) -> Option<&mut Nbt> {
        match at {
            BlockEntityAt::Lookup(p) => self.be_get_mut(p),
            BlockEntityAt::Slot(p, i) => {
                let c = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))?.as_mut()?;
                c.dirty = true;
                c.block_entities.get_mut(i)
            }
        }
    }

    fn flag(&self, at: BlockEntityAt, k: &str) -> bool {
        self.entity(at).and_then(|b| int(b, k)).is_some_and(|v| v != 0)
    }

    fn set_block_entity_field(&mut self, at: BlockEntityAt, k: &str, v: Nbt) {
        if let Some(b) = self.entity_mut(at) {
            set(b, k, v);
        }
    }

    /// True, or (conditional) success of the command block behind.
    fn record_condition_met(&mut self, p: [i32; 3], at: BlockEntityAt) -> bool {
        let s = self.get(p);
        let mut met = true;
        if self.view().get_bool(s, "conditional") {
            let back = crate::blocks::rel(p, self.view().facing(s).opposite());
            met = self.command_block_mode(self.get(back)).is_some() && self.be_int(back, "SuccessCount") > 0;
        }
        self.set_block_entity_field(at, "conditionMet", Nbt::Byte(i8::from(met)));
        met
    }

    /// Neighbour changed: power; rising edge of a non-automatic impulse block.
    pub(crate) fn cmdblock_neighbor_changed(&mut self, s: u32, p: [i32; 3]) {
        if self.command_block_mode(s).is_none() || self.be_get(p).is_none() {
            return;
        }
        let powered = self.has_neighbor_signal(p);
        self.set_command_block_powered(p, s, powered);
    }

    fn set_command_block_powered(&mut self, p: [i32; 3], s: u32, powered: bool) {
        let at = BlockEntityAt::Lookup(p);
        let was = self.flag(at, "powered");
        if powered == was {
            return;
        }
        self.set_block_entity_field(at, "powered", Nbt::Byte(i8::from(powered)));
        if powered {
            if self.flag(at, "auto") || self.command_block_mode(s) == Some(Mode::Chain) {
                return;
            }
            self.record_condition_met(p, at);
            let b = self.view().block(s);
            self.schedule_tick(p, b, 1);
        }
    }

    /// Scheduled tick of a command block; false if it is not one.
    pub(crate) fn cmdblock_tick(&mut self, s: u32, p: [i32; 3]) -> bool {
        let Some(mode) = self.command_block_mode(s) else { return false };
        let Some(at) = self.command_block_entity(p) else { return true };
        let was_met = self.flag(at, "conditionMet");
        let conditional = self.view().get_bool(s, "conditional");
        let b = self.view().block(s);
        match mode {
            Mode::Repeating => {
                self.record_condition_met(p, at);
                if was_met {
                    self.execute_command_block(s, p, at);
                } else if conditional {
                    self.set_block_entity_field(at, "SuccessCount", Nbt::Int(0));
                }
                // The command and its chain may have changed the block entities: found again.
                let again = self.command_block_entity(p);
                if again.is_some_and(|at| self.flag(at, "powered") || self.flag(at, "auto")) {
                    self.schedule_tick(p, b, 1);
                }
            }
            Mode::Impulse => {
                if was_met {
                    self.execute_command_block(s, p, at);
                } else if conditional {
                    self.set_block_entity_field(at, "SuccessCount", Nbt::Int(0));
                }
            }
            Mode::Chain => {}
        }
        self.update_neighbour_for_output_signal(p, b);
        true
    }

    fn execute_command_block(&mut self, s: u32, p: [i32; 3], at: BlockEntityAt) {
        let has_cmd = self.entity(at).and_then(|b| b.get("Command")?.as_str().map(|c| !c.is_empty())).unwrap_or(false);
        if has_cmd {
            self.perform_command_block(p, at);
        } else {
            self.set_block_entity_field(at, "SuccessCount", Nbt::Int(0));
        }
        self.run_chain_after(p, self.view().facing(s));
    }

    /// Execution of a chain behind a block.
    fn run_chain_after(&mut self, start: [i32; 3], mut dir: Dir) {
        let mut pos = start;
        let mut left = self.rules.get("minecraft:max_command_sequence_length").and_then(Nbt::as_i64).unwrap_or(65536);
        while left > 0 {
            left -= 1;
            pos = crate::blocks::rel(pos, dir);
            let st = self.get(pos);
            if self.command_block_mode(st) != Some(Mode::Chain) {
                break;
            }
            let Some(at) = self.command_block_entity(pos) else { break };
            if self.flag(at, "powered") || self.flag(at, "auto") {
                if self.record_condition_met(pos, at) {
                    if !self.perform_command_block(pos, at) {
                        break;
                    }
                    let b = self.view().block(st);
                    self.update_neighbour_for_output_signal(pos, b);
                } else if self.view().get_bool(st, "conditional") {
                    self.set_block_entity_field(at, "SuccessCount", Nbt::Int(0));
                }
            }
            dir = self.view().facing(st);
        }
    }

    /// What a run reads before its command; the former path read it from a copy of the whole
    /// block entity.
    fn run_facts(&self, at: BlockEntityAt) -> Option<RunFacts> {
        if self.cmd.paths.command_block_borrow {
            return self.entity(at).map(RunFacts::read);
        }
        let snapshot = self.entity(at).cloned()?;
        Some(RunFacts::read(&snapshot))
    }

    /// One run of the command block at `p` (`at` found since the last command ran); false if
    /// it already ran this tick.
    fn perform_command_block(&mut self, p: [i32; 3], at: BlockEntityAt) -> bool {
        let Some(before) = self.run_facts(at) else { return false };
        if before.last_execution == self.time {
            return false;
        }
        if before.command.eq_ignore_ascii_case("Searge") {
            self.set_block_entity_field(at, "LastOutput", crate::commands::text::to_nbt(&lit("#itzlipofutzli")));
            self.set_block_entity_field(at, "SuccessCount", Nbt::Int(1));
            return true;
        }
        self.set_block_entity_field(at, "SuccessCount", Nbt::Int(0));
        let mut successes = 0;
        if self.rule("minecraft:command_blocks_work", true) && !before.command.is_empty() {
            if let Some(b) = self.entity_mut(at) {
                crate::nbtio::remove(b, "LastOutput");
            }
            let src = self.command_block_source(p, before.custom_name);
            self.cmd.depth += 1;
            if self.cmd.depth < 64 {
                self.perform_counted(&src, &before.command, &mut successes);
            }
            self.cmd.depth -= 1;
        }
        // The command may have added or removed block entities: the one at `p` found again (the
        // results go to whatever block entity is there now).
        let Some(after) = self.command_block_entity(p) else { return true };
        self.set_block_entity_field(after, "SuccessCount", Nbt::Int(successes));
        let update = before.update_last_execution;
        self.set_block_entity_field(after, "LastExecution", Nbt::Long(if update { self.time } else { -1 }));
        if !update && let Some(b) = self.entity_mut(after) {
            crate::nbtio::remove(b, "LastExecution");
        }
        true
    }

    /// Source of a command block: its centre, facing its front, level 2, named by its custom
    /// name or `@`.
    fn command_block_source(&self, p: [i32; 3], custom_name: Option<Component>) -> Src {
        let s = self.get(p);
        let yaw = match self.view().facing(s) {
            Dir::South => 0.0,
            Dir::West => 90.0,
            Dir::North => 180.0,
            Dir::East => -90.0,
            _ => 0.0,
        };
        let name = custom_name.unwrap_or_else(|| lit("@"));
        Src {
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
        }
    }

    /// Execution counting the successes (adds a callback). Outside any execution context, like
    /// before (no quota opened by the block itself).
    fn perform_counted(&mut self, src: &Src, cmd: &str, successes: &mut i32) {
        let text = cmd.strip_prefix('/').unwrap_or(cmd);
        match self.parse_to_run(src.level, text) {
            None => {}
            Some(Ok(parsed)) => {
                let before = self.cmd.successes;
                self.cmd.successes = 0;
                self.run_parsed(src, &parsed);
                *successes = self.cmd.successes;
                self.cmd.successes = before;
            }
            Some(Err(msgs)) => {
                // Moved out when not shared with the cache.
                let msgs = std::sync::Arc::try_unwrap(msgs).unwrap_or_else(|shared| (*shared).clone());
                for m in msgs {
                    self.fail(src, m);
                }
            }
        }
    }

    /// Output of a command block. A command block tracking its output keeps it in `LastOutput`: a failure always, a success
    /// when `send_command_feedback` allows it (`command_block_output` only governs the relay to
    /// the operators, `ok`). Without tracking, the block has no output at all.
    pub(crate) fn command_block_output(&mut self, p: [i32; 3], msg: Component, success: bool) {
        let Some(at) = self.command_block_entity(p) else { return };
        let tracks = self.entity(at).and_then(|b| int(b, "TrackOutput")).is_none_or(|v| v != 0);
        if !tracks || (success && !self.rule("minecraft:send_command_feedback", true)) {
            return;
        }
        let mut c = lit(output_stamp(self.cmd.paths.local_time_cache));
        c.extra.push(msg);
        self.set_block_entity_field(at, "LastOutput", crate::commands::text::to_nbt(&c));
    }

    pub(crate) fn command_block_tracks_output(&self, p: [i32; 3]) -> bool {
        self.be_get(p).and_then(|b| int(b, "TrackOutput")).is_none_or(|v| v != 0)
    }

    /// Operator (level 2) in creative: placement and data of the game master blocks.
    pub(crate) fn can_use_game_master_blocks(&self) -> bool {
        self.game_mode == 1 && self.cmd.op >= 2
    }

    /// Command block used by an operator in creative: data sent (the screen is
    /// opened by the client); true if it was one.
    pub(crate) fn cmdblock_use(&mut self, p: [i32; 3]) -> bool {
        if self.command_block_mode(self.get(p)).is_none() || self.be_get(p).is_none() {
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
        if self.be_get(p).is_none() || self.command_block_mode(self.get(p)).is_none() {
            return;
        }
        let (track, conditional, auto) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
        let old = self.get(p);
        let facing = self.view().facing(old);
        let block = match mode {
            0 => "chain_command_block",
            1 => "repeating_command_block",
            _ => "command_block",
        };
        let b = self.table().block_id(block);
        let v = self.view();
        let mut st = v.set_dir(v.default_state(b), "facing", facing);
        st = v.set_bool(st, "conditional", conditional);
        if st != old {
            let be = self.be_get(p).cloned();
            self.set_block(p, st, crate::blocks::flag::ALL);
            if let (Some(be), Some(now)) = (be, self.be_get_mut(p)) {
                *now = be;
            }
        }
        // The block may have been replaced above: every field looked up.
        let at = BlockEntityAt::Lookup(p);
        self.set_block_entity_field(at, "Command", Nbt::String(cmd.clone()));
        self.set_block_entity_field(at, "TrackOutput", Nbt::Byte(i8::from(track)));
        if !track && let Some(b) = self.be_get_mut(p) {
            crate::nbtio::remove(b, "LastOutput");
        }
        let was_auto = self.flag(at, "auto");
        self.set_block_entity_field(at, "auto", Nbt::Byte(i8::from(auto)));
        let powered = self.flag(at, "powered");
        let new_mode = self.command_block_mode(st).unwrap_or(Mode::Impulse);
        if !was_auto && auto && !powered && new_mode != Mode::Chain {
            self.record_condition_met(p, at);
            self.schedule_tick(p, b, 1);
        }
        if new_mode == Mode::Repeating && (powered || auto) {
            self.record_condition_met(p, at);
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
        let Some(mode) = self.command_block_mode(s) else { return };
        let auto = mode == Mode::Chain;
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
                self.set_block_entity_field(BlockEntityAt::Lookup(p), k, v);
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
        let Some(mode) = self.command_block_mode(s) else { return };
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
        let at = BlockEntityAt::Lookup(p);
        let auto = self.flag(at, "auto");
        if !was_auto && auto && !self.flag(at, "powered") && mode != Mode::Chain {
            self.record_condition_met(p, at);
            let blk = self.view().block(s);
            self.schedule_tick(p, blk, 1);
        }
    }

    /// Placed by the player: tracked output according to the rule and automatic mode of the
    /// block, unless the stack carried block entity data (its own settings, already loaded);
    /// then power.
    pub(crate) fn cmdblock_placed(&mut self, p: [i32; 3], s: u32, with_data: bool) {
        let Some(mode) = self.command_block_mode(s) else { return };
        if !with_data {
            let auto = mode == Mode::Chain;
            let t = self.rule("minecraft:send_command_feedback", true);
            let at = BlockEntityAt::Lookup(p);
            self.set_block_entity_field(at, "TrackOutput", Nbt::Byte(i8::from(t)));
            // Only the chain block is automatic by itself, and it never schedules on its own.
            self.set_block_entity_field(at, "auto", Nbt::Byte(i8::from(auto)));
        }
        let powered = self.has_neighbor_signal(p);
        self.set_command_block_powered(p, s, powered);
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: seconds since the epoch the output stamps read instead of the clock (two worlds
    /// compared side by side must not see a second change between them).
    pub(crate) static FROZEN_NOW: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn now_secs() -> u64 {
    #[cfg(test)]
    if let Some(s) = FROZEN_NOW.with(std::cell::Cell::get) {
        return s;
    }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `[HH:mm:ss] ` stamp of `secs` at `offset` seconds from UTC.
fn stamp(secs: u64, offset: i64) -> String {
    let t = (secs as i64 + offset).rem_euclid(86400);
    format!("[{:02}:{:02}:{:02}] ", t / 3600, (t / 60) % 60, t % 60)
}

/// Last stamp formatted and the time zone offset read for the current minute.
#[derive(Default)]
struct StampCache {
    second: Option<u64>,
    text: String,
    minute: Option<u64>,
    offset: i64,
}

thread_local! {
    static STAMP_CACHE: std::cell::RefCell<StampCache> = std::cell::RefCell::new(StampCache::default());
}

/// Local time stamp `[HH:mm:ss] ` of a command block's output. `cached`
/// (`LOCALTIMECACHE`): formatted once per second, the system time zone read once per minute (its
/// offset only changes on whole minutes; Java even keeps the zone read at startup); a failing
/// repeating block writes its output every tick.
fn output_stamp(cached: bool) -> String {
    let secs = now_secs();
    if !cached {
        return stamp(secs, local_offset_secs());
    }
    STAMP_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.second != Some(secs) {
            let minute = secs / 60;
            if c.minute != Some(minute) {
                c.offset = local_offset_secs();
                c.minute = Some(minute);
            }
            c.text = stamp(secs, c.offset);
            c.second = Some(secs);
        }
        c.text.clone()
    })
}

/// Offset of the local time (system time zone, daylight saving time included), in seconds;
/// `RUSTCRAFT_TZ_OFFSET` forces it (tests).
fn local_offset_secs() -> i64 {
    rc_data::env_str!("RUSTCRAFT_TZ_OFFSET")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(crate::storage::local_offset_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cached stamp is the one formatted from the clock, second after second, across
    /// minute changes.
    #[test]
    fn cached_stamp_matches_the_clock() {
        for secs in (1_700_000_000u64..1_700_000_200).chain([0, 59, 60, 86_399, 86_400]) {
            FROZEN_NOW.with(|f| f.set(Some(secs)));
            assert_eq!(output_stamp(true), output_stamp(false), "at {secs}");
            assert_eq!(output_stamp(false), stamp(secs, local_offset_secs()));
        }
        FROZEN_NOW.with(|f| f.set(None));
    }
}
