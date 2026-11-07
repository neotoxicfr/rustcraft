//! Software mixer run in the audio thread: no allocation or lock in `process`.
//! Voices arrive ready-made through a bounded channel; finished sources go back to
//! the main thread to be released there.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};

use crate::Category;
use crate::decode::{Pcm, StreamIn};

pub enum Source {
    Buffer(Arc<Pcm>),
    Stream(StreamIn),
}

impl Source {
    fn channels(&self) -> u16 {
        match self {
            Source::Buffer(p) => p.channels,
            Source::Stream(s) => s.channels,
        }
    }

    fn rate(&self) -> u32 {
        match self {
            Source::Buffer(p) => p.rate,
            Source::Stream(s) => s.rate,
        }
    }
}

/// Parameters of a voice at start.
#[derive(Debug, Clone, Copy)]
pub struct VoiceParams {
    pub id: u64,
    pub category: Category,
    /// `clamp(sound_volume, 0, 1)` (§ 3.1), excluding the category gain.
    pub gain: f32,
    /// Speed factor, already bounded to [0.5, 2] (§ 3.2).
    pub pitch: f32,
    pub pos: [f64; 3],
    pub relative: bool,
    pub linear: bool,
    /// Effective attenuation distance `D` (§ 3.3).
    pub distance: f32,
    pub looping: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Listener {
    pub pos: [f64; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
}

impl Default for Listener {
    fn default() -> Self {
        Listener { pos: [0.0; 3], forward: [0.0, 0.0, -1.0], up: [0.0, 1.0, 0.0] }
    }
}

/// "Last value" cell of the listener (`RUSTCRAFT_OPT_AUDIOLISTENER`): the main thread
/// writes it every frame instead of sending a command; the audio thread re-reads it at the start
/// of each block. Wait-free sequence lock: odd counter during the write; a read that
/// crosses a write is abandoned and the mixer keeps the previous listener (one block behind
/// at worst). A SINGLE writer (the engine, which owns it).
pub struct ListenerSlot {
    seq: std::sync::atomic::AtomicU64,
    pos: [std::sync::atomic::AtomicU64; 3],
    dirs: [std::sync::atomic::AtomicU32; 6],
}

impl Default for ListenerSlot {
    fn default() -> Self {
        let s = ListenerSlot { seq: Default::default(), pos: Default::default(), dirs: Default::default() };
        s.store(Listener::default());
        s
    }
}

impl ListenerSlot {
    pub fn store(&self, l: Listener) {
        use std::sync::atomic::{Ordering::*, fence};
        let s = self.seq.load(Relaxed);
        self.seq.store(s.wrapping_add(1), Relaxed);
        fence(Release);
        for (a, v) in self.pos.iter().zip(l.pos) {
            a.store(v.to_bits(), Relaxed);
        }
        for (a, v) in self.dirs.iter().zip(l.forward.into_iter().chain(l.up)) {
            a.store(v.to_bits(), Relaxed);
        }
        self.seq.store(s.wrapping_add(2), Release);
    }

    /// Last complete value, `None` if a write is in progress.
    pub fn load(&self) -> Option<Listener> {
        use std::sync::atomic::{Ordering::*, fence};
        let s1 = self.seq.load(Acquire);
        if s1 & 1 == 1 {
            return None;
        }
        let p = |i: usize| f64::from_bits(self.pos[i].load(Relaxed));
        let d = |i: usize| f32::from_bits(self.dirs[i].load(Relaxed));
        let l = Listener { pos: [p(0), p(1), p(2)], forward: [d(0), d(1), d(2)], up: [d(3), d(4), d(5)] };
        fence(Acquire);
        (self.seq.load(Relaxed) == s1).then_some(l)
    }

    /// Writes since creation (each write adds 2 to the sequence; the creation writes once).
    #[cfg(test)]
    pub(crate) fn writes(&self) -> u64 {
        self.seq.load(std::sync::atomic::Ordering::Relaxed) / 2
    }
}

pub enum Command {
    Play(VoiceParams, Source),
    Stop(u64),
    StopAll,
    Position(u64, [f64; 3]),
    Gain(u64, f32),
    Pitch(u64, f32),
    Listener(Listener),
    /// Effective factor per category: `clamp(option × master, 0, 1) × internal gain`.
    Categories([f32; Category::COUNT]),
    /// Singleplayer pause: running voices of the marked categories frozen (neither rendered nor advanced).
    Pause([bool; Category::COUNT]),
    /// Back in game: all frozen voices resume where they were.
    Resume,
}

pub enum Event {
    /// Voice finished or cut; the source is returned to be released off the audio thread.
    Finished(u64, Source),
}

struct Voice {
    p: VoiceParams,
    src: Source,
    channels: usize,
    /// Read step in source frames per output frame.
    step: f64,
    /// Buffer: position in frames. Stream: fraction between `win[1]` and `win[2]`.
    pos: f64,
    /// Stream: frames i−1, i, i+1, i+2 around the position (4-point interpolation).
    win: [[f32; 2]; 4],
    primed: bool,
    gl: f32,
    gr: f32,
    fresh: bool,
    stopping: bool,
    done: bool,
    /// Frozen by the singleplayer pause: skipped by the mix, position kept.
    paused: bool,
}

/// Mixer optimisation switches, read once (`RUSTCRAFT_OPT_*`, `0` = old path).
/// All give the same output bit for bit (proof: `tests::mix_digest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MixOpt {
    /// `MIXMUTE`: muted voices (zero start and end gains) only advanced.
    pub mute: bool,
    /// `MIXWEIGHTS`: cubic weights once per frame, direct read away from the edges, listener
    /// frame once per block.
    pub weights: bool,
    /// `MIXSIMD`: 4 frames at a time (weights and reads in arrays), mono and stereo buffer voices.
    pub simd: bool,
    /// `AUDIO_MIXER_VOICE_IDS`: a voice is found by id in a compact list of ids (8 bytes each)
    /// instead of walking the voices themselves (~150 bytes each) for every command.
    pub voice_ids: bool,
    /// `MIXER_AVX2`: with `simd`, buffer voices in groups of 8 frames compiled for AVX2 when the
    /// processor has it (detected at run time), same operations per lane, no FMA.
    pub avx2: bool,
    /// `MIXER_PACKED_READS`: with `simd`, the points of a mono or stereo buffer gathered as rows of
    /// 16-bit samples, each row converted to `f32` at once, instead of one conversion per point.
    pub packed_reads: bool,
    /// `MIXER_FIXED_POSITIONS`: with `simd`, lane positions of a group computed in 16-bit fixed
    /// point (exact, like the muted voices) instead of f64 with a conversion per lane.
    pub fixed_positions: bool,
    /// `STREAM_SLICE_READ`: a stream whose ring already holds every frame of the block is read
    /// from one slice of the ring instead of frame by frame.
    pub stream_slice: bool,
}

impl MixOpt {
    pub(crate) fn get() -> MixOpt {
        static O: std::sync::OnceLock<MixOpt> = std::sync::OnceLock::new();
        *O.get_or_init(|| MixOpt {
            mute: crate::switch("MIXMUTE", true),
            weights: crate::switch("MIXWEIGHTS", true),
            simd: crate::switch("MIXSIMD", true),
            voice_ids: crate::switch("AUDIO_MIXER_VOICE_IDS", true),
            avx2: crate::switch("MIXER_AVX2", true),
            packed_reads: crate::switch("MIXER_PACKED_READS", true),
            fixed_positions: crate::switch("MIXER_FIXED_POSITIONS", true),
            stream_slice: crate::switch("STREAM_SLICE_READ", true),
        })
    }
}

pub struct Mixer {
    opt: MixOpt,
    rate: u32,
    voices: Vec<Voice>,
    /// Id of each voice, same order as `voices` (kept by the same push and swap_remove).
    ids: Vec<u64>,
    listener: Listener,
    categories: [f32; Category::COUNT],
    rx: Receiver<Command>,
    tx: Sender<Event>,
    /// Shared listener (re-read at every block); `None`: only `Command::Listener`.
    slot: Option<Arc<ListenerSlot>>,
}

const I16: f32 = 1.0 / 32768.0;

impl Mixer {
    pub fn new(rate: u32, max_voices: usize, rx: Receiver<Command>, tx: Sender<Event>) -> Self {
        Mixer {
            opt: MixOpt::get(),
            rate,
            voices: Vec::with_capacity(max_voices),
            ids: Vec::with_capacity(max_voices),
            listener: Listener::default(),
            categories: [1.0; Category::COUNT],
            rx,
            tx,
            slot: None,
        }
    }

    /// Plugs in the listener cell: re-read at the start of each block, after the commands.
    pub fn share_listener(&mut self, slot: Arc<ListenerSlot>) {
        self.slot = Some(slot);
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate;
    }

    pub fn active(&self) -> usize {
        self.voices.len()
    }

    fn command(&mut self, c: Command) {
        match c {
            Command::Play(p, src) => {
                if self.voices.len() == self.voices.capacity() {
                    let _ = self.tx.try_send(Event::Finished(p.id, src));
                    return;
                }
                let channels = usize::from(src.channels());
                let step = step_of(src.rate(), p.pitch, self.rate);
                self.ids.push(p.id);
                self.voices.push(Voice {
                    p,
                    src,
                    channels,
                    step,
                    pos: 0.0,
                    win: [[0.0; 2]; 4],
                    primed: false,
                    gl: 0.0,
                    gr: 0.0,
                    fresh: true,
                    stopping: false,
                    done: false,
                    paused: false,
                });
            }
            Command::Stop(id) => self.with(id, |v| v.stopping = true),
            Command::StopAll => self.voices.iter_mut().for_each(|v| v.stopping = true),
            Command::Position(id, pos) => self.with(id, |v| v.p.pos = pos),
            Command::Gain(id, g) => self.with(id, |v| v.p.gain = g),
            Command::Pitch(id, pitch) => {
                let rate = self.rate;
                self.with(id, |v| {
                    v.p.pitch = pitch;
                    v.step = step_of(v.src.rate(), pitch, rate);
                });
            }
            Command::Listener(l) => self.listener = l,
            Command::Categories(c) => self.categories = c,
            Command::Pause(mask) => self.voices.iter_mut().for_each(|v| v.paused |= mask[v.p.category as usize]),
            Command::Resume => self.voices.iter_mut().for_each(|v| v.paused = false),
        }
    }

