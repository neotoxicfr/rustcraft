//! Texts kept between frames. A text translated, styled or measured for the interface stays
//! valid as long as the language, the fonts and the bound keys do ([`Gui::text_context`]): a
//! screen that draws the same title on every frame builds it once.

use crate::gui::{Gui, TextContext};

/// A value built from interface texts, kept with the context it was built in.
pub(crate) struct Kept<T>(Option<(TextContext, T)>);

impl<T> Default for Kept<T> {
    fn default() -> Self {
        Kept(None)
    }
}

impl<T> Kept<T> {
    /// The kept value. `build` makes it again when the language, the fonts or the bound keys
    /// changed since it was built, and on every call when `keep` is off (old path of a switch).
    pub(crate) fn get(&mut self, gui: &Gui, keep: bool, build: impl FnOnce(&Gui) -> T) -> &T {
        let context = gui.text_context();
        if !keep || self.0.as_ref().is_some_and(|(built_in, _)| *built_in != context) {
            self.0 = None;
        }
        &self.0.get_or_insert_with(|| (context, build(gui))).1
    }

    /// What the value was built from changed: it is built again at the next call.
    pub(crate) fn forget(&mut self) {
        self.0 = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests_opt;

    /// A kept value is built once, then again after a language change or new fonts; with `keep`
    /// off it is built on every call. Without an installed game or a GPU: skipped.
    #[test]
    fn kept_value_follows_the_text_context() {
        let Some(e) = tests_opt::env() else { return };
        let mut gui = tests_opt::gui(&e, tests_opt::ALL);
        let mut builds = 0;
        let mut kept: Kept<u32> = Kept::default();
        let mut get = |gui: &Gui, keep: bool, kept: &mut Kept<u32>| {
            *kept.get(gui, keep, |_| {
                builds += 1;
                builds
            })
        };
        assert_eq!((get(&gui, true, &mut kept), get(&gui, true, &mut kept)), (1, 1));
        gui.lang = std::sync::Arc::new(rc_text::Lang::load(b"{}", None, None));
        assert_eq!((get(&gui, true, &mut kept), get(&gui, true, &mut kept)), (2, 2), "language changed");
        let fonts = crate::resources::FONT_IDS
            .iter()
            .map(|id| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, Default::default())))
            .collect();
        gui.set_fonts(fonts);
        assert_eq!(get(&gui, true, &mut kept), 3, "fonts replaced");
        assert_eq!((get(&gui, false, &mut kept), get(&gui, false, &mut kept)), (4, 5), "keep off: built every time");
        kept.forget();
        assert_eq!((get(&gui, true, &mut kept), get(&gui, true, &mut kept)), (6, 6), "forgotten: built once more");
    }
}
