//! Client audio engine, built off the main thread (sound table, output requested
//! directly on the chosen device) and joined on first use.
//!
//! The engine is reachable only through [`Audio::get`], which joins the construction in progress, or
//! through the accesses of this module that do not join (guarded listener, settings already applied,
//! debug screen): a forgotten use cannot lose a sound, it joins.
//!
//! Switches (read once): `RUSTCRAFT_OPT_AUDIOJOIN` (background construction, joined on first
//! use; opening the output in the background stays under `RUSTCRAFT_OPT_AUDIOFOND` of
//! rc-audio), `RUSTCRAFT_OPT_AUDIODEVICE1` (device chosen at creation instead of a second
//! opening), `RUSTCRAFT_OPT_RELOADAUDIO` (pack reload: engine built by the "resources" thread,
//! old engine released off the main thread), `RUSTCRAFT_OPT_AUDIOEARLY`
//! (construction started as soon as the resource index exists, in parallel with the font check; a
//! stack abandoned afterwards has its engine released and rebuilt for the retained one, losing no sound).
//! `0` = old path.

use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use rc_assets::Assets;
use rc_audio::{AudioEngine, Category, SoundTable};

/// Category volumes, in the order of [`Category::ALL`].
pub(crate) type Volumes = [f32; Category::COUNT];

/// Engine constructor: [`AudioEngine::with_device`] in game, with no output in tests.
pub(crate) type Make = fn(Arc<Assets>, SoundTable, &str) -> AudioEngine;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Opt {
    /// Construction on the "audio-engine" thread, joined on first use.
    pub join: bool,
    /// Engine created on `soundDevice` (one opening) instead of the default device followed
    /// by a reopening.
    pub device1: bool,
    /// Pack reload: engine built off the main thread, old one released in the background.
    pub reload: bool,
    /// Resources hook called as soon as each tried stack is indexed, before the font check
    /// (otherwise: after, on the retained stack only).
    pub early: bool,
}

pub(crate) fn opt() -> Opt {
    static O: OnceLock<Opt> = OnceLock::new();
    *O.get_or_init(|| {
        let on = |n: &str| rc_data::env_flags::read_os(n).is_none_or(|v| v != "0");
        let o = Opt {
            join: on("RUSTCRAFT_OPT_AUDIOJOIN"),
            device1: on("RUSTCRAFT_OPT_AUDIODEVICE1"),
            reload: on("RUSTCRAFT_OPT_RELOADAUDIO"),
            early: on("RUSTCRAFT_OPT_AUDIOEARLY"),
        };
        eprintln!("[opt] client audio: {o:?}");
        o
    })
}

/// Volumes of the `soundCategory_<category>` setting.
pub(crate) fn volumes(o: &crate::options::Options) -> Volumes {
    Category::ALL.map(|c| o.f(&format!("soundCategory_{}", c.name())) as f32)
}

/// Device the engine is created with: `device` (`AUDIODEVICE1`) or the default one,
/// the setting then being applied afterwards by a reopening (old path).
fn created_on(device: &str, direct: bool) -> &str {
    if direct { device } else { "" }
}

