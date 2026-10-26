//! Main-thread side: event resolution, channel reserves, deferred queues,
//! asynchronous loading (decoding off the main and audio threads), cpal output.

use hashbrown::{HashMap, HashSet};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use rc_assets::Assets;

use crate::decode;
#[cfg(test)]
use crate::decode::Pcm;
use crate::pcm_cache::{self, PcmCache};
use crate::mixer::{Command, Event, Listener, ListenerSlot, Mixer, Source, VoiceParams};
use crate::rng::JavaRandom;
use crate::sounds::{SoundTable, full_id};
use crate::{Category, Error};

/// Assumed mono sources of the device (OpenAL Soft: 255, spec § 3.5).
const SOURCES: usize = 255;
/// Minimum ticks before releasing the entry of a finished sound (§ 3.5).
const MIN_LIFETIME: u64 = 20;
/// Capacity of the command channel to the audio thread.
const CMD_CAP: usize = 4096;
/// Capacity of the finished-voices channel. Between two polls (`pump`, every frame and every
/// tick), at most one end arrives per live voice (≤ mixer capacity, 287) plus one per
/// `Play` sent (≤ 255 channels held by the engine): 4,096 leaves a margin of more than 7×,
/// so no end is lost.
const EV_CAP: usize = 4096;
/// Outbox: beyond this (audio thread frozen for several tens of seconds), only the latest
/// command of each kind is kept for each voice (position, gain, pitch), as well as the
/// last listener and the last category volumes; launches and stops are all kept.
/// The next threshold goes to double what remains (constant amortized cost per send).
const OUTBOX_MAX: usize = 1 << 16;

/// Optimisation switches of this module, read once (`RUSTCRAFT_OPT_*`, `0` = old
/// path).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Opt {
    /// `AUDIOFOND`: output opened on the "audio-output" thread, engine returned right away.
    pub background: bool,
    /// `AUDIOFILE` (default 1): while the output opens in the background, sounds are accepted and
    /// queued, played as soon as the output responds (the game freezes while reloading, then plays
    /// what was requested, including the click of the "Device" setting); 0: dropped as
    /// with no device.
    pub file: bool,
    /// `AUDIOREOPEN`: device change without re-reading the table or emptying the cache.
    pub reopen: bool,
    /// `AUDIOLISTENER`: listener in a shared cell instead of one command per frame.
    pub listener: bool,
    /// `AUDIOOUTBOX`: commands refused by a full channel kept in order and sent again.
    pub outbox: bool,
    /// `SOUNDIDS`: allocation-free launch (interned event, indexed draw, reserve counters
    /// kept up to date, repeated subtitles without an intermediate list).
    pub ids: bool,
    /// `PCMBUDGET`: PCM cache bounded in bytes, short, frequent and held sounds protected
    /// (`pcm_cache.rs`); 0: unbounded, like the game.
    pub budget: bool,
}

impl Opt {
    pub(crate) fn get() -> Opt {
        static O: OnceLock<Opt> = OnceLock::new();
        *O.get_or_init(|| Opt {
            background: crate::switch("AUDIOFOND", true),
            file: crate::switch("AUDIOFILE", true),
            reopen: crate::switch("AUDIOREOPEN", true),
            listener: crate::switch("AUDIOLISTENER", true),
            outbox: crate::switch("AUDIOOUTBOX", true),
            ids: crate::switch("SOUNDIDS", true),
            budget: crate::switch("PCMBUDGET", true),
        })
    }

    /// Old paths everywhere (equivalence proofs).
    #[cfg(test)]
    pub(crate) const OLD: Opt =
        Opt { background: false, file: false, reopen: false, listener: false, outbox: false, ids: false, budget: false };
    #[cfg(test)]
    pub(crate) const NEW: Opt =
        Opt { background: true, file: true, reopen: true, listener: true, outbox: true, ids: true, budget: true };
}

/// Event or file name of a sound: owned (old path, `SOUNDIDS=0`) or shared
/// with the table (copy without allocation).
#[derive(Debug, Clone)]
enum Name {
    Owned(String),
    Shared(Arc<str>),
}

impl std::ops::Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        match self {
            Name::Owned(s) => s,
            Name::Shared(s) => s,
        }
    }
}

/// Durations of the output-opening steps, in ms ("audio: steps" log).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Steps {
    host: f64,
    device: f64,
    config: f64,
    build: f64,
    play: f64,
}

impl std::fmt::Display for Steps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "host {:.1}, device {:.1}, format {:.1}, stream {:.1}, playback {:.1} ms",
            self.host, self.device, self.config, self.build, self.play
        )
    }
}

/// Result of an opening: stream (kept by the thread that opened it), rate, name, steps.
type Opened = (Box<dyn std::any::Any>, u32, String, Steps);
/// Opening run on the "audio-output" thread (real cpal, simulated in the tests).
type Opener = Box<dyn FnOnce(Mixer) -> Result<Opened, Error> + Send>;
/// Reply of the output thread: rate, opened name, steps; or the opening error.
type Answer = Result<(u32, String, Steps), Error>;

