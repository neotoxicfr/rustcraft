//! Client audio engine (spec docs/specs/sounds.md): `sounds.json` table, Ogg Vorbis
//! decoding, software mixer (linear attenuation, mono panning, stereo as is),
//! cpal output. No dependency on the network: the client translates packets into calls.

pub mod ambience;
pub mod decode;
pub mod devices;
mod engine;
pub mod mixer;
mod pcm_cache;
pub mod rng;
pub mod sounds;

pub use engine::{AudioEngine, SoundHandle, SoundRequest, SubtitleEvent, output_devices};
pub use rng::JavaRandom;
pub use sounds::SoundTable;

/// Sound categories, in protocol order (§ 1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Category {
    Master = 0,
    Music,
    Record,
    Weather,
    Block,
    Hostile,
    Neutral,
    Player,
    Ambient,
    Voice,
    Ui,
}

impl Category {
    pub const COUNT: usize = 11;
    pub const ALL: [Category; Category::COUNT] = [
        Category::Master,
        Category::Music,
        Category::Record,
        Category::Weather,
        Category::Block,
        Category::Hostile,
        Category::Neutral,
        Category::Player,
        Category::Ambient,
        Category::Voice,
        Category::Ui,
    ];

    /// Network identifier (`varint`).
    pub fn from_id(id: i32) -> Option<Self> {
        usize::try_from(id).ok().and_then(|i| Self::ALL.get(i).copied())
    }

    pub fn name(self) -> &'static str {
        ["master", "music", "record", "weather", "block", "hostile", "neutral", "player", "ambient", "voice", "ui"]
            [self as usize]
    }

    pub fn from_name(n: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == n)
    }
}

/// Sound log (`RUSTCRAFT_SOUNDLOG=1`): every launched sound is written to the error output.
pub fn sound_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_os!("RUSTCRAFT_SOUNDLOG").is_some_and(|v| v != "0"))
}

/// Device change through [`AudioEngine::reopen`] (`RUSTCRAFT_OPT_AUDIOREOPEN`, default
/// 1) rather than a new engine.
pub fn reopen_enabled() -> bool {
    engine::Opt::get().reopen
}

/// Allocation-free launch of sounds and ambience (`RUSTCRAFT_OPT_SOUNDIDS`, default 1): the
/// client also borrows the audio attributes instead of copying them at each tick.
pub fn sound_ids_enabled() -> bool {
    engine::Opt::get().ids
}

/// Optimisation switch `RUSTCRAFT_OPT_<name>`: `0` = old path, `1` = new,
/// absent = `default`. Read once by each caller (`OnceLock`).
pub(crate) fn switch(name: &str, default: bool) -> bool {
    match rc_data::env_flags::read_os(&format!("RUSTCRAFT_OPT_{name}")).and_then(|v| v.into_string().ok()).as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => default,
    }
}

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