/// New engine: sound table read from `assets`, output requested, volumes set.
pub(crate) fn build(assets: Arc<Assets>, device: &str, volumes: &Volumes, direct: bool, make: Make) -> AudioEngine {
    let t = Instant::now();
    let table = SoundTable::load(&[&*assets]);
    let table_ms = t.elapsed().as_secs_f64() * 1e3;
    let mut a = make(assets, table, created_on(device, direct));
    for c in Category::ALL {
        a.set_volume(c, volumes[c as usize]);
    }
    eprintln!("audio: sound table in {table_ms:.1} ms, engine ready in {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    a
}

// A single instance (in `App`): the engine stays in place, with no indirection on every sound.
#[allow(clippy::large_enum_variant)]
enum State {
    /// No sound (`RUSTCRAFT_NOAUDIO`, or construction failed).
    Off,
    /// Construction in progress on the "audio-engine" thread.
    Pending {
        handle: JoinHandle<AudioEngine>,
        since: Instant,
        /// Device and volumes the engine will have when it leaves the thread.
        device: String,
        volumes: Volumes,
        /// Last listener set during construction, applied at the join.
        listener: Option<([f64; 3], [f32; 3], [f32; 3])>,
    },
    Ready(AudioEngine),
}

pub(crate) struct Audio(State);

// Devices of the engines joined on first use ("audio joined" milestone), for the tests.
#[cfg(test)]
static JOINED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

impl Audio {
    pub fn off() -> Self {
        Audio(State::Off)
    }

    pub fn ready(a: AudioEngine) -> Self {
        Audio(State::Ready(a))
    }

    /// Construction started on the "audio-engine" thread (on this thread if creation fails).
    #[cfg(test)]
    pub fn spawn(assets: Arc<Assets>, device: &str, volumes: Volumes, direct: bool, make: Make) -> Self {
        Self::spawn_after(Audio::off(), assets, device, volumes, direct, make)
    }

    /// Construction (re)started for the `assets` stack, replacing that of a tried and then
    /// abandoned stack (early hook, font fallback): the old engine, which played no sound,
    /// is joined and released on the new one's thread before it is created (only one output open).
    pub fn respawn(&mut self, assets: Arc<Assets>, device: &str, volumes: Volumes, direct: bool, make: Make) {
        let prev = std::mem::replace(self, Audio::off());
        *self = Self::spawn_after(prev, assets, device, volumes, direct, make);
    }

    fn spawn_after(prev: Audio, assets: Arc<Assets>, device: &str, volumes: Volumes, direct: bool, make: Make) -> Self {
        let dev = device.to_owned();
        let a = assets.clone();
        let spawned = std::thread::Builder::new().name("audio-engine".into()).spawn(move || {
            if prev.is_on() {
                prev.discard();
                eprintln!("audio: engine of an abandoned stack released");
            }
            build(a, &dev, &volumes, direct, make)
        });
        match spawned {
            Ok(handle) => Audio(State::Pending {
                handle,
                since: Instant::now(),
                device: created_on(device, direct).to_owned(),
                volumes,
                listener: None,
            }),
            Err(e) => {
                eprintln!("[audio] construction thread not created ({e}): engine built here");
                Audio::ready(build(assets, device, &volumes, direct, make))
            }
        }
    }

    /// Engine, joined if it is still being built (wait logged).
    pub fn get(&mut self) -> Option<&mut AudioEngine> {
        if matches!(self.0, State::Pending { .. }) {
            self.join();
        }
        match &mut self.0 {
            State::Ready(a) => Some(a),
            _ => None,
        }
    }

    /// Engine taken out of the slot (joined if needed); the slot stays without sound.
    pub fn into_engine(mut self) -> Option<AudioEngine> {
        self.get();
        match std::mem::replace(&mut self.0, State::Off) {
            State::Ready(a) => Some(a),
            _ => None,
        }
    }

    /// Engine of an abandoned stack released (construction awaited if needed), without the
    /// first-use join: neither the "audio joined" milestone nor a join log for a discarded engine.
    fn discard(self) {
        if let State::Pending { handle, .. } = self.0 {
            let _ = handle.join();
        }
    }

    fn join(&mut self) {
        let State::Pending { handle, since, listener, .. } = std::mem::replace(&mut self.0, State::Off) else {
            return;
        };
        let t = Instant::now();
        match handle.join() {
            Ok(mut a) => {
                if let Some((pos, fwd, up)) = listener {
                    a.set_listener(pos, fwd, up);
                }
                eprintln!(
                    "audio: engine joined {:.1} ms after its launch, wait {:.1} ms",
                    since.elapsed().as_secs_f64() * 1e3,
                    t.elapsed().as_secs_f64() * 1e3
                );
                crate::boot::mark("audio joined");
                #[cfg(test)]
                JOINED.lock().unwrap().push(a.device().to_owned());
                self.0 = State::Ready(a);
            }
            Err(_) => eprintln!("[audio] engine construction interrupted: no sound"),
        }
    }

    /// Construction in progress (never joined here).
    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        matches!(self.0, State::Pending { .. })
    }

    /// With sound (engine ready or being built).
    pub fn is_on(&self) -> bool {
        !matches!(self.0, State::Off)
    }

    /// Without joining (debug screen): `None` while the engine is being built.
    pub fn peek(&self) -> Option<&AudioEngine> {
        match &self.0 {
            State::Ready(a) => Some(a),
            _ => None,
        }
    }

    /// Listener of the frame: kept without joining during construction, set at the join.
    pub fn set_listener(&mut self, pos: [f64; 3], fwd: [f32; 3], up: [f32; 3]) {
        match &mut self.0 {
            State::Ready(a) => a.set_listener(pos, fwd, up),
            State::Pending { listener, .. } => *listener = Some((pos, fwd, up)),
            State::Off => {}
        }
    }

    /// To apply the volumes: `None` if the engine being built already has them (no
    /// join), otherwise the engine (joined).
    pub fn for_volumes(&mut self, v: &Volumes) -> Option<&mut AudioEngine> {
        if let State::Pending { volumes, .. } = &self.0
            && volumes == v
        {
            return None;
        }
        self.get()
    }

    /// To apply the device: `None` if the engine being built is already created on
    /// `device` (no join), otherwise the engine (joined).
    pub fn for_device(&mut self, device: &str) -> Option<&mut AudioEngine> {
        if let State::Pending { device: d, .. } = &self.0
            && d == device
        {
            return None;
        }
        self.get()
    }
}