/// Thread that opened the output and keeps it: it releases the stream when the stop sender drops.
/// The thread is awaited at release: never two live outputs for one engine.
struct OutputThread {
    stop: Option<Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl OutputThread {
    /// Stop signal without waiting (reopening): the thread releases its stream and ends by itself,
    /// off the main thread (10-20 ms when it is the last stream of the output point).
    fn release(&mut self) {
        self.stop.take();
        self.handle.take();
    }
}

impl Drop for OutputThread {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

enum Output {
    None,
    /// Old path: stream opened on the calling thread.
    Stream { _stream: cpal::Stream },
    /// Output held by the "audio-output" thread; `ready`: reply awaited (opening in progress).
    Thread {
        thread: OutputThread,
        ready: Option<(Receiver<Answer>, Instant)>,
    },
}

/// Starts the opening on the "audio-output" thread, which replies (rate, name) then keeps the
/// stream until the stop signal (the cpal stream is not transferable on every system;
/// under Windows, its COM also stays apart from the window's).
fn spawn_output(mixer: Mixer, opener: Opener) -> Output {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (stop, stop_rx) = crossbeam_channel::bounded::<()>(0);
    let spawned = std::thread::Builder::new().name("audio-output".into()).spawn(move || match opener(mixer) {
        Ok((stream, rate, name, steps)) => {
            if tx.send(Ok((rate, name, steps))).is_ok() {
                let _ = stop_rx.recv();
            }
            drop(stream);
        }
        Err(e) => {
            let _ = tx.send(Err(e));
        }
    });
    match spawned {
        Ok(h) => Output::Thread {
            thread: OutputThread { stop: Some(stop), handle: Some(h) },
            ready: Some((rx, Instant::now())),
        },
        Err(e) => {
            eprintln!("[audio] output unavailable: thread not created ({e})");
            Output::None
        }
    }
}

/// Outbox too full: every state command (position, gain, pitch of a voice,
/// listener, category volumes) replaced by a more recent one of the same kind is removed;
/// the most recent stays in place, launches and stops are all kept (order preserved).
fn compact(outbox: &mut VecDeque<Command>) {
    let mut seen = HashSet::new();
    let mut keep: Vec<bool> = outbox
        .iter()
        .rev()
        .map(|c| match c {
            Command::Position(id, _) => seen.insert((0u8, *id)),
            Command::Gain(id, _) => seen.insert((1, *id)),
            Command::Pitch(id, _) => seen.insert((2, *id)),
            Command::Listener(_) => seen.insert((3, 0)),
            Command::Categories(_) => seen.insert((4, 0)),
            Command::Play(..) | Command::Stop(_) | Command::StopAll | Command::Pause(_) | Command::Resume => true,
        })
        .collect();
    keep.reverse();
    let mut k = keep.into_iter();
    outbox.retain(|_| k.next().unwrap_or(true));
}

/// Handle of a launched (or deferred) sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SoundHandle(pub u64);

/// Full playback request. `SoundRequest::new` gives a one-shot sound relative to
/// the listener, without attenuation (interface, music); `at` positions it with attenuation.
#[derive(Debug, Clone)]
pub struct SoundRequest {
    pub event: String,
    pub category: Category,
    pub pos: [f64; 3],
    pub relative: bool,
    pub linear: bool,
    pub volume: f32,
    pub pitch: f32,
    /// Seed of the variant draw; `None`: uninitialized generator.
    pub seed: Option<i64>,
    pub looping: bool,
    /// For a repeated sound: ticks to wait between two playbacks (0 = native loop).
    pub repeat_delay: u32,
    /// Ticks before the first launch (deferred queue).
    pub delay: u32,
    /// Sound maintained by the client (minecart, bee, mount): launched even at zero volume,
    /// its volume then following the entity (`sounds.md` § 3.7).
    pub start_silent: bool,
}

impl SoundRequest {
    pub fn new(event: &str, category: Category) -> Self {
        SoundRequest {
            event: event.to_owned(),
            category,
            pos: [0.0; 3],
            relative: true,
            linear: false,
            volume: 1.0,
            pitch: 1.0,
            seed: None,
            looping: false,
            repeat_delay: 0,
            delay: 0,
            start_silent: false,
        }
    }

    /// Sound positioned in the world, linear attenuation.
    pub fn at(mut self, pos: [f64; 3]) -> Self {
        self.pos = pos;
        self.relative = false;
        self.linear = true;
        self
    }

    pub fn volume(mut self, v: f32) -> Self {
        self.volume = v;
        self
    }

    pub fn pitch(mut self, p: f32) -> Self {
        self.pitch = p;
        self
    }

    pub fn seed(mut self, s: Option<i64>) -> Self {
        self.seed = s;
        self
    }

    pub fn looping(mut self, repeat_delay: u32) -> Self {
        self.looping = true;
        self.repeat_delay = repeat_delay;
        self
    }

    pub fn delay(mut self, ticks: u32) -> Self {
        self.delay = ticks;
        self
    }

    /// Launched even if the computed volume is zero (maintained sound, see `start_silent`).
    pub fn start_silent(mut self) -> Self {
        self.start_silent = true;
        self
    }

    /// Positioned without attenuation: audible everywhere, panning by position.
    pub fn at_unattenuated(mut self, pos: [f64; 3]) -> Self {
        self.pos = pos;
        self.relative = false;
        self.linear = false;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Loading,
    Playing,
    Finished,
    Stopped,
}

struct Instance {
    params: VoiceParams,
    event: Name,
    /// Chosen file (`assets/<ns>/sounds/….ogg`): title of the current track.
    path: Name,
    stream: bool,
    variant_volume: f32,
    variant_pitch: f32,
    start_tick: u64,
    state: State,
    /// Sound repeated with a delay: request and generator to relaunch it.
    repeat: Option<(SoundRequest, JavaRandom)>,
}

struct Delayed {
    due: u64,
    id: u64,
    req: SoundRequest,
    rng: JavaRandom,
}

struct LoadJob {
    id: u64,
    path: Name,
    stream: bool,
    looping: bool,
    /// Tick of the request (repetition score of the cache).
    tick: u64,
    /// `preload` variant: never forgotten by the cache.
    pin: bool,
}

enum Loaded {
    Ready(u64, Source),
    Failed(u64, String),
}

/// Sounds decoded by path, shared with the loading threads.
type Cache = Arc<Mutex<PcmCache>>;

pub struct AudioEngine {
    table: SoundTable,
    cache: Cache,
    cmd: Sender<Command>,
    events: Receiver<Event>,
    jobs: Sender<LoadJob>,
    loaded: Receiver<Loaded>,
    output: Output,
    opt: Opt,
    /// Commands refused by a full channel, sent again in order at the head of `pump`.
    outbox: VecDeque<Command>,
    /// Size of the outbox that triggers the next compaction.
    outbox_limit: usize,
    /// Listener shared with the mixer (`opt.listener`).
    slot: Arc<ListenerSlot>,
    headless: bool,
    rate: u32,
    instances: HashMap<u64, Instance>,
    /// Instances per reserve (`[normal, stream]`), kept up to date on every entry and exit.
    counts: [usize; 2],
    delayed: Vec<Delayed>,
    next_id: u64,
    ticks: u64,
    options: [f32; Category::COUNT],
    internal: [f32; Category::COUNT],
    max_normal: usize,
    max_stream: usize,
    warned: HashSet<String>,
    /// Subtitle subscription (`sounds.md` § 7): `None` = unsubscribed.
    subtitles: Option<Vec<SubtitleEvent>>,
    /// Audio preview of the volume sliders: last sound launched, category of the last slider.
    preview: (Option<SoundHandle>, Option<Category>),
    /// Requested device (`soundDevice`, empty = default).
    device: String,
    /// Device actually opened (format of the setting), empty without output.
    opened: String,
    /// Output lost (device unplugged), reported by the stream.
    lost: Arc<std::sync::atomic::AtomicBool>,
    /// Previous output of a reopening, stopped and muted, kept until the reply of the
    /// new one (output point kept "warm").
    retiring: Output,
    /// Output openings requested (creation and reopenings), for the client tests.
    opens: u32,
}

/// Subtitle conveyed by a launched (or audibly repeated) sound: translation key, position,
/// range (infinite if relative or without attenuation), repeated sound.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleEvent {
    pub key: String,
    pub pos: [f64; 3],
    pub range: f32,
    pub repeated: bool,
}

impl AudioEngine {
    /// Subscribes (or unsubscribes) subtitles; once unsubscribed, the queue is emptied.
    pub fn set_subtitles(&mut self, on: bool) {
        match (on, self.subtitles.is_some()) {
            (true, false) => self.subtitles = Some(Vec::new()),
            (false, true) => self.subtitles = None,
            _ => {}
        }
    }

