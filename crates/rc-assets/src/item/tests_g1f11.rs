//! Item tints read from the stack or the holder: firework (average of the colours of the
//! explosion), holder's team, custom_model_data.

use super::*;

struct S(Vec<u32>, Option<u32>);

impl StackView for S {
    fn has(&self, _: &str) -> bool {
        false
    }
    fn firework_colors(&self, f: &mut dyn FnMut(u32)) {
        self.0.iter().for_each(|&c| f(c));
    }
    fn custom_model_data_color(&self, _i: usize) -> Option<u32> {
        self.1
    }
}

struct E(Option<u32>);

impl EvalEnv for E {
    fn team_color(&self) -> Option<u32> {
        self.0
    }
}

fn tint(t: Tint, s: &S, e: &E) -> u32 {
    tint_color(&t, s, e)
}

#[test]
fn firework_star_colors() {
    let e = E(None);
    // No colour: the default as is (the game does not make it opaque).
    assert_eq!(tint(Tint::Firework(0x0012_3456), &S(vec![], None), &e), 0x0012_3456);
    // One colour: opaque.
    assert_eq!(tint(Tint::Firework(0), &S(vec![0x00b3_312c], None), &e), 0xffb3_312c);
    // Several: integer average truncated per channel, opaque.
    let two = S(vec![0x00ff_0001, 0x0000_ff00], None);
    assert_eq!(tint(Tint::Firework(0), &two, &e), 0xff7f_7f00);
    let three = S(vec![0x0010_2030, 0x0020_3040, 0x0031_4152], None);
    assert_eq!(tint(Tint::Firework(0), &three, &e), 0xff20_3040);
}

#[test]
fn team_tint_follows_holder() {
    let s = S(vec![], None);
    assert_eq!(tint(Tint::Team(0xff11_2233), &s, &E(Some(0x0055_55ff))), 0xff55_55ff);
    assert_eq!(tint(Tint::Team(0xff11_2233), &s, &E(None)), 0xff11_2233);
}

#[test]
fn custom_model_data_tint() {
    let e = E(None);
    let t = || Tint::CustomModelData { index: 0, default: 0xff00_00ff };
    assert_eq!(tint(t(), &S(vec![], Some(0x0012_3456)), &e), 0xff12_3456);
    assert_eq!(tint(t(), &S(vec![], None), &e), 0xff00_00ff);
}
