//! The game's tutorial (`tutorialStep` setting): steps movement, find a tree, punch a
//! tree, open the inventory, craft planks; each step shows its notification
//! after a delay and moves to the next when the player has done it. Outside survival: finished.

use rc_text::{Arg, Component};

use crate::keys;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Movement,
    FindTree,
    PunchTree,
    OpenInventory,
    CraftPlanks,
    None,
}

impl Step {
    fn from_name(n: &str) -> Step {
        match n {
            "movement" => Step::Movement,
            "find_tree" => Step::FindTree,
            "punch_tree" => Step::PunchTree,
            "open_inventory" => Step::OpenInventory,
            "craft_planks" => Step::CraftPlanks,
            _ => Step::None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Step::Movement => "movement",
            Step::FindTree => "find_tree",
            Step::PunchTree => "punch_tree",
            Step::OpenInventory => "open_inventory",
            Step::CraftPlanks => "craft_planks",
            Step::None => "none",
        }
    }
}

/// Current step: game counters, open notifications.
struct Inst {
    step: Step,
    waiting: i32,
    moved: bool,
    turned: bool,
    time_moved: i32,
    time_looked: i32,
    move_done: i32,
    look_done: i32,
    toast: Option<u32>,
    look_toast: Option<u32>,
    resets: i32,
}

#[derive(Default)]
pub struct Tutorial {
    inst: Option<Inst>,
    /// Social interactions notification displayed (removed when the screen opens).
    social_toast: Option<u32>,
    next_id: u32,
    /// Orientation at the previous tick (mouse rotation).
    last_look: Option<(f32, f32)>,
    /// Log mining at the previous tick: position.
    mining_log: Option<[i32; 3]>,
    inventory_open: bool,
}

/// Key of an assignment in bold (`key.forward` → "W").
fn key(o: &crate::options::Options, lang: &rc_text::Lang, name: &str) -> Arg {
    let bound = o.s(&format!("key_key.{name}"));
    let text = lang.map.get(bound).cloned().unwrap_or_else(|| bound.rsplit('.').next().unwrap_or(bound).to_uppercase());
    let mut c = Component::text(text);
    c.style.bold = Some(true);
    Arg::Component(c)
}

impl crate::App {
    fn tutorial_set(&mut self, step: Step) {
        self.tutorial_clear();
        let _ = self.options.set("tutorialStep", crate::options::Val::S(step.name().to_owned()));
        self.save_options();
        self.fx.tutorial.inst = Some(Inst::new(step));
    }

    fn tutorial_clear(&mut self) {
        if let Some(i) = &mut self.fx.tutorial.inst {
            for id in [i.toast.take(), i.look_toast.take()].into_iter().flatten() {
                self.toasts.tutorial_hide(id);
            }
        }
    }

    fn tutorial_toast(&mut self, icon: &'static str, title: Component, desc: Component, progress: bool) -> Option<u32> {
        let gui = &self.renderer.as_ref()?.gui;
        self.fx.tutorial.next_id += 1;
        let id = self.fx.tutorial.next_id;
        self.toasts.tutorial(gui, id, icon, &title, &desc, progress, 0);
        Some(id)
    }

    /// First multiplayer server (or world open to the LAN): social interactions
    /// notification (8 s, once and for all, `joinedFirstServer` saved),
    /// removed when the social interactions screen opens. World tick (not while paused).
    pub(crate) fn social_toast_tick(&mut self) {
        if self.menus.top() == Some(crate::menus::Id::Social)
            && let Some(id) = self.fx.tutorial.social_toast.take()
        {
            self.toasts.tutorial_hide(id);
        }
        let multiplayer = self.solo.is_none() || self.menus.world.published.is_some();
        if !self.positioned || !multiplayer || self.options.b("joinedFirstServer") {
            return;
        }
        let Some(gui) = self.renderer.as_ref().map(|r| &r.gui) else { return };
        let title = Component::translate("tutorial.socialInteractions.title", vec![]);
        let desc =
            Component::translate("tutorial.socialInteractions.description", vec![key(&self.options, &gui.lang, "socialInteractions")]);
        self.fx.tutorial.next_id += 1;
        let id = self.fx.tutorial.next_id;
        self.toasts.tutorial(gui, id, "toast/social_interactions", &title, &desc, true, 8000);
        self.fx.tutorial.social_toast = Some(id);
        let _ = self.options.set("joinedFirstServer", crate::options::Val::B(true));
        self.save_options();
    }

    fn has_item(&self, f: impl Fn(&str) -> bool) -> bool {
        self.inventory.menu.iter().flatten().any(|s| {
            let n = usize::try_from(s.item).ok().and_then(|i| self.item_names.get(i)).map_or("", String::as_str);
            f(n.strip_prefix("minecraft:").unwrap_or(n))
        })
    }