    /// Subtitles conveyed since the last call.
    pub fn take_subtitles(&mut self) -> Vec<SubtitleEvent> {
        self.subtitles.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// `distance`: attenuation distance `D` of the sound (`sounds.md` § 3.3).
    fn notify_subtitle(&mut self, event: &str, pos: [f64; 3], spatial: bool, distance: f32, repeated: bool) {
        let Some(list) = &mut self.subtitles else { return };
        let Some(key) = self.table.get(event).and_then(|e| e.subtitle.clone()) else { return };
        let range = if spatial { distance } else { f32::INFINITY };
        list.push(SubtitleEvent { key, pos, range, repeated });
    }

    /// Engine on the default output device. `sounds.json` is read from `assets`
    /// (to stack several packs, build the table with `SoundTable::load` and pass it
    /// through `with_table`). Without a device, the engine works but plays nothing.
    pub fn new(assets: Arc<Assets>) -> Self {
        let t = Instant::now();
        let table = SoundTable::load(&[&assets]);
        eprintln!("audio: sound table in {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        Self::with_table(assets, table)
    }

    pub fn with_table(assets: Arc<Assets>, table: SoundTable) -> Self {
        Self::with_device(assets, table, "")
    }

    /// Engine on the `device` device (name from [`output_devices`], `soundDevice` setting);
    /// empty or absent: default device, like the game. With `RUSTCRAFT_OPT_AUDIOFOND`
    /// (default), the output opens in the background: the engine is returned right away and queues
    /// the sounds while it is not ready (or drops them, `AUDIOFILE=0`).
    pub fn with_device(assets: Arc<Assets>, table: SoundTable, device: &str) -> Self {
        let (mut engine, mixer) = Self::build(assets, table, 48_000, Opt::get());
        engine.device = device.to_owned();
        engine.open(mixer);
        engine
    }

    /// Opens the output on `self.device` (in the background or on this thread depending on `opt.background`).
    fn open(&mut self, mixer: Mixer) {
        self.opens += 1;
        if self.headless {
            // Without a device (tests): the opening is only counted.
            return;
        }
        if self.opt.background {
            let (device, lost) = (self.device.clone(), self.lost.clone());
            self.output = spawn_output(
                mixer,
                Box::new(move |m| {
                    open_output(m, &device, lost).map(|(s, rate, name, steps)| {
                        (Box::new(s) as Box<dyn std::any::Any>, rate, name, steps)
                    })
                }),
            );
            return;
        }
        let t = Instant::now();
        match open_output(mixer, &self.device, self.lost.clone()) {
            Ok((stream, rate, opened, steps)) => {
                eprintln!("audio: output opened in {:.0} ms ({steps})", t.elapsed().as_secs_f64() * 1e3);
                self.rate = rate;
                self.output = Output::Stream { _stream: stream };
                self.opened = opened;
            }
            Err(e) => eprintln!("[audio] output unavailable: {e}"),
        }
    }

    /// Background opening not finished yet.
    pub fn is_opening(&self) -> bool {
        matches!(self.output, Output::Thread { ready: Some(_), .. })
    }

    /// Polls the reply of the output thread (called by `pump`).
    fn poll_output(&mut self) {
        let Output::Thread { ready: Some((rx, t0)), .. } = &self.output else { return };
        let r = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(Error("output thread stopped".into())),
        };
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        self.retire();
        match r {
            Ok((rate, opened, steps)) => {
                eprintln!("audio: output opened in the background in {ms:.0} ms ({steps})");
                if let Output::Thread { ready, .. } = &mut self.output {
                    *ready = None;
                }
                self.rate = rate;
                self.opened = opened;
            }
            Err(e) => {
                eprintln!("[audio] output unavailable: {e}");
                // As without a device: queued sounds (option `file`) forgotten.
                self.output = Output::None;
                self.instances.clear();
                self.counts = [0; 2];
                self.outbox.clear();
            }
        }
    }

    /// Engine without a device: the caller drives `Mixer::process` itself (tests, measurements).
    pub fn headless(assets: Arc<Assets>, table: SoundTable, rate: u32) -> (Self, Mixer) {
        Self::headless_with(assets, table, rate, Opt::get())
    }

    /// Engine without a device created "on" `device` like [`with_device`](Self::with_device):
    /// the opening is counted (`output_opens`) without touching the hardware (client tests).
    pub fn headless_on(assets: Arc<Assets>, table: SoundTable, rate: u32, device: &str) -> (Self, Mixer) {
        let (mut e, m) = Self::headless(assets, table, rate);
        e.device = device.to_owned();
        e.opens += 1;
        (e, m)
    }

    /// Output openings requested since creation (creation included).
    pub fn output_opens(&self) -> u32 {
        self.opens
    }

    pub(crate) fn headless_with(assets: Arc<Assets>, table: SoundTable, rate: u32, opt: Opt) -> (Self, Mixer) {
        let (mut e, m) = Self::build(assets, table, rate, opt);
        e.headless = true;
        (e, m)
    }

    /// Mixer voices: one more per reserve to absorb a cut sound not yet rendered;
    /// the real limit is held here.
    fn mixer_voices(&self) -> usize {
        self.max_normal + self.max_stream + 32
    }

    /// New mixer and its two channels (the engine keeps the main ends).
    fn new_mixer(&mut self, rate: u32) -> Mixer {
        let (cmd, cmd_rx) = crossbeam_channel::bounded(CMD_CAP);
        let (ev_tx, events) = crossbeam_channel::bounded(EV_CAP);
        let mut mixer = Mixer::new(rate, self.mixer_voices(), cmd_rx, ev_tx);
        if self.opt.listener {
            mixer.share_listener(self.slot.clone());
        }
        self.cmd = cmd;
        self.events = events;
        mixer
    }

    fn build(assets: Arc<Assets>, table: SoundTable, rate: u32, opt: Opt) -> (Self, Mixer) {
        let flux = (SOURCES as f64).sqrt().floor().clamp(2.0, 8.0) as usize;
        let normal = (SOURCES - flux).clamp(8, 255);
        let (cmd, _) = crossbeam_channel::bounded(0);
        let (_, events) = crossbeam_channel::bounded(0);
        let (jobs, job_rx) = crossbeam_channel::unbounded::<LoadJob>();
        let (loaded_tx, loaded) = crossbeam_channel::unbounded();
        let cache: Cache = Arc::new(Mutex::new(PcmCache::new(opt.budget.then_some(pcm_cache::BUDGET))));
        for i in 0..2 {
            let (rx, tx, assets, cache) = (job_rx.clone(), loaded_tx.clone(), assets.clone(), cache.clone());
            let _ = std::thread::Builder::new().name(format!("rc-audio-load-{i}")).spawn(move || {
                for job in rx {
                    let _ = tx.send(load(&assets, &cache, job));
                }
            });
        }
        let mut engine = AudioEngine {
            table,
            cache,
            cmd,
            events,
            jobs,
            loaded,
            output: Output::None,
            opt,
            outbox: VecDeque::new(),
            outbox_limit: OUTBOX_MAX,
            slot: Arc::default(),
            headless: false,
            rate,
            instances: HashMap::new(),
            counts: [0; 2],
            delayed: Vec::new(),
            next_id: 1,
            ticks: 0,
            options: [1.0; Category::COUNT],
            internal: [1.0; Category::COUNT],
            max_normal: normal,
            max_stream: flux,
            warned: HashSet::new(),
            subtitles: None,
            preview: (None, None),
            device: String::new(),
            opened: String::new(),
            lost: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            retiring: Output::None,
            opens: 0,
        };
        let mixer = engine.new_mixer(rate);
        if opt.ids {
            // Outside the first sound: the engine is built on its own thread (client).
            engine.table.prepare();
        }
        // Preloading of the `preload` variants (none in vanilla).
        for path in engine.table.preloads() {
            let job = LoadJob { id: 0, path: Name::Owned(path), stream: false, looping: false, tick: 0, pin: true };
            let _ = engine.jobs.send(job);
        }
        (engine, mixer)
    }

    /// Device change (`soundDevice`, lost device or changed default) without
    /// re-reading `sounds.json` or emptying the PCM cache (`RUSTCRAFT_OPT_AUDIOREOPEN`): sounds stopped
    /// and channels released like a new engine; table, cache, loading threads, volumes,
    /// subtitles kept. Reopened in the background if `AUDIOFOND`: sounds requested during the switch
    /// are played when the new output replies (`AUDIOFILE`), like the game after its
    /// synchronous reload.
    pub fn reopen(&mut self, device: &str) {
        let t = Instant::now();
        let mixer = self.rewire();
        let released = t.elapsed().as_secs_f64() * 1e3;
        self.device = device.to_owned();
        self.open(mixer);
        if !self.is_opening() {
            // Synchronous (or failed) opening: the old output has no reason to live any more.
            self.retire();
        }
        eprintln!(
            "audio: reopening in {:.0} ms on the main thread (old output released in {released:.1} ms)",
            t.elapsed().as_secs_f64() * 1e3
        );
    }

    /// Old output released, new mixer plugged into new channels; sound state
    /// reset like a new engine's (identifiers continued: a load in flight from the
    /// old output finds no instance).
    fn rewire(&mut self) -> Mixer {
        let stopped = self.halt();
        // The old output (stopped, muted) lives until the new one replies: the output point
        // stays "warm" and the new opening takes ≈ 5 ms instead of ≈ 350.
        self.retire();
        self.retiring = std::mem::replace(&mut self.output, Output::None);
        if !stopped {
            // Total stop left in the outbox, or lost (full channel without outbox):
            // the old output would still play its sounds, so it is released right away.
            self.retire();
        }
        self.instances.clear();
        self.counts = [0; 2];
        self.delayed.clear();
        self.outbox.clear();
        self.outbox_limit = OUTBOX_MAX;
        self.opened.clear();
        self.lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.rate = 48_000;
        let mixer = self.new_mixer(48_000);
        self.send_categories();
        mixer
    }

    /// Releases the old output of a reopening: thread notified without waiting, stream of the
    /// calling thread released here.
    fn retire(&mut self) {
        if let Output::Thread { thread, .. } = &mut self.retiring {
            thread.release();
        }
        self.retiring = Output::None;
    }

    /// Reopening without a device (tests): returns the new mixer to drive.
    #[cfg(test)]
    fn reopen_headless(&mut self) -> Mixer {
        self.rewire()
    }

    /// Sends a command to the audio thread; full channel: kept in the outbox (order
    /// preserved), otherwise lost (old path). Returns `true` if it entered the channel
    /// (or if the audio thread no longer exists); `false` if it waits in the outbox or is lost.
    fn send(&mut self, c: Command) -> bool {
        if !self.opt.outbox {
            return !matches!(self.cmd.try_send(c), Err(TrySendError::Full(_)));
        }
        if !self.outbox.is_empty() {
            self.outbox.push_back(c);
        } else {
            match self.cmd.try_send(c) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => return true,
                Err(TrySendError::Full(c)) => self.outbox.push_back(c),
            }
        }
        if self.outbox.len() > self.outbox_limit {
            compact(&mut self.outbox);
            self.outbox_limit = (2 * self.outbox.len()).max(OUTBOX_MAX);
        }
        false
    }

