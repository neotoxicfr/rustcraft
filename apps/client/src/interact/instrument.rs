//! Instruments (goat horns): synchronized registry `minecraft:instrument` (content sent, or
//! game file for the known pack) and `instrument` component of a stack (reference to the registry
//! or instrument described inline).

use rc_proto::Reader;
use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;

/// Sound played, use duration (seconds, also the cooldown) and range (volume = range / 16).
#[derive(Debug, Clone, PartialEq)]
pub struct Instrument {
    pub sound: String,
    pub seconds: f32,
    pub range: f32,
}

/// Default instrument of a goat horn without its own component.
pub const DEFAULT: &str = "minecraft:ponder_goat_horn";

fn float(n: &Nbt) -> Option<f32> {
    match *n {
        Nbt::Float(v) => Some(v),
        Nbt::Double(v) => Some(v as f32),
        _ => n.as_i64().map(|v| v as f32),
    }
}

fn from_nbt(n: &Nbt) -> Option<Instrument> {
    let sound = match n.get("sound_event")? {
        Nbt::String(s) => s.clone(),
        c => c.get("sound_id")?.as_str()?.to_owned(),
    };
    Some(Instrument { sound, seconds: float(n.get("use_duration")?)?, range: float(n.get("range")?)? })
}

fn from_json(j: &serde_json::Value) -> Option<Instrument> {
    let sound = match &j["sound_event"] {
        serde_json::Value::String(s) => s.clone(),
        c => c["sound_id"].as_str()?.to_owned(),
    };
    Some(Instrument { sound, seconds: j["use_duration"].as_f64()? as f32, range: j["range"].as_f64()? as f32 })
}

/// Table of the received registry, in network order: content sent, otherwise `data/<ns>/instrument/`
/// read by `read` (known pack); unreadable entry kept empty so as not to shift the indices.
pub fn table(entries: &[(String, Option<Nbt>)], read: impl Fn(&str) -> Option<Vec<u8>>) -> Vec<(String, Option<Instrument>)> {
    entries
        .iter()
        .map(|(name, nbt)| {
            let ins = match nbt {
                Some(n) => from_nbt(n),
                None => {
                    let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
                    read(&format!("data/{ns}/instrument/{path}.json"))
                        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                        .and_then(|j| from_json(&j))
                }
            };
            (name.clone(), ins)
        })
        .collect()
}

/// `instrument` component: 0 then inline instrument (sound: 0 then id and optional
/// range, otherwise rank + 1 in `sounds`; duration, range, wear, description), otherwise rank + 1
/// in `table`.
pub fn decode(b: &[u8], table: &[(String, Option<Instrument>)], sounds: &[String]) -> Option<Instrument> {
    let mut r = Reader::new(b);
    let holder = read_varint(&mut r).ok()?;
    if holder > 0 {
        return table.get(usize::try_from(holder - 1).ok()?)?.1.clone();
    }
    let s = read_varint(&mut r).ok()?;
    let sound = if s == 0 {
        let id = rc_proto::read_str(&mut r, 32767).ok()?.to_owned();
        if r.u8().ok()? == 1 {
            r.take(4).ok()?;
        }
        id
    } else {
        sounds.get(usize::try_from(s - 1).ok()?)?.clone()
    };
    let seconds = f32::from_be_bytes(r.take_array().ok()?);
    let range = f32::from_be_bytes(r.take_array().ok()?);
    Some(Instrument { sound, seconds, range })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ponder() -> Instrument {
        Instrument { sound: "minecraft:item.goat_horn.sound.0".into(), seconds: 7.0, range: 256.0 }
    }

    #[test]
    fn known_pack_entries_come_from_the_game_files() {
        let json = br#"{"description":{"translate":"x"},"range":256.0,"sound_event":"minecraft:item.goat_horn.sound.0","use_duration":7.0}"#;
        let t = table(&[("minecraft:ponder_goat_horn".into(), None)], |p| {
            (p == "data/minecraft/instrument/ponder_goat_horn.json").then(|| json.to_vec())
        });
        assert_eq!(t, vec![("minecraft:ponder_goat_horn".to_owned(), Some(ponder()))]);
        let sent = Nbt::Compound(vec![
            ("sound_event".into(), Nbt::Compound(vec![("sound_id".into(), Nbt::String("a:b".into()))])),
            ("use_duration".into(), Nbt::Float(2.5)),
            ("range".into(), Nbt::Float(32.0)),
        ]);
        let t = table(&[("x:y".into(), Some(sent))], |_| None);
        assert_eq!(t[0].1, Some(Instrument { sound: "a:b".into(), seconds: 2.5, range: 32.0 }));
    }

    #[test]
    fn component_reference_and_inline() {
        let t = vec![("minecraft:ponder_goat_horn".to_owned(), Some(ponder()))];
        assert_eq!(decode(&[1], &t, &[]), Some(ponder()));
        assert_eq!(decode(&[2], &t, &[]), None);
        // Inline: sound from the sound registry (rank 1), 3 s, range 64, wear 0, empty description.
        let mut b = vec![0, 2];
        b.extend_from_slice(&3.0f32.to_be_bytes());
        b.extend_from_slice(&64.0f32.to_be_bytes());
        b.push(0);
        let sounds = vec!["s:a".to_owned(), "s:b".to_owned()];
        assert_eq!(decode(&b, &t, &sounds), Some(Instrument { sound: "s:b".into(), seconds: 3.0, range: 64.0 }));
    }
}
