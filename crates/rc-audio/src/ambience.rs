//! Background music (`docs/specs/sounds.md` § 6) and biome ambience (§ 5.4): per-tick logic,
//! driven by inputs the client evaluates (environment attributes, light).

use crate::{AudioEngine, Category, JavaRandom, SoundHandle, SoundRequest};

/// Track (§ 6.1): event, min and max delays (ticks), replaces the current music.
/// Borrowed from the environment attributes, no copy on every tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Track<'a> {
    pub event: &'a str,
    pub min_delay: i32,
    pub max_delay: i32,
    pub replace: bool,
}

/// Music frequency (option): 20 min (default), 10 min, constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    Default,
    Frequent,
    Constant,
}

impl Frequency {
    fn ticks(self) -> Option<i32> {
        match self {
            Frequency::Default => Some(24000),
            Frequency::Frequent => Some(12000),
            Frequency::Constant => None,
        }
    }
}

/// Music manager (§ 6.3).
pub struct Music {
    current: Option<(SoundHandle, String)>,
    next_delay: i32,
    gain: f32,
    frequency: Frequency,
    /// Frequency changed: delay recomputed on the next tick with the situation music.
    reset_delay: bool,
    rng: JavaRandom,
}

impl Default for Music {
    fn default() -> Self {
        Music {
            current: None,
            next_delay: 100,
            gain: 1.0,
            frequency: Frequency::Default,
            reset_delay: false,
            // Random seed at each launch, like the game.
            rng: JavaRandom::from_entropy(),
        }
    }
}

impl Music {
    /// Inclusive uniform integer in [a, b] (a if a ≥ b).
    fn rand(&mut self, a: i32, b: i32) -> i32 {
        if a >= b { a } else { a + self.rng.next_int_bounded(b - a + 1) }
    }

    fn next_delay_for(&mut self, m: Option<&Track<'_>>) -> i32 {
        match (m, self.frequency.ticks()) {
            (_, None) => 100,
            (None, Some(f)) => f,
            (Some(t), Some(f)) => self.rand(t.min_delay.min(f), t.max_delay.min(f)),
        }
    }

    fn stop(&mut self, engine: &mut AudioEngine, m: Option<&Track<'_>>) {
        if let Some((h, _)) = self.current.take() {
            engine.stop_handle(h);
        }
        self.next_delay = self.next_delay_for(m).saturating_add(100);
    }

    /// One tick: `m` = situation music (§ 6.2), `volume` = target volume, `loading` = world
    /// loading screen.
    /// `musicFrequency` setting: like the game, the delay before the next track is redrawn
    /// with the new frequency (here at the next tick, which knows the situation music).
    pub fn set_frequency(&mut self, f: Frequency, reset: bool) {
        if f != self.frequency {
            self.frequency = f;
            self.reset_delay = reset;
        }
    }

    pub fn frequency(&self) -> Frequency {
        self.frequency
    }

    /// Event of the current track ("now playing" notification).
    pub fn now_playing(&self) -> Option<&str> {
        self.current.as_ref().map(|(_, e)| e.as_str())
    }

    /// Translation key of the current file (`music.game.sweden`): sound path without the `sounds/`
    /// folder or extension, namespace omitted if it is `minecraft`, `/` replaced by `.`.
    pub fn now_playing_key(&self, engine: &AudioEngine) -> Option<String> {
        let path = engine.sound_path(self.current.as_ref()?.0)?;
        let rest = path.strip_prefix("assets/")?;
        let (ns, file) = rest.split_once("/sounds/")?;
        let file = file.strip_suffix(".ogg").unwrap_or(file);
        let key = if ns == "minecraft" { file.to_owned() } else { format!("{ns}.{file}") };
        Some(key.replace('/', "."))
    }

    /// Handle of the current track (launch detected by the caller).
    pub fn current_handle(&self) -> Option<SoundHandle> {
        self.current.as_ref().map(|(h, _)| *h)
    }