    /// Sends the outbox again as long as the channel accepts.
    fn flush(&mut self) {
        while let Some(c) = self.outbox.pop_front() {
            match self.cmd.try_send(c) {
                Ok(()) => {}
                Err(TrySendError::Full(c)) => {
                    self.outbox.push_front(c);
                    return;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.outbox.clear();
                    break;
                }
            }
        }
        self.outbox_limit = OUTBOX_MAX;
    }

    /// Device requested at creation (empty = default).
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Device actually opened (empty without output).
    pub fn current_device_name(&self) -> &str {
        &self.opened
    }

    /// The opened device has disappeared (the stream no longer plays).
    pub fn is_device_lost(&self) -> bool {
        self.lost.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn table(&self) -> &SoundTable {
        &self.table
    }

    /// Output rate of the mixer.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn has_output(&self) -> bool {
        self.headless
            || match &self.output {
                Output::None => false,
                Output::Stream { .. } => true,
                Output::Thread { ready, .. } => ready.is_none() || self.opt.file,
            }
    }

    /// Sounds occupying a channel (including those finished for less than 20 ticks).
    pub fn active(&self) -> usize {
        self.instances.len()
    }

    /// Plays a sound event. `pos`: `Some` = positioned, linear attenuation; `None` =
    /// relative to the listener, without attenuation. `seed`: seed of the packet, `None` = random.
    pub fn play(
        &mut self,
        event: &str,
        category: Category,
        pos: Option<[f64; 3]>,
        volume: f32,
        pitch: f32,
        seed: Option<i64>,
    ) -> Option<SoundHandle> {
        if self.opt.ids {
            // Neither deferred nor repeated: the request is never kept, its name stays borrowed.
            let mut req = SoundRequest::new("", category).volume(volume).pitch(pitch).seed(seed);
            if let Some(p) = pos {
                req = req.at(p);
            }
            let rng = req.seed.map_or_else(JavaRandom::from_entropy, JavaRandom::new);
            return self.start(Some(event), req, rng, None).map(SoundHandle);
        }
        let mut req = SoundRequest::new(event, category).volume(volume).pitch(pitch).seed(seed);
        if let Some(p) = pos {
            req = req.at(p);
        }
        self.play_request(req)
    }

