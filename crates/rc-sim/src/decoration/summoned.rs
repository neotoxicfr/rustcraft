//! Armor stand and mannequin created by `/summon`: the compound the game saves for the entity it
//! builds from the given fields. It reads them with its typed reads (any number for a number,
//! out-of-range motion read as 0, angles wrapped), then writes every field it holds, in its order
//! and types; a field it does not read is lost, a living field it holds is written even if absent
//! (health, hurt time, brain…).

use rc_proto::nbt::Nbt;

use super::DKind;
use super::stand::{self, POSES};
use crate::nbtio;

/// Java's `(int)` floor of a double (saturating, NaN read as 0).
fn java_floor(d: f64) -> i32 {
    let i = d as i32;
    if d < f64::from(i) { i - 1 } else { i }
}

/// Integer value of a numeric tag (a float floored, a long cut to its low bits), as the game's
/// integer reads convert any number.
fn int_of(t: &Nbt) -> Option<i32> {
    Some(match t {
        Nbt::Byte(v) => i32::from(*v),
        Nbt::Short(v) => i32::from(*v),
        Nbt::Int(v) => *v,
        Nbt::Long(v) => *v as i32,
        Nbt::Float(v) => java_floor(f64::from(*v)),
        Nbt::Double(v) => java_floor(*v),
        _ => return None,
    })
}

fn num(t: &Nbt) -> Option<f64> {
    Some(match t {
        Nbt::Byte(v) => f64::from(*v),
        Nbt::Short(v) => f64::from(*v),
        Nbt::Int(v) => f64::from(*v),
        Nbt::Long(v) => *v as f64,
        Nbt::Float(v) => f64::from(*v),
        Nbt::Double(v) => *v,
        _ => return None,
    })
}

/// Fields read from the given compound with the game's conversions.
struct Given<'a>(&'a Nbt);

impl Given<'_> {
    fn get(&self, k: &str) -> Option<&Nbt> {
        self.0.get(k)
    }
    fn int(&self, k: &str, d: i32) -> i32 {
        self.get(k).and_then(int_of).unwrap_or(d)
    }
    fn short(&self, k: &str, d: i16) -> i16 {
        match self.get(k) {
            Some(Nbt::Byte(v)) => i16::from(*v),
            Some(Nbt::Short(v)) => *v,
            Some(t) => int_of(t).map_or(d, |v| v as i16),
            None => d,
        }
    }
    fn bool(&self, k: &str) -> bool {
        match self.get(k) {
            Some(Nbt::Byte(v)) => *v != 0,
            Some(t) => int_of(t).is_some_and(|v| v as i8 != 0),
            None => false,
        }
    }
    fn float(&self, k: &str) -> Option<f32> {
        self.get(k).and_then(num).map(|v| v as f32)
    }
    fn double(&self, k: &str) -> Option<f64> {
        self.get(k).and_then(num)
    }
    /// List of `N` numbers.
    fn nums<const N: usize>(&self, k: &str) -> Option<[f64; N]> {
        let l = nbtio::list(self.get(k));
        if l.len() != N {
            return None;
        }
        let v: Vec<f64> = l.iter().filter_map(num).collect();
        (v.len() == N).then(|| std::array::from_fn(|i| v[i]))
    }
    /// Compound or list field kept as given when present (and non-empty for `non_empty`).
    fn keep(&self, k: &str, non_empty: bool) -> Option<Nbt> {
        let v = self.get(k)?;
        let empty = match v {
            Nbt::List(l) => l.is_empty(),
            Nbt::Compound(c) => c.is_empty(),
            _ => false,
        };
        (!(non_empty && empty)).then(|| v.clone())
    }
}

fn dbl(v: [f64; 3]) -> Nbt {
    Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect())
}

/// Saved form of the given `attributes` for a type: the entries of attributes it has, re-encoded
/// (identifier, base, modifiers if any).
fn attributes(ty: &str, given: Option<&Nbt>) -> Vec<Nbt> {
    let mut out = Vec::new();
    for a in nbtio::list(given) {
        let Some(id) = a.get("id").and_then(Nbt::as_str) else { continue };
        let id = crate::commands::args::id(id);
        if crate::attrs::type_default(ty, &id).is_none() || out.iter().any(|o: &Nbt| o.get("id").and_then(Nbt::as_str) == Some(&id)) {
            continue;
        }
        let Some(base) = a.get("base").and_then(num) else { continue };
        let mut f = vec![("id".to_owned(), Nbt::String(id.clone())), ("base".to_owned(), Nbt::Double(base))];
        if let Some((_, mods)) = crate::attrs::saved_entry(std::slice::from_ref(a), a.get("id").and_then(Nbt::as_str).unwrap_or(""))
            && !mods.is_empty()
        {
            let op = |o: i32| ["add_value", "add_multiplied_base", "add_multiplied_total"][o.clamp(0, 2) as usize];
            let list = mods
                .into_iter()
                .map(|(m, v, o)| {
                    Nbt::Compound(vec![
                        ("id".to_owned(), Nbt::String(crate::commands::args::id(&m))),
                        ("amount".to_owned(), Nbt::Double(v)),
                        ("operation".to_owned(), Nbt::String(op(o).to_owned())),
                    ])
                })
                .collect();
            f.push(("modifiers".to_owned(), Nbt::List(list)));
        }
        out.push(Nbt::Compound(f));
    }
    out
}

