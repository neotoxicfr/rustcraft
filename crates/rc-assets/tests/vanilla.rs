//! Checks on the installed jar (skipped if absent): particle of each state, default
//! font.

fn setup() -> Option<(rc_data::VersionData, rc_assets::Assets)> {
    let data = rc_testdata::version_data()?;
    let install = rc_data::locate("26.3").ok()?;
    let assets = rc_assets::Assets::for_install(&install.client_jar, &[], None).ok()?;
    Some((data, assets))
}

fn state(data: &rc_data::VersionData, block: &str, props: &[(&str, &str)]) -> u32 {
    let b = data.blocks.iter().find(|b| b.name == block).expect("block");
    (b.first_state..b.first_state + b.state_count)
        .find(|&s| props.iter().all(|p| data.state_properties(s).any(|kv| kv == *p)))
        .expect("state")
}

#[test]
fn vanilla_particles_come_from_each_state_model() {
    let Some((data, assets)) = setup() else {
        eprintln!("jar 26.3 missing: check skipped");
        return;
    };
    let m = rc_assets::build(&assets, &data);
    // Chest: particle-only model, oak planks like the game.
    assert_eq!(m.particle(state(&data, "minecraft:chest", &[])), Some("minecraft:block/oak_planks"));
    // Furnace: the particle follows the state (lit front).
    let lit = state(&data, "minecraft:furnace", &[("lit", "true")]);
    let off = state(&data, "minecraft:furnace", &[("lit", "false")]);
    assert_eq!(m.particle(lit), Some("minecraft:block/furnace_front_on"));
    assert_eq!(m.particle(off), Some("minecraft:block/furnace_front"));
    // No state of the base game without a resolved particle.
    let missing: Vec<&str> = (0..data.states.len() as u32)
        .filter(|&s| m.particle(s) == Some(rc_assets::model::MISSING_TEXTURE))
        .map(|s| data.blocks[data.states[s as usize].block as usize].name.as_str())
        .collect();
    assert!(missing.is_empty(), "{} states: {:?}", missing.len(), &missing[..missing.len().min(8)]);
}
