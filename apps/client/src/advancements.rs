//! Client-side advancement tree (`docs/specs/advancements-stats.md` § 1.1–1.3): nodes in insertion
//! order, advancements by id, selected tab, notifications to emit.

use hashbrown::HashMap;

use rc_proto_v777::advancements::{Added, UpdateAdvancements};
use rc_proto_v777::item::ItemStack;
use rc_text::Component;

/// Frame of an advancement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Task,
    Challenge,
    Goal,
}

impl Frame {
    fn from_net(v: u8) -> Frame {
        match v {
            1 => Frame::Challenge,
            2 => Frame::Goal,
            _ => Frame::Task,
        }
    }

    /// Name of the sprites and keys (`task`, `goal`, `challenge`).
    pub fn name(self) -> &'static str {
        match self {
            Frame::Task => "task",
            Frame::Challenge => "challenge",
            Frame::Goal => "goal",
        }
    }

    /// Color of the notification label (§ 3.1).
    pub fn toast_color(self) -> u32 {
        if self == Frame::Challenge { 0xFFFF_88FF } else { 0xFFFF_FF00 }
    }

    /// Color applied to the description (§ 4.6): green, or dark purple for a challenge.
    pub fn description_color(self) -> u32 {
        if self == Frame::Challenge { 0xAA_00AA } else { 0x55_FF55 }
    }
}

/// Display of a node.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub title: Component,
    pub description: Component,
    pub icon: ItemStack,
    pub frame: Frame,
    /// Background texture: `<ns>:textures/<path>.png`.
    pub background: Option<String>,
    pub toast: bool,
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
    pub display: Option<Display>,
    pub requirements: Vec<Vec<String>>,
    pub x: f32,
    pub y: f32,
}

/// Progress of a node: requirements and criteria (name → date obtained).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    pub requirements: Vec<Vec<String>>,
    pub criteria: HashMap<String, Option<i64>>,
}

impl Progress {
    fn obtained(&self, c: &str) -> bool {
        self.criteria.get(c).is_some_and(Option::is_some)
    }

    /// Done: non-empty groups, each one satisfied (§ 1.1).
    pub fn done(&self) -> bool {
        !self.requirements.is_empty() && self.requirements.iter().all(|g| g.iter().any(|c| self.obtained(c)))
    }

    /// Groups with at least one criterion obtained.
    pub fn satisfied(&self) -> usize {
        self.requirements.iter().filter(|g| g.iter().any(|c| self.obtained(c))).count()
    }

    /// Percentage (0 if the criteria table is empty).
    pub fn percent(&self) -> f32 {
        if self.criteria.is_empty() || self.requirements.is_empty() {
            return 0.0;
        }
        self.satisfied() as f32 / self.requirements.len() as f32
    }

    /// "x/y" text (absent if there is a single group or no known criterion).
    pub fn progress_text(&self) -> Option<String> {
        (self.requirements.len() > 1 && !self.criteria.is_empty())
            .then(|| format!("{}/{}", self.satisfied(), self.requirements.len()))
    }
}

/// "Advancement made" notification to display.
pub struct Earned {
    pub title: Component,
    pub frame: Frame,
    pub icon: ItemStack,
}

#[derive(Default)]
pub struct Advancements {
    /// Nodes in insertion order.
    pub nodes: Vec<Node>,
    index: HashMap<String, usize>,
    pub progress: HashMap<String, Progress>,
    /// Selected tab (root), remembered outside the screen.
    pub selected_tab: Option<String>,
    /// Incremented on every update (the open screen rebuilds itself).
    pub generation: u64,
    /// Reset received since the screen last read it.
    pub reset_seen: bool,
}

fn component(n: Option<&rc_proto::nbt::Nbt>) -> Component {
    n.and_then(rc_text::from_nbt).unwrap_or_else(|| Component::text(""))
}

fn node_of(a: Added) -> Node {
    let display = a.display.map(|d| Display {
        title: component(d.title.as_ref()),
        description: component(d.description.as_ref()),
        icon: d.icon,
        frame: Frame::from_net(d.frame),
        background: d.background.map(|b| {
            let (ns, path) = b.split_once(':').unwrap_or(("minecraft", &b));
            format!("{ns}:textures/{path}.png")
        }),
        toast: d.toast,
        hidden: d.hidden,
    });
    Node { id: a.id, parent: a.parent, display, requirements: a.requirements, x: a.x, y: a.y }
}

impl Advancements {
    pub fn get(&self, id: &str) -> Option<&Node> {
        self.index.get(id).map(|&i| &self.nodes[i])
    }

    fn reindex(&mut self) {
        self.index = self.nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
    }

