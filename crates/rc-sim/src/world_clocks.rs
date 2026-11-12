//! World clocks of the integrated server: one clock per entry of the `minecraft:world_clock`
//! registry (the overworld's, the End's, those of the data packs), each with its total ticks, the
//! fraction of a tick a fractional rate leaves over, its rate and its pause. Kept in
//! `data/minecraft/world_clocks.dat`.
//!
//! `Server::clocks` holds the totals (read everywhere: daylight, spawns, sleep, predicates);
//! `Server::clock_paces` holds the pace of the clocks that left the default one.

use std::path::{Path, PathBuf};

use rc_client::Event;
use rc_proto::nbt::Nbt;

use crate::nbtio;
use crate::server::Server;

/// Pace of one clock: what `time pause`, `time resume` and `time rate` change, and the fraction
/// of a tick left over by a fractional rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClockPace {
    pub partial_tick: f32,
    pub rate: f32,
    pub paused: bool,
}

impl Default for ClockPace {
    fn default() -> ClockPace {
        ClockPace { partial_tick: 0.0, rate: 1.0, paused: false }
    }
}

impl ClockPace {
    /// Whole ticks the clock gains during one server tick; the rest stays in the fraction.
    /// Single-precision steps of the game, taken at rate 1 too: a fraction left by an earlier
    /// rate keeps the game's rounding.
    fn whole_ticks_of_one_tick(&mut self) -> i32 {
        if self.paused {
            return 0;
        }
        self.partial_tick += self.rate;
        let whole = rc_data::java_math::floor_f32_to_i32(self.partial_tick);
        self.partial_tick -= whole as f32;
        whole
    }
}

/// One clock as `world_clocks.dat` keeps it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SavedClock {
    pub name: String,
    pub total_ticks: i64,
    pub pace: ClockPace,
}

pub(crate) fn file_path(world: &Path) -> PathBuf {
    world.join("data").join("minecraft").join("world_clocks.dat")
}

/// Clocks of a world at opening: one per registry entry, in registry order, at zero and at the
/// default pace, then the saved states laid over them (a new world has no End clock in its file,
/// a world of an older version neither: both get one). A saved clock that the registry does not
/// know is dropped, like in the game.
pub(crate) fn read(world: &Path, registry: &[String]) -> Vec<SavedClock> {
    let saved = nbtio::read_dat(&file_path(world)).ok().and_then(|root| root.get("data").cloned());
    registry
        .iter()
        .map(|name| {
            let (total_ticks, pace) = saved
                .as_ref()
                .and_then(|clocks| file_entry(clocks, name))
                .and_then(accepted_entry)
                .unwrap_or_else(|| (0, ClockPace::default()));
            SavedClock { name: name.clone(), total_ticks, pace }
        })
        .collect()
}

/// Entry of clock `name` in the file's map; the default namespace may be left out of its key.
fn file_entry<'a>(clocks: &'a Nbt, name: &str) -> Option<&'a Nbt> {
    clocks.get(name).or_else(|| clocks.get(name.strip_prefix("minecraft:")?))
}

/// Total and pace of a file entry, `None` when the game would refuse it (no total, rate not
/// above zero): the clock then starts from zero.
fn accepted_entry(clock: &Nbt) -> Option<(i64, ClockPace)> {
    let total_ticks = nbtio::int(clock, "total_ticks")?;
    let rate = nbtio::float(clock, "rate").unwrap_or(1.0);
    let rate_accepted = rate > 0.0 && rate <= f32::MAX;
    if !rate_accepted {
        return None;
    }
    let partial_tick = nbtio::float(clock, "partial_tick").unwrap_or(0.0);
    let paused = nbtio::int(clock, "paused").is_some_and(|v| v != 0);
    Some((total_ticks, ClockPace { partial_tick, rate, paused }))
}

/// Contents of `world_clocks.dat`. A field at its default value is left out, like the game does
/// (compared bit for bit: a negative zero fraction is written).
pub(crate) fn file_nbt(clocks: &[SavedClock]) -> Nbt {
    let default = ClockPace::default();
    let clock_nbt = |c: &SavedClock| {
        let mut fields = vec![("total_ticks".to_owned(), Nbt::Long(c.total_ticks))];
        if c.pace.partial_tick.to_bits() != default.partial_tick.to_bits() {
            fields.push(("partial_tick".to_owned(), Nbt::Float(c.pace.partial_tick)));
        }
        if c.pace.rate.to_bits() != default.rate.to_bits() {
            fields.push(("rate".to_owned(), Nbt::Float(c.pace.rate)));
        }
        if c.pace.paused {
            fields.push(("paused".to_owned(), Nbt::Byte(1)));
        }
        Nbt::Compound(fields)
    };
    Nbt::Compound(vec![
        ("data".to_owned(), Nbt::Compound(clocks.iter().map(|c| (c.name.clone(), clock_nbt(c))).collect())),
        ("DataVersion".to_owned(), Nbt::Int(crate::chunk::DATA_VERSION)),
    ])
}