    fn block_tag(&self, tag: &str, state: u32) -> bool {
        let Some(info) = self.data.state(state) else { return false };
        let b = u32::from(info.block);
        self.act.tags.contains(tag, b) || self.act.tags.contains(tag.trim_start_matches("minecraft:"), b)
    }

    /// Tutorial tick (world loaded): input, look, mining, inventory, steps.
    pub(crate) fn tutorial_tick(&mut self) {
        if !self.positioned {
            self.tutorial_clear();
            self.fx.tutorial.inst = None;
            return;
        }
        if self.fx.tutorial.inst.is_none() {
            let step = Step::from_name(self.options.s("tutorialStep"));
            self.fx.tutorial.inst = Some(Inst::new(step));
        }
        // Input: movement keys, view rotation.
        let moved =
            [keys::FORWARD, keys::BACK, keys::LEFT, keys::RIGHT, keys::JUMP].iter().any(|&k| self.binds.down(k));
        let look = (self.yaw, self.pitch);
        let turned =
            self.fx.tutorial.last_look.is_some_and(|(y, p)| (y - look.0).abs() > 0.01 || (p - look.1).abs() > 0.01);
        self.fx.tutorial.last_look = Some(look);
        let survival = self.env.game_mode == 0;
        let Some(step) = self.fx.tutorial.inst.as_ref().map(|i| i.step) else { return };
        if let Some(i) = &mut self.fx.tutorial.inst {
            i.waiting += 1;
            i.moved |= moved && self.grabbed;
            i.turned |= turned && self.grabbed;
        }
        match step {
            Step::Movement => self.tutorial_movement(survival),
            Step::None => {}
            _ if !survival => self.tutorial_set(Step::None),
            Step::FindTree => self.tutorial_find_tree(),
            Step::PunchTree => self.tutorial_punch_tree(),
            Step::OpenInventory => self.tutorial_open_inventory(),
            Step::CraftPlanks => self.tutorial_craft_planks(),
        }
        self.fx.tutorial.inventory_open = self.screens.open && self.inventory.container.is_none();
    }

    fn tutorial_movement(&mut self, survival: bool) {
        let Some(i) = &mut self.fx.tutorial.inst else { return };
        if std::mem::take(&mut i.moved) {
            i.time_moved += 1;
        }
        if std::mem::take(&mut i.turned) {
            i.time_looked += 1;
        }
        let mut hide = Vec::new();
        if i.move_done == -1 && i.time_moved > 40 {
            hide.extend(i.toast.take());
            i.move_done = i.waiting;
        }
        if i.look_done == -1 && i.time_looked > 40 {
            hide.extend(i.look_toast.take());
            i.look_done = i.waiting;
        }
        let (moved, looked) = (i.time_moved as f32 / 40.0, i.time_looked as f32 / 40.0);
        let (toast, look_toast) = (i.toast, i.look_toast);
        let done = i.move_done != -1 && i.look_done != -1;
        let (waiting, move_done, look_done) = (i.waiting, i.move_done, i.look_done);
        for id in hide {
            self.toasts.tutorial_hide(id);
        }
        if done {
            self.tutorial_set(if survival { Step::FindTree } else { Step::None });
            return;
        }
        if let Some(id) = toast {
            self.toasts.tutorial_progress(id, moved);
        }
        if let Some(id) = look_toast {
            self.toasts.tutorial_progress(id, looked);
        }
        if waiting < 100 {
            return;
        }
        let lang = self.renderer.as_ref().map(|r| r.gui.lang.clone());
        let Some(lang) = lang else { return };
        if move_done == -1 && toast.is_none() {
            let k = |n| key(&self.options, &lang, n);
            let title =
                Component::translate("tutorial.move.title", vec![k("forward"), k("left"), k("back"), k("right")]);
            let desc = Component::translate("tutorial.move.description", vec![k("jump")]);
            let id = self.tutorial_toast("toast/movement_keys", title, desc, true);
            if let Some(i) = &mut self.fx.tutorial.inst {
                i.toast = id;
            }
        } else if move_done != -1 && waiting - move_done >= 20 && look_done == -1 && look_toast.is_none() {
            let title = Component::translate("tutorial.look.title", vec![]);
            let desc = Component::translate("tutorial.look.description", vec![]);
            let id = self.tutorial_toast("toast/mouse", title, desc, true);
            if let Some(i) = &mut self.fx.tutorial.inst {
                i.look_toast = id;
            }
        }
    }

    /// Items that complete "find a tree" (logs, leaves, wart blocks).
    fn tree_item(n: &str) -> bool {
        is_log(n) || n.ends_with("_leaves") || n == "nether_wart_block" || n == "warped_wart_block"
    }