    fn with(&mut self, id: u64, f: impl FnOnce(&mut Voice)) {
        let found = if self.opt.voice_ids {
            // First voice with this id, like the walk below.
            self.ids.iter().position(|&x| x == id).map(|i| &mut self.voices[i])
        } else {
            self.voices.iter_mut().find(|v| v.p.id == id)
        };
        if let Some(v) = found {
            f(v);
        }
    }

    /// Fills `out` (interleaved stereo, `out.len() / 2` frames).
    pub fn process(&mut self, out: &mut [f32]) {
        while let Ok(c) = self.rx.try_recv() {
            self.command(c);
        }
        if let Some(l) = self.slot.as_ref().and_then(|s| s.load()) {
            self.listener = l;
        }
        out.fill(0.0);
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        let l = self.listener;
        let right = normalize(cross(l.forward, l.up));
        let opt = self.opt;
        // Listener frame, computed once per block (same operations as per voice).
        let frame = opt.weights.then(|| basis(&l, right));
        for v in &mut self.voices {
            if v.paused {
                // Frozen: nothing rendered, nothing advanced; a stop during the pause removes it without a fade
                // (it is already muted).
                v.done |= v.stopping;
                continue;
            }
            let cat = self.categories[v.p.category as usize];
            let (mut tl, mut tr) = match frame {
                Some(b) => target_gain_in(v, &l, b, cat),
                None => target_gain(v, &l, right, cat),
            };
            if v.stopping {
                // Fade-out over one block to avoid the click.
                tl = 0.0;
                tr = 0.0;
            }
            if v.fresh {
                v.gl = tl;
                v.gr = tr;
                v.fresh = false;
            }
            mix_voice(v, out, frames, tl, tr, opt);
            if let Source::Stream(s) = &v.src {
                s.wake_if_drained();
            }
            if v.stopping {
                v.done = true;
            }
        }
        // Removal of finished voices, source returned to the main thread.
        let mut i = 0;
        while i < self.voices.len() {
            if self.voices[i].done {
                self.ids.swap_remove(i);
                let v = self.voices.swap_remove(i);
                let _ = self.tx.try_send(Event::Finished(v.p.id, v.src));
            } else {
                i += 1;
            }
        }
    }
}

#[cfg(test)]
impl Mixer {
    /// Pending commands, removed without being applied (log of the equivalence proofs;
    /// voice identifier set to 0 to compare two engines).
    pub(crate) fn drain_log(&mut self) -> Vec<String> {
        self.drain_log_ids(false)
    }

