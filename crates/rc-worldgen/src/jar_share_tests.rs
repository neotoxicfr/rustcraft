//! Equivalence of the shared jar index (`rc_data::jar`) and of the shared generation data
//! (`WorldData::shared`) with the old read of the jar by each reader.

use std::path::PathBuf;
use std::sync::Arc;

use crate::chunkgen::WorldData;
use crate::data::DataPack;
use crate::decor_data::DecorData;

fn inputs() -> Option<(PathBuf, PathBuf, rc_data::VersionData)> {
    let jar = rc_testdata::need(
        rc_data::locate("26.3").ok().map(|i| i.client_jar).filter(|p| p.is_file()),
        "jar client 26.3",
    )?;
    let v = rc_testdata::version_data()?;
    let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
    Some((jar, reports, v))
}

#[test]
fn generation_readers_identical_to_re_read() {
    let Some((jar, _, _)) = inputs() else { return };
    let old = DataPack::from_jar_direct(&jar).unwrap();
    let new = DataPack::from_index(&jar).unwrap();
    assert!(old.len() > 1000, "too few generation files: {}", old.len());
    assert!(old.entries() == new.entries(), "DataPack: index differs from the jar re-read");
    let old = DecorData::from_jar_direct(&jar).unwrap();
    let new = DecorData::from_index(&jar).unwrap();
    assert!(old.entries().len() > 1000);
    assert!(old.entries().keys().any(|k| k.starts_with("template/")), "no template");
    assert!(old.entries() == new.entries(), "DecorData: index differs from the jar re-read");
    assert_eq!(old.files_of("template"), new.files_of("template"));
}

#[test]
fn shared_world_data_identical_to_loading() {
    let Some((jar, reports, v)) = inputs() else { return };
    let reports = reports.is_dir().then_some(reports.as_path());
    let a = WorldData::shared(&jar, reports, &v).unwrap();
    let b = WorldData::shared(&jar, reports, &v).unwrap();
    // `RUSTCRAFT_OPT_WORLDDATA_SHARE=0`: old path, each call reloads.
    if rc_data::env_str!("RUSTCRAFT_OPT_WORLDDATA_SHARE") != Some("0") {
        assert!(Arc::ptr_eq(&a, &b), "data reloaded while a user holds it");
    } else {
        assert!(!Arc::ptr_eq(&a, &b), "data shared despite the switch being off");
        assert!(a.pack.entries() == b.pack.entries());
    }
    let fresh = WorldData::load(&jar, reports, &v).unwrap();
    assert!(a.pack.entries() == fresh.pack.entries());
    assert!(a.decor.entries() == fresh.decor.entries());
    assert_eq!(format!("{:?}", a.blocks), format!("{:?}", fresh.blocks));
    // No user left: the cache keeps nothing, the next call reloads.
    let w = Arc::downgrade(&a);
    drop((a, b));
    assert!(w.upgrade().is_none(), "data kept after the last user");
    let c = WorldData::shared(&jar, reports, &v).unwrap();
    assert!(c.pack.entries() == fresh.pack.entries());
}
