//! Vorbis decoder of symphonia compared with lewton (the engine's) on the game sounds:
//! time per sample and difference of the 16-bit PCM (the engine requires the same PCM bit for bit).
//! `cargo run --release -p rc-audio --example vorbis_compare [number of sounds]`

use std::time::Instant;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

fn symphonia_pcm(bytes: Vec<u8>) -> Option<Vec<i16>> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let id = track.id;
    let mut dec = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).ok()?;
    let mut out = Vec::new();
    let mut buf: Option<SampleBuffer<i16>> = None;
    while let Ok(pkt) = format.next_packet() {
        if pkt.track_id() != id {
            continue;
        }
        let Ok(d) = dec.decode(&pkt) else { break };
        let b = buf.get_or_insert_with(|| SampleBuffer::new(d.capacity() as u64, *d.spec()));
        if b.capacity() < d.capacity() * d.spec().channels.count() {
            *b = SampleBuffer::new(d.capacity() as u64, *d.spec());
        }
        b.copy_interleaved_ref(d);
        out.extend_from_slice(b.samples());
    }
    Some(out)
}

fn main() {
    let install = rc_data::locate("26.3").expect("installation 26.3");
    let index = install.asset_index();
    let assets = rc_assets::Assets::for_install(
        &install.client_jar,
        &[],
        index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())),
    )
    .expect("resources");
    let limit: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(500);
    let mut all = assets.list("assets/minecraft/sounds/", ".ogg");
    all.sort();
    let step = (all.len() / limit).max(1);
    let files: Vec<Vec<u8>> = all.iter().step_by(step).filter_map(|p| assets.read(p)).collect();
    let (mut tl, mut ts, mut n) = (0u128, 0u128, 0usize);
    let (mut same, mut len_diff, mut differing, mut worst) = (0usize, 0usize, 0usize, 0i32);
    for f in &files {
        let t = Instant::now();
        let a = rc_audio::decode::decode_all(f.clone()).expect("lewton").data;
        tl += t.elapsed().as_nanos();
        let t = Instant::now();
        let Some(b) = symphonia_pcm(f.clone()) else { continue };
        ts += t.elapsed().as_nanos();
        n += a.len();
        if a == b {
            same += 1;
            continue;
        }
        if a.len() != b.len() {
            len_diff += 1;
        }
        for (x, y) in a.iter().zip(&b) {
            let d = (i32::from(*x) - i32::from(*y)).abs();
            if d != 0 {
                differing += 1;
                worst = worst.max(d);
            }
        }
    }
    println!(
        "{} sounds, {n} samples: lewton {:.2} ns / sample, symphonia {:.2} ns / sample; PCM identical for {same} sounds, \
         different length for {len_diff}, {differing} different samples (max difference {worst})",
        files.len(),
        tl as f64 / n.max(1) as f64,
        ts as f64 / n.max(1) as f64,
    );
}