    /// [`drain_log`](Self::drain_log) that keeps the voice identifiers when `ids` (two engines
    /// launching the same sounds in the same order number them alike).
    pub(crate) fn drain_log_ids(&mut self, ids: bool) -> Vec<String> {
        let tag = |id: u64| if ids { format!("{id} ") } else { String::new() };
        let mut out = Vec::new();
        while let Ok(c) = self.rx.try_recv() {
            out.push(match c {
                Command::Play(mut p, src) => {
                    let id = tag(p.id);
                    p.id = 0;
                    let n = match &src {
                        Source::Buffer(b) => b.data.len(),
                        Source::Stream(_) => usize::MAX,
                    };
                    format!("play {id}{p:?} channels={} n={n}", src.channels())
                }
                Command::Stop(id) => format!("stop {}", tag(id)).trim_end().to_owned(),
                Command::StopAll => "stop_all".into(),
                Command::Position(id, p) => format!("pos {}{p:?}", tag(id)),
                Command::Gain(id, g) => format!("gain {}{g}", tag(id)),
                Command::Pitch(id, p) => format!("pitch {}{p}", tag(id)),
                Command::Listener(l) => format!("listener {l:?}"),
                Command::Categories(c) => format!("cat {c:?}"),
                Command::Pause(m) => format!("pause {m:?}"),
                Command::Resume => "resume".into(),
            });
        }
        out
    }
}

/// Target left/right gains. Stereo: played as is, neither attenuated nor spatialised (like OpenAL).
/// Mono: bounded linear attenuation then panning of OpenAL Soft's stereo output (measured
/// on the game's library): direction in the listener frame (relative: already in that
/// frame), front azimuth widened ×1.5 (bounded to ±90°), horizontal first-order decoding
/// `0.5 ∓ 0.5·sin(az)·cos(el) + K·cos(az)·cos(el)`; source at the origin: in front.
fn target_gain(v: &Voice, l: &Listener, right: [f32; 3], category: f32) -> (f32, f32) {
    let base = v.p.gain * category;
    if v.channels == 2 {
        return (base, base);
    }
    let rel = if v.p.relative {
        [v.p.pos[0] as f32, v.p.pos[1] as f32, v.p.pos[2] as f32]
    } else {
        let d = [(v.p.pos[0] - l.pos[0]) as f32, (v.p.pos[1] - l.pos[1]) as f32, (v.p.pos[2] - l.pos[2]) as f32];
        let fwd = normalize(l.forward);
        let up = cross(right, fwd);
        [dot(d, right), dot(d, up), -dot(d, fwd)]
    };
    let dist = dot(rel, rel).sqrt();
    let att = if v.p.linear { attenuation(dist, v.p.distance) } else { 1.0 };
    let (gl, gr) = pan_gains(rel, dist);
    (base * att * gl, base * att * gr)
}

/// Listener frame `(right, up, forward)`, normalised as in [`target_gain`].
type Basis = ([f32; 3], [f32; 3], [f32; 3]);

fn basis(l: &Listener, right: [f32; 3]) -> Basis {
    let fwd = normalize(l.forward);
    (right, cross(right, fwd), fwd)
}

/// [`target_gain`] with the frame computed once per block (`MIXWEIGHTS`); same operations.
fn target_gain_in(v: &Voice, l: &Listener, (right, up, fwd): Basis, category: f32) -> (f32, f32) {
    let base = v.p.gain * category;
    if v.channels == 2 {
        return (base, base);
    }
    let rel = if v.p.relative {
        [v.p.pos[0] as f32, v.p.pos[1] as f32, v.p.pos[2] as f32]
    } else {
        let d = [(v.p.pos[0] - l.pos[0]) as f32, (v.p.pos[1] - l.pos[1]) as f32, (v.p.pos[2] - l.pos[2]) as f32];
        [dot(d, right), dot(d, up), -dot(d, fwd)]
    };
    let dist = dot(rel, rel).sqrt();
    let att = if v.p.linear { attenuation(dist, v.p.distance) } else { 1.0 };
    let (gl, gr) = pan_gains(rel, dist);
    (base * att * gl, base * att * gr)
}

/// Front/back coefficient of OpenAL Soft's stereo decoder (0.0552305643 × √3).
const PAN_FRONT: f32 = 0.055_230_564 * 1.732_050_8;

/// Gains of a mono source at position `rel` (listener frame, −Z in front), `dist` = |rel|.
pub fn pan_gains(rel: [f32; 3], dist: f32) -> (f32, f32) {
    let (x, y, z) = if dist > f32::EPSILON { (rel[0] / dist, rel[1] / dist, rel[2] / dist) } else { (0.0, 0.0, -1.0) };
    let ev = y.clamp(-1.0, 1.0).asin();
    let az = x.atan2(-z);
    let half_pi = std::f32::consts::FRAC_PI_2;
    let az = if az.abs() < half_pi { (az.abs() * 1.5).min(half_pi).copysign(az) } else { az };
    let ce = ev.cos();
    let (s, c) = (az.sin() * ce, az.cos() * ce);
    (0.5 - 0.5 * s + PAN_FRONT * c, 0.5 + 0.5 * s + PAN_FRONT * c)
}

/// Read step: ratio of the rates × pitch, in 16-bit fixed point (rounded), like
/// OpenAL Soft's mixer.
fn step_of(src_rate: u32, pitch: f32, out_rate: u32) -> f64 {
    (f64::from(src_rate) * f64::from(pitch) / f64::from(out_rate) * 65536.0).round().max(1.0) / 65536.0
}

/// 4-point interpolation (cubic Lagrange, OpenAL Soft's "Cubic Spline") between `x1` and
/// `x2` at fraction `f`.
#[inline]
fn cubic(x0: f32, x1: f32, x2: f32, x3: f32, f: f32) -> f32 {
    let (fm1, fp1, fm2) = (f - 1.0, f + 1.0, f - 2.0);
    -f * fm1 * fm2 / 6.0 * x0 + fp1 * fm1 * fm2 / 2.0 * x1 - fp1 * f * fm2 / 2.0 * x2 + fp1 * f * fm1 / 6.0 * x3
}

/// "Bounded linear" model, reference distance 0, factor 1 (§ 3.3).
pub fn attenuation(d: f32, max: f32) -> f32 {
    if max <= 0.0 {
        return 0.0;
    }
    (1.0 - d / max).clamp(0.0, 1.0)
}

/// Mixes one voice into the block (or only advances it when muted) with the enabled paths; every
/// path gives the same output bit for bit.
fn mix_voice(v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32, opt: MixOpt) {
    if opt.mute && tl == 0.0 && tr == 0.0 && v.gl == 0.0 && v.gr == 0.0 {
        // Zero contribution over the whole block (x·0 added to a sum does not change it):
        // position, looping, stream window and end advanced as in rendering.
        if !(opt.stream_slice && advance_stream_slice(v, frames)) {
            advance(v, frames);
        }
        return;
    }
    if opt.simd && matches!(v.src, Source::Buffer(_)) {
        return render_simd(v, out, frames, tl, tr, opt);
    }
    if opt.stream_slice && render_stream_slice(v, out, frames, tl, tr) {
        return;
    }
    if opt.weights {
        render_fast(v, out, frames, tl, tr);
    } else {
        render(v, out, frames, tl, tr);
    }
}

fn render(v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32) {
    let inv = 1.0 / frames as f32;
    let (dl, dr) = ((tl - v.gl) * inv, (tr - v.gr) * inv);
    let (mut gl, mut gr) = (v.gl, v.gr);
    let stereo = v.channels == 2;
    match &mut v.src {
        Source::Buffer(pcm) => {
            let data = &pcm.data[..];
            let n = data.len() / v.channels;
            let mut pos = v.pos;
            for o in out.as_chunks_mut::<2>().0.iter_mut().take(frames) {
                if pos >= n as f64 {
                    if v.p.looping && n > 0 {
                        pos %= n as f64;
                    } else {
                        v.done = true;
                        break;
                    }
                }
                let i = pos as usize;
                let f = (pos - i as f64) as f32;
                // Frame k (i−1 … i+2): outside the buffer, silence (looped: wraps to the start).
                let ch = v.channels;
                let at = |k: isize, c: usize| -> f32 {
                    let k = if k >= n as isize && v.p.looping { k - n as isize } else { k };
                    if k < 0 || k >= n as isize { 0.0 } else { f32::from(data[k as usize * ch + c]) }
                };
                let ii = i as isize;
                let sample = |c: usize| cubic(at(ii - 1, c), at(ii, c), at(ii + 1, c), at(ii + 2, c), f) * I16;
                gl += dl;
                gr += dr;
                if stereo {
                    o[0] += sample(0) * gl;
                    o[1] += sample(1) * gr;
                } else {
                    let s = sample(0);
                    o[0] += s * gl;
                    o[1] += s * gr;
                }
                pos += v.step;
            }
            v.pos = pos;
        }
        Source::Stream(s) => {
            let ch = v.channels;
            let pull = |ring: &mut rtrb::Consumer<i16>| -> Option<[f32; 2]> {
                if ring.slots() < ch {
                    return None;
                }
                let a = ring.pop().ok()? as f32 * I16;
                let b = if ch == 2 { ring.pop().ok()? as f32 * I16 } else { a };
                Some([a, b])
            };
            if !v.primed {
                // Zero history before the first frame, then the first 3 frames.
                if s.ring.slots() < 3 * ch {
                    if s.ring.is_abandoned() && s.ring.slots() < 2 * ch {
                        v.done = true;
                    }
                    // No data yet: silence, the decoding thread is filling.
                    v.gl = tl;
                    v.gr = tr;
                    return;
                }
                v.win[0] = [0.0; 2];
                for k in 1..4 {
                    v.win[k] = pull(&mut s.ring).unwrap_or([0.0; 2]);
                }
                v.primed = true;
            }
            let mut f = v.pos;
            for o in out.as_chunks_mut::<2>().0.iter_mut().take(frames) {
                gl += dl;
                gr += dr;
                let w = &v.win;
                let ff = f as f32;
                let l = cubic(w[0][0], w[1][0], w[2][0], w[3][0], ff);
                if stereo {
                    let r = cubic(w[0][1], w[1][1], w[2][1], w[3][1], ff);
                    o[0] += l * gl;
                    o[1] += r * gr;
                } else {
                    o[0] += l * gl;
                    o[1] += l * gr;
                }
                f += v.step;
                while f >= 1.0 {
                    f -= 1.0;
                    match pull(&mut s.ring) {
                        Some(nf) => {
                            v.win = [v.win[1], v.win[2], v.win[3], nf];
                        }
                        None => {
                            if s.ring.is_abandoned() {
                                v.done = true;
                            }
                            // Underfeeding: we hold the window until it returns.
                            f = 0.0;
                            break;
                        }
                    }
                }
                if v.done {
                    break;
                }
            }
            v.pos = f;
        }
    }
    v.gl = tl;
    v.gr = tr;
}

/// Weights of the 4 points of [`cubic`] at fraction `f`, written in the same order of operations:
/// `cubic(x0, x1, x2, x3, f) == dot4(weights(f), x0, x1, x2, x3)` bit for bit.
#[inline]
fn weights(f: f32) -> [f32; 4] {
    let (fm1, fp1, fm2) = (f - 1.0, f + 1.0, f - 2.0);
    [-f * fm1 * fm2 / 6.0, fp1 * fm1 * fm2 / 2.0, fp1 * f * fm2 / 2.0, fp1 * f * fm1 / 6.0]
}

#[inline]
fn dot4(w: [f32; 4], x0: f32, x1: f32, x2: f32, x3: f32) -> f32 {
    w[0] * x0 + w[1] * x1 - w[2] * x2 + w[3] * x3
}

/// Sample `c` of frame `k` of a buffer of `n` frames: outside the buffer, silence (looped:
/// wraps to the start).
#[inline]
fn edge_at(data: &[i16], ch: usize, n: usize, looping: bool, k: isize, c: usize) -> f32 {
    let k = if k >= n as isize && looping { k - n as isize } else { k };
    if k < 0 || k >= n as isize { 0.0 } else { f32::from(data[k as usize * ch + c]) }
}

/// Channel `c` interpolated around frame `i` (weights `w`), without the 1/32768 factor.
#[inline]
fn tap(data: &[i16], ch: usize, n: usize, looping: bool, i: usize, c: usize, w: [f32; 4]) -> f32 {
    if i >= 1 && i + 2 < n {
        let b = (i - 1) * ch + c;
        let x = |k: usize| f32::from(data[b + k * ch]);
        dot4(w, x(0), x(1), x(2), x(3))
    } else {
        let at = |k: isize| edge_at(data, ch, n, looping, k, c);
        let ii = i as isize;
        dot4(w, at(ii - 1), at(ii), at(ii + 1), at(ii + 2))
    }
}

/// Priming of a stream: window filled as soon as 3 frames are there. False: nothing to read this block
/// (`done` set if the producer left without data).
fn prime(v: &mut Voice) -> bool {
    if v.primed {
        return true;
    }
    let Source::Stream(s) = &mut v.src else { return true };
    let ch = v.channels;
    if s.ring.slots() < 3 * ch {
        if s.ring.is_abandoned() && s.ring.slots() < 2 * ch {
            v.done = true;
        }
        return false;
    }
    v.win[0] = [0.0; 2];
    for k in 1..4 {
        v.win[k] = pull(&mut s.ring, ch).unwrap_or([0.0; 2]);
    }
    v.primed = true;
    true
}

/// One stream frame (`ch` samples), `None` if the ring does not have a whole one.
#[inline]
fn pull(ring: &mut rtrb::Consumer<i16>, ch: usize) -> Option<[f32; 2]> {
    if ring.slots() < ch {
        return None;
    }
    let a = ring.pop().ok()? as f32 * I16;
    let b = if ch == 2 { ring.pop().ok()? as f32 * I16 } else { a };
    Some([a, b])
}

/// Advances the stream by one source frame; `false` = underfed (window held, fraction 0).
#[inline]
fn stream_step(v: &mut Voice, f: &mut f64) -> bool {
    let Source::Stream(s) = &mut v.src else { return false };
    while *f >= 1.0 {
        *f -= 1.0;
        match pull(&mut s.ring, v.channels) {
            Some(nf) => v.win = [v.win[1], v.win[2], v.win[3], nf],
            None => {
                if s.ring.is_abandoned() {
                    v.done = true;
                }
                *f = 0.0;
                return false;
            }
        }
    }
    true
}

/// Muted voice (`MIXMUTE`): same path as [`render`] without writing anything. Buffer: in
/// 16-bit fixed point, exact (the step and the position are multiples of 2⁻¹⁶:
/// the f64 additions of [`render`] are too), frame-by-frame loop otherwise.
fn advance(v: &mut Voice, frames: usize) {
    match &v.src {
        Source::Buffer(pcm) => {
            let n = pcm.data.len() / v.channels;
            let looping = v.p.looping;
            const ONE: f64 = 65536.0;
            if let (Some(mut p), Some(s), Ok(nn)) = (to_fixed16(v.pos), to_fixed16(v.step), i64::try_from(n)) {
                let nn = nn * 65536;
                let mut left = frames as i64;
                while left > 0 {
                    if p >= nn {
                        if looping && nn > 0 {
                            p %= nn;
                        } else {
                            v.done = true;
                            break;
                        }
                    }
                    // Frames read before reaching the end (≥ 1: p < nn, s ≥ 1).
                    let k = ((nn - p + s - 1) / s).min(left);
                    p += k * s;
                    left -= k;
                }
                v.pos = p as f64 / ONE;
            } else {
                let mut pos = v.pos;
                for _ in 0..frames {
                    if pos >= n as f64 {
                        if looping && n > 0 {
                            pos %= n as f64;
                        } else {
                            v.done = true;
                            break;
                        }
                    }
                    pos += v.step;
                }
                v.pos = pos;
            }
        }
        Source::Stream(_) => {
            if !prime(v) {
                return;
            }
            let mut f = v.pos;
            for _ in 0..frames {
                f += v.step;
                if !stream_step(v, &mut f) && v.done {
                    break;
                }
            }
            v.pos = f;
        }
    }
}

/// [`render`] with the cubic weights once per frame (shared by both channels) and
/// direct buffer reading away from the edges (`MIXWEIGHTS`); same output bit for bit.
fn render_fast(v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32) {
    let inv = 1.0 / frames as f32;
    let (dl, dr) = ((tl - v.gl) * inv, (tr - v.gr) * inv);
    let (mut gl, mut gr) = (v.gl, v.gr);
    let stereo = v.channels == 2;
    if let Source::Buffer(pcm) = &v.src {
        let data = &pcm.data[..];
        let ch = v.channels;
        let n = data.len() / ch;
        let looping = v.p.looping;
        let mut pos = v.pos;
        for o in out.as_chunks_mut::<2>().0.iter_mut().take(frames) {
            if pos >= n as f64 {
                if looping && n > 0 {
                    pos %= n as f64;
                } else {
                    v.done = true;
                    break;
                }
            }
            let i = pos as usize;
            let w = weights((pos - i as f64) as f32);
            gl += dl;
            gr += dr;
            let s0 = tap(data, ch, n, looping, i, 0, w) * I16;
            if stereo {
                o[0] += s0 * gl;
                o[1] += tap(data, ch, n, looping, i, 1, w) * I16 * gr;
            } else {
                o[0] += s0 * gl;
                o[1] += s0 * gr;
            }
            pos += v.step;
        }
        v.pos = pos;
    } else {
        if !prime(v) {
            v.gl = tl;
            v.gr = tr;
            return;
        }
        let mut f = v.pos;
        for o in out.as_chunks_mut::<2>().0.iter_mut().take(frames) {
            gl += dl;
            gr += dr;
            let (l, r) = stream_sample(&v.win, f, stereo);
            o[0] += l * gl;
            o[1] += r * gr;
            f += v.step;
            if !stream_step(v, &mut f) && v.done {
                break;
            }
        }
        v.pos = f;
    }
    v.gl = tl;
    v.gr = tr;
}

/// Stream window interpolated at fraction `f`: left and right samples (mono: the same twice).
#[inline]
fn stream_sample(win: &[[f32; 2]; 4], f: f64, stereo: bool) -> (f32, f32) {
    let k = weights(f as f32);
    let l = dot4(k, win[0][0], win[1][0], win[2][0], win[3][0]);
    let r = if stereo { dot4(k, win[0][1], win[1][1], win[2][1], win[3][1]) } else { l };
    (l, r)
}

/// Source frames a stream reads over `frames` output frames from fraction `pos`, plus one. The
/// fraction and the step are multiples of 2⁻¹⁶ (see [`step_of`]), so the frame loop's sums are
/// exact and it reads exactly `floor(pos + frames × step)` frames when it is not underfed.
fn stream_frames_needed(pos: f64, step: f64, frames: usize) -> usize {
    (pos + frames as f64 * step) as usize + 1
}

/// Slice of the ring holding every frame a primed mono or stereo stream reads over the block;
/// `None` when the ring holds less (or the stream is not primed, or has more channels: [`pull`]
/// reads those one sample per frame): the frame-by-frame paths then run as before.
fn block_slice(
    ring: &mut rtrb::Consumer<i16>,
    primed: bool,
    channels: usize,
    pos: f64,
    step: f64,
    frames: usize,
) -> Option<rtrb::chunks::ReadChunk<'_, i16>> {
    if !primed || channels > 2 {
        return None;
    }
    ring.read_chunk(stream_frames_needed(pos, step, frames) * channels).ok()
}