    pub fn tick(&mut self, engine: &mut AudioEngine, m: Option<&Track<'_>>, volume: f32, loading: bool) {
        if std::mem::take(&mut self.reset_delay) {
            self.next_delay = self.next_delay_for(m);
        }
        // 1. Gain towards the target volume.
        if self.current.is_some() && self.gain != volume {
            if self.gain < volume {
                self.gain = (self.gain + self.gain.clamp(0.0005, 0.005)).min(volume);
            } else {
                let g = 0.03 * volume + 0.97 * self.gain;
                self.gain = if (g - volume).abs() < 0.0001 || g < volume { volume } else { g };
            }
            self.gain = self.gain.clamp(0.0, 1.0);
            if self.gain <= 0.0001 {
                self.stop(engine, m);
                return;
            }
            engine.set_category_fade(Category::Music, self.gain);
        }
        // 2-3. Choice and launch.
        let Some(t) = m else {
            self.next_delay = self.next_delay.max(100);
            return;
        };
        // A stopped track (replaced or finished) stays "current" until the engine releases its
        // channel (next tick): the delay only counts down from then on, and a
        // replacement redraws its delay at every tick until then, like the game.
        if t.replace
            && let Some((h, ev)) = &self.current
            && *ev != t.event
        {
            engine.stop_handle(*h);
            self.next_delay = self.rand(0, t.min_delay / 2);
        }
        if let Some((h, _)) = &self.current
            && !engine.is_active(*h)
        {
            self.current = None;
            let d = self.next_delay_for(Some(t));
            self.next_delay = self.next_delay.min(d);
        }
        self.next_delay = self.next_delay.min(t.max_delay);
        if self.current.is_none() && !loading {
            self.next_delay -= 1;
            if self.next_delay <= 0 {
                if crate::sound_log() {
                    eprintln!("[music] delay elapsed: {} ({}–{} ticks)", t.event, t.min_delay, t.max_delay);
                }
                // The current gain is kept (not reset to the target volume): after a fade-out,
                // the next track fades back in gradually, like the game.
                let req = SoundRequest::new(t.event, Category::Music);
                if let Some(h) = engine.play_request(req) {
                    self.current = Some((h, t.event.to_owned()));
                }
                self.next_delay = i32::MAX;
            }
        }
    }
}

/// Mood of an ambience: sound, delay, search extent, offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mood<'a> {
    pub sound: &'a str,
    pub tick_delay: i32,
    pub block_search_extent: i32,
    pub offset: f64,
}

/// Ambience evaluated at the player's position, borrowed from the environment attributes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AmbientSettings<'a> {
    pub loop_sound: Option<&'a str>,
    pub additions: &'a [(String, f64)],
    pub mood: Option<Mood<'a>>,
}

/// Ambience manager (§ 5.4): fading loops, additions, mood.
#[derive(Default)]
pub struct Ambience {
    /// Loops: (event, sound, fade, incoming).
    loops: Vec<(String, SoundHandle, i32, bool)>,
    mood: f32,
    rng: Option<JavaRandom>,
    ticks: u32,
}

impl Ambience {
    /// Mood value (debug screen).
    pub fn mood(&self) -> f32 {
        self.mood
    }

    /// One tick: `eye` = eye position; `light(x, y, z)` = (sky, block).
    pub fn tick(
        &mut self,
        engine: &mut AudioEngine,
        s: &AmbientSettings<'_>,
        eye: [f64; 3],
        light: &dyn Fn(i32, i32, i32) -> (u8, u8),
    ) {
        let rng = self.rng.get_or_insert_with(|| JavaRandom::new(0xA3B1_E0C3));
        // 1. Loops: the requested one fades in, the others fade out.
        let want = s.loop_sound;
        if let Some(w) = want
            && !self.loops.iter().any(|l| l.0 == w)
            // Launched at volume 1 (otherwise refused, zero gain), brought back to the fade right after.
            && let Some(h) = engine.play_request(SoundRequest::new(w, Category::Ambient).looping(0))
        {
            self.loops.push((w.to_owned(), h, 0, true));
        }
        self.loops.retain_mut(|(ev, h, fade, entering)| {
            *entering = want == Some(ev.as_str());
            if *entering {
                *fade = (*fade).max(0) + 1;
            } else {
                *fade = (*fade).min(40) - 1;
            }
            *fade = (*fade).clamp(-1, 40);
            if *fade < 0 {
                engine.stop_handle(*h);
                return false;
            }
            engine.set_sound_params(*h, (*fade as f32 / 40.0).clamp(0.0, 1.0), 1.0);
            true
        });
        // 2. Additions.
        for (sound, chance) in s.additions {
            if rng.next_double() < *chance {
                engine.play(sound, Category::Ambient, None, 1.0, 1.0, None);
            }
        }
        // 3. Mood.
        let Some(m) = &s.mood else {
            self.mood = 0.0;
            return;
        };
        let e = m.block_search_extent;
        let n = 2 * e + 1;
        let p = [
            eye[0].floor() as i32 + rng.next_int_bounded(n) - e,
            eye[1].floor() as i32 + rng.next_int_bounded(n) - e,
            eye[2].floor() as i32 + rng.next_int_bounded(n) - e,
        ];
        let (sky, block) = light(p[0], p[1], p[2]);
        if sky > 0 {
            self.mood -= f32::from(sky) / 15.0 * 0.001;
        } else {
            self.mood -= (f32::from(block) - 1.0) / m.tick_delay.max(1) as f32;
        }
        if self.mood >= 1.0 {
            let c = p.map(|v| f64::from(v) + 0.5);
            let u = [c[0] - eye[0], c[1] - eye[1], c[2] - eye[2]];
            let d = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt().max(1e-6);
            let k = (d + m.offset) / d;
            let at = [eye[0] + u[0] * k, eye[1] + u[1] * k, eye[2] + u[2] * k];
            engine.play(m.sound, Category::Ambient, Some(at), 1.0, 1.0, None);
            self.mood = 0.0;
        } else {
            self.mood = self.mood.max(0.0);
        }
        self.ticks = self.ticks.wrapping_add(1);
        if crate::sound_log() && self.ticks.is_multiple_of(100) {
            eprintln!("[mood] {:.4} (last point: sky {sky}, block {block})", self.mood);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Engine without a device on the real resources; `None` without a 26.3 installation.
    fn engine() -> Option<AudioEngine> {
        let install = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game")?;
        let index = install.asset_index();
        let assets = rc_testdata::need(
            rc_assets::Assets::for_install(
                &install.client_jar,
                &[],
                index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())),
            )
            .ok(),
            "26.3 game resources",
        )?;
        let table = crate::SoundTable::load(&[&assets]);
        Some(AudioEngine::headless(std::sync::Arc::new(assets), table, 48_000).0)
    }

