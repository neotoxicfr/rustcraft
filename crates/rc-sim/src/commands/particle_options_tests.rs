//! L10: options of the `trail`, `vibration` and geyser particles decoded like the game's codecs, and
//! the messages of every particle's failed fields joined last field first.

use rc_proto_v777::entity::{Particle, VibDest};

fn decode(text: &str) -> Result<Particle, String> {
    let brace = text.find('{').unwrap_or(text.len());
    let opts = super::super::snbt::parse(&text[brace..]).unwrap_or(rc_proto::nbt::Nbt::Compound(Vec::new()));
    super::decode_particle(0, &text[..brace], &opts, &|_| Some(1), &|_| Some(1))
}

fn err(text: &str) -> String {
    decode(text).expect_err(text)
}

/// `trail`: target (3 numbers, list or array), RGB color kept whole, positive duration; any missing
/// key refuses the command, the last field's message first.
#[test]
fn trail_options_like_the_game() {
    let p = decode("minecraft:trail{target:[0.5,70,0.5],color:16545810,duration:40}").expect("trail");
    assert_eq!((p.trail, p.color), (Some(([0.5, 70.0, 0.5], 40)), 16_545_810));
    // Color as floats (opaque), target as an int array, duration cut like the game's integers.
    let p = decode("trail{target:[I;1,2,3],color:[1.0,0.5,0.0],duration:7.9d}").expect("trail");
    assert_eq!((p.trail, p.color), (Some(([1.0, 2.0, 3.0], 7)), 0xFFFF_7F00));
    // Integer colors keep their 32 bits (the alpha byte goes to the guests as given).
    assert_eq!(decode("trail{target:[0,0,0],color:-1,duration:1}").expect("trail").color, u32::MAX);
    assert_eq!(
        err("minecraft:trail"),
        "No key duration in MapLike[{}]; No key color in MapLike[{}]; No key target in MapLike[{}]"
    );
    assert_eq!(
        err("trail{color:5,target:[1,2]}"),
        "No key duration in MapLike[{color:5,target:[1,2]}]; Input is not a list of 3 elements"
    );
    assert_eq!(err("trail{target:[0,0,0],color:1,duration:0}"), "Value must be positive: 0");
    assert_eq!(err("trail{target:\"x\",color:1,duration:2}"), "Not a list: \"x\"");
    assert_eq!(err("trail{target:[0,\"y\",0,1],color:1,duration:2}"), "Not a number");
    assert_eq!(err("trail{target:[0,\"y\"],color:1,duration:2}"), "Not a number; Input is not a list of 3 elements");
}

/// `vibration`: block destination and trip duration; an entity source is refused once decoded;
/// unknown source type, bad position.
#[test]
fn vibration_options_like_the_game() {
    let p = decode("vibration{destination:{type:\"block\",pos:[-12,64,900]},arrival_in_ticks:25}").expect("vib");
    assert_eq!(p.vib, Some((VibDest::Block([-12, 64, 900]), 25)));
    let p = decode("vibration{destination:{type:\"minecraft:block\",pos:[I;1,2,3]},arrival_in_ticks:-4}").expect("vib");
    assert_eq!(p.vib, Some((VibDest::Block([1, 2, 3]), -4)));
    assert_eq!(err("vibration"), "No key arrival_in_ticks in MapLike[{}]; No key destination in MapLike[{}]");
    assert_eq!(
        err("vibration{destination:{type:\"entity\",source_entity:[I;1,2,3,4]},arrival_in_ticks:1}"),
        "Entity position sources are not allowed"
    );
    assert_eq!(
        err("vibration{destination:{type:\"entity\"},arrival_in_ticks:1}"),
        "No key source_entity in MapLike[{type:\"entity\"}]"
    );
    assert_eq!(
        err("vibration{destination:{type:\"cave\",pos:[0,0,0]},arrival_in_ticks:1}"),
        "Unknown registry key in ResourceKey[minecraft:root / minecraft:position_source_type]: minecraft:cave"
    );
    assert_eq!(err("vibration{destination:{pos:[0,0,0]},arrival_in_ticks:1}"), "No key type in MapLike[{pos:[0,0,0]}]");
    assert_eq!(
        err("vibration{destination:{type:\"block\",pos:[0,0]},arrival_in_ticks:1}"),
        "Input is not a list of 3 ints"
    );
    assert_eq!(err("vibration{destination:5,arrival_in_ticks:1}"), "Not a map: 5");
}

/// Geyser particles: water blocks (positive) and burst impulse of the base, carried to the guests.
#[test]
fn geyser_options_like_the_game() {
    let p = decode("geyser{water_blocks:3}").expect("geyser");
    assert_eq!(p.state, 3);
    let p = decode("geyser_poof{water_blocks:2,burst_impulse_base:0.75f}").expect("poof");
    assert_eq!((p.state, p.scale), (2, 0.75));
    assert_eq!(err("geyser_plume"), "No key water_blocks in MapLike[{}]");
    assert_eq!(
        err("geyser_base{water_blocks:-1}"),
        "No key burst_impulse_base in MapLike[{water_blocks:-1}]; Value must be positive: -1"
    );
}

/// Every particle's failed fields joined last field first; Java float in the scale message; a
/// decimal `shriek` delay is cut like the game's integers.
#[test]
fn particle_messages_like_the_game() {
    assert_eq!(err("dust"), "No key scale in MapLike[{}]; No key color in MapLike[{}]");
    assert_eq!(err("dust{color:1,scale:5}"), "Value must be within range [0.01;4.0]: 5.0");
    assert_eq!(err("sculk_charge{roll:\"r\"}"), "Not a number");
    assert_eq!(decode("shriek{delay:2.7d}").expect("shriek").state, 2);
    assert_eq!(decode("effect").expect("effect").color, u32::MAX);
    assert!(decode("flame").is_ok());
}
