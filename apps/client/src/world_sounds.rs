//! World sounds played by the client without a packet (`docs/specs/sounds.md` § 5, § 6): background
//! music, biome ambience, underwater ambience, local player footsteps, and the sounds of the block
//! sound sets for world events 2001 (block broken) and 2020 (mining hit).

use rc_audio::ambience::{Ambience, AmbientSettings, Mood, Music, Track};
use rc_audio::{AudioEngine, Category, JavaRandom, SoundHandle, SoundRequest};
use rc_data::{SoundType, VersionData, flags};
use rc_env::{AmbientSounds, Attr, BackgroundMusic, MusicTrack};
use rc_physics::{Player, Tables, World};

use crate::environ::{EnvState, WorldView};

/// Menu music (no player) and End fight music: coded by the game, not attributes.
fn fixed_track(event: &str, min_delay: i32, max_delay: i32) -> Track<'_> {
    Track { event, min_delay, max_delay, replace: true }
}

fn track(t: &MusicTrack) -> Track<'_> {
    Track { event: &t.sound, min_delay: t.min_delay, max_delay: t.max_delay, replace: t.replace }
}

/// Sound set of a block state.
pub fn sound_type(data: &VersionData, state: u32) -> Option<&SoundType> {
    data.states.get(state as usize).and_then(|s| data.sound_types.get(usize::from(s.sound)))
}

fn is_air(data: &VersionData, state: u32) -> bool {
    data.states.get(state as usize).is_none_or(|s| s.flags & flags::AIR != 0)
}

/// Client world state for the packet-less sounds.
pub struct WorldSounds {
    /// Music imposed by the open screen (end screen: `music.credits`).
    pub screen_music: Option<&'static str>,
    music: Music,
    ambience: Ambience,
    /// Client world randomness (variants and draws of local sounds).
    rng: JavaRandom,
    /// Abilities: instant build (creative).
    pub instabuild: bool,
    /// Footsteps: cumulated distance (× 0.6) and threshold of the next step.
    move_dist: f32,
    next_step: f32,
    /// Player tick counter, intensity and last tick of the amethyst chime.
    tick_count: i32,
    crystal: f32,
    crystal_tick: i32,
    /// Block tags, by block index.
    inside_step: Vec<bool>,
    combination_step: Vec<bool>,
    crystal_blocks: Vec<bool>,
    /// Underwater: previous state, loop (sound, fade), additions in progress, wait.
    underwater: bool,
    water_loop: Option<(SoundHandle, i32)>,
    water_subs: Vec<SoundHandle>,
    water_delay: i32,
    /// Gliding: state at the last check, loops in progress (sound, ticks elapsed). Resuming
    /// flight before the old loop stops starts a second one, like the game.
    was_fall_flying: bool,
    elytra: Vec<(SoundHandle, i32)>,
    /// End flash: pending sound (ticks left, direction ×10), current sound (sound,
    /// direction ×10) that follows the camera.
    end_flash_wait: Option<(i32, [f64; 3])>,
    end_flash: Option<(SoundHandle, [f64; 3])>,
}

/// Delay of the End flash sound after its start (ticks).
const END_FLASH_DELAY: i32 = 30;
/// Gliding ticks before the elytra loop becomes audible.
const ELYTRA_DELAY: i32 = 20;

/// Volume and pitch of the elytra loop at loop tick `time`, for a squared
/// speed `speed_sq`: speed² / 4 capped at 1 (zero under 1e-7), silent before 20 ticks, fade-in
/// up to 40; beyond 0.8 volume, the pitch rises by as much.
pub fn elytra_params(time: i32, speed_sq: f32) -> (f32, f32) {
    let mut v = if speed_sq >= 1.0e-7 { (speed_sq / 4.0).clamp(0.0, 1.0) } else { 0.0 };
    if time < ELYTRA_DELAY {
        v = 0.0;
    } else if time < 2 * ELYTRA_DELAY {
        v *= (time - ELYTRA_DELAY) as f32 / 20.0;
    }
    (v, if v > 0.8 { 1.0 + (v - 0.8) } else { 1.0 })
}

impl Default for WorldSounds {
    fn default() -> Self {
        WorldSounds {
            screen_music: None,
            music: Music::default(),
            ambience: Ambience::default(),
            rng: JavaRandom::from_entropy(),
            instabuild: false,
            move_dist: 0.0,
            next_step: 1.0,
            tick_count: 0,
            crystal: 0.0,
            crystal_tick: 0,
            inside_step: Vec::new(),
            combination_step: Vec::new(),
            crystal_blocks: Vec::new(),
            underwater: false,
            water_loop: None,
            water_subs: Vec::new(),
            water_delay: 0,
            was_fall_flying: false,
            elytra: Vec::new(),
            end_flash_wait: None,
            end_flash: None,
        }
    }
}