/// Maximum health from the saved attributes (the equipment acts only from the first tick).
fn max_health(ty: &str, attrs: &[Nbt]) -> f32 {
    let name = "minecraft:max_health";
    let def = crate::attrs::type_default(ty, name).unwrap_or(20.0);
    let (base, mods) = crate::attrs::saved_entry(attrs, name).unwrap_or((None, Vec::new()));
    crate::attrs::sanitize(name, crate::effects::attribute_value(base.unwrap_or(def), &mods)) as f32
}

/// Parts of the player's skin a mannequin can hide, in the game's order.
const LAYERS: [&str; 7] = ["cape", "jacket", "left_sleeve", "right_sleeve", "left_pants_leg", "right_pants_leg", "hat"];

/// Saved compound of an armor stand or mannequin summoned at `pos` with the fields `given`.
pub(crate) fn summoned(k: DKind, pos: [f64; 3], uuid: u128, given: &Nbt) -> Nbt {
    let g = Given(given);
    let ty = k.name();
    // Entity: position of the command, motion beyond 10 read as 0, angles wrapped (pitch clamped).
    let motion = g.nums::<3>("Motion").unwrap_or_default().map(|v| if v.abs() > 10.0 { 0.0 } else { v });
    let [yaw, pitch] = g.nums::<2>("Rotation").map(|r| r.map(|v| v as f32)).unwrap_or_default();
    let yaw = if yaw.is_finite() { yaw % 360.0 } else { 0.0 };
    let pitch = if pitch.is_finite() { (pitch % 360.0).clamp(-90.0, 90.0) } else { 0.0 };
    let uuid = nbtio::uuid(g.get("UUID")).unwrap_or(uuid);
    let mut f: Vec<(String, Nbt)> = vec![
        ("id".into(), Nbt::String(ty.into())),
        ("Pos".into(), dbl(pos)),
        ("Motion".into(), dbl(motion)),
        ("Rotation".into(), Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(pitch)])),
        ("fall_distance".into(), Nbt::Double(g.double("fall_distance").unwrap_or(0.0))),
        ("Fire".into(), Nbt::Short(g.short("Fire", 0))),
        ("Air".into(), Nbt::Short(g.int("Air", 300) as i16)),
        ("OnGround".into(), Nbt::Byte(i8::from(g.bool("OnGround")))),
        ("Invulnerable".into(), Nbt::Byte(i8::from(g.bool("Invulnerable")))),
        ("PortalCooldown".into(), Nbt::Int(g.int("PortalCooldown", 0))),
    ];
    let invulnerable_time = g.int("invulnerable_time", 0);
    if invulnerable_time > 0 {
        f.push(("invulnerable_time".into(), Nbt::Int(invulnerable_time)));
    }
    f.push(("UUID".into(), nbtio::uuid_nbt(uuid)));
    if let Some(n) = g.get("CustomName").filter(|n| matches!(n, Nbt::String(_) | Nbt::Compound(_) | Nbt::List(_))) {
        f.push(("CustomName".into(), n.clone()));
    }
    for key in ["CustomNameVisible", "Silent", "NoGravity", "Glowing"] {
        if g.bool(key) {
            f.push((key.into(), Nbt::Byte(1)));
        }
    }
    let frozen = g.int("TicksFrozen", 0);
    if frozen > 0 {
        f.push(("TicksFrozen".into(), Nbt::Int(frozen)));
    }
    if g.bool("HasVisualFire") {
        f.push(("HasVisualFire".into(), Nbt::Byte(1)));
    }
    let tags: Vec<Nbt> = nbtio::list(g.get("Tags")).iter().filter(|t| matches!(t, Nbt::String(_))).cloned().collect();
    if !tags.is_empty() {
        f.push(("Tags".into(), Nbt::List(tags)));
    }
    if let Some(d) = g.keep("data", true).filter(|d| matches!(d, Nbt::Compound(_))) {
        f.push(("data".into(), d));
    }
    // Living entity: health within [0, maximum], the given fields, a brain without memories.
    let attrs = attributes(ty, g.get("attributes"));
    let max = max_health(ty, &attrs);
    let health = g.float("Health").unwrap_or(max);
    let health = if health.is_nan() { 0.0 } else { health.clamp(0.0, max) };
    f.extend([
        ("Health".into(), Nbt::Float(health)),
        ("HurtTime".into(), Nbt::Short(g.short("HurtTime", 0))),
        ("DeathTime".into(), Nbt::Short(g.short("DeathTime", 0))),
        ("AbsorptionAmount".into(), Nbt::Float(g.float("AbsorptionAmount").unwrap_or(0.0))),
        ("current_impulse_context_reset_grace_time".into(), Nbt::Int(g.int("current_impulse_context_reset_grace_time", 0))),
    ]);
    if let Some(p) = g.nums::<3>("current_explosion_impact_pos") {
        f.push(("current_explosion_impact_pos".into(), dbl(p)));
    }
    f.push(("attributes".into(), Nbt::List(attrs)));
    if let Some(e) = g.keep("active_effects", true).filter(|e| matches!(e, Nbt::List(_))) {
        f.push(("active_effects".into(), e));
    }
    f.push(("FallFlying".into(), Nbt::Byte(i8::from(g.bool("FallFlying")))));
    if let Some(p @ Nbt::IntArray(a)) = g.get("sleeping_pos")
        && a.len() == 3
    {
        f.push(("sleeping_pos".into(), p.clone()));
    }
    f.push(("Brain".into(), Nbt::Compound(vec![("memories".into(), Nbt::Compound(Vec::new()))])));
    for (who, time) in [("last_hurt_by_player", "last_hurt_by_player_memory_time"), ("last_hurt_by_mob", "ticks_since_last_hurt_by_mob")] {
        if let Some(u) = nbtio::uuid(g.get(who)) {
            f.push((who.into(), nbtio::uuid_nbt(u)));
            f.push((time.into(), Nbt::Int(g.int(time, 0))));
        }
    }
    if let Some(e) = g.keep("equipment", true).filter(|e| matches!(e, Nbt::Compound(_))) {
        f.push(("equipment".into(), e));
    }
    if let Some(i) = g.keep("locator_bar_icon", true).filter(|i| matches!(i, Nbt::Compound(_))) {
        f.push(("locator_bar_icon".into(), i));
    }
    match k {
        DKind::Stand => {
            f.extend([
                ("Invisible".into(), Nbt::Byte(i8::from(g.bool("Invisible")))),
                ("Small".into(), Nbt::Byte(i8::from(g.bool("Small")))),
                ("ShowArms".into(), Nbt::Byte(i8::from(g.bool("ShowArms")))),
                ("DisabledSlots".into(), Nbt::Int(g.int("DisabledSlots", 0))),
                ("NoBasePlate".into(), Nbt::Byte(i8::from(g.bool("NoBasePlate")))),
            ]);
            if g.bool("Marker") {
                f.push(("Marker".into(), Nbt::Byte(1)));
            }
            // Pose: the parts other than their default, three angles each.
            let pose = g.get("Pose");
            let mut parts = Vec::new();
            for (name, d) in POSES {
                let r = stand::rotations(pose.and_then(|p| p.get(name)), d);
                if r != d {
                    parts.push((name.to_owned(), Nbt::List(r.iter().map(|&a| Nbt::Float(a)).collect())));
                }
            }
            f.push(("Pose".into(), Nbt::Compound(parts)));
        }
        _ => {
            // Profile as the game re-encodes it (empty when absent or unreadable).
            let profile = match g.get("profile") {
                Some(Nbt::String(n)) => rc_text::canonical_profile(&Nbt::String(n.clone()))
                    .map(|_| Nbt::Compound(vec![("name".to_owned(), Nbt::String(n.clone()))])),
                Some(c @ Nbt::Compound(_)) => rc_text::canonical_profile(c),
                _ => None,
            };
            f.push(("profile".into(), profile.unwrap_or(Nbt::Compound(Vec::new()))));
            // Hidden parts: the given list if every name is valid, in the game's order.
            let names = nbtio::list(g.get("hidden_layers"));
            let valid = names.iter().all(|n| n.as_str().is_some_and(|s| LAYERS.contains(&s)));
            let hidden = LAYERS
                .iter()
                .filter(|l| valid && names.iter().any(|n| n.as_str() == Some(**l)))
                .map(|l| Nbt::String((*l).to_owned()))
                .collect();
            f.push(("hidden_layers".into(), Nbt::List(hidden)));
            let hand = match g.get("main_hand").and_then(Nbt::as_str) {
                Some("left") => "left",
                _ => "right",
            };
            f.push(("main_hand".into(), Nbt::String(hand.into())));
            let pose = match g.get("pose").and_then(Nbt::as_str) {
                Some(p @ ("crouching" | "swimming" | "fall_flying" | "sleeping")) => p,
                _ => "standing",
            };
            f.push(("pose".into(), Nbt::String(pose.into())));
            f.push(("immovable".into(), Nbt::Byte(i8::from(g.bool("immovable")))));
            if g.bool("hide_description") {
                f.push(("hide_description".into(), Nbt::Byte(1)));
            } else if let Some(d) = g.get("description").filter(|d| !is_default_description(d)) {
                f.push(("description".into(), d.clone()));
            }
        }
    }
    Nbt::Compound(f)
}

/// The mannequin's default description (`entity.minecraft.mannequin.label`), not written.
fn is_default_description(d: &Nbt) -> bool {
    match d {
        Nbt::Compound(c) => {
            c.len() == 1 && d.get("translate").and_then(Nbt::as_str) == Some("entity.minecraft.mannequin.label")
        }
        _ => false,
    }
}
