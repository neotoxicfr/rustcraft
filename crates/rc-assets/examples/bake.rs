//! Bakes the models of every state from the installed jar and prints a timed summary.

use std::time::Instant;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let t = Instant::now();
    let data = rc_data::load_or_prepare("26.3", |s| eprintln!("{s}")).expect("data");
    let install = rc_data::locate("26.3").expect("installation");
    let t1 = Instant::now();
    let assets = rc_assets::Assets::for_install(&install.client_jar, &[], None).expect("jar");
    let t2 = Instant::now();
    let m = rc_assets::build(&assets, &data);
    let t3 = Instant::now();
    let layers = |l| m.quads.iter().filter(|q| q.layer == l).count();
    println!(
        "jar {:.0} ms, baking {:.0} ms: {} baked faces ({} opaque, {} cutout, {} translucent), {} textures {}×{}",
        (t2 - t1).as_secs_f64() * 1e3,
        (t3 - t2).as_secs_f64() * 1e3,
        m.quads.len(),
        layers(rc_assets::Layer::Solid),
        layers(rc_assets::Layer::Cutout),
        layers(rc_assets::Layer::Translucent),
        m.textures.len(),
        m.texture_size,
        m.texture_size
    );
    let empty = m.states.iter().filter(|s| s.parts.is_empty()).count();
    println!(
        "states without model: {empty}; missing blockstate files: {} states; missing models: {} {:?}; missing textures: {} {:?}",
        m.missing.blockstates,
        m.missing.models.len(),
        m.missing.models.iter().take(5).collect::<Vec<_>>(),
        m.missing.textures.len(),
        m.missing.textures.iter().take(5).collect::<Vec<_>>()
    );
    println!("total {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
}