/// Tick reads: world, environment, player.
pub struct TickCtx<'a> {
    pub view: &'a WorldView<'a>,
    pub env: &'a mut EnvState,
    pub player: Option<&'a Player>,
    /// Camera (music probe).
    pub camera: [f64; 3],
    pub dimension: &'a str,
    /// Accumulated boss bar flags (bit 2: boss music).
    pub boss_flags: u8,
    /// World being loaded (no music yet).
    pub loading: bool,
    /// Singleplayer paused: only the music advances (ambiences stopped with the player).
    pub paused: bool,
}

impl WorldSounds {
    /// Tags `inside_step_sound_blocks`, `combination_step_sound_blocks`, `crystal_sound_blocks`.
    pub fn set_block_tags(&mut self, blocks: usize, tags: &[(String, Vec<i32>)]) {
        for (name, v) in [
            ("minecraft:inside_step_sound_blocks", &mut self.inside_step),
            ("minecraft:combination_step_sound_blocks", &mut self.combination_step),
            ("minecraft:crystal_sound_blocks", &mut self.crystal_blocks),
        ] {
            *v = vec![false; blocks];
            if let Some((_, ids)) = tags.iter().find(|(n, _)| n == name) {
                for &i in ids {
                    if let Some(b) = usize::try_from(i).ok().and_then(|i| v.get_mut(i)) {
                        *b = true;
                    }
                }
            }
        }
    }

    /// Music manager (current song: "now playing" notification).
    pub fn music(&self) -> &Music {
        &self.music
    }

    /// Setting `musicFrequency` ("DEFAULT", "FREQUENT", "CONSTANT").
    pub fn set_music_frequency(&mut self, name: &str, reset: bool) {
        use rc_audio::ambience::Frequency;
        self.music.set_frequency(
            match name {
                "FREQUENT" => Frequency::Frequent,
                "CONSTANT" => Frequency::Constant,
                _ => Frequency::Default,
            },
            reset,
        );
    }

    /// Ambient mood (0 to 1), F3 line `sound_mood`.
    pub fn mood(&self) -> f32 {
        self.ambience.mood()
    }

    /// Entering a world: ambience and underwater reset to zero (the music continues).
    pub fn join(&mut self, audio: Option<&mut AudioEngine>) {
        if let Some(a) = audio {
            if let Some((h, _)) = self.water_loop.take() {
                a.stop_handle(h);
            }
            for h in self.water_subs.drain(..) {
                a.stop_handle(h);
            }
            // New local player: the old one's elytra loops stop.
            for (h, _) in self.elytra.drain(..) {
                a.stop_handle(h);
            }
            // World change: the game cuts all sounds, queue included.
            if let Some((h, _)) = self.end_flash.take() {
                a.stop_handle(h);
            }
        }
        self.elytra.clear();
        self.was_fall_flying = false;
        self.end_flash_wait = None;
        self.end_flash = None;
        self.ambience = Ambience::default();
        self.underwater = false;
        self.move_dist = 0.0;
        self.next_step = 1.0;
    }

    /// Respawn (respawn packet). Dimension change: new client world, the
    /// game then cuts all sounds (deferred queue included), as on entering the game. Same
    /// dimension: only the local player is replaced, its elytra loops stop and the
    /// new player restarts out of gliding (a loop restarts if it is still gliding).
    pub fn respawn(&mut self, audio: Option<&mut AudioEngine>, dimension_changed: bool) {
        if dimension_changed {
            return self.join(audio);
        }
        if let Some(a) = audio {
            for (h, _) in self.elytra.drain(..) {
                a.stop_handle(h);
            }
        }
        self.elytra.clear();
        self.was_fall_flying = false;
    }

    /// End flash started at this tick (flash angles): sound delayed by 30 ticks.
    pub fn queue_end_flash(&mut self, (x_angle, y_angle): (f32, f32)) {
        let d = crate::environ::end_flash_direction(x_angle, y_angle).map(|v| v * 10.0);
        self.end_flash_wait = Some((END_FLASH_DELAY, d));
    }

