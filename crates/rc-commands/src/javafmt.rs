//! Writing reals like the game displays them (`1.0`, `0.5`, `1.0E7`, `1.0E-4`).

/// Double real as the game writes it.
pub fn jd(v: f64) -> String {
    java_fmt(v, || format!("{v:e}"), || format!("{v}"))
}

/// Single real as the game writes it.
pub fn jf(v: f32) -> String {
    java_fmt(f64::from(v), || format!("{v:e}"), || format!("{v}"))
}

/// Only the form the range needs is written: the plain one in [1e-3, 1e7[, the scientific one
/// elsewhere (both were written for every number before).
fn java_fmt(v: f64, sci: impl FnOnce() -> String, plain: impl FnOnce() -> String) -> String {
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
        let plain = plain();
        if plain.contains('.') { plain } else { format!("{plain}.0") }
    } else {
        // `1.5e7` → `1.5E7`, `1e7` → `1.0E7`.
        let sci = sci();
        let (m, e) = sci.split_once('e').unwrap_or((&sci, "0"));
        let m = if m.contains('.') { m.to_owned() } else { format!("{m}.0") };
        format!("{m}E{e}")
    }
}

#[cfg(test)]
mod tests {
    use super::{jd, jf};

    /// The former writer, both forms written first: the reference of the equivalence test.
    fn former(v: f64, sci: String, plain: String) -> String {
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
            let (m, e) = sci.split_once('e').unwrap_or((&sci, "0"));
            let m = if m.contains('.') { m.to_owned() } else { format!("{m}.0") };
            format!("{m}E{e}")
        }
    }

    /// Doubles and floats around both range limits, special values and 200,000 bit patterns:
    /// the same text as the former writer.
    #[test]
    fn one_form_writes_the_former_text() {
        let mut doubles = vec![0.0, -0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.0, -1.5, 1e7, 1e-3, 0.5];
        for limit in [1e-3f64, 1e7] {
            let mut x = limit;
            for _ in 0..20 {
                doubles.extend([x, -x, x.next_up(), x.next_down()]);
                x = x.next_down();
            }
        }
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..200_000 {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            doubles.push(f64::from_bits(state));
        }
        for v in doubles {
            assert_eq!(jd(v), former(v, format!("{v:e}"), format!("{v}")), "{v:?}");
            let f = v as f32;
            assert_eq!(jf(f), former(f64::from(f), format!("{f:e}"), format!("{f}")), "{f:?}");
            let g = f32::from_bits((v.to_bits() >> 32) as u32);
            assert_eq!(jf(g), former(f64::from(g), format!("{g:e}"), format!("{g}")), "{g:?}");
        }
        assert_eq!(jd(1e7), "1.0E7");
        assert_eq!(jf(0.5), "0.5");
        assert_eq!(jd(1e-4), "1.0E-4");
    }
}
