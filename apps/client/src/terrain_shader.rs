//! Variants of the terrain shader (`terrain.wgsl`), chosen once at startup (C3-M05).
//! The source carries `//#if NAME`, `//#if !NAME`, `//#else`, `//#endif` blocks (nestable);
//! [`source`] keeps the block of each variant in force. The image is the same to the bit in
//! all the variants (proofs: tests below and `render_gpu_test.rs`).

/// Measurement of the vertex / fragment share (`RUSTCRAFT_GPUPROBE`, diagnostic never active by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Off,
    /// `vs`: every vertex leaves the field of view (everything is clipped: vertex work alone).
    Vs,
    /// `fs`: fragments with constant colour (no texture or fog).
    Fs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderOpts {
    /// `RUSTCRAFT_OPT_VSREAD`: quad fields read one by one in their buffer, instead of a
    /// local copy of the whole quad then indexed by the corner.
    pub vsread: bool,
    /// `RUSTCRAFT_OPT_QUADBITS`: vertex corner drawn from a nibble constant instead of a
    /// constant indexed array (which forces an array in local memory).
    pub quadbits: bool,
    /// `RUSTCRAFT_OPT_VARYINGS`: vertex outputs grouped into 3 slots instead of 7.
    pub varyings: bool,
    /// `RUSTCRAFT_OPT_FADEGPU`: fade-in visibility computed on the GPU from the column's
    /// birth instant (no more rewriting of the ring on every frame).
    pub fadegpu: bool,
    /// `RUSTCRAFT_OPT_FACEIDX`: block faces indexed without instances (4 vertices per face,
    /// vertex cache), entry point `vs_idx`.
    pub faceidx: bool,
    /// `RUSTCRAFT_OPT_STRIP`: block faces as a strip of 4 vertices (only without FACEIDX).
    pub strip: bool,
    pub probe: Probe,
}

impl ShaderOpts {
    /// Complete old path (each switch at 0).
    #[cfg(test)]
    pub const OLD: ShaderOpts = ShaderOpts {
        vsread: false,
        quadbits: false,
        varyings: false,
        fadegpu: false,
        faceidx: false,
        strip: false,
        probe: Probe::Off,
    };

    /// Default: each new version.
    #[cfg(test)]
    pub const NEW: ShaderOpts = ShaderOpts {
        vsread: true,
        quadbits: true,
        varyings: true,
        fadegpu: true,
        faceidx: true,
        strip: true,
        probe: Probe::Off,
    };

    /// Strip in force (indexing wins).
    pub fn strip_on(self) -> bool {
        self.strip && !self.faceidx
    }

    /// Vertices per block face drawn without index.
    pub fn face_vertices(self) -> u32 {
        if self.strip_on() { 4 } else { 6 }
    }

    /// Entry point of the block faces.
    pub fn face_entry(self) -> &'static str {
        if self.faceidx { "vs_idx" } else { "vs" }
    }

    pub fn topology(self) -> wgpu::PrimitiveTopology {
        if self.strip_on() { wgpu::PrimitiveTopology::TriangleStrip } else { wgpu::PrimitiveTopology::TriangleList }
    }

    fn defined(self, name: &str) -> bool {
        match name {
            "VSREAD" => self.vsread,
            "QUADBITS" => self.quadbits,
            "VARYINGS" => self.varyings,
            "FADEGPU" => self.fadegpu,
            "STRIP" => self.strip_on(),
            "PROBE_VS" => self.probe == Probe::Vs,
            "PROBE_FS" => self.probe == Probe::Fs,
            _ => panic!("terrain.wgsl: unknown variant '{name}'"),
        }
    }
}

static OPTS: std::sync::OnceLock<ShaderOpts> = std::sync::OnceLock::new();

/// Variants in force, read once (default 1, `=0`: old path).
pub fn opts() -> ShaderOpts {
    *OPTS.get_or_init(|| {
        let off = |v: Option<&str>| v == Some("0");
        let probe = match rc_data::env_str!("RUSTCRAFT_GPUPROBE") {
            Some("vs") => Probe::Vs,
            Some("fs") => Probe::Fs,
            _ => Probe::Off,
        };
        let o = ShaderOpts {
            vsread: !off(rc_data::env_str!("RUSTCRAFT_OPT_VSREAD")),
            quadbits: !off(rc_data::env_str!("RUSTCRAFT_OPT_QUADBITS")),
            varyings: !off(rc_data::env_str!("RUSTCRAFT_OPT_VARYINGS")),
            fadegpu: !off(rc_data::env_str!("RUSTCRAFT_OPT_FADEGPU")),
            faceidx: !off(rc_data::env_str!("RUSTCRAFT_OPT_FACEIDX")),
            strip: !off(rc_data::env_str!("RUSTCRAFT_OPT_STRIP")),
            probe,
        };
        eprintln!("[opt] terrain shader: {o:?}");
        o
    })
}

