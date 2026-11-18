//! Replay of the official client's reference capture (outside the repository). Silently ignored if
//! the capture or the data cache is absent.

use rc_data::{Bound, Phase, flags};
use rc_proto::Reader;
use rc_proto::varint::read_varint;
use rc_proto_v777::chunk::{ChunkContext, read_chunk};
use rc_proto_v777::config::{DimensionHeight, read_login, read_registry_data};
use rc_testdata::{records, reference_capture, version_data as data};

#[test]
fn every_chunk_of_the_reference_capture_decodes_exactly() {
    let (Some(cap), Some(d)) = (reference_capture(), data()) else {
        eprintln!("capture or cache absent: test skipped");
        return;
    };
    let id = |phase, name| d.packet_id(phase, Bound::ToClient, name).unwrap();
    let registry_data = id(Phase::Configuration, "minecraft:registry_data");
    let login = id(Phase::Play, "minecraft:login");
    let chunk = id(Phase::Play, "minecraft:level_chunk_with_light");
    assert_eq!(d.fluids[0], "minecraft:empty");

    let mut dims: Vec<Option<DimensionHeight>> = Vec::new();
    let mut biomes = 0u32;
    let mut ctx = None;
    let (mut chunks, mut sections_checked, mut block_entities) = (0, 0, 0);
    let mut values = vec![0u32; 4096];
    for rec in records(&cap).iter().filter(|r| !r.to_server) {
        let mut r = Reader::new(rec.packet);
        let pid = read_varint(&mut r).unwrap();
        match (rec.phase, pid) {
            (3, x) if x == registry_data => {
                let reg = read_registry_data(&mut r).unwrap();
                r.finish().unwrap();
                match reg.registry.as_str() {
                    "minecraft:dimension_type" => {
                        dims = reg
                            .entries
                            .iter()
                            .map(|(id, n)| match n {
                                Some(n) => DimensionHeight::from_nbt(n),
                                // Content omitted (known pack): height read from the cache.
                                None => d.dimension_types.iter().find(|t| &t.name == id).map(|t| DimensionHeight {
                                    min_y: t.min_y,
                                    height: t.height,
                                    nether_light: t.nether_light,
                                }),
                            })
                            .collect();
                    }
                    "minecraft:worldgen/biome" => biomes = reg.entries.len() as u32,
                    _ => {}
                }
            }
            (4, x) if x == login => {
                let l = read_login(&mut r).unwrap();
                r.finish().unwrap();
                let h = dims[l.spawn.dimension_type as usize].expect("dimension height");
                ctx = Some(ChunkContext::new(h.sections(), d.states.len() as u32, biomes));
            }
            (4, x) if x == chunk => {
                let ctx = ctx.as_ref().expect("chunk before entering the game");
                let c = read_chunk(&mut r, ctx).unwrap_or_else(|e| panic!("chunk n°{chunks} : {e}"));
                r.finish().unwrap_or_else(|e| panic!("chunk ({}, {}) : {e}", c.x, c.z));
                assert_eq!(c.sections.len(), ctx.sections);
                assert_eq!(c.light.sky.len(), ctx.sections + 2);
                for (i, s) in c.sections.iter().enumerate() {
                    s.blocks.unpack_into(&mut values);
                    let non_air = values.iter().filter(|&&v| !d.states[v as usize].has(flags::AIR)).count();
                    assert_eq!(non_air, s.non_empty as usize, "chunk ({}, {}) section {i}: non-empty blocks", c.x, c.z);
                    sections_checked += 1;
                }
                block_entities += c.block_entities.len();
                chunks += 1;
            }
            _ => {}
        }
    }
    eprintln!(
        "{chunks} chunks, {sections_checked} sections checked, {block_entities} block entities, {biomes} biomes"
    );
    assert!(chunks > 100);
}

#[test]
fn abilities_attributes_and_tags_decode_exactly() {
    use rc_proto_v777::play::{read_abilities, read_update_attributes, read_update_tags};
    let (Some(cap), Some(d)) = (reference_capture(), data()) else { return };
    let (mut attrs, mut abilities, mut tags) = (0, 0, 0);
    let id = |p, n| d.packet_id(p, Bound::ToClient, n);
    let cfg_tags = id(Phase::Configuration, "minecraft:update_tags");
    let play_tags = id(Phase::Play, "minecraft:update_tags");
    let attr = id(Phase::Play, "minecraft:update_attributes");
    let abil = id(Phase::Play, "minecraft:player_abilities");
    for rec in records(&cap).iter().filter(|r| !r.to_server) {
        let mut r = Reader::new(rec.packet);
        let pid = Some(read_varint(&mut r).unwrap());
        let phase = if rec.phase == 3 { Phase::Configuration } else { Phase::Play };
        if (phase == Phase::Configuration && pid == cfg_tags) || (phase == Phase::Play && pid == play_tags) {
            let t = read_update_tags(&mut r).unwrap();
            assert!(t.iter().any(|(reg, tags)| reg == "minecraft:block" && tags.iter().any(|(n, _)| n == "minecraft:climbable")));
            tags += 1;
        } else if phase == Phase::Play && pid == attr {
            read_update_attributes(&mut r).unwrap();
            attrs += 1;
        } else if phase == Phase::Play && pid == abil {
            let a = read_abilities(&mut r).unwrap();
            assert!(a.flying_speed > 0.0);
            abilities += 1;
        } else {
            continue;
        }
        r.finish().unwrap();
    }
    eprintln!("{tags} tags, {attrs} attributes, {abilities} abilities");
    assert!(tags > 0 && attrs > 0 && abilities > 0);
}
