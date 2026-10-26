//! Player statistics (`docs/specs/advancements-stats.md` § 2): values received by
//! `award_stats`, custom statistics and display formats.

use hashbrown::HashMap;

/// Statistic types (network id).
pub const MINED: i32 = 0;
pub const CRAFTED: i32 = 1;
pub const USED: i32 = 2;
pub const BROKEN: i32 = 3;
pub const PICKED_UP: i32 = 4;
pub const DROPPED: i32 = 5;
pub const KILLED: i32 = 6;
pub const KILLED_BY: i32 = 7;
pub const CUSTOM: i32 = 8;

/// Value format (§ 2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Int,
    Time,
    Distance,
    Tenths,
}

use Format::{Distance as D, Int as E, Tenths as X, Time as T};

/// Custom statistics in the order of their network id (§ 2.3).
pub const CUSTOM_STATS: [(&str, Format); 78] = [
    ("leave_game", E),
    ("play_time", T),
    ("total_world_time", T),
    ("time_since_death", T),
    ("time_since_rest", T),
    ("sneak_time", T),
    ("walk_one_cm", D),
    ("crouch_one_cm", D),
    ("sprint_one_cm", D),
    ("walk_on_water_one_cm", D),
    ("fall_one_cm", D),
    ("climb_one_cm", D),
    ("fly_one_cm", D),
    ("walk_under_water_one_cm", D),
    ("minecart_one_cm", D),
    ("boat_one_cm", D),
    ("pig_one_cm", D),
    ("happy_ghast_one_cm", D),
    ("horse_one_cm", D),
    ("aviate_one_cm", D),
    ("swim_one_cm", D),
    ("strider_one_cm", D),
    ("nautilus_one_cm", D),
    ("jump", E),
    ("drop", E),
    ("damage_dealt", X),
    ("damage_dealt_absorbed", X),
    ("damage_dealt_resisted", X),
    ("damage_taken", X),
    ("damage_blocked_by_shield", X),
    ("damage_absorbed", X),
    ("damage_resisted", X),
    ("deaths", E),
    ("mob_kills", E),
    ("animals_bred", E),
    ("player_kills", E),
    ("fish_caught", E),
    ("talked_to_villager", E),
    ("traded_with_villager", E),
    ("eat_cake_slice", E),
    ("fill_cauldron", E),
    ("use_cauldron", E),
    ("clean_armor", E),
    ("clean_banner", E),
    ("clean_shulker_box", E),
    ("interact_with_brewingstand", E),
    ("interact_with_beacon", E),
    ("inspect_dropper", E),
    ("inspect_hopper", E),
    ("inspect_dispenser", E),
    ("play_noteblock", E),
    ("tune_noteblock", E),
    ("pot_flower", E),
    ("trigger_trapped_chest", E),
    ("open_enderchest", E),
    ("enchant_item", E),
    ("play_record", E),
    ("interact_with_furnace", E),
    ("interact_with_crafting_table", E),
    ("open_chest", E),
    ("sleep_in_bed", E),
    ("sleep_in_straw_bed", E),
    ("open_shulker_box", E),
    ("open_barrel", E),
    ("interact_with_blast_furnace", E),
    ("interact_with_smoker", E),
    ("interact_with_lectern", E),
    ("interact_with_campfire", E),
    ("interact_with_cartography_table", E),
    ("interact_with_loom", E),
    ("interact_with_stonecutter", E),
    ("bell_ring", E),
    ("raid_trigger", E),
    ("raid_win", E),
    ("interact_with_anvil", E),
    ("interact_with_grindstone", E),
    ("target_hit", E),
    ("interact_with_smithing_table", E),
];

/// Counters received: (type, registry value) → number (default 0).
#[derive(Default)]
pub struct Stats {
    values: HashMap<(i32, i32), i32>,
    /// Incremented on every `award_stats` (the open screen is notified).
    pub generation: u64,
}

impl Stats {
    pub fn get(&self, kind: i32, id: i32) -> i32 {
        self.values.get(&(kind, id)).copied().unwrap_or(0)
    }

    /// `award_stats`: each value replaces the previous one.
    pub fn award(&mut self, v: &[(i32, i32, i32)]) {
        for &(kind, id, n) in v {
            self.values.insert((kind, id), n);
        }
        self.generation += 1;
    }

    /// Ids with a non-zero value for a type.
    pub fn ids(&self, kind: i32) -> impl Iterator<Item = i32> + '_ {
        self.values.iter().filter(move |(k, v)| k.0 == kind && **v > 0).map(|(k, _)| k.1)
    }
}

/// "2 decimals" template: rounding to nearest, ties to even (§ 2.4).
fn two_decimals(x: f64) -> String {
    let s = x * 100.0;
    let r = if (s - s.trunc()).abs() == 0.5 { 2.0 * (s / 2.0).round() } else { s.round() };
    let r = r as i64;
    let (neg, a) = (r < 0, r.unsigned_abs());
    format!("{}{}.{:02}", if neg { "-" } else { "" }, a / 100, a % 100)
}

/// Integer with thousands separator ",".
pub fn thousands(v: i32) -> String {
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if v < 0 { format!("-{out}") } else { out }
}

/// Double written "Java style" (shortest form, at least one decimal).
fn java_double(x: f64) -> String {
    let s = format!("{x}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") }
}

pub fn format(f: Format, v: i32) -> String {
    let d = f64::from(v);
    match f {
        Format::Int => thousands(v),
        Format::Tenths => two_decimals(d * 0.1),
        Format::Distance => {
            let m = d / 100.0;
            let km = m / 1000.0;
            if km > 0.5 {
                format!("{} km", two_decimals(km))
            } else if m > 0.5 {
                format!("{} m", two_decimals(m))
            } else {
                format!("{v} cm")
            }
        }
        Format::Time => {
            let s = d / 20.0;
            let min = s / 60.0;
            let h = min / 60.0;
            let days = h / 24.0;
            let y = days / 365.0;
            if y > 0.5 {
                format!("{} y", two_decimals(y))
            } else if days > 0.5 {
                format!("{} d", two_decimals(days))
            } else if h > 0.5 {
                format!("{} h", two_decimals(h))
            } else if min > 0.5 {
                format!("{} min", two_decimals(min))
            } else {
                format!("{} s", java_double(s))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_formats() {
        assert_eq!(format(Format::Int, 1_234_567), "1,234,567");
        assert_eq!(format(Format::Int, 12), "12");
        assert_eq!(format(Format::Tenths, 125), "12.50");
        assert_eq!(format(Format::Distance, 40), "40 cm");
        assert_eq!(format(Format::Distance, 51), "0.51 m");
        assert_eq!(format(Format::Distance, 123_456), "1.23 km");
        assert_eq!(format(Format::Time, 0), "0.0 s");
        assert_eq!(format(Format::Time, 20), "1.0 s");
        assert_eq!(format(Format::Time, 247), "12.35 s");
        assert_eq!(format(Format::Time, 20 * 60), "1.00 min");
        assert_eq!(format(Format::Time, 20 * 3600 * 2), "2.00 h");
        // Ties to even.
        assert_eq!(two_decimals(0.125), "0.12");
        assert_eq!(two_decimals(0.375), "0.38");
    }
}
