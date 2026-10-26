//! Writing reals like the game displays them (`1.0`, `0.5`, `1.0E7`, `1.0E-4`).

/// Double real as the game writes it.
pub fn jd(v: f64) -> String {
    java_fmt(v, format!("{v:e}"), format!("{v}"))
}

/// Single real as the game writes it.
pub fn jf(v: f32) -> String {
    java_fmt(f64::from(v), format!("{v:e}"), format!("{v}"))
}

fn java_fmt(v: f64, sci: String, plain: String) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let a = v.abs();
    if (1e-3..1e7).contains(&a) {
        if plain.contains('.') { plain } else { format!("{plain}.0") }
    } else {
        // `1.5e7` → `1.5E7`, `1e7` → `1.0E7`.
        let (m, e) = sci.split_once('e').unwrap_or((&sci, "0"));
        let m = if m.contains('.') { m.to_owned() } else { format!("{m}.0") };
        format!("{m}E{e}")
    }
}
