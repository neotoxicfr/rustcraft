//! Vorbis decoder of symphonia compared with lewton (the engine's) on the game sounds:
//! time per sample and difference of the 16-bit PCM (the engine requires the same PCM bit for bit).
//! `cargo run --release -p rc-audio --example vorbis_compare [number of sounds]`

use std::time::Instant;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

fn symphonia_pcm(bytes: Vec<u8>) -> Option<Vec<i16>> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .ok()?;
    let track = format.default_track(TrackType::Audio)?;
    let id = track.id;
    let opts = AudioDecoderOptions::default();
    let mut dec = symphonia::default::get_codecs().make_audio_decoder(track.codec_params.as_ref()?.audio()?, &opts).ok()?;
    let mut out = Vec::new();
    let mut buf = Vec::new();
    while let Ok(Some(pkt)) = format.next_packet() {
        if pkt.track_id != id {
            continue;
        }
        let Ok(d) = dec.decode(&pkt) else { break };
        buf.resize(d.samples_interleaved(), 0i16);
        d.copy_to_slice_interleaved(&mut buf);
        out.extend_from_slice(&buf);
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