impl Server {
    /// Pace of a clock (the default one when it never left it).
    pub(crate) fn clock_pace(&self, clock: &str) -> ClockPace {
        self.clock_paces.get(clock).copied().unwrap_or_default()
    }

    /// Total ticks of a clock (0 if unknown).
    pub(crate) fn clock_total(&self, clock: &str) -> i64 {
        self.clocks.iter().find(|c| c.0 == clock).map_or(0, |c| c.1)
    }

    /// Clocks as `world_clocks.dat` keeps them.
    pub(crate) fn clocks_to_save(&self) -> Vec<SavedClock> {
        self.clocks
            .iter()
            .map(|(name, total_ticks)| SavedClock {
                name: name.clone(),
                total_ticks: *total_ticks,
                pace: self.clock_pace(name),
            })
            .collect()
    }

    /// One server tick of every clock (rule `advance_time` on).
    pub(crate) fn advance_clocks(&mut self) {
        for (name, total_ticks) in &mut self.clocks {
            // No pace entry (nearly every clock of every world): the default pace gives exactly
            // one tick and leaves no fraction, without touching a float.
            let whole = self.clock_paces.get_mut(name.as_str()).map_or(1, ClockPace::whole_ticks_of_one_tick);
            *total_ticks = total_ticks.wrapping_add(i64::from(whole));
        }
    }

    /// State of a clock as the time packet carries it: total, fraction, and its rate, 0 when it
    /// stands still (paused, or rule `advance_time` off).
    fn clock_in_time_packet(&self, clock: &str, total_ticks: i64) -> (String, i64, f32, f32) {
        let pace = self.clock_pace(clock);
        let stands_still = pace.paused || !self.rule("minecraft:advance_time", true);
        (clock.to_owned(), total_ticks, pace.partial_tick, if stands_still { 0.0 } else { pace.rate })
    }

    /// Time packet: the game time alone (every second), or `with_every_clock` (entering the game
    /// or a dimension, rule `advance_time` changed).
    pub(crate) fn send_time(&self, with_every_clock: bool) {
        let clocks = if with_every_clock {
            self.clocks.iter().map(|(name, total_ticks)| self.clock_in_time_packet(name, *total_ticks)).collect()
        } else {
            Vec::new()
        };
        let _ = self.send(Event::Time { game_time: self.time, clocks });
    }

    /// Time packet carrying the one clock that just changed: the game sends no other clock, and
    /// sends it to every player. The clocks belong to the whole world, so the packet also leaves
    /// while the server works in a parked dimension (`execute in`, a command block ticking
    /// there), where `send` is cut off: it goes straight to the players.
    pub(crate) fn send_clock(&self, clock: &str) {
        let Some((_, total_ticks)) = self.clocks.iter().find(|c| c.0 == clock) else { return };
        let clocks = vec![self.clock_in_time_packet(clock, *total_ticks)];
        let _ = self.lan_route(Event::Time { game_time: self.time, clocks });
    }

    fn write_clock_total(&mut self, clock: &str, total_ticks: i64) {
        match self.clocks.iter_mut().find(|c| c.0 == clock) {
            Some(c) => c.1 = total_ticks,
            None => self.clocks.push((clock.to_owned(), total_ticks)),
        }
    }

    /// Sets the total of a clock (`time set`, time marker, wake-up, test environment): the
    /// fraction of a tick starts again from zero.
    pub(crate) fn set_clock(&mut self, clock: &str, total_ticks: i64) {
        self.write_clock_total(clock, total_ticks);
        if let Some(pace) = self.clock_paces.get_mut(clock) {
            pace.partial_tick = 0.0;
        }
        self.send_clock(clock);
    }

    /// Adds ticks to a clock (`time add`), never below zero; the fraction of a tick is kept.
    /// Returns the new total.
    pub(crate) fn add_clock_ticks(&mut self, clock: &str, ticks: i32) -> i64 {
        let total_ticks = self.clock_total(clock).wrapping_add(i64::from(ticks)).max(0);
        self.write_clock_total(clock, total_ticks);
        self.send_clock(clock);
        total_ticks
    }

    /// `time pause` and `time resume`.
    pub(crate) fn set_clock_paused(&mut self, clock: &str, paused: bool) {
        self.clock_paces.entry(clock.to_owned()).or_default().paused = paused;
        self.send_clock(clock);
    }

    /// `time rate`.
    pub(crate) fn set_clock_rate(&mut self, clock: &str, rate: f32) {
        self.clock_paces.entry(clock.to_owned()).or_default().rate = rate;
        self.send_clock(clock);
    }
}