/// Stream frames taken in order from a slice of the ring (its two halves when it wraps).
struct SliceFrames<'a> {
    samples: std::iter::Chain<std::slice::Iter<'a, i16>, std::slice::Iter<'a, i16>>,
    stereo: bool,
    /// Samples taken so far, committed back to the ring at the end of the block.
    taken: usize,
}

impl<'a> SliceFrames<'a> {
    fn new(chunk: &'a rtrb::chunks::ReadChunk<'_, i16>, stereo: bool) -> Self {
        let (first, second) = chunk.as_slices();
        SliceFrames { samples: first.iter().chain(second), stereo, taken: 0 }
    }

    /// Next frame, converted like [`pull`].
    #[inline]
    fn next_frame(&mut self) -> Option<[f32; 2]> {
        let a = f32::from(*self.samples.next()?) * I16;
        let b = if self.stereo { f32::from(*self.samples.next()?) * I16 } else { a };
        self.taken += 1 + usize::from(self.stereo);
        Some([a, b])
    }

    /// Moves the window past every whole source frame in `f`, like [`stream_step`]. The slice
    /// holds the whole block, so it never runs out; if it did, the window would be held as when
    /// underfed.
    #[inline]
    fn slide(&mut self, win: &mut [[f32; 2]; 4], f: &mut f64) {
        while *f >= 1.0 {
            *f -= 1.0;
            let Some(next) = self.next_frame() else {
                *f = 0.0;
                return;
            };
            *win = [win[1], win[2], win[3], next];
        }
    }
}

/// Stream voice read from one slice of its ring (`STREAM_SLICE_READ`). When the ring already holds
/// every frame of the block, no frame-by-frame read could fail, so this reads exactly the samples
/// [`render_fast`] reads and gives the same output, with one exchange of ring positions with the
/// decoding thread per block instead of one or two per sample. `false`: nothing done, see
/// [`block_slice`].
fn render_stream_slice(v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32) -> bool {
    let stereo = v.channels == 2;
    let Source::Stream(s) = &mut v.src else { return false };
    let Some(chunk) = block_slice(&mut s.ring, v.primed, v.channels, v.pos, v.step, frames) else { return false };
    let mut source = SliceFrames::new(&chunk, stereo);
    let inv = 1.0 / frames as f32;
    let (dl, dr) = ((tl - v.gl) * inv, (tr - v.gr) * inv);
    let (mut gl, mut gr) = (v.gl, v.gr);
    let mut f = v.pos;
    for o in out.as_chunks_mut::<2>().0.iter_mut().take(frames) {
        gl += dl;
        gr += dr;
        let (l, r) = stream_sample(&v.win, f, stereo);
        o[0] += l * gl;
        o[1] += r * gr;
        f += v.step;
        source.slide(&mut v.win, &mut f);
    }
    let taken = source.taken;
    chunk.commit(taken);
    v.pos = f;
    v.gl = tl;
    v.gr = tr;
    true
}

/// Muted stream advanced from one slice of its ring (`STREAM_SLICE_READ`): the reads of the
/// frame loop of [`advance`], from the slice. `false`: nothing done, see [`block_slice`].
fn advance_stream_slice(v: &mut Voice, frames: usize) -> bool {
    let Source::Stream(s) = &mut v.src else { return false };
    let Some(chunk) = block_slice(&mut s.ring, v.primed, v.channels, v.pos, v.step, frames) else { return false };
    let mut source = SliceFrames::new(&chunk, v.channels == 2);
    let mut f = v.pos;
    for _ in 0..frames {
        f += v.step;
        source.slide(&mut v.win, &mut f);
    }
    let taken = source.taken;
    chunk.commit(taken);
    v.pos = f;
    true
}

/// Buffer in groups of frames (`MIXSIMD`): 8 frames compiled for AVX2 when `MIXER_AVX2` is on and
/// the processor has it, 4 frames otherwise. Same output bit for bit.
fn render_simd(v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32, opt: MixOpt) {
    let ways = GroupWays { packed_reads: opt.packed_reads, fixed_positions: opt.fixed_positions };
    if opt.avx2 {
        render_buffer_avx2(v, out, frames, tl, tr, ways);
    } else {
        render_buffer_groups::<4>(v, out, frames, tl, tr, ways);
    }
}

/// How a group of frames finds and reads its points (each way gives the same output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GroupWays {
    /// `MIXER_PACKED_READS`: mono and stereo points gathered as 16-bit rows, then converted a
    /// whole row at a time ([`row_samples`]).
    packed_reads: bool,
    /// `MIXER_FIXED_POSITIONS`: lane positions in 16-bit fixed point ([`fixed_lanes`]).
    fixed_positions: bool,
}

/// [`render_buffer_groups`] with 8 lanes compiled for AVX2 (one 256-bit register per row of
/// weights or samples); 4 lanes when the processor lacks AVX2.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
fn render_buffer_avx2(v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32, ways: GroupWays) {
    #[target_feature(enable = "avx2")]
    fn groups_of_8(v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32, ways: GroupWays) {
        render_buffer_groups::<8>(v, out, frames, tl, tr, ways);
    }
    // The switch says nothing about the processor: the detection (done once, one atomic read) is
    // the only guard of the call below.
    if !rc_data::cpu::has_avx2() {
        return render_buffer_groups::<4>(v, out, frames, tl, tr, ways);
    }
    // SAFETY: AVX2 has just been detected on this processor.
    unsafe { groups_of_8(v, out, frames, tl, tr, ways) }
}

#[cfg(not(target_arch = "x86_64"))]
fn render_buffer_avx2(v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32, ways: GroupWays) {
    render_buffer_groups::<4>(v, out, frames, tl, tr, ways);
}

/// Frame index and fraction of the `LANES` frames of a group starting at `pos`, positions added
/// one step at a time like the frame loop; and the position after the group.
#[inline(always)]
fn float_lanes<const LANES: usize>(pos: f64, step: f64) -> ([usize; LANES], [f32; LANES], f64) {
    let mut p = [pos; LANES];
    for j in 1..LANES {
        p[j] = p[j - 1] + step;
    }
    let mut idx = [0usize; LANES];
    let mut frac = [0.0f32; LANES];
    for j in 0..LANES {
        idx[j] = p[j] as usize;
        frac[j] = (p[j] - idx[j] as f64) as f32;
    }
    (idx, frac, p[LANES - 1] + step)
}

/// [`float_lanes`] in 16-bit fixed point (`MIXER_FIXED_POSITIONS`), for a position and a step
/// given as multiples of 2⁻¹⁶ ([`to_fixed16`]). Same values: every f64 sum of the frame loop is
/// exact for such numbers, and the 16 fraction bits fit an f32 exactly. Integer additions, shifts
/// and masks pack into vector registers, where the f64 to integer conversions of [`float_lanes`]
/// go one lane at a time.
#[inline(always)]
fn fixed_lanes<const LANES: usize>(pos: i64, step: i64) -> ([usize; LANES], [f32; LANES], i64) {
    let mut idx = [0usize; LANES];
    let mut frac = [0.0f32; LANES];
    for j in 0..LANES {
        let q = pos + j as i64 * step;
        idx[j] = (q >> 16) as usize;
        frac[j] = (q & 0xFFFF) as i32 as f32 * (1.0 / 65_536.0);
    }
    (idx, frac, pos + LANES as i64 * step)
}

