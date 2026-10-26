//! Integrated server jukebox: inserted disc (item with the `jukebox_playable` component),
//! playback (event 1010, note particle and game event every 20 ticks, stop 1011 at the end
//! of the track + 20 ticks), ejection with an empty hand, signal 15 during playback, comparator
//! output specific to the track, disc returned when the block disappears.
//!
//! Block entity data (the game's save keys): `RecordItem` (the stack) and
//! `ticks_since_song_started` (present only during playback). The current track
//! (registry index, duration) is kept in the tickers table so the pack is not read again.

use rc_proto::nbt::Nbt;

use super::level::BeTick;
use super::use_block::ItemOn;
use super::{P, flag};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

/// Current track: index in the `jukebox_song` registry, end ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Song {
    pub index: i32,
    pub finish: i64,
}

impl Server {
    /// Identifier of the track of a stack (component on the stack, otherwise that of the item).
    pub(crate) fn jukebox_song_of(&self, s: &Stack) -> Option<String> {
        if let Some(c) = crate::items::component(s, "minecraft:jukebox_playable") {
            return c.as_str().or_else(|| c.get("song").and_then(Nbt::as_str)).map(str::to_owned);
        }
        let v = self.life.items.default_component(s, "minecraft:jukebox_playable")?;
        v.as_str().or_else(|| v["song"].as_str()).map(str::to_owned)
    }

    /// Registry index and end ticks (`ceil(duration × 20) + 20`) of a track.
    fn song_info(&self, id: &str) -> Option<(Song, i32)> {
        let full = if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") };
        let index = self.regs.entries("minecraft:jukebox_song").iter().position(|e| *e == full)? as i32;
        let v = self.life.pack.get("jukebox_song", &full)?;
        let secs = v["length_in_seconds"].as_f64().unwrap_or(0.0) as f32;
        let finish = i64::from((secs * 20.0).ceil() as i32) + 20;
        let comparator = v["comparator_output"].as_i64().unwrap_or(0) as i32;
        Some((Song { index, finish }, comparator))
    }

    /// Disc of the block entity.
    fn jukebox_item(&self, p: P) -> Option<Stack> {
        self.be_get(p).and_then(|b| b.get("RecordItem")).and_then(Stack::from_nbt)
    }

    /// Disc inserted into an empty jukebox.
    pub(crate) fn jukebox_use_item(&mut self, p: P, s: u32, slot: usize) -> ItemOn {
        if self.v().get_bool(s, "has_record") {
            return ItemOn::TryEmptyHand;
        }
        let Some(held) = self.slots[slot].clone() else { return ItemOn::TryEmptyHand };
        if self.jukebox_song_of(&held).is_none() {
            return ItemOn::TryEmptyHand;
        }
        let inserted = Stack { count: 1, ..held };
        self.consume_held(slot);
        if self.be_get(p).is_some() {
            self.jukebox_set_item(p, Some(inserted), s);
            self.game_event_block("minecraft:block_change", p, Some(Ent::Player), Some(s));
        }
        self.award_stat("minecraft:custom", "minecraft:play_record", 1);
        ItemOn::Done
    }

    /// Disc ejected.
    pub(crate) fn jukebox_use_without_item(&mut self, p: P, s: u32) -> bool {
        if !self.v().get_bool(s, "has_record") || self.be_get(p).is_none() {
            return false;
        }
        self.jukebox_pop_out(p, s);
        true
    }

    /// `has_record` state (only if the world still has the block entity's `be_state`
    /// state), track playback or stop.
    pub(crate) fn jukebox_set_item(&mut self, p: P, item: Option<Stack>, be_state: u32) {
        if let Some(be) = self.be_get_mut(p) {
            match &item {
                Some(it) => crate::nbtio::set(be, "RecordItem", it.to_nbt(None)),
                None => crate::nbtio::remove(be, "RecordItem"),
            }
        }
        let song = item.as_ref().and_then(|it| self.jukebox_song_of(it)).and_then(|id| self.song_info(&id));
        let mut be_state = be_state;
        if self.get(p) == be_state {
            be_state = self.v().set_bool(be_state, "has_record", item.is_some());
            self.set_block(p, be_state, flag::CLIENTS);
            self.game_event_block("minecraft:block_change", p, None, Some(be_state));
        }
        match song {
            Some((song, _)) if item.is_some() => self.jukebox_play(p, song),
            _ => self.jukebox_stop(p, be_state),
        }
    }