    pub fn play_request(&mut self, req: SoundRequest) -> Option<SoundHandle> {
        let rng = req.seed.map_or_else(JavaRandom::from_entropy, JavaRandom::new);
        if req.delay > 0 {
            let id = self.alloc_id();
            self.delayed.push(Delayed { due: self.ticks + u64::from(req.delay), id, req, rng });
            return Some(SoundHandle(id));
        }
        self.start(None, req, rng, None).map(SoundHandle)
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn category_factor(&self, c: Category) -> f32 {
        let opt = if c == Category::Master { self.options[0] } else { self.options[c as usize] * self.options[0] };
        opt.clamp(0.0, 1.0) * self.internal[c as usize].clamp(0.0, 1.0)
    }

    /// Entry of an instance: reserve counters kept up to date (replaced instance removed).
    fn admit(&mut self, id: u64, inst: Instance) {
        self.counts[usize::from(inst.stream)] += 1;
        if let Some(old) = self.instances.insert(id, inst) {
            self.counts[usize::from(old.stream)] -= 1;
        }
    }

    /// Launches `req`; `name`: name borrowed in place of `req.event` (direct launch).
    fn start(&mut self, name: Option<&str>, req: SoundRequest, rng: JavaRandom, id: Option<u64>) -> Option<u64> {
        if !self.has_output() {
            return None;
        }
        if !self.opt.ids {
            return self.start_old(req, rng, id);
        }
        let name = name.unwrap_or(&req.event);
        // Comparison with the full identifier, without building it.
        if name == "minecraft:intentionally_empty" || name == "intentionally_empty" {
            return None;
        }
        let Some(e) = self.table.event_index(name) else {
            let event = full_id(name);
            if self.warned.insert(event.clone()) {
                eprintln!("[audio] unknown event: {event}");
            }
            return None;
        };
        let mut rng = rng;
        let r = self.table.pick_index(e, &mut rng)?;
        let volume = req.volume * r.volume;
        let distance = volume.max(1.0) * r.attenuation_distance as f32;
        if let Some(list) = &mut self.subtitles
            && let Some(key) = self.table.get(self.table.event_id(e)).and_then(|ev| ev.subtitle.clone())
        {
            let spatial = !req.relative && req.linear;
            list.push(SubtitleEvent { key, pos: req.pos, range: if spatial { distance } else { f32::INFINITY }, repeated: false });
        }
        let gain = volume.clamp(0.0, 1.0);
        if gain * self.category_factor(req.category) <= 0.0 && req.category != Category::Music && !req.start_silent {
            return None;
        }
        let max = if r.stream { self.max_stream } else { self.max_normal };
        if self.counts[usize::from(r.stream)] >= max {
            return None;
        }
        let id = id.unwrap_or_else(|| self.alloc_id());
        let native_loop = req.looping && req.repeat_delay == 0;
        let params = VoiceParams {
            id,
            category: req.category,
            gain,
            pitch: (req.pitch * r.pitch).clamp(0.5, 2.0),
            pos: req.pos,
            relative: req.relative,
            linear: req.linear,
            distance,
            looping: native_loop,
        };
        let event = self.table.event_id(e).clone();
        let path = self.table.file_path(r.file).clone();
        if crate::sound_log() {
            self.log_start(&event, &req, &path);
        }
        let repeat = (req.looping && req.repeat_delay > 0).then_some((req, rng));
        let now = self.ticks;
        let cached = if r.stream { None } else { self.cache.lock().ok().and_then(|mut c| c.get(&path, now)) };
        let state = if let Some(pcm) = cached {
            self.send(Command::Play(params, Source::Buffer(pcm)));
            State::Playing
        } else {
            let job = LoadJob {
                id,
                path: Name::Shared(path.clone()),
                stream: r.stream,
                looping: native_loop,
                tick: now,
                pin: false,
            };
            let _ = self.jobs.send(job);
            State::Loading
        };
        let inst = Instance {
            params,
            event: Name::Shared(event),
            path: Name::Shared(path),
            stream: r.stream,
            variant_volume: r.volume,
            variant_pitch: r.pitch,
            start_tick: self.ticks,
            state,
            repeat,
        };
        self.admit(id, inst);
        Some(id)
    }

    /// Line of the sound log (`RUSTCRAFT_SOUNDLOG`).
    fn log_start(&self, event: &str, req: &SoundRequest, path: &str) {
        let at = if req.relative {
            "relative".to_owned()
        } else {
            format!("({:.2}, {:.2}, {:.2})", req.pos[0], req.pos[1], req.pos[2])
        };
        eprintln!(
            "[sound] t={} {} cat={} pos={} vol={:.3} pitch={:.3}{} variant={}",
            self.ticks,
            event,
            req.category.name(),
            at,
            req.volume,
            req.pitch,
            if req.looping { " loop" } else { "" },
            path
        );
    }

    /// Old launch (`SOUNDIDS=0`): full identifier built, recursive draw, path
    /// copied, reserves recounted.
    fn start_old(&mut self, req: SoundRequest, mut rng: JavaRandom, id: Option<u64>) -> Option<u64> {
        let event = full_id(&req.event);
        if event == "minecraft:intentionally_empty" {
            return None;
        }
        if self.table.get(&event).is_none() {
            if self.warned.insert(event.clone()) {
                eprintln!("[audio] unknown event: {event}");
            }
            return None;
        }
        let r = self.table.pick(&event, &mut rng)?;
        let volume = req.volume * r.volume;
        // Subtitle before the gain: displayed even if the category is at 0 %.
        let distance = volume.max(1.0) * r.attenuation_distance as f32;
        self.notify_subtitle(&event, req.pos, !req.relative && req.linear, distance, false);
        let gain = volume.clamp(0.0, 1.0);
        if gain * self.category_factor(req.category) <= 0.0 && req.category != Category::Music && !req.start_silent {
            return None;
        }
        let (count, max) = {
            let n = self.instances.values().filter(|i| i.stream == r.stream).count();
            (n, if r.stream { self.max_stream } else { self.max_normal })
        };
        if count >= max {
            return None;
        }
        let id = id.unwrap_or_else(|| self.alloc_id());
        let native_loop = req.looping && req.repeat_delay == 0;
        let params = VoiceParams {
            id,
            category: req.category,
            gain,
            pitch: (req.pitch * r.pitch).clamp(0.5, 2.0),
            pos: req.pos,
            relative: req.relative,
            linear: req.linear,
            distance: volume.max(1.0) * r.attenuation_distance as f32,
            looping: native_loop,
        };
        if crate::sound_log() {
            self.log_start(&event, &req, &r.path);
        }
        let repeat = (req.looping && req.repeat_delay > 0).then_some((req, rng));
        let now = self.ticks;
        let cached = if r.stream { None } else { self.cache.lock().ok().and_then(|mut c| c.get(&r.path, now)) };
        let state = if let Some(pcm) = cached {
            self.send(Command::Play(params, Source::Buffer(pcm)));
            State::Playing
        } else {
            let job = LoadJob {
                id,
                path: Name::Owned(r.path.clone()),
                stream: r.stream,
                looping: native_loop,
                tick: now,
                pin: false,
            };
            let _ = self.jobs.send(job);
            State::Loading
        };
        let inst = Instance {
            params,
            event: Name::Owned(event),
            path: Name::Owned(r.path),
            stream: r.stream,
            variant_volume: r.volume,
            variant_pitch: r.pitch,
            start_tick: self.ticks,
            state,
            repeat,
        };
        self.admit(id, inst);
        Some(id)
    }

    /// Polls loaded sounds and finished voices. Called by `tick` and `set_listener`.
    fn pump(&mut self) {
        self.poll_output();
        self.flush();
        while let Ok(l) = self.loaded.try_recv() {
            match l {
                Loaded::Ready(id, src) => match self.instances.get_mut(&id) {
                    Some(inst) if inst.state == State::Loading => {
                        inst.state = State::Playing;
                        let params = inst.params;
                        self.send(Command::Play(params, src));
                    }
                    _ => drop(src),
                },
                Loaded::Failed(id, e) => {
                    if let Some(inst) = self.instances.remove(&id) {
                        self.counts[usize::from(inst.stream)] -= 1;
                        if self.warned.insert(inst.event.to_string()) {
                            eprintln!("[audio] {}: {e}", &*inst.event);
                        }
                    }
                }
            }
        }
        while let Ok(Event::Finished(id, src)) = self.events.try_recv() {
            drop(src);
            let Some(inst) = self.instances.get_mut(&id) else { continue };
            if inst.state == State::Playing {
                inst.state = State::Finished;
                if let Some((req, rng)) = inst.repeat.take() {
                    let due = self.ticks + u64::from(req.repeat_delay);
                    self.delayed.push(Delayed { due, id, req, rng });
                }
            }
        }
    }

    /// Client tick (20 Hz): deferred queue, channel cleanup (§ 3.8).
    pub fn tick(&mut self) {
        self.ticks += 1;
        self.pump();
        let mut i = 0;
        while i < self.delayed.len() {
            if self.delayed[i].due <= self.ticks {
                let d = self.delayed.swap_remove(i);
                self.start(None, d.req, d.rng, Some(d.id));
            } else {
                i += 1;
            }
        }
        let now = self.ticks;
        // Audible repeated sounds: subtitle retransmitted every 10 ticks (`sounds.md` § 7).
        if self.opt.ids && self.subtitles.is_some() && now.is_multiple_of(10) {
            // Same walk, pushed directly (table, instances and queue: distinct fields).
            let factor: [f32; Category::COUNT] = std::array::from_fn(|c| self.category_factor(Category::ALL[c]));
            let list = self.subtitles.get_or_insert_default();
            for i in self.instances.values() {
                if i.state == State::Playing
                    && (i.params.looping || i.repeat.is_some())
                    && i.params.gain * factor[i.params.category as usize] > 0.0
                    && let Some(key) = self.table.get(&i.event).and_then(|e| e.subtitle.clone())
                {
                    let spatial = !i.params.relative && i.params.linear;
                    let range = if spatial { i.params.distance } else { f32::INFINITY };
                    list.push(SubtitleEvent { key, pos: i.params.pos, range, repeated: true });
                }
            }
        } else if self.subtitles.is_some() && now.is_multiple_of(10) {
            // Final volume (categories included) non-zero, like the game.
            let repeats: Vec<_> = self
                .instances
                .values()
                .filter(|i| {
                    i.state == State::Playing
                        && (i.params.looping || i.repeat.is_some())
                        && i.params.gain * self.category_factor(i.params.category) > 0.0
                })
                .map(|i| (i.event.clone(), i.params.pos, !i.params.relative && i.params.linear, i.params.distance))
                .collect();
            for (event, pos, spatial, distance) in repeats {
                self.notify_subtitle(&event, pos, spatial, distance, true);
            }
        }
        let counts = &mut self.counts;
        self.instances.retain(|_, inst| {
            let keep =
                !(matches!(inst.state, State::Finished | State::Stopped) && now - inst.start_tick >= MIN_LIFETIME);
            if !keep {
                counts[usize::from(inst.stream)] -= 1;
            }
            keep
        });
    }

    /// Client tick during the singleplayer pause: no deferred queue, no repeated subtitles, no counter
    /// (the game then only releases finished music channels); voice ends polled.
    pub fn tick_paused(&mut self) {
        self.pump();
    }

    /// Entering the singleplayer pause: running voices frozen, except those of the `keep` categories.
    pub fn pause_all_except(&mut self, keep: &[Category]) {
        let mask = std::array::from_fn(|c| !keep.contains(&Category::ALL[c]));
        self.send(Command::Pause(mask));
    }

    /// Back in game (no screen): frozen voices resumed.
    pub fn resume(&mut self) {
        self.send(Command::Resume);
    }

    /// Listener, every frame: camera position, forward and up vectors.
    pub fn set_listener(&mut self, pos: [f64; 3], forward: [f32; 3], up: [f32; 3]) {
        let l = Listener { pos, forward, up };
        if self.opt.listener {
            self.slot.store(l);
        } else {
            self.send(Command::Listener(l));
        }
        self.pump();
    }

    /// Volume slider of a category (0 – 1); applies to running sounds.
    pub fn set_volume(&mut self, category: Category, v: f32) {
        self.options[category as usize] = v.clamp(0.0, 1.0);
        self.send_categories();
    }

    pub fn volume(&self, category: Category) -> f32 {
        self.options[category as usize]
    }

    /// Internal gain of a category (music fade, § 6.3).
    pub fn set_category_fade(&mut self, category: Category, g: f32) {
        self.internal[category as usize] = g.clamp(0.0, 1.0);
        self.send_categories();
    }

    fn send_categories(&mut self) {
        let mut f = [0.0; Category::COUNT];
        for c in Category::ALL {
            f[c as usize] = self.category_factor(c);
        }
        self.send(Command::Categories(f));
    }

    /// `stop_sound` (§ 1.5): category and/or event; neither = total stop.
    pub fn stop(&mut self, category: Option<Category>, name: Option<&str>) {
        if category.is_none() && name.is_none() {
            self.stop_all();
            return;
        }
        let name = name.map(full_id);
        let ids: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, i)| category.is_none_or(|c| i.params.category == c))
            .filter(|(_, i)| name.as_ref().is_none_or(|n| *i.event == **n))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.stop_handle(SoundHandle(id));
        }
    }

