//! Equivalence of the shared jar index (`rc_data::jar`) on the server side: base pack files,
//! synced registries and tags, recipes and other data, English language, identical to
//! the old re-reading of the jar.

use rc_testdata::need;

use crate::registries::{Jar, client_jar, load_jar};

#[test]
fn shared_jar_matches_rereading() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(path) = need(client_jar(&data), "jar client 26.3") else { return };
    let old = Jar::read_direct(&path).unwrap();
    let new = Jar::from_index(&path).unwrap();
    assert!(old.files.len() > 5000, "too few files: {}", old.files.len());
    assert!(old.files == new.files, "Jar: index differs from re-reading the jar");
    let (a, b) = (load_jar(&old, &data), load_jar(&new, &data));
    assert!(a.synced == b.synced && a.tags == b.tags, "registries differ");
    assert!(!a.synced.is_empty() && !a.tags.is_empty());
    let (a, b) = (crate::datapack::DataPack::from_jar(&old), crate::datapack::DataPack::from_jar(&new));
    for dir in ["recipe", "loot_table", "worldgen/biome", "dimension_type", "tags/enchantment"] {
        assert!(a.all(dir) == b.all(dir), "data pack: folder {dir} differs");
        assert!(!a.all(dir).is_empty(), "data pack: folder {dir} empty");
    }
    // English language: the same entry as when opening the jar by name.
    let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let mut want = Vec::new();
    std::io::Read::read_to_end(&mut z.by_name(rc_data::jar::EN_US).unwrap(), &mut want).unwrap();
    let got = rc_data::jar::index(&path).unwrap().get(rc_data::jar::EN_US).unwrap().as_bytes().to_vec();
    assert!(got == want, "English language differs");
}