    /// Playback from 0, 1010 (track index), neighbours notified.
    fn jukebox_play(&mut self, p: P, song: Song) {
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::set(be, "ticks_since_song_started", Nbt::Long(0));
        }
        if let Some((_, BeTick::Jukebox(cur))) = self.bs.be_ticks.get_mut(&p) {
            *cur = Some(song);
        }
        self.level_event(1010, p, song.index);
        self.jukebox_song_changed(p);
    }

    /// Only if a track is playing (`s`: block entity state).
    fn jukebox_stop(&mut self, p: P, s: u32) {
        let playing = self.be_get(p).is_some_and(|b| b.get("ticks_since_song_started").is_some());
        if !playing {
            return;
        }
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::remove(be, "ticks_since_song_started");
        }
        if let Some((_, BeTick::Jukebox(cur))) = self.bs.be_ticks.get_mut(&p) {
            *cur = None;
        }
        self.game_event_block("minecraft:jukebox_stop_play", p, None, Some(s));
        self.level_event(1011, p, 0);
        self.jukebox_song_changed(p);
    }

    /// Neighbours notified (signal 15 / 0), then comparators (including
    /// behind a full block), in the name of the jukebox itself even when the block was just
    /// replaced.
    fn jukebox_song_changed(&mut self, p: P) {
        let b = self.tb().block_id("minecraft:jukebox");
        self.update_neighbors_at(p, b);
        self.update_neighbour_for_output_signal(p, b);
    }

    /// Disc removed (stop), item placed above the block, neighbours notified.
    fn jukebox_pop_out(&mut self, p: P, be_state: u32) {
        let Some(item) = self.jukebox_item(p) else { return };
        self.jukebox_set_item(p, None, be_state);
        let r = &mut self.life.rng;
        let ox = f64::from((r.next_float() - 0.5) * 0.7);
        let oz = f64::from((r.next_float() - 0.5) * 0.7);
        let at = [f64::from(p[0]) + 0.5 + ox, f64::from(p[1]) + 1.01, f64::from(p[2]) + 0.5 + oz];
        self.spawn_item_random(at, item, 10);
        self.jukebox_song_changed(p);
    }

    /// Block entity ticker (only with a disc): end of the track, or every 20
    /// ticks the `jukebox_play` event and a note; then counter + 1.
    pub(crate) fn jukebox_tick(&mut self, p: P, song: Option<Song>) {
        let Some(song) = song else { return };
        let Some(t) = self.be_get(p).and_then(|b| b.get("ticks_since_song_started")).and_then(Nbt::as_i64) else {
            return;
        };
        if t >= song.finish {
            let s = self.get(p);
            self.jukebox_stop(p, s);
            return;
        }
        if t % 20 == 0 {
            let s = self.get(p);
            self.game_event_block("minecraft:jukebox_play", p, None, Some(s));
            let color = self.life.rng.next_int_bound(4) as f32 / 24.0;
            let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + f64::from(1.2f32), f64::from(p[2]) + 0.5];
            self.particles("minecraft:note", at, [color, 0.0, 0.0], 1.0, 0);
        }
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::set(be, "ticks_since_song_started", Nbt::Long(t + 1));
        }
    }

    /// Ticker up to date after a state change: present while `has_record` (a new
    /// ticker at the end of the list when it reappears, like the game).
    pub(crate) fn jukebox_ticker_sync(&mut self, p: P, s: u32) {
        let want = self.v().get_bool(s, "has_record");
        let have = matches!(self.bs.be_ticks.get(&p), Some((_, BeTick::Jukebox(_))));
        if want && !have {
            // Playback in progress (counter present): the disc's track, kept by the ticker.
            let song = self.jukebox_playing_song(p);
            let id = self.add_ticker(p);
            self.bs.be_ticks.insert(p, (id, BeTick::Jukebox(song)));
        } else if !want && have {
            self.bs.be_ticks.remove(&p);
        }
    }

    /// Current track: counter present (the key exists only during playback) and track
    /// of the disc.
    fn jukebox_playing_song(&self, p: P) -> Option<Song> {
        self.be_get(p)?.get("ticks_since_song_started")?;
        let item = self.jukebox_item(p)?;
        self.song_info(&self.jukebox_song_of(&item)?).map(|(s, _)| s)
    }

    /// Block entity read back from a save: a counter without a track
    /// or with a finished track is not playback, the key disappears (signal 0).
    pub(crate) fn jukebox_loaded_from_save(&mut self, p: P) {
        let Some(t) = self.be_get(p).and_then(|b| b.get("ticks_since_song_started")).and_then(Nbt::as_i64) else {
            return;
        };
        if self.jukebox_playing_song(p).is_none_or(|s| t >= s.finish)
            && let Some(be) = self.be_get_mut(p)
        {
            crate::nbtio::remove(be, "ticks_since_song_started");
        }
    }

    /// Block entity rewritten by a command (reading its own fields on the live object, `old`: the
    /// data from before): another disc stops the playback; a valid counter (unfinished track)
    /// resumes it at that point; otherwise the earlier playback continues (earlier counter).
    pub(crate) fn jukebox_reloaded(&mut self, p: P, old: &Nbt) {
        let old_item = old.get("RecordItem").and_then(Stack::from_nbt);
        let new_item = self.jukebox_item(p);
        let ticker_song = match self.bs.be_ticks.get(&p) {
            Some((_, BeTick::Jukebox(s))) => *s,
            _ => None,
        };
        // Earlier playback: counter and track (the ticker's, otherwise that of the old disc).
        let mut playing = old.get("ticks_since_song_started").and_then(Nbt::as_i64).and_then(|t| {
            let song = ticker_song.or_else(|| {
                let id = self.jukebox_song_of(old_item.as_ref()?)?;
                self.song_info(&id).map(|(s, _)| s)
            })?;
            Some((song, t))
        });
        let changed = match (&old_item, &new_item) {
            (Some(a), Some(b)) => !crate::items::same(a, b),
            (Some(_), None) => true,
            _ => false,
        };
        if changed && playing.take().is_some() {
            let s = self.get(p);
            self.game_event_block("minecraft:jukebox_stop_play", p, None, Some(s));
            self.level_event(1011, p, 0);
            self.jukebox_song_changed(p);
        }
        if let Some(t) = self.be_get(p).and_then(|b| b.get("ticks_since_song_started")).and_then(Nbt::as_i64)
            && let Some(id) = new_item.as_ref().and_then(|it| self.jukebox_song_of(it))
            && let Some((song, _)) = self.song_info(&id)
            && t < song.finish
        {
            playing = Some((song, t));
        }
        if let Some(be) = self.be_get_mut(p) {
            match playing {
                Some((_, t)) => crate::nbtio::set(be, "ticks_since_song_started", Nbt::Long(t)),
                None => crate::nbtio::remove(be, "ticks_since_song_started"),
            }
        }
        if let Some((_, BeTick::Jukebox(cur))) = self.bs.be_ticks.get_mut(&p) {
            *cur = playing.map(|(s, _)| s);
        }
    }

    /// Weak signal: 15 during playback.
    pub(crate) fn jukebox_signal(&self, p: P) -> i32 {
        if self.be_get(p).is_some_and(|b| b.get("ticks_since_song_started").is_some()) { 15 } else { 0 }
    }

    /// Comparator output: that of the disc's track, 0 without a disc.
    pub(crate) fn jukebox_comparator(&self, p: P) -> i32 {
        self.jukebox_item(p)
            .and_then(|it| self.jukebox_song_of(&it))
            .and_then(|id| self.song_info(&id))
            .map_or(0, |(_, c)| c)
    }

    /// Block entity removed by a placement (side effects before removal, then the removal of the entity): disc
    /// ejected (the state is no longer the jukebox's), then stop announced.
    pub(crate) fn jukebox_pre_remove(&mut self, p: P, old: u32) {
        self.jukebox_pop_out(p, old);
        self.bs.be_ticks.remove(&p);
        self.game_event_block("minecraft:jukebox_stop_play", p, None, Some(old));
        self.level_event(1011, p, 0);
    }
}