/// `x` in 16-bit fixed point if it is an exact multiple of 2⁻¹⁶ below 2³⁶ (positions and steps
/// always are: [`step_of`] rounds the step, the position starts at 0 and only adds steps or wraps).
fn to_fixed16(x: f64) -> Option<i64> {
    let q = (x * 65_536.0) as i64;
    (q as f64 / 65_536.0 == x && q < 1 << 52).then_some(q)
}

/// Cubic weights of every lane at fractions `frac`, row `t` = weight of point `t` (operations of
/// [`weights`]).
#[inline(always)]
fn lane_weights<const LANES: usize>(frac: &[f32; LANES]) -> [[f32; LANES]; 4] {
    let mut w = [[0.0f32; LANES]; 4];
    for j in 0..LANES {
        let f = frac[j];
        let (fm1, fp1, fm2) = (f - 1.0, f + 1.0, f - 2.0);
        w[0][j] = -f * fm1 * fm2 / 6.0;
        w[1][j] = fp1 * fm1 * fm2 / 2.0;
        w[2][j] = fp1 * f * fm2 / 2.0;
        w[3][j] = fp1 * f * fm1 / 6.0;
    }
    w
}

/// Channel `c` of every lane interpolated and scaled to ±1 (operations of [`dot4`]), the 4
/// points read straight from the buffer: every lane is away from the buffer edges.
#[inline(always)]
fn lane_samples<const LANES: usize>(
    data: &[i16],
    ch: usize,
    c: usize,
    idx: &[usize; LANES],
    w: &[[f32; LANES]; 4],
) -> [f32; LANES] {
    let mut x = [[0.0f32; LANES]; 4];
    for j in 0..LANES {
        let b = (idx[j] - 1) * ch + c;
        for t in 0..4 {
            x[t][j] = f32::from(data[b + t * ch]);
        }
    }
    let mut s = [0.0f32; LANES];
    for j in 0..LANES {
        s[j] = (w[0][j] * x[0][j] + w[1][j] * x[1][j] - w[2][j] * x[2][j] + w[3][j] * x[3][j]) * I16;
    }
    s
}

/// Mono points of every lane as 16-bit rows (`MIXER_PACKED_READS`), row `t` = point `t` of each
/// lane: a row is gathered into one vector register before a single conversion.
#[inline(always)]
fn mono_rows<const LANES: usize>(data: &[i16], idx: &[usize; LANES]) -> [[i16; LANES]; 4] {
    let mut x = [[0i16; LANES]; 4];
    for t in 0..4 {
        for j in 0..LANES {
            x[t][j] = data[idx[j] - 1 + t];
        }
    }
    x
}

/// Stereo points of every lane as 16-bit rows (`MIXER_PACKED_READS`), `[left, right]`, rows as
/// in [`mono_rows`].
#[inline(always)]
fn stereo_rows<const LANES: usize>(data: &[i16], idx: &[usize; LANES]) -> [[[i16; LANES]; 4]; 2] {
    let mut x = [[[0i16; LANES]; 4]; 2];
    for t in 0..4 {
        for j in 0..LANES {
            x[0][t][j] = data[2 * (idx[j] - 1 + t)];
            x[1][t][j] = data[2 * (idx[j] - 1 + t) + 1];
        }
    }
    x
}

/// Rows of points converted to `f32` a whole row at a time (one packed conversion instead of one
/// per point; exact either way), then interpolated and scaled like [`lane_samples`].
#[inline(always)]
fn row_samples<const LANES: usize>(points: &[[i16; LANES]; 4], w: &[[f32; LANES]; 4]) -> [f32; LANES] {
    let mut x = [[0.0f32; LANES]; 4];
    for t in 0..4 {
        for j in 0..LANES {
            x[t][j] = f32::from(points[t][j]);
        }
    }
    let mut s = [0.0f32; LANES];
    for j in 0..LANES {
        s[j] = (w[0][j] * x[0][j] + w[1][j] * x[1][j] - w[2][j] * x[2][j] + w[3][j] * x[3][j]) * I16;
    }
    s
}

/// Left and right samples of every lane (mono: the same twice). `packed`: mono and stereo points
/// gathered as rows ([`row_samples`]); other channel counts, and `packed` off, point by point
/// ([`lane_samples`]).
#[inline(always)]
fn lane_pair<const LANES: usize>(
    data: &[i16],
    ch: usize,
    idx: &[usize; LANES],
    w: &[[f32; LANES]; 4],
    packed: bool,
) -> ([f32; LANES], [f32; LANES]) {
    if packed && ch == 1 {
        let s = row_samples(&mono_rows(data, idx), w);
        return (s, s);
    }
    if packed && ch == 2 {
        let [left, right] = stereo_rows(data, idx);
        return (row_samples(&left, w), row_samples(&right, w));
    }
    let sl = lane_samples(data, ch, 0, idx, w);
    let sr = if ch == 2 { lane_samples(data, ch, 1, idx, w) } else { sl };
    (sl, sr)
}

