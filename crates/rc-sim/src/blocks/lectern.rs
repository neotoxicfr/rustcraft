//! Integrated-server lectern: book placed (tag `lectern_books`), reading screen
//! (menu `lectern`, one slot and one datum: the page), pages turned (2-tick pulse
//! downward, event 1043), book taken back, comparator output according to the page, book
//! dropped when the block disappears.
//!
//! Block entity data (the game's keys): `Book` (the stack) and `Page`.

use rc_proto::nbt::Nbt;

use super::use_block::ItemOn;
use super::{Dir, Kind, P, below};
use crate::menus::{Kind as MenuKind, Menu, Role, SlotDef, Src};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

/// Network type of the lectern menu (registry `menu`).
pub(crate) const MENU_TYPE: i32 = 17;

/// Number of pages of the book (written content, otherwise editable content).
fn page_count(book: &Stack) -> i32 {
    for key in ["minecraft:written_book_content", "minecraft:writable_book_content"] {
        if let Some(c) = crate::items::component(book, key) {
            return crate::nbtio::list(c.get("pages")).len() as i32;
        }
    }
    0
}

/// Readable book (one of the two contents present).
fn readable(book: &Stack) -> bool {
    crate::items::component(book, "minecraft:written_book_content").is_some()
        || crate::items::component(book, "minecraft:writable_book_content").is_some()
}

impl Server {
    fn lectern_book(&self, p: P) -> Option<Stack> {
        self.be_get(p).and_then(|b| b.get("Book")).and_then(Stack::from_nbt)
    }

    /// Book placed on an empty lectern; empty hand → nothing (no empty attempt).
    pub(crate) fn lectern_use_item(&mut self, p: P, s: u32, slot: usize) -> ItemOn {
        if self.v().get_bool(s, "has_book") {
            return ItemOn::TryEmptyHand;
        }
        let Some(held) = self.slots[slot].clone() else { return ItemOn::Pass };
        if !self.life.items.has_tag("minecraft:lectern_books", &held.id) {
            return ItemOn::TryEmptyHand;
        }
        if self.be_get(p).is_some() {
            let book = Stack { count: 1, ..held };
            self.consume_held(slot);
            if let Some(be) = self.be_get_mut(p) {
                crate::nbtio::set(be, "Book", book.to_nbt(None));
                crate::nbtio::set(be, "Page", Nbt::Int(0));
            }
            self.lectern_reset(p, s, true, Some(Ent::Player));
            self.block_sound(p, "minecraft:item.book.put", 1.0, 1.0);
        }
        ItemOn::Done
    }

    /// Reading screen with a book (statistic), otherwise action kept.
    pub(crate) fn lectern_use_without_item(&mut self, p: P, s: u32) -> bool {
        if !self.v().get_bool(s, "has_book") {
            return true;
        }
        if self.be_get(p).is_none() {
            return true;
        }
        let book = self.lectern_book(p);
        let page = self.be_int(p, "Page");
        let window = {
            self.life.counter = self.life.counter % 100 + 1;
            self.life.counter
        };
        let slots = vec![SlotDef { src: Src::Own(0), role: Role::TakeOnly }];
        let mut m = Menu::base(window, MenuKind::Container { menu_type: MENU_TYPE, size: 1 }, slots, 1, 1);
        m.own = vec![book];
        m.data[0] = page;
        m.block = Some(p);
        let custom = self.be_get(p).and_then(|b| b.get("CustomName")).and_then(rc_text::from_nbt);
        let title = custom.or_else(|| Some(rc_text::Component::translate("container.lectern", Vec::new())));
        self.open_menu(m, title);
        self.award_stat("minecraft:custom", "minecraft:interact_with_lectern", 1);
        true
    }

    /// Not powered, book present or not, event, neighbours below.
    fn lectern_reset(&mut self, p: P, s: u32, has_book: bool, by: Option<Ent>) {
        let v = self.v();
        let ns = v.set_bool(v.set_bool(s, "powered", false), "has_book", has_book);
        self.set_update(p, ns);
        self.game_event_block("minecraft:block_change", p, by, Some(ns));
        self.lectern_update_below(p, s);
    }