/// `base` (terrain.wgsl) reduced to the blocks of variant `o`.
pub fn source(base: &str, o: ShaderOpts) -> String {
    let mut out = String::with_capacity(base.len());
    // Stack of open blocks: (block kept, parent kept).
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let mut keep = true;
    for line in base.lines() {
        let t = line.trim_start();
        if let Some(cond) = t.strip_prefix("//#if ") {
            let cond = cond.trim();
            let on = match cond.strip_prefix('!') {
                Some(n) => !o.defined(n),
                None => o.defined(cond),
            };
            stack.push((on, keep));
            keep = keep && on;
        } else if t.starts_with("//#else") {
            let (on, parent) = stack.last_mut().expect("terrain.wgsl: //#else without //#if");
            *on = !*on;
            keep = *parent && *on;
        } else if t.starts_with("//#endif") {
            let (_, parent) = stack.pop().expect("terrain.wgsl: //#endif without //#if");
            keep = parent;
        } else if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(stack.is_empty(), "terrain.wgsl: //#if not closed");
    out
}

/// Terrain shader of variant `o`.
pub fn terrain(o: ShaderOpts) -> String {
    source(include_str!("terrain.wgsl"), o)
}

/// Instant of the fade-in: microseconds since `epoch`, modulo 2^32 (the integer difference
/// stays exact as long as it fits under 71 minutes; a finished column receives the
/// sentinel well before). Never equal to the sentinel.
pub fn fade_clock(epoch: std::time::Instant, t: std::time::Instant) -> u32 {
    let us = t.saturating_duration_since(epoch).as_micros() as u32;
    us.min(FADE_DONE - 1)
}

/// Ring slot whose fade is finished (or never started): visibility 1.
pub const FADE_DONE: u32 = u32::MAX;

/// Fade-in visibility as the shader computes it (`FADEGPU`), in f32 like it.
#[cfg(test)]
pub fn fade_visibility(now: u32, born: u32, len_us: f32) -> f32 {
    let e = now.wrapping_sub(born) as f32;
    if born == FADE_DONE || e >= len_us { 1.0 } else { e / len_us }
}

/// Side of the fade-in ring (column x, z modulo this side).
pub const FADE_RING: usize = 128;

/// Beyond this number of slots changed in the frame, the ring is sent whole.
const FADE_FEW: usize = 32;

/// Ring slot of column `k`.
pub fn fade_slot(k: (i32, i32)) -> usize {
    (k.0.rem_euclid(FADE_RING as i32) as usize) * FADE_RING + k.1.rem_euclid(FADE_RING as i32) as usize
}

/// Writes of the fade ring to do for the frame.
pub enum FadeWrite<'a> {
    None,
    /// The whole ring (words of 4 bytes).
    All(&'a [u32]),
    /// These slots only (indices in `ring`).
    Slots {
        slots: &'a [u32],
        ring: &'a [u32],
    },
}

/// Fade-in of the columns: first mesh of each column (empty included, like the
/// game which also dates the empty sections), columns still fading, ring contents read by
/// the shader. Old path: visibilities rewritten on every frame as long as a column fades;
/// `FADEGPU`: birth instant written once, sentinel when the fade ends.
pub struct FadeRing {
    gpu: bool,
    epoch: std::time::Instant,
    born: hashbrown::HashMap<(i32, i32), std::time::Instant>,
    fading: Vec<(i32, i32)>,
    ring: Vec<u32>,
    /// Slots changed since the last frame, and those rendered in the last frame.
    dirty: Vec<u32>,
    sent: Vec<u32>,
}

impl FadeRing {
    pub fn new(gpu: bool, epoch: std::time::Instant) -> Self {
        let init = if gpu { FADE_DONE } else { 1.0f32.to_bits() };
        FadeRing {
            gpu,
            epoch,
            born: hashbrown::HashMap::new(),
            fading: Vec::new(),
            ring: vec![init; FADE_RING * FADE_RING],
            dirty: Vec::new(),
            sent: Vec::new(),
        }
    }

    /// Initial contents of the ring.
    pub fn ring(&self) -> &[u32] {
        &self.ring
    }

    /// Frame instant for the shader; constant 0 when it is not read (old path) or
    /// when no column fades (all the slots sent then carry the sentinel): the
    /// camera stays identical from one frame to the next and its upload can be skipped.
    pub fn clock(&self, now: std::time::Instant) -> u32 {
        if !self.gpu || self.fading.is_empty() {
            return 0;
        }
        fade_clock(self.epoch, now)
    }

    /// First mesh of `k` at instant `t` (a refresh does not restart the fade).
    pub fn birth(&mut self, k: (i32, i32), t: std::time::Instant) {
        if let hashbrown::hash_map::Entry::Vacant(e) = self.born.entry(k) {
            e.insert(t);
            self.fading.push(k);
            if self.gpu {
                let s = fade_slot(k);
                self.ring[s] = fade_clock(self.epoch, t);
                self.dirty.push(s as u32);
            }
        }
    }

    pub fn remove(&mut self, k: (i32, i32)) {
        self.born.remove(&k);
    }

    pub fn clear(&mut self) {
        self.born.clear();
        self.fading.clear();
    }

    /// Visibility (0 to 1) of `k` already meshed, `None` before.
    pub fn visibility(&self, k: (i32, i32), fade_secs: f64) -> Option<f32> {
        let t = self.born.get(&k)?;
        let e = t.elapsed().as_secs_f64();
        Some(if e >= fade_secs { 1.0 } else { (e / fade_secs) as f32 })
    }

    /// Frame at instant `now`: finished columns removed, ring updated; writes to do.
    pub fn frame(&mut self, now: std::time::Instant, fade_secs: f64) -> FadeWrite<'_> {
        let (born, ring, dirty, gpu) = (&self.born, &mut self.ring, &mut self.dirty, self.gpu);
        if self.fading.is_empty() && dirty.is_empty() {
            return FadeWrite::None;
        }
        self.fading.retain(|k| {
            let Some(t) = born.get(k) else { return false };
            let e = now.saturating_duration_since(*t).as_secs_f64();
            let done = e >= fade_secs;
            let s = fade_slot(*k);
            if !gpu {
                ring[s] = if done { 1.0f32 } else { (e / fade_secs) as f32 }.to_bits();
            } else if done {
                ring[s] = FADE_DONE;
                dirty.push(s as u32);
            }
            !done
        });
        if !gpu {
            return FadeWrite::All(&self.ring);
        }
        if self.dirty.is_empty() {
            return FadeWrite::None;
        }
        if self.dirty.len() > FADE_FEW {
            self.dirty.clear();
            return FadeWrite::All(&self.ring);
        }
        std::mem::swap(&mut self.dirty, &mut self.sent);
        self.dirty.clear();
        self.sent.sort_unstable();
        self.sent.dedup();
        FadeWrite::Slots { slots: &self.sent, ring: &self.ring }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variants() -> Vec<ShaderOpts> {
        let mut v = Vec::new();
        for bits in 0..64u32 {
            let b = |i: u32| bits & (1 << i) != 0;
            v.push(ShaderOpts {
                vsread: b(0),
                quadbits: b(1),
                varyings: b(2),
                fadegpu: b(3),
                faceidx: b(4),
                strip: b(5),
                probe: Probe::Off,
            });
        }
        for probe in [Probe::Vs, Probe::Fs] {
            v.push(ShaderOpts { probe, ..ShaderOpts::NEW });
        }
        v
    }

    fn validate(src: &str) -> (naga::Module, naga::valid::ModuleInfo) {
        let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{e:?}"));
        (module, info)
    }

    /// Access by non-constant index to an array held by value (local copy or constant) in
    /// the function: the compiler must then store it in local memory.
    fn value_array_indexing(module: &naga::Module, f: &naga::Function, info: &naga::valid::FunctionInfo) -> usize {
        let locals =
            f.local_variables.iter().filter(|(_, l)| matches!(module.types[l.ty].inner, naga::TypeInner::Array { .. }));
        let dynamic = f.expressions.iter().filter(|(_, e)| match e {
            naga::Expression::Access { base, .. } => {
                matches!(info[*base].ty.inner_with(&module.types), naga::TypeInner::Array { .. })
            }
            _ => false,
        });
        locals.count() + dynamic.count()
    }

    /// Each combination of the variants gives a valid WGSL (classic shader and improved
    /// transparency) with all the entry points used.
    #[test]
    fn every_variant_is_valid() {
        for o in variants() {
            let src = terrain(o);
            let (m, _) = validate(&src);
            for e in [
                "vs",
                "vs_idx",
                "vs_fluid",
                "vs_fluid_c",
                "vs_sorted",
                "vs_sorted_c",
                "fs_solid",
                "fs_cutout",
                "fs_translucent",
                "vs_sky",
            ] {
                assert!(m.entry_points.iter().any(|p| p.name == e), "{o:?} : {e} absent");
            }
            validate(&format!("{src}\n{}\n{}", crate::oit::WGSL, include_str!("terrain_oit.wgsl")));
        }
    }

    /// Targeted read and corners in nibbles: no more array held by value and indexed in the
    /// terrain vertices; the old path has some (the check sees what it looks for).
    #[test]
    fn new_vertex_shaders_index_no_value_array() {
        // Functions called (recursively) from a block.
        fn calls(block: &naga::Block, out: &mut Vec<naga::Handle<naga::Function>>) {
            for s in block.iter() {
                match s {
                    naga::Statement::Call { function, .. } => out.push(*function),
                    naga::Statement::Block(b) => calls(b, out),
                    naga::Statement::If { accept, reject, .. } => {
                        calls(accept, out);
                        calls(reject, out);
                    }
                    naga::Statement::Loop { body, continuing, .. } => {
                        calls(body, out);
                        calls(continuing, out);
                    }
                    naga::Statement::Switch { cases, .. } => cases.iter().for_each(|c| calls(&c.body, out)),
                    _ => {}
                }
            }
        }
        let count = |o: ShaderOpts| -> usize {
            let src = terrain(o);
            let (m, info) = validate(&src);
            let mut n = 0;
            let mut todo = Vec::new();
            for (i, ep) in m.entry_points.iter().enumerate() {
                if ep.stage == naga::ShaderStage::Vertex && ep.name != "vs_sky" {
                    n += value_array_indexing(&m, &ep.function, info.get_entry_point(i));
                    calls(&ep.function.body, &mut todo);
                }
            }
            // Functions reached from the terrain vertices (the fragments are off topic).
            let mut seen = std::collections::HashSet::new();
            while let Some(h) = todo.pop() {
                if seen.insert(h) {
                    n += value_array_indexing(&m, &m.functions[h], &info[h]);
                    calls(&m.functions[h].body, &mut todo);
                }
            }
            n
        };
        assert_eq!(count(ShaderOpts::NEW), 0, "array indexed by value in the new path");
        assert!(count(ShaderOpts { quadbits: false, ..ShaderOpts::NEW }) > 0, "indexed QUAD not seen");
        assert!(count(ShaderOpts { vsread: false, ..ShaderOpts::NEW }) > 0, "indexed quad copy not seen");
    }

    /// The nibbles give corners 0, 1, 2, 0, 2, 3; the strip 1, 2, 0, 3 and the index give the
    /// same oriented triangles, in the same order.
    #[test]
    fn corner_orders_match_the_quad() {
        const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];
        let bits = |c: u32, v: u32| (c >> (v * 4)) & 0xf;
        for v in 0..6 {
            assert_eq!(bits(0x0032_0210, v), QUAD[v as usize]);
        }
        // Rotation of a triangle to its canonical form (same orientation).
        let canon = |t: [u32; 3]| {
            let k = (0..3).min_by_key(|&k| t[k]).unwrap();
            [t[k], t[(k + 1) % 3], t[(k + 2) % 3]]
        };
        let list: Vec<[u32; 3]> = QUAD.chunks(3).map(|t| canon([t[0], t[1], t[2]])).collect();
        // Strip: triangle i = (i, i + 1, i + 2), vertices 1 and 2 swapped for odd i.
        let strip: Vec<u32> = (0..4).map(|v| bits(0x3021, v)).collect();
        let tris: Vec<[u32; 3]> = (0..2)
            .map(|i| {
                if i % 2 == 0 { [strip[i], strip[i + 1], strip[i + 2]] } else { [strip[i + 1], strip[i], strip[i + 2]] }
            })
            .map(canon)
            .collect();
        assert_eq!(tris, list, "strip");
        let index = crate::render::face_indices(3);
        for f in 0..3u16 {
            let t: Vec<u32> = index[f as usize * 6..f as usize * 6 + 6].iter().map(|&i| u32::from(i - 4 * f)).collect();
            assert!(index[f as usize * 6..f as usize * 6 + 6].iter().all(|&i| i >> 2 == f), "face {f}");
            assert_eq!(t, QUAD, "index of face {f}");
        }
    }

    /// Fade ring, old path against `FADEGPU`, on simulated frames: births,
    /// refreshes (no restart), removal mid-fade then return, reset, duration
    /// changed during a fade; for each column present, the visibility the shader reads
    /// is the same to within 2 µs of fade. Writes: no more whole ring per frame.
    #[test]
    fn gpu_fade_ring_matches_cpu_ring() {
        use std::time::{Duration, Instant};
        let epoch = Instant::now();
        let (mut old, mut new) = (FadeRing::new(false, epoch), FadeRing::new(true, epoch));
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 32) as u32
        };
        let mut live: Vec<(i32, i32)> = Vec::new();
        let (mut all_old, mut all_new, mut slots_new) = (0, 0, 0);
        let mut fade_secs = 0.75;
        // Frame 1 and following: each birth (up to 5 ms before the frame) follows the origin.
        for frame in 1..4000u32 {
            let now = epoch + Duration::from_micros(u64::from(frame) * 16_667 + u64::from(next() % 1000));
            // A few births per frame, including columns already present (refresh).
            for _ in 0..next() % 4 {
                let k = ((next() % 40) as i32 - 20, (next() % 40) as i32 - 20);
                let t = now - Duration::from_micros(u64::from(next() % 5000));
                old.birth(k, t);
                new.birth(k, t);
                if !live.contains(&k) {
                    live.push(k);
                }
            }
            if frame % 97 == 0 && !live.is_empty() {
                let k = live.swap_remove(next() as usize % live.len());
                old.remove(k);
                new.remove(k);
            }
            if frame == 2000 {
                old.clear();
                new.clear();
                live.clear();
            }
            if frame % 500 == 250 {
                fade_secs = [0.0, 0.05, 0.75, 2.0][(frame / 500 % 4) as usize];
            }
            match old.frame(now, fade_secs) {
                FadeWrite::All(_) => all_old += 1,
                FadeWrite::None => {}
                FadeWrite::Slots { .. } => panic!("old path: whole ring expected"),
            }
            match new.frame(now, fade_secs) {
                FadeWrite::All(_) => all_new += 1,
                FadeWrite::Slots { slots, .. } => slots_new += slots.len(),
                FadeWrite::None => {}
            }
            let clock = new.clock(now);
            let len = (fade_secs * 1e6) as f32;
            for &k in &live {
                let i = fade_slot(k);
                let a = f32::from_bits(old.ring()[i]);
                let b = fade_visibility(clock, new.ring()[i], len);
                let bound = if fade_secs > 0.0 { 2.0e-6 / fade_secs as f32 + 1.0e-6 } else { 0.0 };
                assert!((a - b).abs() <= bound, "frame {frame}, column {k:?}: {a} against {b}");
            }
        }
        eprintln!("whole ring sent: old {all_old} frames, FADEGPU {all_new}; slots only {slots_new}");
        assert!(all_new * 10 < all_old, "FADEGPU still sends the ring too often");
    }

    /// CPU micro-bench of the fade (flight scene: 3 births per frame, ≈ 135 columns fading for
    /// 0.75 s): ring update and copy of the bytes to upload, per frame, 1 thread (render
    /// thread work, nothing to parallelise):
    /// `cargo test --release -p rc-app --bin rustcraft fade_ring_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn fade_ring_cost() {
        use std::time::{Duration, Instant};
        let epoch = Instant::now();
        let mut best = [f64::MAX; 2];
        for _ in 0..5 {
            for (m, gpu) in [false, true].into_iter().enumerate() {
                let mut r = FadeRing::new(gpu, epoch);
                let mut staging = vec![0u32; FADE_RING * FADE_RING];
                let frames = 20_000u32;
                let t0 = Instant::now();
                for f in 0..frames {
                    let now = epoch + Duration::from_micros(u64::from(f) * 16_667);
                    for j in 0..3 {
                        let n = (f * 3 + j) as i32;
                        r.birth((n % 65, n / 65), now);
                    }
                    let sent = match r.frame(now, 0.75) {
                        FadeWrite::None => 0,
                        FadeWrite::All(ring) => {
                            staging.copy_from_slice(ring);
                            ring.len()
                        }
                        FadeWrite::Slots { slots, ring } => {
                            for &s in slots {
                                staging[s as usize] = ring[s as usize];
                            }
                            slots.len()
                        }
                    };
                    std::hint::black_box(sent);
                }
                best[m] = best[m].min(t0.elapsed().as_secs_f64() * 1e6 / f64::from(frames));
            }
        }
        eprintln!(
            "fade per frame: old {:.2} µs, FADEGPU {:.2} µs ({:+.0}%) (loaded machine, without the cost of the GPU upload)",
            best[0],
            best[1],
            (best[1] / best[0] - 1.0) * 100.0
        );
    }

    /// Fade clock in the camera: constant while nothing fades (or on the old
    /// path), so that two frames without change upload the same camera bytes;
    /// real as soon as a column is born, then constant again once its fade is finished.
    #[test]
    fn fade_clock_is_constant_when_nothing_fades() {
        use std::time::{Duration, Instant};
        let epoch = Instant::now();
        let at = |ms| epoch + Duration::from_millis(ms);
        for gpu in [false, true] {
            let mut r = FadeRing::new(gpu, epoch);
            assert_eq!((r.clock(at(10)), r.clock(at(20))), (0, 0), "gpu={gpu}");
            r.birth((3, 4), at(30));
            let _ = r.frame(at(40), 0.5);
            if gpu {
                assert_eq!((r.clock(at(40)), r.clock(at(50))), (40_000, 50_000));
            } else {
                assert_eq!((r.clock(at(40)), r.clock(at(50))), (0, 0));
            }
            let _ = r.frame(at(600), 0.5);
            assert_eq!((r.clock(at(700)), r.clock(at(800))), (0, 0), "gpu={gpu}");
        }
    }

    #[test]
    fn preprocessor_nests_and_negates() {
        let src = "a\n//#if VSREAD\nb\n//#if !QUADBITS\nc\n//#else\nd\n//#endif\n//#else\ne\n//#endif\nf\n";
        let o = |vsread, quadbits| ShaderOpts { vsread, quadbits, ..ShaderOpts::OLD };
        assert_eq!(source(src, o(true, false)), "a\nb\nc\nf\n");
        assert_eq!(source(src, o(true, true)), "a\nb\nd\nf\n");
        assert_eq!(source(src, o(false, true)), "a\ne\nf\n");
    }

    /// Shader formula (u32 then f32) against the old one (f64 from the instant, then f32) over
    /// 10^6 instants: durations 0, 0.05, 0.75 and 2 s, clock near the 2^32 µs wrap.
    #[test]
    fn gpu_fade_formula_matches_cpu() {
        // Fixed seed (xorshift): no result depends on the clock.
        let mut s = 0x0C30_5C30_5C30_5C30u64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 32) as u32
        };
        let mut worst = 0f32;
        for i in 0..1_000_000u32 {
            let secs = [0.0, 0.05, 0.75, 2.0][(i % 4) as usize];
            // Instants in nanoseconds like `Instant`; the shader receives truncated µs.
            let born_ns = if i % 3 == 0 {
                // Near the clock wrap: the frame instant passes through 0 again.
                (u64::from(u32::MAX) - 1_000_000) * 1000 + u64::from(next() % 1000)
            } else {
                u64::from(next() % (u32::MAX - 1)) * 1000 + u64::from(next() % 1000)
            };
            let now_ns = born_ns + u64::from(next() % 2_500_000) * 1000 + u64::from(next() % 1000);
            let old = {
                let e = (now_ns - born_ns) as f64 / 1e9;
                if e >= secs { 1.0 } else { (e / secs) as f32 }
            };
            let us = |ns: u64| ((ns / 1000) as u32).min(FADE_DONE - 1);
            let new = fade_visibility(us(now_ns), us(born_ns), (secs * 1e6) as f32);
            assert!(new.is_finite());
            let d = (old - new).abs();
            // Truncation of both instants to the µs: at most 1 µs of difference on the fade duration.
            let bound = if secs > 0.0 { 1.0e-6 / secs as f32 + 1.0e-6 } else { 0.0 };
            assert!(d <= bound, "difference {d} > {bound} (duration {secs} s)");
            worst = worst.max(d);
        }
        assert!(worst <= 1.0 / 255.0, "difference {worst}");
        eprintln!("maximum GPU fade difference: {worst:e}");
        assert_eq!(fade_visibility(5, FADE_DONE, 750_000.0), 1.0);
        assert_eq!(fade_visibility(5, 5, 0.0), 1.0, "zero duration: 1, not NaN");
    }
}