/// Device change (`soundDevice`) on an existing engine: reopening (table and
/// cache kept, `AUDIOREOPEN`) or new engine (old path); volumes set again.
pub(crate) fn reload_audio(a: &mut AudioEngine, assets: &Arc<Assets>, volumes: &Volumes, device: &str) {
    let t = Instant::now();
    if rc_audio::reopen_enabled() {
        // Table, decoded cache and volumes kept; output reopened (in the background by default).
        a.reopen(device);
    } else {
        a.stop_all();
        let table = SoundTable::load(&[&**assets]);
        *a = AudioEngine::with_device(assets.clone(), table, device);
    }
    eprintln!("audio: engine rebuilt in {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    for c in Category::ALL {
        a.set_volume(c, volumes[c as usize]);
    }
}

/// `soundDevice` setting applied (`apply_video_options`): engine rebuilt if its device
/// differs, followed by a UI click outside the first pass. True if the engine was rebuilt.
pub(crate) fn apply_device(audio: &mut Audio, assets: &Arc<Assets>, volumes: &Volumes, device: &str, first: bool) -> bool {
    // Engine still being built already created on this device: nothing to do, no join.
    let Some(a) = audio.for_device(device) else { return false };
    if a.device() == device {
        return false;
    }
    reload_audio(a, assets, volumes, device);
    if !first {
        a.play(crate::widgets::CLICK_SOUND, Category::Ui, None, 0.25, 1.0, None);
    }
    true
}

/// Pack reload, on the "resources" thread: `load` receives the hook that starts the
/// engine construction as soon as the new stack is indexed (`cfg`: device, volumes,
/// direct creation); called again for each tried stack (`AUDIOEARLY`), the last call is the
/// retained stack. The engine is returned with the result. Without `cfg`: no engine.
pub(crate) fn load_with_engine<L>(
    cfg: Option<(String, Volumes, bool)>,
    make: Make,
    load: impl FnOnce(&mut dyn FnMut(&Arc<Assets>)) -> L,
) -> (L, Option<AudioEngine>) {
    let mut slot = Audio::off();
    let l = load(&mut |a| {
        if let Some((device, volumes, direct)) = &cfg {
            slot.respawn(a.clone(), device, *volumes, *direct, make);
        }
    });
    (l, slot.into_engine())
}

/// Engine of the new stack on reload: the one built during baking if it exists
/// (`RELOADAUDIO`), otherwise built here, on `device` (`AUDIODEVICE1`) or on the default
/// device (old path: the setting is then applied by a reopening).
pub(crate) fn reload_engine(
    prebuilt: Option<AudioEngine>,
    assets: &Arc<Assets>,
    device: &str,
    volumes: &Volumes,
    direct: bool,
    make: Make,
) -> AudioEngine {
    prebuilt.unwrap_or_else(|| {
        if direct {
            build(assets.clone(), device, volumes, true, make)
        } else {
            make(assets.clone(), SoundTable::load(&[&**assets]), "")
        }
    })
}

/// Engine replaced on pack reload: the old one is released on a separate thread
/// (`RELOADAUDIO`: its output closes in 10-20 ms when it is the last stream, thread returned), otherwise
/// here.
pub(crate) fn retire(old: AudioEngine, background: bool) -> Option<JoinHandle<()>> {
    if background {
        // Thread not created: the engine is released here along with the refused closure.
        return std::thread::Builder::new()
            .name("audio-release".into())
            .spawn(move || {
                let t = Instant::now();
                drop(old);
                eprintln!("audio: old engine released in {:.1} ms (background)", t.elapsed().as_secs_f64() * 1e3);
            })
            .ok();
    }
    let t = Instant::now();
    drop(old);
    eprintln!("audio: old engine released in {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    None
}

#[cfg(test)]
mod tests;
