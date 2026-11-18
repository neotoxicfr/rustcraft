//! A mannequin whose profile is reduced to a name takes the skin and cape its online lookup
//! brings, as the game does. 26.3 data and game, windowless GPU; without them, test skipped
//! (failure under `RUSTCRAFT_OPT_REQUIRE=1`).

use super::tests_g2f03::{Kit, kit};
use rc_proto_v777::entity::Meta;

/// First draw: the lookup is asked once (`name:Notch`), the default skin is drawn; once the answer
/// is in, the mannequin wears the downloaded skin and its cape.
#[test]
fn mannequin_reduced_to_a_name_wears_the_looked_up_skin() {
    let Some(mut k) = kit() else { return };
    // Partial profile: name "Notch", no id, no property, empty patch.
    let mut profile = vec![0, 1, 5];
    profile.extend_from_slice(b"Notch");
    profile.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mannequin = Kit::entity("mannequin", 3, |e| {
        e.meta.insert(17, Meta::Profile(profile.into()));
        // Every skin part shown, cape included.
        e.meta.insert(16, Meta::Byte(0x7f));
    });
    k.draw(&mannequin);
    let key = rc_text::head_hash("name:notch");
    let asked = |k: &Kit| k.r.pending_heads.iter().filter(|(key_asked, _)| *key_asked == key).count();
    assert!(k.r.pending_heads.contains(&(key, "name:Notch".to_owned())), "lookup asked");
    k.draw(&mannequin);
    assert_eq!(asked(&k), 1, "asked once");
    let skin = rc_assets::texture::Image::missing(64);
    let cape = rc_assets::texture::Image::missing(64);
    k.r.set_skin(key, Some((skin, true, "test/notch".into())), Some((cape, "test/notch_cape".into())), None);
    k.draw(&mannequin);
    let rect = k.r.atlas.rect("test/notch").expect("downloaded skin placed");
    let wears = k.r.instances.iter().flatten().any(|(_, _, _, instance)| instance.rect == rect);
    assert!(wears, "the mannequin wears the downloaded skin");
    assert_eq!(k.r.avatar_extras(&mannequin).0, k.r.atlas.rect("test/notch_cape"), "its cape");
}
