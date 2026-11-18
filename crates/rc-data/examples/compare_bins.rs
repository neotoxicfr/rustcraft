//! Compares two `data.bin`: equal field by field, with or without the creative data (whose
//! painting order varies from one Java extraction to another).
//! `cargo run -p rc-data --example compare_bins --release -- a/data.bin b/data.bin`

fn load(p: &str) -> (rc_data::VersionData, u64) {
    let bytes = std::fs::read(p).unwrap();
    let tag = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    (rc_data::decode(&bytes, tag).unwrap(), tag)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let ((x, tx), (mut y, ty)) = (load(&a[0]), load(&a[1]));
    println!("extractor fingerprints equal: {}", tx == ty);
    println!("data equal: {}", x == y);
    let mut px: Vec<_> = x.creative.paintings.clone();
    let mut py: Vec<_> = y.creative.paintings.clone();
    px.sort();
    py.sort();
    println!("paintings equal once sorted: {}", px == py);
    let mut ex = x.creative.enchantments.clone();
    let mut ey = y.creative.enchantments.clone();
    ex.sort();
    ey.sort();
    println!("enchantments equal once sorted: {}", ex == ey);
    y.creative.paintings = x.creative.paintings.clone();
    y.creative.enchantments = x.creative.enchantments.clone();
    println!("data equal, paintings aligned: {}", x == y);
    println!("bytes equal, paintings aligned: {}", rc_data::encode(&x, tx) == rc_data::encode(&y, tx));
    macro_rules! fields {
        ($($f:ident),*) => {$(
            if x.$f != y.$f {
                println!("field differs: {}", stringify!($f));
            }
        )*};
    }
    fields!(
        version,
        protocol,
        aabbs,
        shapes,
        states,
        blocks,
        properties,
        state_values,
        fluids,
        packets,
        registries,
        dimension_types,
        client_states,
        biomes,
        timelines,
        items,
        potions,
        effect_colors,
        entity_dims,
        entity_models,
        entity_animations,
        creative,
        sound_types,
        hardness,
        needs_tool,
        sim,
        blast,
        instrument
    );
    if x.creative != y.creative {
        for (i, (a, b)) in x.creative.tabs.iter().zip(&y.creative.tabs).enumerate() {
            if a != b {
                println!("tab {i} {}: {} / {} entries", a.name, a.entries.len(), b.entries.len());
            }
        }
        println!("enchantments equal: {}", x.creative.enchantments == y.creative.enchantments);
    }
}