    fn tutorial_find_tree(&mut self) {
        let waiting = self.fx.tutorial.inst.as_ref().map_or(0, |i| i.waiting);
        if (waiting == 1 && self.has_item(Self::tree_item)) || self.has_item(Self::tree_item) {
            self.tutorial_set(Step::CraftPlanks);
            return;
        }
        if let crate::pick::Hit::Block { pos, .. } = self.pick() {
            let state = self.fx_state_at(pos);
            if self.block_tag("minecraft:completes_find_tree_tutorial", state) {
                self.tutorial_set(Step::PunchTree);
                return;
            }
        }
        if waiting >= 6000 && self.fx.tutorial.inst.as_ref().is_some_and(|i| i.toast.is_none()) {
            let title = Component::translate("tutorial.find_tree.title", vec![]);
            let desc = Component::translate("tutorial.find_tree.description", vec![]);
            let id = self.tutorial_toast("toast/tree", title, desc, false);
            if let Some(i) = &mut self.fx.tutorial.inst {
                i.toast = id;
            }
        }
    }

    fn tutorial_punch_tree(&mut self) {
        let waiting = self.fx.tutorial.inst.as_ref().map_or(0, |i| i.waiting);
        if self.has_item(is_log) {
            self.tutorial_set(Step::CraftPlanks);
            return;
        }
        // Mining a log: progress; log gone under the cursor: next step.
        let mining = self.act.mining_state();
        let toast = self.fx.tutorial.inst.as_ref().and_then(|i| i.toast);
        if let Some(pos) = self.fx.tutorial.mining_log.take() {
            let state = self.fx_state_at(pos);
            if !self.block_tag("minecraft:logs", state) {
                self.tutorial_set(Step::OpenInventory);
                return;
            }
        }
        match mining {
            Some((pos, p)) if self.block_tag("minecraft:logs", self.fx_state_at(pos)) && p > 0.0 => {
                self.fx.tutorial.mining_log = Some(pos);
                if let Some(id) = toast {
                    self.toasts.tutorial_progress(id, p);
                }
            }
            Some((pos, _)) => {
                if let Some(id) = toast {
                    self.toasts.tutorial_progress(id, 0.0);
                } else if self.block_tag("minecraft:logs", self.fx_state_at(pos))
                    && let Some(i) = &mut self.fx.tutorial.inst
                {
                    i.resets += 1;
                }
            }
            None => {}
        }
        let resets = self.fx.tutorial.inst.as_ref().map_or(0, |i| i.resets);
        if (waiting >= 600 || resets > 3) && toast.is_none() {
            let lang = self.renderer.as_ref().map(|r| r.gui.lang.clone());
            let Some(lang) = lang else { return };
            let title = Component::translate("tutorial.punch_tree.title", vec![]);
            let desc =
                Component::translate("tutorial.punch_tree.description", vec![key(&self.options, &lang, "attack")]);
            let id = self.tutorial_toast("toast/tree", title, desc, true);
            if let Some(i) = &mut self.fx.tutorial.inst {
                i.toast = id;
            }
        }
    }

    fn tutorial_open_inventory(&mut self) {
        if !self.fx.tutorial.inventory_open && self.screens.open && self.inventory.container.is_none() {
            self.tutorial_set(Step::CraftPlanks);
            return;
        }
        let i = self.fx.tutorial.inst.as_ref();
        if i.is_some_and(|i| i.waiting >= 600 && i.toast.is_none()) {
            let lang = self.renderer.as_ref().map(|r| r.gui.lang.clone());
            let Some(lang) = lang else { return };
            let title = Component::translate("tutorial.open_inventory.title", vec![]);
            let desc = Component::translate(
                "tutorial.open_inventory.description",
                vec![key(&self.options, &lang, "inventory")],
            );
            let id = self.tutorial_toast("toast/recipe_book", title, desc, false);
            if let Some(i) = &mut self.fx.tutorial.inst {
                i.toast = id;
            }
        }
    }

    fn tutorial_craft_planks(&mut self) {
        if self.has_item(|n| n.ends_with("_planks")) {
            self.tutorial_set(Step::None);
            return;
        }
        let i = self.fx.tutorial.inst.as_ref();
        if i.is_some_and(|i| i.waiting >= 1200 && i.toast.is_none()) {
            let title = Component::translate("tutorial.craft_planks.title", vec![]);
            let desc = Component::translate("tutorial.craft_planks.description", vec![]);
            let id = self.tutorial_toast("toast/wooden_planks", title, desc, false);
            if let Some(i) = &mut self.fx.tutorial.inst {
                i.toast = id;
            }
        }
    }

    fn fx_state_at(&self, p: [i32; 3]) -> u32 {
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        view.state(p[0], p[1], p[2])
    }
}

/// Item tag `logs` (logs, wood, stems, hyphae, stripped ones included).
fn is_log(n: &str) -> bool {
    n.ends_with("_log") || n.ends_with("_wood") || n.ends_with("_stem") || n.ends_with("_hyphae")
}

impl Inst {
    fn new(step: Step) -> Inst {
        Inst {
            step,
            waiting: 0,
            moved: false,
            turned: false,
            time_moved: 0,
            time_looked: 0,
            move_done: -1,
            look_done: -1,
            toast: None,
            look_toast: None,
            resets: 0,
        }
    }
}