    pub fn stop_handle(&mut self, h: SoundHandle) {
        self.delayed.retain(|d| d.id != h.0);
        if let Some(inst) = self.instances.get_mut(&h.0)
            && matches!(inst.state, State::Loading | State::Playing | State::Finished)
        {
            let playing = inst.state == State::Playing;
            inst.state = State::Stopped;
            inst.repeat = None;
            if playing {
                self.send(Command::Stop(h.0));
            }
        }
    }

    /// Total stop: channels, queues, internal gains reset to 1.
    pub fn stop_all(&mut self) {
        self.halt();
    }

    /// `stop_all`; returns `true` if the total stop entered the channel.
    fn halt(&mut self) -> bool {
        let sent = self.send(Command::StopAll);
        self.delayed.clear();
        for inst in self.instances.values_mut() {
            inst.state = State::Stopped;
            inst.repeat = None;
        }
        self.internal = [1.0; Category::COUNT];
        self.send_categories();
        sent
    }

    /// Events of the sounds occupying a channel (checks and tests).
    pub fn events(&self) -> impl Iterator<Item = &str> {
        self.instances.values().map(|i| &*i.event)
    }

    /// True while the sound's channel is not released (waiting, playing, or stopped for
    /// less than the minimum duration): the game's notion of "sound still active".
    pub fn is_active(&self, h: SoundHandle) -> bool {
        self.delayed.iter().any(|d| d.id == h.0) || self.instances.contains_key(&h.0)
    }