    /// End flash sound (`weather.end_flash`, weather): started at the deadline 10 blocks from
    /// the camera in the direction of the flash, unattenuated, then repositioned every tick
    /// to follow the camera (`camera`: camera position of the last frame).
    pub fn tick_end_flash(&mut self, audio: &mut AudioEngine, camera: [f64; 3]) {
        let at = |d: [f64; 3]| std::array::from_fn(|i| camera[i] + d[i]);
        if let Some((h, d)) = self.end_flash {
            if audio.is_playing(h) {
                audio.set_position(h, at(d));
            } else {
                self.end_flash = None;
            }
        }
        let Some((left, d)) = &mut self.end_flash_wait else { return };
        *left -= 1;
        if *left > 0 {
            return;
        }
        let d = *d;
        self.end_flash_wait = None;
        let req = SoundRequest::new("minecraft:weather.end_flash", Category::Weather).at_unattenuated(at(d));
        if let Some(h) = audio.play_request(req) {
            self.end_flash = Some((h, d));
        }
    }

    /// Elytra loop (`item.elytra.flying`, players), after the tick's movement: a
    /// loop starts at each start of gliding; each one follows the player, takes the volume and
    /// pitch of `elytra_params`, and stops after 20 ticks if the flight has ceased.
    pub fn tick_elytra(&mut self, audio: &mut AudioEngine, p: &Player) {
        if p.fall_flying && !self.was_fall_flying {
            // The game starts it at 0.1 volume before placing it: null position at the start.
            let req = SoundRequest::new("minecraft:item.elytra.flying", Category::Player)
                .at([0.0; 3])
                .volume(0.1)
                .looping(0);
            if let Some(h) = audio.play_request(req) {
                self.elytra.push((h, 0));
            }
        }
        self.was_fall_flying = p.fall_flying;
        if self.elytra.is_empty() {
            return;
        }
        let pos = p.pos.map(|v| f64::from(v as f32));
        let v = p.vel;
        let speed_sq = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) as f32;
        self.elytra.retain_mut(|(h, time)| {
            *time += 1;
            if !audio.is_playing(*h) {
                return false;
            }
            if *time > ELYTRA_DELAY && !p.fall_flying {
                audio.stop_handle(*h);
                return false;
            }
            let (volume, pitch) = elytra_params(*time, speed_sq);
            audio.set_position(*h, pos);
            audio.set_sound_params(*h, volume, pitch);
            true
        });
    }

    fn tagged(v: &[bool], data: &VersionData, state: u32) -> bool {
        data.states.get(state as usize).is_some_and(|s| v.get(usize::from(s.block)).copied().unwrap_or(false))
    }

    /// Music (§ 6), then biome ambience (§ 5.4) and underwater (§ 5.5): start of the client tick.
    pub fn tick(&mut self, audio: &mut AudioEngine, c: TickCtx<'_>) {
        // `RUSTCRAFT_OPT_SOUNDIDS`: audio attributes borrowed; 0 = copied on every tick (old).
        let borrow = rc_audio::sound_ids_enabled();
        let (bm_copy, track_copy, ambient_copy): (BackgroundMusic, MusicTrack, AmbientSounds);
        // 6.2 Situation music.
        let (situation, volume) = match c.player {
            _ if self.screen_music.is_some() => (self.screen_music.map(|m| fixed_track(m, 0, 0)), 1.0),
            None => (Some(fixed_track("minecraft:music.menu", 20, 600)), 1.0),
            Some(p) => {
                let biome = c.env.biome_at(c.view, c.camera);
                let volume = c.env.env.eval_in_biome(Attr::MusicVolume, biome)[0];
                let m = if c.dimension == "minecraft:the_end" && c.boss_flags & 2 != 0 {
                    Some(fixed_track("minecraft:music.dragon", 0, 0))
                } else {
                    let bm = if borrow {
                        c.env.env.background_music_ref(biome)
                    } else {
                        bm_copy = c.env.env.background_music(biome);
                        &bm_copy
                    };
                    let creative = self.instabuild && p.may_fly;
                    let pick = if p.eyes_in_water && bm.underwater.is_some() {
                        bm.underwater.as_ref()
                    } else if creative && bm.creative.is_some() {
                        bm.creative.as_ref()
                    } else {
                        bm.default.as_ref()
                    };
                    match pick {
                        Some(t) if !borrow => {
                            track_copy = t.clone();
                            Some(track(&track_copy))
                        }
                        _ => pick.map(track),
                    }
                };
                (m, volume)
            }
        };
        self.music.tick(audio, situation.as_ref(), volume, c.loading);

        let Some(p) = c.player.filter(|_| !c.paused) else { return };
        // 5.4 Ambience of the biome under the feet (quart containing the player's position).
        let biome = c.env.biome_at(c.view, p.pos);
        let a = if borrow {
            c.env.env.ambient_sounds_ref(biome)
        } else {
            ambient_copy = c.env.env.ambient_sounds(biome);
            &ambient_copy
        };
        let settings = AmbientSettings {
            loop_sound: a.loop_sound.as_deref(),
            additions: &a.additions,
            mood: a.mood.as_ref().map(|m| Mood {
                sound: &m.sound,
                tick_delay: m.tick_delay,
                block_search_extent: m.block_search_extent,
                offset: m.offset,
            }),
        };
        let eye = [p.pos[0], p.pos[1] + p.eye_height(), p.pos[2]];
        let view = c.view;
        let top = view.min_y + view.height;
        let light = move |x: i32, y: i32, z: i32| {
            if y >= top {
                return (15, 0);
            }
            let (block, sky) = view.light_pair(x, y, z);
            (sky, block)
        };
        self.ambience.tick(audio, &settings, eye, &light);

        // 5.5 Underwater (not in spectator).
        let under = if c.env.game_mode == 3 { self.underwater } else { p.eyes_in_water };
        if under != self.underwater {
            let ev = if under { "minecraft:ambient.underwater.enter" } else { "minecraft:ambient.underwater.exit" };
            audio.play(ev, Category::Ambient, Some(p.pos), 1.0, 1.0, None);
            if under && self.water_loop.is_none() {
                let req = SoundRequest::new("minecraft:ambient.underwater.loop", Category::Ambient).looping(0);
                self.water_loop = audio.play_request(req).map(|h| (h, 0));
            }
            if !under {
                for h in self.water_subs.drain(..) {
                    audio.stop_handle(h);
                }
            }
            self.underwater = under;
        }
        if let Some((h, fade)) = &mut self.water_loop {
            if *fade >= 0 {
                *fade = (*fade + if under { 1 } else { -2 }).min(40);
                audio.set_sound_params(*h, (*fade as f32 / 40.0).clamp(0.0, 1.0), 1.0);
            }
            if *fade < 0 {
                audio.stop_handle(*h);
                self.water_loop = None;
            }
        }
        self.water_subs.retain(|&h| audio.is_playing(h));
        self.water_delay -= 1;
        if self.water_delay <= 0 && under {
            let f = self.rng.next_float();
            let ev = if f < 1.0e-4 {
                Some("minecraft:ambient.underwater.loop.additions.ultra_rare")
            } else if f < 0.001 {
                Some("minecraft:ambient.underwater.loop.additions.rare")
            } else if f < 0.01 {
                Some("minecraft:ambient.underwater.loop.additions")
            } else {
                None
            };
            if let Some(ev) = ev {
                self.water_delay = 0;
                if let Some(h) = audio.play(ev, Category::Ambient, None, 1.0, 1.0, None) {
                    self.water_subs.push(h);
                }
            }
        }
    }

    /// Local player footsteps (§ 5.1), after its movement of the tick: `moved` = applied
    /// movement (end − start of the tick; teleportations are not part of it).
    pub fn steps(
        &mut self,
        audio: &mut AudioEngine,
        data: &VersionData,
        phys: &Tables,
        world: &dyn World,
        p: &Player,
        moved: [f64; 3],
    ) {
        self.tick_count += 1;
        // Zero emission: in flight, or crouching on the ground.
        if p.flying || (p.on_ground && p.crouching) {
            return;
        }
        let state = |q: [i32; 3]| world.state(q[0], q[1], q[2]).unwrap_or(0);
        let support_pos = p.on_pos(phys, world, 1.0e-5);
        let effect_pos = p.on_pos(phys, world, 0.2);
        let (support, effect) = (state(support_pos), state(effect_pos));
        let len = (moved[0] * moved[0] + moved[1] * moved[1] + moved[2] * moved[2]).sqrt();
        let horiz = (moved[0] * moved[0] + moved[2] * moved[2]).sqrt();
        let climbing = phys.climbable(support);
        self.move_dist += if climbing { (len * 0.6) as f32 } else { (horiz * 0.6) as f32 };
        if !(self.move_dist > self.next_step && !is_air(data, support)) {
            return;
        }
        let ok = |s: u32| {
            !is_air(data, s) && (p.on_ground || phys.climbable(s) || (p.crouching && moved[1] == 0.0)) && !p.swimming
        };
        let mut produced = ok(effect);
        if produced {
            self.step_sound(audio, data, world, p, effect_pos, effect);
        }
        if support_pos != effect_pos {
            produced |= ok(support);
        }
        let in_water = p.water_height > 0.0;
        if produced {
            self.next_step = self.move_dist.floor() + 1.0;
        } else if in_water {
            self.next_step = self.move_dist.floor() + 1.0;
            self.swim_sound(audio, p);
        }
    }

    fn play_at(&mut self, audio: &mut AudioEngine, ev: &str, pos: [f64; 3], volume: f32, pitch: f32) {
        audio.play(ev, Category::Player, Some(pos), volume, pitch, None);
    }

    fn swim_sound(&mut self, audio: &mut AudioEngine, p: &Player) {
        let v = p.vel;
        let speed = (((v[0] * v[0] * 0.2 + v[1] * v[1] + v[2] * v[2] * 0.2) as f32).sqrt() * 0.35).min(1.0);
        let pitch = 1.0 + (self.rng.next_float() - self.rng.next_float()) * 0.4;
        self.play_at(audio, "minecraft:entity.player.swim", p.pos, speed, pitch);
    }

    fn step_sound(
        &mut self,
        audio: &mut AudioEngine,
        data: &VersionData,
        world: &dyn World,
        p: &Player,
        pos: [i32; 3],
        state: u32,
    ) {
        let muffled = |me: &mut Self, audio: &mut AudioEngine, s: u32| {
            if let Some(t) = sound_type(data, s) {
                me.play_at(audio, &t.step, p.pos, t.volume * 0.05, t.pitch * 0.8);
            }
        };
        let plain = |me: &mut Self, audio: &mut AudioEngine, s: u32| {
            if let Some(t) = sound_type(data, s) {
                me.play_at(audio, &t.step, p.pos, t.volume * 0.15, t.pitch);
            }
        };
        if p.water_height > 0.0 {
            self.swim_sound(audio, p);
            muffled(self, audio, state);
        } else {
            let above = world.state(pos[0], pos[1] + 1, pos[2]).unwrap_or(0);
            let combination = Self::tagged(&self.combination_step, data, above);
            if combination || Self::tagged(&self.inside_step, data, above) {
                plain(self, audio, above);
                if combination {
                    muffled(self, audio, state);
                }
            } else {
                plain(self, audio, state);
            }
        }
        // Amethyst chime, at most once per 20 ticks.
        if Self::tagged(&self.crystal_blocks, data, state) && self.tick_count >= self.crystal_tick + 20 {
            self.crystal *= 0.997f32.powi(self.tick_count - self.crystal_tick);
            self.crystal = (self.crystal + 0.07).min(1.0);
            let pitch = 0.5 + self.crystal * self.rng.next_float() * 1.2;
            let volume = 0.1 + self.crystal * 1.2;
            self.play_at(audio, "minecraft:block.amethyst_block.chime", p.pos, volume, pitch);
            self.crystal_tick = self.tick_count;
        }
    }
}

