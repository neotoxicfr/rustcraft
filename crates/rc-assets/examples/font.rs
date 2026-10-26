//! Checks the fonts on the installed resources: advances of a few characters.

fn main() {
    let install = rc_data::locate("26.3").expect("installation");
    let index = install.asset_index();
    let assets = rc_assets::Assets::for_install(
        &install.client_jar,
        &[],
        index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())),
    )
    .expect("resources");
    let t = std::time::Instant::now();
    let font = rc_assets::font::Font::load(&assets, "minecraft:default", Default::default());
    println!("{} glyphs in {:.0} ms", font.len(), t.elapsed().as_secs_f64() * 1e3);
    for c in ['A', 'i', ' ', 'é', '中', '한', 'Ж'] {
        match font.glyph(c as u32) {
            Some(g) => println!(
                "{c:?}: advance {} offset {} quad {}..{} × {}..{}",
                g.advance, g.offset, g.left, g.right, g.top, g.bottom
            ),
            None => println!("{c:?} : missing"),
        }
    }
}