    /// "Neighbour changed" around the block below.
    fn lectern_update_below(&mut self, p: P, s: u32) {
        let b = self.v().block(s);
        self.update_neighbors_at(below(p), b);
    }

    /// Menu button: previous page (1), next (2), book taken (3,
    /// if the player can build), chosen page (≥ 100).
    pub(crate) fn lectern_button(&mut self, button: i32) {
        let Some(p) = self.life.menu.as_ref().and_then(|m| m.block) else { return };
        let cur = self.life.menu.as_ref().map_or(0, |m| m.data[0]);
        match button {
            1 => self.lectern_set_page(p, cur - 1),
            2 => self.lectern_set_page(p, cur + 1),
            3 => {
                if self.game_mode == 2 {
                    return;
                }
                let Some(book) = self.lectern_take_book(p) else { return };
                if let Some(m) = &mut self.life.menu {
                    m.own[0] = None;
                }
                let selected = self.held as usize;
                let creative = self.game_mode == 1;
                if let Some(rest) = self.pickup_into_inventory(book, creative, selected) {
                    self.drop_from_player(rest, false);
                }
            }
            b if b >= 100 => self.lectern_set_page(p, b - 100),
            _ => return,
        }
        self.broadcast_menus();
        self.after_menu_action();
    }

    /// Page clamped (at least 0, then at most pages − 1: −1 for a book without
    /// pages, as in the game); if it changes, pulse and menu datum sent back.
    fn lectern_set_page(&mut self, p: P, page: i32) {
        let n = self.lectern_book(p).map_or(0, |b| page_count(&b));
        let new = page.max(0).min(n - 1);
        if new == self.be_int(p, "Page") {
            return;
        }
        self.be_set_int(p, "Page", new);
        if let Some(m) = &mut self.life.menu {
            m.data[0] = new;
        }
        let s = self.get(p);
        let ns = self.v().set_bool(s, "powered", true);
        self.set_update(p, ns);
        self.lectern_update_below(p, s);
        let b = self.v().block(s);
        self.schedule_tick(p, b, 2);
        self.level_event(1043, p, 0);
    }

    /// Book removed from the block entity (removal without update, then removal of the book).
    pub(crate) fn lectern_take_book(&mut self, p: P) -> Option<Stack> {
        let book = self.lectern_book(p)?;
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::remove(be, "Book");
            crate::nbtio::remove(be, "Page");
        }
        let s = self.get(p);
        if self.v().kind(s) == Kind::Lectern {
            self.lectern_reset(p, s, false, None);
        }
        Some(book)
    }

    /// Scheduled tick: end of the page pulse.
    pub(crate) fn lectern_tick(&mut self, s: u32, p: P) {
        let ns = self.v().set_bool(s, "powered", false);
        self.set_update(p, ns);
        self.lectern_update_below(p, s);
    }

    /// Comparator output: `floor(page / (pages − 1) × 14) + 1` (1 if a single page).
    pub(crate) fn lectern_comparator(&self, p: P, s: u32) -> i32 {
        if !self.v().get_bool(s, "has_book") {
            return 0;
        }
        let Some(book) = self.lectern_book(p) else { return 0 };
        let n = page_count(&book);
        let page = self.be_int(p, "Page");
        let progress = if n > 1 { page as f32 / (n as f32 - 1.0) } else { 1.0 };
        (progress * 14.0).floor() as i32 + i32::from(readable(&book))
    }

    /// Is the menu opened on this lectern still valid?
    pub(crate) fn lectern_menu_valid(&self, p: P) -> bool {
        self.lectern_book(p).is_some_and(|b| readable(&b))
    }

    /// Block entity removed: book dropped in front of the lectern.
    pub(crate) fn lectern_pre_remove(&mut self, p: P, old: u32) {
        if !self.v().get_bool(old, "has_book") {
            return;
        }
        let Some(book) = self.lectern_book(p) else { return };
        let f: Dir = self.v().facing(old);
        let at = [
            f64::from(p[0]) + 0.5 + f64::from(0.25f32 * f.step_x() as f32),
            f64::from(p[1] + 1),
            f64::from(p[2]) + 0.5 + f64::from(0.25f32 * f.step_z() as f32),
        ];
        self.spawn_item_random(at, book, 10);
    }
}
