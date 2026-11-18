//! Window title, refreshed once a second in a world and outside: the system call that sets
//! it (a message to the window thread and a redraw of the caption) is skipped while the text
//! is the one already shown. `RUSTCRAFT_OPT_WINDOW_TITLE_CACHE=0` sets it every second, as before.

pub(crate) struct WindowTitle {
    /// Text last passed to the window (empty before the first one).
    shown: String,
    skip_repeats: bool,
}

impl WindowTitle {
    pub(crate) fn new() -> Self {
        Self::with_cache(rc_data::opt_switch!("RUSTCRAFT_OPT_WINDOW_TITLE_CACHE"))
    }

    fn with_cache(skip_repeats: bool) -> Self {
        WindowTitle { shown: String::new(), skip_repeats }
    }

    /// Passes `title` to the window unless it is the one shown (with the cache on).
    pub(crate) fn set(&mut self, window: &winit::window::Window, title: &str) {
        if self.needs_setting(title) {
            window.set_title(title);
        }
    }

    /// Whether `title` must be passed to the window; it then becomes the one shown.
    fn needs_setting(&mut self, title: &str) -> bool {
        if self.skip_repeats && self.shown == title {
            return false;
        }
        self.shown.clear();
        self.shown.push_str(title);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Titles set over one second each: game, menu, game, game in another mode.
    fn titles_set(skip_repeats: bool) -> Vec<&'static str> {
        let seconds = [
            "Rustcraft 26.3 - Singleplayer",
            "Rustcraft 26.3 - Singleplayer",
            "Rustcraft 26.3",
            "Rustcraft 26.3",
            "Rustcraft 26.3 - Singleplayer",
            "Rustcraft 26.3 - Multiplayer (LAN)",
            "Rustcraft 26.3 - Multiplayer (LAN)",
        ];
        let mut title = WindowTitle::with_cache(skip_repeats);
        seconds.into_iter().filter(|t| title.needs_setting(t)).collect()
    }

    /// Same titles in the same order; only the repeats of the one shown are dropped.
    #[test]
    fn only_repeated_titles_are_skipped() {
        let every_second = titles_set(false);
        assert_eq!(every_second.len(), 7, "without the cache, every second sets the title");
        let mut deduplicated = every_second.clone();
        deduplicated.dedup();
        assert_eq!(titles_set(true), deduplicated);
        assert_eq!(
            titles_set(true),
            ["Rustcraft 26.3 - Singleplayer", "Rustcraft 26.3", "Rustcraft 26.3 - Singleplayer", "Rustcraft 26.3 - Multiplayer (LAN)"]
        );
    }
}