    #[test]
    fn music_starts_after_initial_delay() {
        let Some(mut e) = engine() else { return };
        let mut m = Music::default();
        let t = Track { event: "minecraft:music.game", min_delay: 12000, max_delay: 24000, replace: false };
        for tick in 1..=100 {
            m.tick(&mut e, Some(&t), 1.0, false);
            e.tick();
            let playing = e.events().any(|ev| ev == "minecraft:music.game");
            assert_eq!(playing, tick == 100, "tick {tick}");
            if playing {
                break;
            }
        }
    }

    /// After a fade-out (target volume 0), the next track starts from the low gain and
    /// rises by +0.0005…+0.005 per tick instead of restarting at full volume.
    #[test]
    fn music_fades_in_after_fade_out() {
        let Some(mut e) = engine() else { return };
        let mut m = Music::default();
        m.set_frequency(Frequency::Constant, false);
        let t = Track { event: "minecraft:music.game", min_delay: 12000, max_delay: 24000, replace: false };
        let tick = |m: &mut Music, e: &mut AudioEngine, v: f32| {
            m.tick(e, Some(&t), v, false);
            e.tick();
        };
        while m.current.is_none() {
            tick(&mut m, &mut e, 1.0);
        }
        while m.current.is_some() {
            tick(&mut m, &mut e, 0.0);
        }
        // Below 1e-4 of difference, the gain is set to the target (0) and the track stops.
        assert_eq!(m.gain, 0.0, "gain after the fade-out");
        while m.current.is_none() {
            tick(&mut m, &mut e, 1.0);
        }
        assert_eq!(m.gain, 0.0, "gain kept at the launch of the next track");
        tick(&mut m, &mut e, 1.0);
        assert_eq!(m.gain, 0.0005, "gradual rise");
        tick(&mut m, &mut e, 1.0);
        assert_eq!(m.gain, 0.001);
        for _ in 0..400 {
            tick(&mut m, &mut e, 1.0);
        }
        assert_eq!(m.gain, 1.0);
    }

    /// Replacement: the drawn delay only counts down once the channel is released (next tick).
    #[test]
    fn replaced_music_waits_for_channel_release() {
        let Some(mut e) = engine() else { return };
        let mut m = Music::default();
        let a = Track { event: "minecraft:music.game", min_delay: 12000, max_delay: 24000, replace: false };
        while m.current.is_none() {
            m.tick(&mut e, Some(&a), 1.0, false);
            e.tick();
        }
        // Channel kept at least 20 ticks after the launch (game engine): we go past that delay.
        for _ in 0..25 {
            m.tick(&mut e, Some(&a), 1.0, false);
            e.tick();
        }
        let b = Track { event: "minecraft:music.dragon", min_delay: 0, max_delay: 0, replace: true };
        m.tick(&mut e, Some(&b), 1.0, false);
        assert!(m.current.is_some(), "replaced track still current at the replacement tick");
        e.tick();
        m.tick(&mut e, Some(&b), 1.0, false);
        assert_eq!(m.now_playing(), Some("minecraft:music.dragon"), "zero delay: launched on the next tick");
    }

    #[test]
    fn darkness_raises_mood_until_cave_sound() {
        let Some(mut e) = engine() else { return };
        let mut a = Ambience::default();
        let s = AmbientSettings {
            loop_sound: Some("minecraft:ambient.crimson_forest.loop"),
            additions: &[],
            mood: Some(Mood {
                sound: "minecraft:ambient.cave",
                tick_delay: 6000,
                block_search_extent: 8,
                offset: 2.0,
            }),
        };
        let dark = |_: i32, _: i32, _: i32| (0u8, 0u8);
        a.tick(&mut e, &s, [0.5, 64.6, 0.5], &dark);
        assert!(e.events().any(|ev| ev == "minecraft:ambient.crimson_forest.loop"), "loop launched");
        let mut peak = 0.0f32;
        for _ in 1..6010 {
            a.tick(&mut e, &s, [0.5, 64.6, 0.5], &dark);
            e.tick();
            peak = peak.max(a.mood());
        }
        // 1/6000 per tick in the dark: the mood reaches 1 at tick 6000, plays the cave sound and restarts from 0.
        assert!(peak > 0.99, "maximum mood {peak}");
        assert!(a.mood() < 0.01);
        assert!(e.events().any(|ev| ev == "minecraft:ambient.cave"), "mood sound played");
    }
}
