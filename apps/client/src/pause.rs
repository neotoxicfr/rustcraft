//! Singleplayer pause (`integrated-server.md` § 9): decided once per frame from the top
//! screen; when paused, the client no longer runs the world (player, entities, particles,
//! weather, ambiences), freezes the render tick fraction and freezes the sounds other than music and interface.
//! The game has only one screen at a time; ours are sorted by kind, read here from the highest to the
//! lowest (draw order).

use crate::App;
use crate::special::Overlay;
use rc_audio::Category;

/// Top screen, reduced to what decides the pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Top {
    None,
    /// In-game menus (pause, options, credits…); social interactions do not pause.
    Menu { social: bool },
    /// Advancements, statistics, resource packs, F3 options, confirmation of a link or a
    /// command, resource loading (this overlay always pauses).
    Full,
    /// End screen: the poem one does not pause.
    Win { poem: bool },
    /// Server dialog according to its `pause` field (waiting for a reply: no).
    Dialog { pauses: bool },
    Death,
    /// Book read or written (the lectern one does not pause).
    Book { lectern: bool },
    Sign,
    /// Editors of command, structure, jigsaw and test blocks.
    Op,
    /// Containers, creative inventory included.
    Container,
    Chat,
}

/// Does the top screen pause the game? (by default yes, like the game; no for the chat,
/// death, containers, signs, the lectern, social interactions, the end poem).
pub(crate) fn pauses(top: Top) -> bool {
    match top {
        Top::None | Top::Death | Top::Sign | Top::Container | Top::Chat => false,
        Top::Menu { social } => !social,
        Top::Win { poem } => !poem,
        Top::Dialog { pauses } => pauses,
        Top::Book { lectern } => !lectern,
        Top::Full | Top::Op => true,
    }
}

/// Effective pause: singleplayer not open to the LAN, top screen that pauses.
pub(crate) fn paused(solo: bool, published: bool, top: Top) -> bool {
    solo && !published && pauses(top)
}

/// "In-game" screen (containers, signs, books, technical block editors): background as a
/// transparent veil, subtitles go between this veil and the panel.
fn in_game_ui(top: Top) -> bool {
    matches!(top, Top::Container | Top::Sign | Top::Book { .. } | Top::Op)
}

/// Place of the subtitles in the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subs {
    Off,
    /// With the HUD, under the open screen.
    Before,
    /// Right above the in-game screen's veil, under its panel (`Gui::draw_after_veil`).
    InScreen,
    /// Without a screen: after the notifications and F3 (under the loading overlay if it is there).
    Deferred,
}

/// Subtitles: under an in-game screen, between its veil and its panel (even with the HUD hidden); without a
/// screen, last; under the other screens, with the HUD; absent with the HUD hidden outside an in-game screen.
pub(crate) fn subtitles_at(hidden: bool, top: Top) -> Subs {
    if in_game_ui(top) {
        return Subs::InScreen;
    }
    match (hidden, top) {
        (true, _) => Subs::Off,
        (false, Top::None) => Subs::Deferred,
        (false, _) => Subs::Before,
    }
}

/// Chat drawn: closed with the HUD only; open it is a screen, drawn even with the HUD hidden.
pub(crate) fn chat_drawn(hidden: bool, open: bool) -> bool {
    !hidden || open
}

/// What is open, reduced to what decides the top screen.
#[derive(Default)]
pub(crate) struct Shown {
    /// Overlay of loading of resources (until the end of its exit fade).
    reload: bool,
    /// Confirmation of a link or a command (`links.rs`), drawn above everything.
    confirm: bool,
    /// F3 options, advancements, statistics, resource packs.
    full: bool,
    /// In-game menu open: social interactions?
    menu: Option<bool>,
    /// End screen: the poem one?
    win: Option<bool>,
    /// Server dialog: its `pause` field.
    dialog: Option<bool>,
    death: bool,
    /// Book, sign or technical block editor.
    overlay: Option<Top>,
    container: bool,
    chat: bool,
}

/// Top screen, from the highest to the lowest in draw order.
pub(crate) fn top_of(s: &Shown) -> Top {
    if s.reload || s.confirm || s.full {
        Top::Full
    } else if let Some(social) = s.menu {
        Top::Menu { social }
    } else if let Some(poem) = s.win {
        Top::Win { poem }
    } else if let Some(pauses) = s.dialog {
        Top::Dialog { pauses }
    } else if s.death {
        Top::Death
    } else if let Some(t) = s.overlay {
        t
    } else if s.container {
        Top::Container
    } else if s.chat {
        Top::Chat
    } else {
        Top::None
    }
}

impl App {
    /// Top screen (draw order, from the highest to the lowest).
    pub(crate) fn pause_top(&self) -> Top {
        top_of(&Shown {
            reload: self.packs.reload.is_some(),
            confirm: self.chat.confirm.is_some(),
            full: self.debug.options_open() || self.progress.screen.is_some() || self.packs.screen.is_some(),
            menu: (self.menus.is_open() && !self.menus.outside)
                .then(|| self.menus.top() == Some(crate::menus::Id::Social)),
            win: self.win.as_ref().map(crate::win_screen::WinScreen::portal_background),
            dialog: self.exact.dialog.as_ref().map(|d| d.pauses()),
            death: self.death.is_some(),
            overlay: match &self.special.overlay {
                Some(Overlay::Book(b)) => Some(Top::Book { lectern: b.is_lectern() }),
                Some(Overlay::Sign(_)) => Some(Top::Sign),
                Some(Overlay::Op(_)) => Some(Top::Op),
                None => None,
            },
            container: self.screens.open || self.creative_ui.open,
            chat: self.chat.is_open(),
        })
    }