    /// `update_advancements` packet (§ 1.2, strict order); returns the notifications to emit.
    pub fn apply(&mut self, u: UpdateAdvancements) -> Vec<Earned> {
        if u.reset {
            self.nodes.clear();
            self.index.clear();
            self.progress.clear();
            self.reset_seen = true;
        }
        // Removals: the node and all its descendants.
        if !u.removed.is_empty() {
            let mut gone: hashbrown::HashSet<String> =
                u.removed.into_iter().filter(|id| self.index.contains_key(id)).collect();
            loop {
                let more: Vec<String> = self
                    .nodes
                    .iter()
                    .filter(|n| !gone.contains(&n.id) && n.parent.as_ref().is_some_and(|p| gone.contains(p)))
                    .map(|n| n.id.clone())
                    .collect();
                if more.is_empty() {
                    break;
                }
                gone.extend(more);
            }
            self.nodes.retain(|n| !gone.contains(&n.id));
            self.reindex();
        }
        // Additions: successive passes as long as a pass inserts something.
        let mut rest: Vec<Added> = u.added;
        while !rest.is_empty() {
            let before = rest.len();
            let mut left = Vec::new();
            for a in rest {
                let ok = a.parent.as_ref().is_none_or(|p| self.index.contains_key(p));
                if !ok {
                    left.push(a);
                    continue;
                }
                let n = node_of(a);
                match self.index.get(&n.id) {
                    Some(&i) => self.nodes[i] = n,
                    None => {
                        self.index.insert(n.id.clone(), self.nodes.len());
                        self.nodes.push(n);
                    }
                }
            }
            if left.len() == before {
                eprintln!("advancements: {} additions without a known parent, dropped", left.len());
                break;
            }
            rest = left;
        }
        // Advancements.
        let mut earned = Vec::new();
        for (id, criteria) in u.progress {
            let Some(node) = self.get(&id) else {
                eprintln!("advancements: '{id}' unknown to the tree, ignored");
                continue;
            };
            let names: hashbrown::HashSet<&String> = node.requirements.iter().flatten().collect();
            let mut map: HashMap<String, Option<i64>> =
                criteria.into_iter().filter(|(c, _)| names.contains(c)).collect();
            for c in &names {
                map.entry((*c).clone()).or_insert(None);
            }
            drop(names); // tree borrow released before the update

            let p = Progress { requirements: node.requirements.clone(), criteria: map };
            if !u.reset
                && p.done()
                && u.show
                && let Some(d) = node.display.as_ref().filter(|d| d.toast)
            {
                earned.push(Earned { title: d.title.clone(), frame: d.frame, icon: d.icon.clone() });
            }
            self.progress.insert(id, p);
        }
        self.generation += 1;
        earned
    }

    /// `select_advancements_tab`: unknown id → none (§ 1.3).
    pub fn select_tab(&mut self, id: Option<String>) {
        self.selected_tab = id.filter(|i| self.index.contains_key(i));
    }

    /// Cleared on disconnect.
    pub fn clear(&mut self) {
        *self = Advancements { generation: self.generation + 1, ..Default::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(id: &str, parent: Option<&str>, reqs: &[&[&str]]) -> Added {
        Added {
            id: id.into(),
            parent: parent.map(Into::into),
            display: Some(rc_proto_v777::advancements::Display {
                title: None,
                description: None,
                icon: ItemStack { item: 1, count: 1, added: Vec::new(), removed: Vec::new() },
                frame: 1,
                background: None,
                toast: true,
                hidden: false,
            }),
            requirements: reqs.iter().map(|g| g.iter().map(|s| s.to_string()).collect()).collect(),
            x: 0.0,
            y: 0.0,
        }
    }

    fn update(
        reset: bool,
        added: Vec<Added>,
        removed: &[&str],
        progress: Vec<(&str, &[(&str, bool)])>,
    ) -> UpdateAdvancements {
        UpdateAdvancements {
            reset,
            added,
            removed: removed.iter().map(|s| s.to_string()).collect(),
            progress: progress
                .into_iter()
                .map(|(id, c)| (id.to_string(), c.iter().map(|(n, d)| (n.to_string(), d.then_some(5))).collect()))
                .collect(),
            show: true,
        }
    }

    #[test]
    fn insertion_passes_keep_parent_first_and_order() {
        let mut a = Advancements::default();
        // Child before its parent: inserted on the second pass; orphan dropped.
        a.apply(update(
            true,
            vec![add("c", Some("r"), &[&["x"]]), add("r", None, &[&["x"]]), add("o", Some("zz"), &[])],
            &[],
            vec![],
        ));
        let ids: Vec<&str> = a.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["r", "c"]);
        // Removal of the root: descendants forgotten.
        a.apply(update(false, vec![], &["r"], vec![]));
        assert!(a.nodes.is_empty());
    }

    #[test]
    fn progress_and_toasts() {
        let mut a = Advancements::default();
        let e = a.apply(update(
            true,
            vec![add("r", None, &[&["a", "b"], &["c"]])],
            &[],
            vec![("r", &[("a", true), ("c", true), ("zzz", true)])],
        ));
        // Reset: no notification.
        assert!(e.is_empty());
        let p = &a.progress["r"];
        assert!(p.done());
        assert_eq!(p.criteria.len(), 3);
        assert!(!p.criteria.contains_key("zzz"));
        let e = a.apply(update(false, vec![], &[], vec![("r", &[("b", true), ("c", true)])]));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].frame, Frame::Challenge);
        let e = a.apply(update(false, vec![], &[], vec![("r", &[("b", true)])]));
        assert!(e.is_empty());
        let p = &a.progress["r"];
        assert_eq!((p.satisfied(), p.percent()), (1, 0.5));
        assert_eq!(p.progress_text().as_deref(), Some("1/2"));
    }
}