    /// Audio preview of a volume slider (`sounds.md` § 8, title screen only): a sound
    /// specific to the category, played in the interface category at the slider's volume; nothing while
    /// the previous preview is active; changing slider cuts the running preview. Master,
    /// music and voice have no sound.
    pub fn preview(&mut self, category: Category, volume: f32) {
        if self.preview.1 != Some(category) {
            self.preview.1 = Some(category);
            if let Some(h) = self.preview.0 {
                self.stop_handle(h);
            }
        }
        if self.preview.0.is_some_and(|h| self.is_active(h)) {
            return;
        }
        let event = match category {
            Category::Record => "minecraft:block.note_block.guitar",
            Category::Weather => "minecraft:entity.lightning_bolt.thunder",
            Category::Block => "minecraft:block.grass.place",
            Category::Hostile => "minecraft:entity.zombie.ambient",
            Category::Neutral => "minecraft:entity.cow.ambient",
            Category::Player => "minecraft:entity.generic.eat",
            Category::Ambient => "minecraft:ambient.cave",
            Category::Ui => "minecraft:ui.button.click",
            Category::Master | Category::Music | Category::Voice => return,
        };
        // The game keeps the preview even if it did not start (zero volume): not active afterwards.
        self.preview.0 = self.play_request(SoundRequest::new(event, Category::Ui).volume(volume));
    }

    /// True while the sound is waiting, loading or playing.
    pub fn is_playing(&self, h: SoundHandle) -> bool {
        self.delayed.iter().any(|d| d.id == h.0)
            || self.instances.get(&h.0).is_some_and(|i| matches!(i.state, State::Loading | State::Playing))
    }

    /// File chosen for a launched sound (`assets/<ns>/sounds/….ogg`), as long as it is known.
    pub fn sound_path(&self, h: SoundHandle) -> Option<&str> {
        self.instances.get(&h.0).map(|i| &*i.path)
    }

    /// Current position, gain and pitch of a launched sound (sounds maintained by the client, tests).
    pub fn sound_params(&self, h: SoundHandle) -> Option<([f64; 3], f32, f32)> {
        self.instances.get(&h.0).map(|i| (i.params.pos, i.params.gain, i.params.pitch))
    }

    /// Moves a running sound (sound attached to an entity).
    pub fn set_position(&mut self, h: SoundHandle, pos: [f64; 3]) {
        if let Some(inst) = self.instances.get_mut(&h.0) {
            inst.params.pos = pos;
            self.send(Command::Position(h.0, pos));
        }
    }

    /// Requested volume and pitch of a running sound (periodic sounds, § 3.7).
    pub fn set_sound_params(&mut self, h: SoundHandle, volume: f32, pitch: f32) {
        if let Some(inst) = self.instances.get_mut(&h.0) {
            let gain = (volume * inst.variant_volume).clamp(0.0, 1.0);
            let pitch = (pitch * inst.variant_pitch).clamp(0.5, 2.0);
            let new_gain = gain != inst.params.gain;
            let new_pitch = pitch != inst.params.pitch;
            inst.params.gain = gain;
            inst.params.pitch = pitch;
            if new_gain {
                self.send(Command::Gain(h.0, gain));
            }
            if new_pitch {
                self.send(Command::Pitch(h.0, pitch));
            }
        }
    }

    /// Empties the cache of decoded sounds (resource reload).
    pub fn clear_cache(&mut self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }

    /// Decoded buffers in cache (debug screen).
    pub fn cache_len(&self) -> usize {
        self.cache.lock().map_or(0, |c| c.len())
    }

    /// Bytes of PCM in cache.
    pub fn cache_bytes(&self) -> usize {
        self.cache.lock().map_or(0, |c| c.bytes())
    }

    /// Sounds forgotten by the bounded cache since the engine's creation (`PCMBUDGET`).
    pub fn cache_evicted(&self) -> u64 {
        self.cache.lock().map_or(0, |c| c.evicted)
    }
}

fn load(assets: &Assets, cache: &Cache, job: LoadJob) -> Loaded {
    if !job.stream
        && let Some(p) = cache.lock().ok().and_then(|mut c| c.get(&job.path, job.tick))
    {
        return Loaded::Ready(job.id, Source::Buffer(p));
    }
    let Some(bytes) = assets.read(&job.path) else {
        return Loaded::Failed(job.id, format!("{} not found", &*job.path));
    };
    // `RUSTCRAFT_OPT_DECODECOPY`: bytes read are decoded without copying (old path: `Arc<[u8]>`).
    let copy = decode::DecodeOpt::get().copy;
    if job.stream {
        let s = if copy {
            decode::spawn_stream(decode::SharedBytes(Arc::new(bytes)), job.looping)
        } else {
            decode::spawn_stream(Arc::<[u8]>::from(bytes), job.looping)
        };
        return match s {
            Ok(s) => Loaded::Ready(job.id, Source::Stream(s)),
            Err(e) => Loaded::Failed(job.id, e),
        };
    }
    let pcm = if copy { decode::decode_all_with(bytes, true) } else { decode::decode_all_with(Arc::<[u8]>::from(bytes), false) };
    match pcm {
        Ok(pcm) => {
            let pcm = Arc::new(pcm);
            let key = match job.path {
                Name::Shared(p) => p,
                Name::Owned(p) => Arc::from(p),
            };
            if let Ok(mut c) = cache.lock() {
                c.put(key, pcm.clone(), job.tick, job.pin);
            }
            Loaded::Ready(job.id, Source::Buffer(pcm))
        }
        Err(e) => Loaded::Failed(job.id, e),
    }
}

/// Prefix of the game's device names (OpenAL Soft), kept in `soundDevice` so that
/// options.txt stays interchangeable with the official client.
const DEVICE_PREFIX: &str = "OpenAL Soft on ";