/// World event (`level_event`): 2001 (block broken) and 2020 (mining hit) with the block's
/// sound set (§ 1.6), the others delegated to `level_events`. `here` = state present at `pos`.
#[allow(clippy::too_many_arguments)]
pub fn level_event(
    audio: &mut AudioEngine,
    data: &VersionData,
    rnd: &mut crate::level_events::WorldRandom,
    kind: i32,
    pos: [i32; 3],
    value: i32,
    global: bool,
    camera: [f64; 3],
    here: u32,
) {
    let center = pos.map(|v| f64::from(v) + 0.5);
    match kind {
        2001 => {
            let s = u32::try_from(value).unwrap_or(0);
            if !is_air(data, s)
                && let Some(t) = sound_type(data, s)
            {
                audio.play(&t.break_sound, Category::Block, Some(center), (t.volume + 1.0) / 2.0, t.pitch * 0.8, None);
            }
        }
        2020 => {
            if let Some(t) = sound_type(data, here) {
                audio.play(&t.hit, Category::Block, Some(center), (t.volume + 1.0) / 8.0, t.pitch * 0.5, None);
            }
        }
        _ => crate::level_events::play(audio, rnd, kind, pos, value, global, camera),
    }
}

#[cfg(test)]
#[path = "world_sounds_tests.rs"]
mod tests;