/// Buffer in groups of `LANES` frames: lane positions in order ([`float_lanes`] or
/// [`fixed_lanes`]), weights and reads in `LANES`-wide arrays (vectorised by the compiler, no
/// FMA), gains ramped frame by frame like [`render`]. A group that touches the buffer edge, and the
/// end of the block: frame by frame.
#[inline(always)]
fn render_buffer_groups<const LANES: usize>(
    v: &mut Voice,
    out: &mut [f32],
    frames: usize,
    tl: f32,
    tr: f32,
    ways: GroupWays,
) {
    let Source::Buffer(pcm) = &v.src else { return render_fast(v, out, frames, tl, tr) };
    let inv = 1.0 / frames as f32;
    let (dl, dr) = ((tl - v.gl) * inv, (tr - v.gr) * inv);
    let (mut gl, mut gr) = (v.gl, v.gr);
    let data = &pcm.data[..];
    let ch = v.channels;
    let stereo = ch == 2;
    let n = data.len() / ch;
    let looping = v.p.looping;
    let step = v.step;
    let mut pos = v.pos;
    // Fixed point only when the position and the step are exact multiples of 2⁻¹⁶ (always, see
    // `to_fixed16`); the position stays exact over the block, so one check per block suffices.
    let fixed_step = to_fixed16(step).filter(|_| ways.fixed_positions && to_fixed16(pos).is_some());
    let out = &mut out.as_chunks_mut::<2>().0[..frames];
    let mut k = 0;
    while k < frames {
        if k + LANES <= frames {
            let (idx, frac, next) = match fixed_step {
                Some(s) => {
                    let (idx, frac, next) = fixed_lanes::<LANES>((pos * 65_536.0) as i64, s);
                    (idx, frac, next as f64 / 65_536.0)
                }
                None => float_lanes::<LANES>(pos, step),
            };
            if pos < n as f64 && idx[0] >= 1 && idx[LANES - 1] + 2 < n {
                let w = lane_weights(&frac);
                let (sl, sr) = lane_pair(data, ch, &idx, &w, ways.packed_reads);
                for j in 0..LANES {
                    gl += dl;
                    gr += dr;
                    let o = &mut out[k + j];
                    o[0] += sl[j] * gl;
                    o[1] += sr[j] * gr;
                }
                pos = next;
                k += LANES;
                continue;
            }
        }
        if pos >= n as f64 {
            if looping && n > 0 {
                pos %= n as f64;
            } else {
                v.done = true;
                break;
            }
        }
        let i = pos as usize;
        let w = weights((pos - i as f64) as f32);
        gl += dl;
        gr += dr;
        let s0 = tap(data, ch, n, looping, i, 0, w) * I16;
        let s1 = if stereo { tap(data, ch, n, looping, i, 1, w) * I16 } else { s0 };
        out[k][0] += s0 * gl;
        out[k][1] += s1 * gr;
        pos += step;
        k += 1;
    }
    v.pos = pos;
    v.gl = tl;
    v.gr = tr;
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let n = dot(a, a).sqrt();
    if n > 1e-12 { [a[0] / n, a[1] / n, a[2] / n] } else { [1.0, 0.0, 0.0] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_clamped_attenuation() {
        assert_eq!(attenuation(0.0, 16.0), 1.0);
        assert_eq!(attenuation(8.0, 16.0), 0.5);
        assert_eq!(attenuation(16.0, 16.0), 0.0);
        assert_eq!(attenuation(40.0, 16.0), 0.0);
        // Volume 4 → D = 4 × 16 = 64.
        assert_eq!(attenuation(32.0, 64.0), 0.5);
    }

    fn voice(pos: [f64; 3], relative: bool, channels: u16) -> Voice {
        let pcm = Arc::new(Pcm { channels, rate: 48_000, data: vec![0; 4] });
        Voice {
            p: VoiceParams {
                id: 1,
                category: Category::Block,
                gain: 1.0,
                pitch: 1.0,
                pos,
                relative,
                linear: true,
                distance: 16.0,
                looping: false,
            },
            src: Source::Buffer(pcm),
            channels: usize::from(channels),
            step: 1.0,
            pos: 0.0,
            win: [[0.0; 2]; 4],
            primed: false,
            gl: 0.0,
            gr: 0.0,
            fresh: true,
            stopping: false,
            done: false,
            paused: false,
        }
    }

    /// Singleplayer pause: the frozen voice no longer sounds and does not finish; resumes where it was
    /// (same end as a playback without pause, shifted by the pause duration).
    #[test]
    fn paused_voice_is_silent_and_resumes_where_it_was() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let (etx, erx) = crossbeam_channel::bounded(16);
        let mut m = Mixer::new(48_000, 8, rx, etx);
        let pcm = Arc::new(Pcm { channels: 1, rate: 48_000, data: vec![1000; 4800] });
        tx.send(Command::Play(voice([0.0; 3], true, 1).p, Source::Buffer(pcm.clone()))).unwrap();
        let mut music = voice([0.0; 3], true, 1).p;
        music.id = 2;
        music.category = Category::Music;
        tx.send(Command::Play(music, Source::Buffer(pcm))).unwrap();
        let mut out = vec![0.0; 2 * 100];
        for _ in 0..10 {
            m.process(&mut out);
        }
        let mut mask = [false; Category::COUNT];
        mask[Category::Block as usize] = true;
        tx.send(Command::Pause(mask)).unwrap();
        // Music not frozen: ends at 4,900 frames; the frozen block stays silent and alive.
        let mut music_done = None;
        for b in 11..=200 {
            m.process(&mut out);
            if let Ok(Event::Finished(id, _)) = erx.try_recv() {
                assert_eq!(id, 2, "only the music ends during the pause");
                music_done = Some(b * 100);
            }
            if music_done.is_some() {
                assert!(out.iter().all(|&s| s == 0.0), "frozen voice heard");
            }
        }
        assert_eq!(music_done, Some(4900));
        tx.send(Command::Resume).unwrap();
        // Resume: 3,900 frames were left (+ the fade block, as without pause).
        for b in 1..=200 {
            m.process(&mut out);
            if b == 1 {
                assert!(out[0] > 0.0, "silent resume");
            }
            if let Ok(Event::Finished(id, _)) = erx.try_recv() {
                assert_eq!((id, b * 100), (1, 3900));
                return;
            }
        }
        panic!("voice never finished after the resume");
    }

    /// Playback duration of a buffer depending on the pitch, end signalled with the source returned.
    fn frames_until_done(pitch: f32, looping: bool) -> Option<usize> {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let (etx, erx) = crossbeam_channel::bounded(16);
        let mut m = Mixer::new(48_000, 8, rx, etx);
        let pcm = Arc::new(Pcm { channels: 1, rate: 48_000, data: vec![1000; 4800] });
        let mut v = voice([0.0; 3], true, 1).p;
        v.pitch = pitch;
        v.looping = looping;
        tx.send(Command::Play(v, Source::Buffer(pcm))).unwrap();
        let mut out = vec![0.0; 2 * 100];
        for b in 1..=200 {
            m.process(&mut out);
            if b == 1 {
                assert!(out[0] > 0.0 && (out[0] - out[1]).abs() < 1e-6);
            }
            if let Ok(Event::Finished(id, _)) = erx.try_recv() {
                assert_eq!(id, 1);
                return Some(b * 100);
            }
        }
        None
    }

    #[test]
    fn playback_length_follows_pitch() {
        assert_eq!(frames_until_done(1.0, false), Some(4900));
        assert_eq!(frames_until_done(2.0, false), Some(2500));
        assert_eq!(frames_until_done(0.5, false), Some(9700));
        assert_eq!(frames_until_done(1.0, true), None);
    }

    #[test]
    fn panning_follows_listener() {
        let l = Listener::default();
        let right = normalize(cross(l.forward, l.up));
        assert_eq!(right, [1.0, 0.0, 0.0]);
        // Source on the right (+X) at 8 blocks: fully right, attenuated by half.
        let (gl, gr) = target_gain(&voice([8.0, 0.0, 0.0], false, 1), &l, right, 1.0);
        assert!(gl.abs() < 1e-6 && (gr - 0.5).abs() < 1e-6, "{gl} {gr}");
        // In front: centred, 0.5957 per channel (OpenAL Soft's stereo decoder).
        let (gl, gr) = target_gain(&voice([0.0, 0.0, -8.0], false, 1), &l, right, 1.0);
        assert!((gl - gr).abs() < 1e-6 && (gl - 0.5 * 0.595_662).abs() < 1e-5, "{gl}");
        // Listener turned towards +X: the same source is in front.
        let l2 = Listener { forward: [1.0, 0.0, 0.0], ..l };
        let r2 = normalize(cross(l2.forward, l2.up));
        let (gl, gr) = target_gain(&voice([8.0, 0.0, 0.0], false, 1), &l2, r2, 1.0);
        assert!((gl - gr).abs() < 1e-6);
        // Stereo: neither attenuated nor spatialised.
        assert_eq!(target_gain(&voice([100.0, 0.0, 0.0], false, 2), &l, right, 0.5), (0.5, 0.5));
        // Relative to the origin: centred, no attenuation.
        let (gl, gr) = target_gain(&voice([0.0; 3], true, 1), &l, right, 1.0);
        assert!((gl - 0.595_662).abs() < 1e-5 && (gl - gr).abs() < 1e-6);
    }

    /// Bench (ignored), loaded machine figures only: cost of a command found by voice id
    /// (position of an attached sound), 64 and 247 live voices, ids list against the voice walk.
    /// `cargo-file.ps1 test --release -p rc-audio voice_lookup_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "bench"]
    fn voice_lookup_bench() {
        for pass in 0..2 {
            for voices in [64u64, 247] {
                for voice_ids in [false, true] {
                    let (_tx, rx) = crossbeam_channel::bounded(1);
                    let (etx, _erx) = crossbeam_channel::bounded(1);
                    let mut m = Mixer::new(48_000, 300, rx, etx);
                    m.opt.voice_ids = voice_ids;
                    let pcm = noise(1, 100, 1, 48_000);
                    for id in 0..voices {
                        m.command(Command::Play(params(id, [0.0; 3], 1.0, true), Source::Buffer(pcm.clone())));
                    }
                    const N: u64 = 2_000_000;
                    let t = std::time::Instant::now();
                    for k in 0..N {
                        // Spread over the voices like the attached sounds of a crowd.
                        let id = k.wrapping_mul(0x9E37_79B9) % voices;
                        m.command(Command::Position(id, [k as f64, 0.0, 0.0]));
                    }
                    let ns = t.elapsed().as_secs_f64() * 1e9 / N as f64;
                    println!("pass {pass}: {voices} voices, voice_ids={voice_ids}: {ns:.1} ns/command");
                }
            }
        }
    }

    /// Deterministic noise (xorshift), `frames` frames of `ch` channels.
    fn noise(ch: u16, frames: usize, seed: u32, rate: u32) -> Arc<Pcm> {
        let mut x = seed | 1;
        let data = (0..frames * usize::from(ch))
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 16) as i16
            })
            .collect();
        Arc::new(Pcm { channels: ch, rate, data })
    }

    fn params(id: u64, pos: [f64; 3], pitch: f32, looping: bool) -> VoiceParams {
        VoiceParams {
            id,
            category: if id.is_multiple_of(3) { Category::Ambient } else { Category::Block },
            gain: 0.9,
            pitch,
            pos,
            relative: id.is_multiple_of(5),
            linear: true,
            distance: 16.0,
            looping,
        }
    }

    /// Scenario of 2,000 blocks (buffers and streams, mono and stereo, pitches 0.5-2, looping,
    /// buffers of 0 to 3 frames, out of range then back, Position/Gain/Pitch changed, underfed
    /// stream then abandoned, category at 0, fades, Stop, StopAll, turning listener,
    /// blocks of 480 and 437 frames): FNV-1a fingerprint (per 32-bit word) of the output f32s and of the
    /// voice ends.
    fn mix_digest_with(opt: MixOpt) -> u64 {
        let (tx, rx) = crossbeam_channel::bounded(256);
        let (etx, erx) = crossbeam_channel::bounded(256);
        let mut m = Mixer::new(48_000, 64, rx, etx);
        m.opt = opt;
        let play = |id, p: VoiceParams, src| tx.send(Command::Play(VoiceParams { id, ..p }, src)).unwrap();
        let buf = |p: Arc<Pcm>| Source::Buffer(p);
        play(1, params(1, [3.0, 0.0, -4.0], 1.0, false), buf(noise(1, 30_000, 1, 44_100)));
        play(2, params(2, [1.0, 2.0, 0.5], 0.5, true), buf(noise(1, 7_001, 2, 44_100)));
        play(3, params(3, [0.0; 3], 2.0, true), buf(noise(2, 5_000, 3, 48_000)));
        play(4, params(4, [0.0; 3], 1.37, false), buf(noise(2, 90_000, 4, 22_050)));
        play(6, params(6, [100.0, 0.0, 0.0], 1.0, true), buf(noise(1, 9_000, 6, 44_100)));
        play(7, VoiceParams { gain: 0.0, ..params(7, [2.0, 0.0, 2.0], 0.75, true) }, buf(noise(1, 4_000, 7, 32_000)));
        for (id, n) in [(8u64, 0usize), (9, 1), (11, 2), (12, 3)] {
            play(id, params(id, [-2.0, 1.0, 1.0], 0.66, true), buf(noise(1, n, id as u32, 44_100)));
        }
        play(13, params(13, [-1.0, 0.0, 0.0], 1.0, false), buf(noise(1, 2, 13, 44_100)));
        play(14, params(14, [5.0, 0.0, 5.0], 1.9, true), buf(noise(2, 3_333, 14, 44_100)));
        // Integer step (48 kHz, pitch 1) out of range: end of buffer reached exactly.
        play(22, params(22, [60.0, 0.0, 0.0], 1.0, true), buf(noise(1, 4_801, 22, 48_000)));
        // Streams: underfed mono (300 samples per block), full stereo then abandoned,
        // mono out of range then brought back.
        let (mut p16, c16) = rtrb::RingBuffer::new(20_000);
        play(16, params(16, [0.0, 0.0, -3.0], 1.0, true), Source::Stream(StreamIn::manual(1, 44_100, c16)));
        let feed16 = noise(1, 400_000, 16, 44_100);
        let (mut p17, c17) = rtrb::RingBuffer::new(200_000);
        for &s in &noise(2, 90_000, 17, 48_000).data {
            p17.push(s).unwrap();
        }
        drop(p17);
        play(17, params(17, [0.0; 3], 1.25, false), Source::Stream(StreamIn::manual(2, 48_000, c17)));
        let (mut p18, c18) = rtrb::RingBuffer::new(400_000);
        for &s in &noise(1, 300_000, 18, 44_100).data {
            p18.push(s).unwrap();
        }
        play(18, params(18, [50.0, 0.0, 0.0], 1.0, true), Source::Stream(StreamIn::manual(1, 44_100, c18)));
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        fn eat(h: &mut u64, b: u32) {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut out = vec![0.0f32; 2 * 480];
        let mut fed = 0;
        for b in 0..2000u32 {
            let a = b as f32 * 0.013;
            m.listener = Listener { pos: [0.5, 0.0, 0.25], forward: [a.sin(), 0.1, -a.cos()], up: [0.0, 1.0, 0.0] };
            for _ in 0..300 {
                if fed < feed16.data.len() && p16.push(feed16.data[fed]).is_ok() {
                    fed += 1;
                }
            }
            let c = |cmd| tx.send(cmd).unwrap();
            match b {
                300 => {
                    c(Command::Position(6, [4.0, 0.0, 0.0]));
                    c(Command::Position(18, [2.0, 0.0, 1.0]));
                }
                400 => c(Command::Gain(7, 0.8)),
                700 => c(Command::Position(22, [1.0, 0.0, 3.0])),
                500 => c(Command::Pitch(2, 1.8)),
                600 => {
                    c(Command::Position(6, [40.0, 0.0, 0.0]));
                    c(Command::Pitch(18, 0.5));
                }
                800 => c(Command::Gain(7, 0.0)),
                1000 => {
                    let mut k = [1.0; Category::COUNT];
                    k[Category::Block as usize] = 0.0;
                    c(Command::Categories(k));
                }
                1100 => {
                    let mut k = [1.0; Category::COUNT];
                    k[Category::Block as usize] = 0.5;
                    c(Command::Categories(k));
                }
                1200 => {
                    play(20, params(20, [1.0, 0.0, -1.0], 0.5, true), buf(noise(1, 12_000, 20, 44_100)));
                    play(21, params(21, [0.0; 3], 1.1, true), buf(noise(2, 6_000, 21, 44_100)));
                }
                1500 => c(Command::Stop(2)),
                1900 => c(Command::StopAll),
                _ => {}
            }
            let len = if b % 3 == 0 { 437 } else { 480 };
            m.process(&mut out[..2 * len]);
            for x in &out[..2 * len] {
                eat(&mut h, x.to_bits());
            }
            while let Ok(Event::Finished(id, _)) = erx.try_recv() {
                eat(&mut h, 0xF000_0000 | id as u32);
                eat(&mut h, b);
            }
        }
        h
    }

    /// Frozen fingerprint of the old mixer (all switches at 0), recorded on Windows: the listener
    /// turns with the C library's `sin`/`cos`, whose last bits differ on Linux.
    const MIX_DIGEST: u64 = 0x9b76_17dd_9db9_66f2;

    /// Every combination of the optimisations reproduces the old mixer bit for bit (the AVX2 ones
    /// run the 4-lane path on a processor without AVX2).
    #[test]
    fn mix_digest() {
        if !rc_data::cpu::has_avx2() {
            eprintln!("no AVX2 on this processor: MIXER_AVX2 combinations run the 4-lane path");
        }
        let mut seen = Vec::new();
        for k in 0..256 {
            let opt = MixOpt {
                mute: k & 1 != 0,
                weights: k & 2 != 0,
                simd: k & 4 != 0,
                voice_ids: k & 8 != 0,
                avx2: k & 16 != 0,
                packed_reads: k & 32 != 0,
                fixed_positions: k & 64 != 0,
                stream_slice: k & 128 != 0,
            };
            if !opt.simd && (opt.avx2 || opt.packed_reads || opt.fixed_positions) {
                // Ways of the buffer groups, unused without them: same run as with them off.
                continue;
            }
            seen.push((opt, mix_digest_with(opt)));
        }
        assert_eq!(seen.len(), 144);
        let expected = if cfg!(windows) { MIX_DIGEST } else { seen[0].1 };
        for (opt, d) in &seen {
            assert_eq!(*d, expected, "{opt:?}: fingerprint {d:#018x}, expected {expected:#018x}; all: {seen:x?}");
        }
    }

    /// Every optimisation on (the benches turn one off at a time).
    const ALL_ON: MixOpt = MixOpt {
        mute: true,
        weights: true,
        simd: true,
        voice_ids: true,
        avx2: true,
        packed_reads: true,
        fixed_positions: true,
        stream_slice: true,
    };

    /// Width of the buffer groups under test.
    #[derive(Debug, Clone, Copy)]
    enum Width {
        Four,
        /// 8 lanes compiled for the baseline x86-64 (SSE2).
        EightPortable,
        /// 8 lanes compiled for AVX2 (4 without it).
        EightAvx2,
    }

    /// One buffer group renderer: its width and its ways.
    type Renderer = (Width, GroupWays);

    /// The 4 combinations of the ways, the old one (both off) first.
    const WAYS: [GroupWays; 4] = [
        GroupWays { packed_reads: false, fixed_positions: false },
        GroupWays { packed_reads: true, fixed_positions: false },
        GroupWays { packed_reads: false, fixed_positions: true },
        GroupWays { packed_reads: true, fixed_positions: true },
    ];

    fn render_with((width, ways): Renderer, v: &mut Voice, out: &mut [f32], frames: usize, tl: f32, tr: f32) {
        match width {
            Width::Four => render_buffer_groups::<4>(v, out, frames, tl, tr, ways),
            Width::EightPortable => render_buffer_groups::<8>(v, out, frames, tl, tr, ways),
            Width::EightAvx2 => render_buffer_avx2(v, out, frames, tl, tr, ways),
        }
    }

    /// Every renderer, the old one (4 lanes, both ways off) first.
    fn renderers() -> Vec<Renderer> {
        [Width::Four, Width::EightPortable, Width::EightAvx2]
            .into_iter()
            .flat_map(|width| WAYS.map(|ways| (width, ways)))
            .collect()
    }

    /// Median of `x` (sorted in place).
    fn median(x: &mut [f64]) -> f64 {
        x.sort_by(f64::total_cmp);
        x[x.len() / 2]
    }

    /// Bench (ignored), loaded machine figures only. Buffer group renderers called directly on 64
    /// and 247 looping voices of synthetic noise (one in four stereo, steps of pitches 0.5-2 from
    /// 44.1 kHz, 6 shared buffers of 1 s), 31 rounds of 50 blocks per renderer, interleaved: best
    /// time (the load of the other processes only ever adds some) and median, over the rounds, of
    /// the ratio to the old renderer measured in the same round. Then whole mixer blocks with every
    /// other voice out of range, and the read of a music (stereo) and a record (mono) stream whose
    /// rings are kept full, slice against frame by frame; 2 alternating passes.
    /// `cargo-file.ps1 test --release -p rc-audio mixer_lanes_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "bench"]
    fn mixer_lanes_bench() {
        const BLOCK: usize = 480;
        const BLOCKS: usize = 2_000;
        const ROUNDS: usize = 31;
        println!("AVX2 detected: {}", rc_data::cpu::has_avx2());
        let buffers: Vec<_> = (0..6u32).map(|k| noise(if k % 4 == 0 { 2 } else { 1 }, 44_100, k + 1, 44_100)).collect();
        let all = renderers();
        for voices in [64usize, 247] {
            let mut set: Vec<Voice> = (0..voices)
                .map(|id| {
                    let pitch = 0.5 + (id % 7) as f32 * 0.25;
                    let step = (step_of(44_100, pitch, 48_000) * 65_536.0) as u32;
                    ready_voice(Source::Buffer(buffers[id % buffers.len()].clone()), true, (id * 97) as f64, step)
                })
                .collect();
            let mut times = vec![Vec::with_capacity(ROUNDS); all.len()];
            let mut out = vec![0.0f32; 2 * BLOCK];
            for _ in 0..ROUNDS {
                for (renderer, t_renderer) in all.iter().zip(&mut times) {
                    let t = std::time::Instant::now();
                    for _ in 0..50 {
                        out.fill(0.0);
                        for v in &mut set {
                            render_with(*renderer, v, &mut out, BLOCK, 0.5, 0.4);
                        }
                    }
                    t_renderer.push(t.elapsed().as_secs_f64() * 1e9 / (50 * BLOCK * voices) as f64);
                }
            }
            for (renderer, t) in all.iter().zip(&times) {
                let mut ratios: Vec<f64> = t.iter().zip(&times[0]).map(|(a, b)| a / b).collect();
                println!(
                    "{voices} voices, {renderer:?}: best {:.2} ns / voice·frame, median ratio to the old one {:.3}",
                    t.iter().copied().fold(f64::INFINITY, f64::min),
                    median(&mut ratios),
                );
            }
        }
        for pass in 0..2 {
            for new in [false, true] {
                let (_tx, rx) = crossbeam_channel::bounded(1);
                let (etx, _erx) = crossbeam_channel::bounded(1);
                let mut m = Mixer::new(48_000, 300, rx, etx);
                m.opt = MixOpt { avx2: new, packed_reads: new, fixed_positions: new, ..ALL_ON };
                for id in 0..247u64 {
                    let a = id as f64 * 0.7;
                    let r = if id % 2 == 1 { 40.0 } else { 6.0 };
                    let pitch = 0.5 + (id % 7) as f32 * 0.25;
                    let pcm = buffers[id as usize % buffers.len()].clone();
                    m.command(Command::Play(params(id, [a.cos() * r, 0.0, a.sin() * r], pitch, true), Source::Buffer(pcm)));
                }
                let mut out = vec![0.0f32; 2 * BLOCK];
                m.process(&mut out);
                let t = std::time::Instant::now();
                for _ in 0..BLOCKS {
                    m.process(&mut out);
                }
                println!(
                    "pass {pass}: mixer, 247 voices, half out of range, {}: {:.1} µs / block",
                    if new { "8 lanes AVX2, packed reads, fixed positions" } else { "4 lanes (old)" },
                    t.elapsed().as_secs_f64() * 1e6 / BLOCKS as f64,
                );
            }
            for stream_slice in [false, true] {
                let (_tx, rx) = crossbeam_channel::bounded(1);
                let (etx, _erx) = crossbeam_channel::bounded(1);
                let mut m = Mixer::new(48_000, 8, rx, etx);
                m.opt = MixOpt { stream_slice, ..ALL_ON };
                let feeds = [noise(2, 96_000, 41, 44_100), noise(1, 96_000, 42, 44_100)];
                let mut producers = Vec::new();
                for (id, feed) in feeds.iter().enumerate() {
                    let (p, ring) = rtrb::RingBuffer::new(2 * 44_100 * usize::from(feed.channels));
                    producers.push((p, 0usize));
                    let src = Source::Stream(StreamIn::manual(feed.channels, feed.rate, ring));
                    m.command(Command::Play(params(id as u64 + 1, [0.0; 3], 1.0, true), src));
                }
                let mut out = vec![0.0f32; 2 * BLOCK];
                let mut timed = std::time::Duration::ZERO;
                for _ in 0..BLOCKS {
                    // Rings refilled outside the timed part, as the decoding threads do.
                    for ((p, next), feed) in producers.iter_mut().zip(&feeds) {
                        while p.slots() > 0 {
                            p.push(feed.data[*next]).unwrap();
                            *next = (*next + 1) % feed.data.len();
                        }
                    }
                    let t = std::time::Instant::now();
                    m.process(&mut out);
                    timed += t.elapsed();
                }
                println!(
                    "pass {pass}: 2 streams, {}: {:.2} µs / block",
                    if stream_slice { "read from a slice" } else { "read frame by frame" },
                    timed.as_secs_f64() * 1e6 / BLOCKS as f64,
                );
            }
        }
    }

    /// A voice past its first block (gains ramping from 0.3 / 0.7) on source `src`, at `pos`
    /// with a step of `step_units` / 65,536 (as [`step_of`] rounds it).
    fn ready_voice(src: Source, looping: bool, pos: f64, step_units: u32) -> Voice {
        let mut v = voice([0.0; 3], true, src.channels());
        v.src = src;
        v.p.looping = looping;
        v.pos = pos;
        v.step = f64::from(step_units) / 65_536.0;
        v.gl = 0.3;
        v.gr = 0.7;
        v.fresh = false;
        v
    }

    fn bits(out: &[f32]) -> Vec<u32> {
        out.iter().map(|x| x.to_bits()).collect()
    }

    /// Steps below and above one source frame per output frame, and the largest one (pitch 2).
    const STEPS: [u32; 5] = [1 << 14, 30_000, 1 << 16, 65_536 + 4_097, 150_000];

    /// Every buffer group renderer (4 or 8 lanes, generic or AVX2, each combination of the ways)
    /// gives the output, position and end of the old one (4 lanes, both ways off) bit for bit: mono
    /// and stereo, short and long buffers, looping or not, starts at and near both buffer edges,
    /// blocks shorter than a group and with a partial last group.
    #[test]
    fn group_renderers_match_the_old_one() {
        let mut cases = 0;
        for ch in [1u16, 2] {
            for (len, looping) in [(3usize, true), (11, false), (40, true), (5_000, false), (5_000, true)] {
                let pcm = noise(ch, len, 7 + len as u32, 44_100);
                for step in STEPS {
                    for start in [0.0, 1.5, len as f64 - 9.25, len as f64 - 0.5] {
                        for frames in [1usize, 7, 8, 437, 480] {
                            let run = |renderer: Renderer| {
                                let mut v = ready_voice(Source::Buffer(pcm.clone()), looping, start.max(0.0), step);
                                let mut out = vec![0.25f32; 2 * frames];
                                render_with(renderer, &mut v, &mut out, frames, 0.9, 0.1);
                                (bits(&out), v.pos.to_bits(), v.done)
                            };
                            let all = renderers();
                            let old = run(all[0]);
                            for renderer in &all[1..] {
                                assert_eq!(run(*renderer), old, "{renderer:?}: {ch} ch, {len} frames, step {step}, start {start}");
                            }
                            cases += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 2 * 5 * STEPS.len() * 4 * 5);
    }

    /// Primed stream voice over a ring holding `feed`; the producer is returned so the ring is
    /// not abandoned.
    fn stream_voice(feed: &Pcm, step: u32) -> (rtrb::Producer<i16>, Voice) {
        let (mut producer, ring) = rtrb::RingBuffer::new(feed.data.len());
        for &s in &feed.data {
            producer.push(s).unwrap();
        }
        let v = ready_voice(Source::Stream(StreamIn::manual(feed.channels, feed.rate, ring)), true, 0.0, step);
        (producer, v)
    }

    fn ring_slots(v: &Voice) -> usize {
        match &v.src {
            Source::Stream(s) => s.ring.slots(),
            Source::Buffer(_) => 0,
        }
    }

    /// Two identical streams, one read from slices of its ring and the other frame by frame,
    /// audible and muted blocks alternated until the ring runs dry: same output, fraction, window,
    /// end and samples left after every block. A ring that does not hold the whole block is
    /// refused untouched, the frame-by-frame path then underfeeds as before.
    #[test]
    fn stream_slice_matches_frame_reads() {
        for ch in [1u16, 2] {
            let feed = noise(ch, 20_000, 3, 44_100);
            for step in STEPS {
                let (_keep_a, mut a) = stream_voice(&feed, step);
                let (_keep_b, mut b) = stream_voice(&feed, step);
                let (mut sliced, mut refused) = (0, 0);
                for block in 0..60 {
                    let frames = if block % 3 == 0 { 437 } else { 480 };
                    let (mut out_a, mut out_b) = (vec![0.0f32; 2 * frames], vec![0.0f32; 2 * frames]);
                    let (before, primed) = (ring_slots(&b), b.primed);
                    let took_slice = if block % 4 == 3 {
                        advance(&mut a, frames);
                        let ok = advance_stream_slice(&mut b, frames);
                        if !ok {
                            assert_eq!(ring_slots(&b), before, "refused slice consumed samples");
                            advance(&mut b, frames);
                        }
                        ok
                    } else {
                        render_fast(&mut a, &mut out_a, frames, 0.8, 0.2);
                        let ok = render_stream_slice(&mut b, &mut out_b, frames, 0.8, 0.2);
                        if !ok {
                            assert_eq!(ring_slots(&b), before, "refused slice consumed samples");
                            render_fast(&mut b, &mut out_b, frames, 0.8, 0.2);
                        }
                        ok
                    };
                    if took_slice {
                        sliced += 1;
                    } else if primed {
                        refused += 1;
                    }
                    let state = |v: &Voice| (v.pos.to_bits(), v.win.map(|f| f.map(f32::to_bits)), v.done, ring_slots(v));
                    assert_eq!(bits(&out_b), bits(&out_a), "{ch} ch, step {step}, block {block}");
                    assert_eq!(state(&b), state(&a), "{ch} ch, step {step}, block {block}");
                }
                assert!(sliced > 10, "{ch} ch, step {step}: only {sliced} blocks read from a slice");
                if step > 1 << 15 {
                    assert!(refused > 0, "{ch} ch, step {step}: the ring never ran dry");
                }
            }
        }
    }

    /// Comparison with the probe of the game's OpenAL Soft (`RUSTCRAFT_AL_PROBE`: file from
    /// `al_probe.py`): gains by azimuth, pitch and distance, then output of a 44,100 Hz
    /// noise rendered at 48,000 Hz (pitches 1 and 1.5; the probe has 48 frames of delay, those
    /// of its output limiter's look-ahead).
    #[test]
    fn matches_openal_probe() {
        let Some(path) = rc_data::env_os!("RUSTCRAFT_AL_PROBE") else { return };
        let j: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let l = Listener::default();
        let right = normalize(cross(l.forward, l.up));
        let f = |v: &serde_json::Value| v.as_f64().unwrap() as f32;
        for e in j["pan"].as_array().unwrap() {
            let a = f(&e[0]).to_radians();
            let (gl, gr) = target_gain(&voice([f64::from(a.sin()), 0.0, -f64::from(a.cos())], true, 1), &l, right, 0.5);
            assert!(
                (gl - f(&e[1])).abs() < 2e-5 && (gr - f(&e[2])).abs() < 2e-5,
                "{}: {gl} {gr} / {} {}",
                e[0],
                e[1],
                e[2]
            );
        }
        let (gl, _) = target_gain(&voice([0.0, 1.0, 0.0], true, 1), &l, right, 0.5);
        assert!((gl - f(&j["above"][0])).abs() < 2e-5);
        for e in j["dist"].as_array().unwrap() {
            let (gl, _) = target_gain(&voice([0.0, 0.0, -f64::from(f(&e[0]))], true, 1), &l, right, 0.5);
            assert!((gl - f(&e[1])).abs() < 2e-5, "{e}");
        }
        let noise: Vec<i16> = j["noise"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap() as i16).collect();
        for (key, pitch, tol) in [("res_1", 1.0f32, 1e-3f32), ("res_15", 1.5, 5e-3)] {
            let (tx, rx) = crossbeam_channel::bounded(16);
            let (etx, _erx) = crossbeam_channel::bounded(16);
            let mut m = Mixer::new(48_000, 8, rx, etx);
            let pcm = Arc::new(Pcm { channels: 1, rate: 44_100, data: noise.clone() });
            let mut p = voice([0.0, 0.0, -0.001], true, 1).p;
            p.pitch = pitch;
            tx.send(Command::Play(p, Source::Buffer(pcm))).unwrap();
            let mut out = vec![0.0; 2 * 2000];
            m.process(&mut out);
            let want = j[key].as_array().unwrap();
            let mut worst = 0.0f32;
            for n in 100..1900 {
                worst = worst.max((out[2 * n] - f(&want[n + 48])).abs());
            }
            assert!(worst < tol, "{key}: difference {worst}");
        }
    }
}