    /// Once per frame, before the ticks: new pause state, applied to the integrated server,
    /// the render clock and, on entering pause, the sounds (music and interface continue).
    pub(crate) fn update_pause(&mut self) {
        let on = paused(self.solo.is_some(), self.menus.world.published.is_some(), self.pause_top());
        if self.pause.update(on, &mut self.tick_acc)
            && let Some(a) = self.audio.get()
        {
            a.pause_all_except(&[Category::Music, Category::Ui]);
        }
        if let Some(h) = &self.solo {
            h.set_paused(on);
        }
    }

    /// Render tick fraction (frozen during the pause).
    pub(crate) fn partial(&self) -> f64 {
        self.pause.acc(self.tick_acc) / self.exact.rate.tick_len()
    }

    /// Client tick while paused: the chat queue, the interface and screens, the save
    /// indicator, the music and the sound engine; nothing of the world (no rate clock, no
    /// player, entities, block entities, particles, weather, tutorial, end of tick sent).
    pub(crate) fn paused_tick(&mut self) {
        self.chat.tick_queue();
        self.commands_tick();
        self.front.loading_tick();
        crate::world_list::tick_autosave(self.solo.as_ref().is_some_and(rc_sim::Handle::saving));
        self.watch_audio_devices();
        if let Some(a) = self.audio.get() {
            let view = crate::environ::WorldView {
                grid: &self.world.grid,
                min_y: self.world.min_y,
                height: self.world.height,
            };
            let ctx = crate::world_sounds::TickCtx {
                view: &view,
                env: &mut self.env,
                player: self.positioned.then_some(&self.player),
                camera: self.eye,
                dimension: &self.dimension,
                boss_flags: self.hud_extra.boss_flags(),
                loading: !self.positioned,
                paused: true,
            };
            self.world_sounds.tick(a, ctx);
            a.tick_paused();
        }
        self.dialog_tick();
        self.special_tick();
        self.menus_tick();
        self.solo_pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Screens that pause singleplayer (read in game); never in multiplayer nor open to the LAN.
    #[test]
    fn pausing_screens_follow_the_game() {
        let yes = [
            Top::Menu { social: false },
            Top::Full,
            Top::Win { poem: false },
            Top::Dialog { pauses: true },
            Top::Book { lectern: false },
            Top::Op,
        ];
        let no = [
            Top::None,
            Top::Menu { social: true },
            Top::Win { poem: true },
            Top::Dialog { pauses: false },
            Top::Death,
            Top::Book { lectern: true },
            Top::Sign,
            Top::Container,
            Top::Chat,
        ];
        for t in yes {
            assert!(paused(true, false, t), "{t:?} must pause");
            assert!(!paused(false, false, t) && !paused(true, true, t), "{t:?}: multiplayer or LAN");
        }
        for t in no {
            assert!(!paused(true, false, t), "{t:?} must not pause");
        }
    }

    /// Top screen: the resource loading overlay and the confirmation of a link
    /// pause above everything (container, chat); then the draw order.
    #[test]
    fn reload_overlay_and_link_confirmation_pause_over_everything() {
        let under = || Shown { container: true, chat: true, ..Default::default() };
        assert_eq!(top_of(&under()), Top::Container);
        assert!(!paused(true, false, top_of(&under())));
        let reload = Shown { reload: true, ..under() };
        let confirm = Shown { confirm: true, chat: true, ..Default::default() };
        for s in [reload, confirm] {
            assert_eq!(top_of(&s), Top::Full);
            assert!(paused(true, false, top_of(&s)) && !paused(true, true, top_of(&s)));
        }
        let chat = Shown { chat: true, ..Default::default() };
        assert_eq!(top_of(&chat), Top::Chat);
        let s = Shown { menu: Some(true), death: true, overlay: Some(Top::Sign), ..under() };
        assert_eq!(top_of(&s), Top::Menu { social: true });
        let s = Shown { death: true, overlay: Some(Top::Sign), ..under() };
        assert_eq!(top_of(&s), Top::Death);
        let s = Shown { overlay: Some(Top::Book { lectern: true }), ..under() };
        assert_eq!(top_of(&s), Top::Book { lectern: true });
        assert_eq!(top_of(&Shown::default()), Top::None);
    }

    /// Interface order of the game: subtitles and chat according to F1 and the top screen.
    #[test]
    fn subtitles_and_chat_follow_the_hud_order() {
        assert_eq!(subtitles_at(false, Top::None), Subs::Deferred);
        for t in [Top::Container, Top::Book { lectern: true }, Top::Sign, Top::Op] {
            assert_eq!(subtitles_at(false, t), Subs::InScreen, "{t:?}: under the panel, on the veil");
            assert_eq!(subtitles_at(true, t), Subs::InScreen, "{t:?} with F1");
        }
        assert_eq!(subtitles_at(false, Top::Menu { social: false }), Subs::Before);
        assert_eq!(subtitles_at(false, Top::Chat), Subs::Before);
        assert_eq!(subtitles_at(true, Top::None), Subs::Off);
        assert_eq!(subtitles_at(true, Top::Full), Subs::Off);
        assert!(chat_drawn(false, false) && chat_drawn(true, true) && chat_drawn(false, true));
        assert!(!chat_drawn(true, false), "closed chat drawn with F1");
    }
}
