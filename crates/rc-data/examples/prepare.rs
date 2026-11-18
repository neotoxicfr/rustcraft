//! Prepares (or reloads) the data of a version and prints a summary.
//! `cargo run -p rc-data --example prepare --release -- 26.3`

use std::time::Instant;

fn main() {
    let version = std::env::args().nth(1).unwrap_or_else(|| "26.3".into());
    let t = Instant::now();
    let d = match rc_data::load_or_prepare(&version, |step| eprintln!("[{:>6.2} s] {step}", t.elapsed().as_secs_f64()))
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "{} (protocol {}) in {:.1} ms: {} states ({} bits), {} blocks, {} shapes, {} properties, {} packets, {} registries",
        d.version,
        d.protocol,
        t.elapsed().as_secs_f64() * 1e3,
        d.states.len(),
        d.state_bits(),
        d.blocks.len(),
        d.shapes.len(),
        d.properties.len(),
        d.packets.len(),
        d.registries.len()
    );
    // Mining: hardness and required tool of a few blocks, tool of a pickaxe.
    for name in
        ["minecraft:stone", "minecraft:obsidian", "minecraft:bedrock", "minecraft:short_grass", "minecraft:dirt"]
    {
        if let Some(b) = d.blocks.iter().find(|b| b.name == name) {
            let s = b.default_state as usize;
            println!("{name}: hardness {:?}, required tool {:?}", d.hardness.get(s), d.needs_tool.get(s));
        }
    }
    println!(
        "creative tabs: {}",
        d.creative.tabs.iter().map(|t| format!("{} ({})", t.name, t.entries.len())).collect::<Vec<_>>().join(", ")
    );
    let items = d.registry("minecraft:item").map(|r| r.entries.clone()).unwrap_or_default();
    for name in ["minecraft:diamond_pickaxe", "minecraft:cooked_beef", "minecraft:diamond_sword"] {
        if let Some(i) = items.iter().position(|e| e == name).and_then(|i| d.items.get(i)) {
            println!("{name}: tool {:?}, consumable {:?}, food {:?}", i.tool, i.consumable, i.food);
        }
    }
}