/// Name of the default output device, in the format of the `soundDevice` setting.
pub fn default_device() -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let d = cpal::default_host().default_output_device()?;
    d.description().ok().map(|x| format!("{DEVICE_PREFIX}{}", x.name()))
}

/// Names of the available output devices, in the format of the `soundDevice` setting.
pub fn output_devices() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else { return Vec::new() };
    devices.filter_map(|d| d.description().ok()).map(|d| format!("{DEVICE_PREFIX}{}", d.name())).collect()
}

/// Identifiers of the default device and of all outputs, without reading their names (no
/// property store): cheap probe for device tracking (`AUDIODEVIDS`).
pub(crate) fn output_ids() -> (Option<cpal::DeviceId>, Vec<cpal::DeviceId>) {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.id().ok());
    let all = host.output_devices().map(|it| it.filter_map(|d| d.id().ok()).collect()).unwrap_or_default();
    (default, all)
}

/// Opens the output (WASAPI under Windows): the named device if it exists, otherwise the default
/// one; plugs the mixer into it. Also returns the name of the opened device; `lost` becomes true
/// if the device disappears.
fn open_output(
    mut mixer: Mixer,
    name: &str,
    lost: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(cpal::Stream, u32, String, Steps), Error> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let mut steps = Steps::default();
    let mut t = Instant::now();
    let mut lap = |slot: &mut f64| {
        *slot = t.elapsed().as_secs_f64() * 1e3;
        t = Instant::now();
    };
    let host = cpal::default_host();
    lap(&mut steps.host);
    let wanted = name.strip_prefix(DEVICE_PREFIX).unwrap_or(name);
    let named = (!wanted.is_empty())
        .then(|| host.output_devices().ok())
        .flatten()
        .and_then(|mut it| it.find(|d| d.description().is_ok_and(|x| x.name() == wanted)));
    if !wanted.is_empty() && named.is_none() {
        eprintln!("[audio] device \"{name}\" missing: default output");
    }
    let device =
        named.or_else(|| host.default_output_device()).ok_or_else(|| Error("no output device".into()))?;
    let opened = device.description().map(|x| format!("{DEVICE_PREFIX}{}", x.name())).unwrap_or_default();
    lap(&mut steps.device);
    let supported = device.default_output_config().map_err(|e| Error(e.to_string()))?;
    lap(&mut steps.config);
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate;
    let channels = usize::from(config.channels);
    mixer.set_rate(rate);
    // Preallocated stereo buffer; the callback splits it if it receives more.
    let mut scratch = vec![0.0f32; 2 * 8192];
    let err = move |e: cpal::Error| {
        if e.kind() == cpal::ErrorKind::DeviceNotAvailable {
            lost.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        eprintln!("[audio] stream error: {e}");
    };
    macro_rules! build {
        ($t:ty) => {
            device.build_output_stream::<$t, _, _>(
                config,
                move |out: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    write_out(&mut mixer, &mut scratch, out, channels);
                },
                err,
                None,
            )
        };
    }
    let stream = match format {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::I32 => build!(i32),
        f => return Err(Error(format!("unsupported sample format: {f}"))),
    }
    .map_err(|e| Error(e.to_string()))?;
    lap(&mut steps.build);
    stream.play().map_err(|e| Error(e.to_string()))?;
    lap(&mut steps.play);
    Ok((stream, rate, opened, steps))
}

fn write_out<T: cpal::Sample + cpal::FromSample<f32>>(
    mixer: &mut Mixer,
    scratch: &mut [f32],
    out: &mut [T],
    channels: usize,
) {
    let max = scratch.len() / 2;
    for block in out.chunks_mut(max * channels) {
        let frames = block.len() / channels;
        let buf = &mut scratch[..frames * 2];
        mixer.process(buf);
        for (o, s) in block.chunks_exact_mut(channels).zip(buf.as_chunks::<2>().0) {
            // Hard clipping (OpenAL applies a limiter; difference noted in the module spec).
            let (l, r) = (s[0].clamp(-1.0, 1.0), s[1].clamp(-1.0, 1.0));
            if channels == 1 {
                o[0] = T::from_sample(0.5 * (l + r));
            } else {
                o[0] = T::from_sample(l);
                o[1] = T::from_sample(r);
                for x in &mut o[2..] {
                    *x = T::EQUILIBRIUM;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Engine without a device on the real resources; `None` without a 26.3 installation.
    fn engine() -> Option<AudioEngine> {
        let install = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game")?;
        let index = Some(rc_testdata::need(install.asset_index(), "26.3 sound files and fonts (asset index)")?);
        let assets = rc_testdata::need(
            Assets::for_install(&install.client_jar, &[], index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())))
                .ok(),
            "26.3 game resources",
        )?;
        let table = SoundTable::load(&[&assets]);
        Some(AudioEngine::headless(Arc::new(assets), table, 48_000).0)
    }

    /// A maintained sound launched at zero volume occupies a channel (the game: silent start);
    /// an ordinary sound at zero volume is not launched.
    #[test]
    fn silent_start_only_when_requested() {
        let Some(mut e) = engine() else { return };
        let req = SoundRequest::new("minecraft:entity.minecart.riding", Category::Neutral).at([0.0; 3]).volume(0.0);
        assert!(e.play_request(req.clone().looping(0)).is_none(), "ordinary sound at zero volume refused");
        let h = e.play_request(req.looping(0).start_silent()).expect("maintained sound launched");
        assert!(e.is_playing(h));
    }

    /// Slider preview: sound of the category in the interface category; no second sound while
    /// the first is active; changing slider cuts the preview; nothing for master.
    #[test]
    fn slider_preview() {
        let Some(mut e) = engine() else { return };
        e.preview(Category::Hostile, 0.4);
        let first = e.preview.0.expect("preview launched");
        assert_eq!(&*e.instances[&first.0].event, "minecraft:entity.zombie.ambient");
        assert_eq!(e.instances[&first.0].params.category, Category::Ui);
        e.preview(Category::Hostile, 0.5);
        assert_eq!(e.preview.0, Some(first), "previous preview still active");
        // Other slider: the running preview is cut, but its channel stays active 20 ticks after
        // its launch (game engine): no new sound yet.
        e.preview(Category::Neutral, 0.5);
        assert!(!e.is_playing(first), "preview of the other slider cut");
        assert_eq!(e.preview.0, Some(first));
        for _ in 0..20 {
            e.tick();
        }
        e.preview(Category::Neutral, 0.5);
        let cow = e.preview.0.expect("new preview");
        assert_ne!(cow, first);
        assert_eq!(&*e.instances[&cow.0].event, "minecraft:entity.cow.ambient");
        e.preview(Category::Master, 1.0);
        assert_eq!(e.preview.0, Some(cow), "master: no sound");
    }
}

#[cfg(test)]
#[path = "engine_tests_opt.rs"]
mod tests_opt;

#[cfg(test)]
#[path = "engine_tests_pcm.rs"]
mod tests_pcm;
