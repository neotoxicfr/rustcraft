//! Ogg Vorbis → interleaved 16-bit PCM decoding (spec § 2.4): whole file for ordinary sounds,
//! progressive stream in a dedicated thread for `stream` sounds.

use std::io::Cursor;
use std::sync::OnceLock;
use std::time::Duration;

use lewton::inside_ogg::OggStreamReader;

/// Sound decoded in full.
#[derive(Debug)]
pub struct Pcm {
    pub channels: u16,
    pub rate: u32,
    /// Interleaved samples.
    pub data: Vec<i16>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.data.len() / usize::from(self.channels.max(1))
    }

    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / self.rate.max(1) as f32
    }
}

/// Decoding switches, read once (`RUSTCRAFT_OPT_*`, `0` = old path).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecodeOpt {
    /// `DECODECOPY`: the `.ogg` read is decoded without copying, PCM reserved from the last
    /// granule of the file (one allocation instead of a series of doublings).
    pub copy: bool,
    /// `RINGCHUNK`: the stream thread writes a whole packet per ring slice and sleeps
    /// until the mixer has half emptied it (instead of one sample at a time and
    /// wake-ups every 20 ms).
    pub ring_chunk: bool,
}

impl DecodeOpt {
    pub(crate) fn get() -> DecodeOpt {
        static O: OnceLock<DecodeOpt> = OnceLock::new();
        *O.get_or_init(|| DecodeOpt {
            copy: crate::switch("DECODECOPY", true),
            ring_chunk: crate::switch("RINGCHUNK", true),
        })
    }
}

/// Bytes of an `.ogg` shared without copying between the loading and the stream thread (which
/// reopens the file at each loop).
#[derive(Clone)]
pub struct SharedBytes(pub std::sync::Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

fn open<B: AsRef<[u8]>>(bytes: B) -> Result<OggStreamReader<Cursor<B>>, String> {
    let r = OggStreamReader::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    match r.ident_hdr.audio_channels {
        1 | 2 => Ok(r),
        n => Err(format!("{n} unsupported channels")),
    }
}

/// Position (in frames) of the last Ogg page that ends a packet: the length of the sound.
fn last_granule(b: &[u8]) -> Option<u64> {
    let mut end = b.len();
    while end >= 4 {
        let i = b[..end].windows(4).rposition(|w| w == b"OggS")?;
        if b.len() >= i + 14 && b[i + 4] == 0 {
            let g = u64::from_le_bytes(b[i + 6..i + 14].try_into().ok()?);
            if g != u64::MAX {
                return Some(g);
            }
        }
        end = i + 3;
    }
    None
}

/// Reserve for the PCM of a file of `len` bytes announcing `granule` frames: bounded (a malformed
/// pack file must not reserve gigabytes), plus one maximal Vorbis half-window.
fn reserve_for(granule: u64, channels: usize, len: usize) -> usize {
    let want = usize::try_from(granule).unwrap_or(usize::MAX).saturating_add(4096).saturating_mul(channels);
    want.min(len.saturating_mul(256))
}

/// Decodes the whole file. With `DECODECOPY`, pass the `Vec` read (no copy); the old
/// path copies into an `Arc<[u8]>` and lets the PCM grow by doublings.
pub fn decode_all<B: AsRef<[u8]>>(bytes: B) -> Result<Pcm, String> {
    decode_all_with(bytes, DecodeOpt::get().copy)
}

pub(crate) fn decode_all_with<B: AsRef<[u8]>>(bytes: B, reserve: bool) -> Result<Pcm, String> {
    let cap = if reserve { last_granule(bytes.as_ref()).map(|g| (g, bytes.as_ref().len())) } else { None };
    let mut r = open(bytes)?;
    let channels = u16::from(r.ident_hdr.audio_channels);
    let rate = r.ident_hdr.audio_sample_rate;
    let mut data = match cap {
        Some((g, len)) => Vec::with_capacity(reserve_for(g, usize::from(channels), len)),
        None => Vec::new(),
    };
    while let Some(pkt) = r.read_dec_packet_itl().map_err(|e| e.to_string())? {
        data.extend_from_slice(&pkt);
    }
    data.truncate(data.len() / usize::from(channels) * usize::from(channels));
    if reserve {
        // Margin of the reserve returned (the cache keeps this PCM for the whole game).
        data.shrink_to_fit();
    }
    Ok(Pcm { channels, rate, data })
}

/// Read side of a stream: lock-free ring filled by the decoding thread. Producer
/// dropped and ring empty = end of stream.
pub struct StreamIn {
    pub channels: u16,
    pub rate: u32,
    pub ring: rtrb::Consumer<i16>,
    /// Decoding thread to wake when the ring is half empty (`RINGCHUNK`).
    waker: Option<std::thread::Thread>,
}

impl StreamIn {
    /// Stream whose test holds the producer itself.
    #[cfg(test)]
    pub(crate) fn manual(channels: u16, rate: u32, ring: rtrb::Consumer<i16>) -> Self {
        StreamIn { channels, rate, ring, waker: None }
    }

    /// Called by the mixer after each block: wakes the sleeping decoding thread as soon as
    /// half of the ring is free (no effect on the old path, which sleeps 20 ms).
    pub(crate) fn wake_if_drained(&self) {
        if let Some(t) = &self.waker
            && self.ring.slots() <= self.ring.buffer().capacity() / 2
        {
            t.unpark();
        }
    }
}

/// Opens the stream and starts its decoding thread. The thread stops when the consumer is
/// dropped (sound cut) or at the end of the file if `looping` is false (otherwise it reopens the file).
pub fn spawn_stream<B: AsRef<[u8]> + Clone + Send + 'static>(bytes: B, looping: bool) -> Result<StreamIn, String> {
    spawn_stream_with(bytes, looping, DecodeOpt::get().ring_chunk)
}

