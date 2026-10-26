//! Entities created by the structures at placement (elder guardians, villagers, witches, drowned,
//! chest minecarts…): NBT compounds in the format of the entity chunks, read by singleplayer as by
//! the official server. Absent fields take their default value at load.

use rc_proto::nbt::Nbt;

/// Mob at the given position (look `yaw`), persistent or not.
pub fn mob(id: &str, pos: [f64; 3], yaw: f32, persistent: bool) -> Nbt {
    let mut f = base(id, pos, yaw);
    if persistent {
        f.push(("PersistenceRequired".into(), Nbt::Byte(1)));
    }
    Nbt::Compound(f)
}

/// Common fields of an entity.
pub fn base(id: &str, pos: [f64; 3], yaw: f32) -> Vec<(String, Nbt)> {
    vec![
        ("id".into(), Nbt::String(id.into())),
        ("Pos".into(), Nbt::List(pos.iter().map(|v| Nbt::Double(*v)).collect())),
        ("Motion".into(), Nbt::List(vec![Nbt::Double(0.0); 3])),
        ("Rotation".into(), Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(0.0)])),
    ]
}

/// Adds fields to a compound.
pub fn with(e: Nbt, extra: Vec<(String, Nbt)>) -> Nbt {
    match e {
        Nbt::Compound(mut f) => {
            f.extend(extra);
            Nbt::Compound(f)
        }
        o => o,
    }
}
