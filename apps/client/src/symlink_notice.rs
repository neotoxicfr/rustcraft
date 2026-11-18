//! Warning about refused symbolic links (world or pack), like the game: bold title,
//! message with the help address, buttons "Open in browser", "Copy link" and
//! "Back" (120 px, spaced by 5), Escape = Back.

use rc_text::Component;

use crate::server_list::Column;
use crate::widgets::{Event, tr};

/// Help page of the game about symbolic links.
pub(crate) const HELP: &str = "https://aka.ms/MinecraftSymLinks";
pub(crate) const OPEN: u32 = 41;
pub(crate) const COPY: u32 = 42;
pub(crate) const BACK: u32 = 43;

/// Column of the warning: world (`world`) or pack.
pub(crate) fn column(lang: &rc_text::Lang, world: bool) -> Column {
    let what = if world { "world" } else { "pack" };
    let mut title = Component::translate(format!("symlink_warning.title.{what}"), Vec::new());
    title.style.bold = Some(true);
    let message =
        Component::translate(format!("symlink_warning.message.{what}"), vec![rc_text::Arg::Text(HELP.to_owned())]);
    let buttons = vec![
        (OPEN, tr(lang, "chat.link.open", &[]), 120),
        (COPY, tr(lang, "gui.copy_link_to_clipboard", &[]), 120),
        (BACK, tr(lang, "gui.back", &[]), 120),
    ];
    Column::new(title, message, buttons, 10, 0).spaced(5)
}

/// Buttons pressed: link opened or copied (the screen stays); true for "Back" or Escape.
pub(crate) fn handle(events: &[Event], escape: bool) -> bool {
    for e in events {
        match e {
            Event::Press(OPEN) => crate::links::open_url(HELP),
            Event::Press(COPY) => crate::links::set_clipboard(HELP),
            _ => {}
        }
    }
    escape || events.contains(&Event::Press(BACK))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_escape_close() {
        assert!(handle(&[Event::Press(BACK)], false));
        assert!(handle(&[], true));
        assert!(!handle(&[Event::Press(999)], false));
    }
}