pub(crate) fn spawn_stream_with<B: AsRef<[u8]> + Clone + Send + 'static>(
    bytes: B,
    looping: bool,
    chunked: bool,
) -> Result<StreamIn, String> {
    let reader = open(bytes.clone())?;
    let channels = u16::from(reader.ident_hdr.audio_channels);
    let rate = reader.ident_hdr.audio_sample_rate;
    // About 2 s of PCM ahead (the spec describes 4 buffers of about 1 s).
    let (mut prod, ring) = rtrb::RingBuffer::new(rate as usize * usize::from(channels) * 2);
    let handle = std::thread::Builder::new()
        .name("rc-audio-stream".into())
        .spawn(move || {
            let mut reader = reader;
            loop {
                match reader.read_dec_packet_itl() {
                    Ok(Some(pkt)) if chunked => {
                        let mut rest = &pkt[..];
                        while !rest.is_empty() {
                            let k = prod.slots().min(rest.len());
                            if k == 0 {
                                if prod.is_abandoned() {
                                    return;
                                }
                                // Woken by the mixer (ring half empty); the delay is only
                                // used to notice a consumer dropped during the sleep.
                                std::thread::park_timeout(Duration::from_millis(250));
                                continue;
                            }
                            if let Ok(chunk) = prod.write_chunk_uninit(k) {
                                chunk.fill_from_iter(rest[..k].iter().copied());
                            }
                            rest = &rest[k..];
                        }
                    }
                    Ok(Some(pkt)) => {
                        for s in pkt {
                            let mut v = s;
                            loop {
                                match prod.push(v) {
                                    Ok(()) => break,
                                    Err(rtrb::PushError::Full(back)) => {
                                        if prod.is_abandoned() {
                                            return;
                                        }
                                        v = back;
                                        std::thread::sleep(Duration::from_millis(20));
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) if looping => match open(bytes.clone()) {
                        Ok(r) => reader = r,
                        Err(_) => return,
                    },
                    Ok(None) | Err(_) => return,
                }
                if prod.is_abandoned() {
                    return;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let waker = chunked.then(|| handle.thread().clone());
    Ok(StreamIn { channels, rate, ring, waker })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// About forty real `.ogg` files spread in alphabetical order, plus the smallest music
    /// file; `None` (or failure under `RUSTCRAFT_OPT_REQUIRE=1`) without the game installed.
    fn real_oggs() -> Option<Vec<(String, Vec<u8>)>> {
        let install = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game")?;
        let index = install.asset_index();
        let assets = rc_testdata::need(
            rc_assets::Assets::for_install(&install.client_jar, &[], index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())))
                .ok(),
            "26.3 game resources",
        )?;
        let mut all = assets.list("assets/minecraft/sounds/", ".ogg");
        all.sort();
        let all = rc_testdata::need((all.len() > 400).then_some(all), "26.3 game sounds")?;
        let mut pick: Vec<String> = all.iter().step_by(all.len() / 40).cloned().collect();
        let music = all.iter().filter(|p| p.contains("/music/")).min_by_key(|p| assets.read(p).map_or(usize::MAX, |b| b.len()));
        pick.extend(music.cloned());
        Some(pick.into_iter().map(|p| {
            let b = assets.read(&p).expect("listed sound unreadable");
            (p, b)
        }).collect())
    }

    fn fnv(h: &mut u64, pcm: &Pcm) {
        for b in [pcm.channels as u32, pcm.rate].into_iter().chain(pcm.data.iter().map(|&s| s as u16 as u32)) {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    /// Frozen fingerprint of the PCM decoded by the old path (copy, no reserve).
    const PCM_DIGEST: u64 = 0x8746_6ef6_c018_a4dc;

    /// `DECODECOPY`: same PCM bit for bit, sufficient reserve (no reallocation) and no
    /// excess of more than one Vorbis half-window, final capacity = length.
    #[test]
    fn decode_digest() {
        let Some(files) = real_oggs() else { return };
        let (mut old, mut new) = (0xcbf2_9ce4_8422_2325u64, 0xcbf2_9ce4_8422_2325u64);
        for (path, bytes) in &files {
            let a = decode_all_with(Arc::<[u8]>::from(bytes.clone()), false).unwrap();
            let b = decode_all_with(bytes.clone(), true).unwrap();
            fnv(&mut old, &a);
            fnv(&mut new, &b);
            let g = last_granule(bytes).expect("granule");
            let want = reserve_for(g, usize::from(b.channels), bytes.len());
            // Sufficient reserve (no growth during decoding), margin returned afterwards.
            assert!(b.data.len() <= want, "{path}: reserve {want} exceeded ({})", b.data.len());
            assert!(want - b.data.len() <= 4096 * 2 + 2, "{path}: reserve {want} for {}", b.data.len());
            assert_eq!(b.data.capacity(), b.data.len(), "{path}: margin not returned");
        }
        eprintln!("{} files, fingerprint {old:#018x}", files.len());
        assert_eq!(old, new, "PCM differs between the old and the new decoding");
        assert_eq!(old, PCM_DIGEST, "PCM fingerprint {old:#018x}, expected {PCM_DIGEST:#018x}");
    }

    /// The stream read from the ring (two loop passes) == `decode_all` twice, old and
    /// new write thread.
    #[test]
    fn stream_matches_decode_all() {
        let Some(files) = real_oggs() else { return };
        for (path, bytes) in files.iter().filter(|(p, _)| !p.contains("/music/")).take(6) {
            let pcm = decode_all_with(bytes.clone(), true).unwrap();
            let want: Vec<i16> = pcm.data.iter().chain(&pcm.data).copied().collect();
            for chunked in [false, true] {
                let mut s = spawn_stream_with(SharedBytes(Arc::new(bytes.clone())), true, chunked).unwrap();
                let mut got = Vec::with_capacity(want.len());
                let t = std::time::Instant::now();
                while got.len() < want.len() {
                    match s.ring.pop() {
                        Ok(v) => got.push(v),
                        Err(_) => {
                            s.wake_if_drained();
                            assert!(t.elapsed().as_secs() < 30, "{path}: stream stuck");
                            std::thread::yield_now();
                        }
                    }
                }
                assert!(got == want, "{path} (slices {chunked}): stream differs from decode_all");
            }
        }
    }
}
