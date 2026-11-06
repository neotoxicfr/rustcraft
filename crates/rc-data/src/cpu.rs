//! Processor features detected at run time, shared by every crate that carries a body compiled
//! for a newer instruction set.
//!
//! The release is built for plain x86-64 so that it runs on any 64-bit PC. A faster body
//! (`#[target_feature(enable = "avx2")]`, `"fma"`…) is only called after asking here, right
//! before the call: the answer is the safety guard of that call.
//!
//! `RUSTCRAFT_OPT_CPU_SPECIFIC_CODE=0` makes every answer `false`, so the generic bodies run
//! everywhere on the same executable (scene bench of the whole family against plain x86-64).
//! The switch can only hide a feature, never claim one the processor lacks.

use std::sync::LazyLock;

/// Instruction sets the code may use on this processor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    /// 256-bit integer vectors. A body compiled with `avx2` also gets AVX, SSE4.2 and SSE4.1
    /// (native `floor`, `round`, `trunc` instead of a library call).
    pub avx2: bool,
    /// Native rounding instructions (`floor`, `ceil`, `trunc`, `round`).
    pub sse41: bool,
    /// Fused multiply-add in one instruction (`mul_add` without a library call).
    pub fma: bool,
}

impl Features {
    /// What the processor reports (the standard library asks it once and keeps the answer).
    pub fn detected() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            Self {
                avx2: std::arch::is_x86_feature_detected!("avx2"),
                sse41: std::arch::is_x86_feature_detected!("sse4.1"),
                fma: std::arch::is_x86_feature_detected!("fma"),
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            Self::default()
        }
    }

    /// `self` when processor-specific code is allowed, no feature at all otherwise.
    pub fn allowed_if(self, cpu_specific_code: bool) -> Self {
        if cpu_specific_code { self } else { Self::default() }
    }
}

static USABLE: LazyLock<Features> =
    LazyLock::new(|| Features::detected().allowed_if(crate::opt_switch!("RUSTCRAFT_OPT_CPU_SPECIFIC_CODE")));

/// Features the code may use: detected once, hidden by `RUSTCRAFT_OPT_CPU_SPECIFIC_CODE=0`.
#[inline]
pub fn usable() -> Features {
    *USABLE
}

/// AVX2 may be used (detected on this processor and not hidden by the switch).
#[inline]
pub fn has_avx2() -> bool {
    USABLE.avx2
}

/// SSE4.1 may be used (detected on this processor and not hidden by the switch).
#[inline]
pub fn has_sse41() -> bool {
    USABLE.sse41
}

/// FMA may be used (detected on this processor and not hidden by the switch).
#[inline]
pub fn has_fma() -> bool {
    USABLE.fma
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answers are those of the processor (all of them, so the faster bodies do run under the
    /// tests), or none at all when the switch is at 0: never a feature it lacks.
    #[test]
    fn usable_features_are_the_detected_ones_or_none() {
        let detected = Features::detected();
        let switch = crate::env_flags::read_os("RUSTCRAFT_OPT_CPU_SPECIFIC_CODE");
        let expected = detected.allowed_if(crate::env_flags::switch_state(switch.as_deref(), true));
        assert_eq!(usable(), expected, "detected {detected:?}");
        assert_eq!((has_avx2(), has_sse41(), has_fma()), (expected.avx2, expected.sse41, expected.fma));
        eprintln!("detected {detected:?}, usable {expected:?}");
    }

    #[test]
    fn the_switch_hides_every_feature() {
        let all = Features { avx2: true, sse41: true, fma: true };
        assert_eq!(all.allowed_if(true), all);
        assert_eq!(all.allowed_if(false), Features::default());
    }

    /// Each field reads its own feature (a swapped name would show on a processor that has one
    /// of the two only; on a recent one every field is `true` and this only checks the plumbing).
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn each_field_reads_its_own_feature() {
        let d = Features::detected();
        assert_eq!(d.avx2, std::arch::is_x86_feature_detected!("avx2"));
        assert_eq!(d.sse41, std::arch::is_x86_feature_detected!("sse4.1"));
        assert_eq!(d.fma, std::arch::is_x86_feature_detected!("fma"));
        // Every processor with AVX2 has SSE4.1: the AVX2 bodies may round natively.
        assert!(!d.avx2 || d.sse41);
    }
}
