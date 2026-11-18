//! Player list of the server (`docs/specs/players.md`): profile, game mode, presence
//! in the list, latency, display name, order, hat.

use std::sync::atomic::{AtomicU64, Ordering};

use hashbrown::HashMap;

use rc_proto_v777::players::{InfoUpdate, Property};

/// Stamp of a content state, for the caches built from it: changes on every modification.
/// Stamps come from one counter for the whole process, so two lists (the one of a previous
/// connection and a new one, for example) never share a stamp and a cache cannot mistake one
/// for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision(u64);

impl Revision {
    fn fresh() -> Revision {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Revision(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// Marks the content as changed.
    pub fn bump(&mut self) {
        *self = Revision::fresh();
    }
}

impl Default for Revision {
    fn default() -> Revision {
        Revision::fresh()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PlayerEntry {
    pub name: String,
    /// Profile properties (appearance: `textures`).
    pub properties: Vec<Property>,
    pub game_mode: i32,
    pub listed: bool,
    pub latency: i32,
    pub display_name: Option<rc_proto::nbt::Nbt>,
    pub list_order: i32,
    pub show_hat: bool,
    pub chat_session: bool,
}

#[derive(Default)]
pub struct PlayerList {
    /// Written only through [`PlayerList::apply`] and [`PlayerList::remove`], which keep the
    /// revisions (the test skin of `skins.rs` only adds a profile property, read by no cache).
    pub map: HashMap<u128, PlayerEntry>,
    /// Changes on every addition, removal or update other than the latency alone.
    revision: Revision,
    /// Changes on every latency update (the server sends them periodically for everyone).
    latency_revision: Revision,
}

impl PlayerList {
    /// Update: an entry without an add for an unknown player is ignored (like the game).
    pub fn apply(&mut self, u: InfoUpdate) {
        let (mut changed, mut latency_changed) = (false, false);
        for e in u.entries {
            if let Some((name, properties)) = e.profile {
                self.map.insert(e.uuid, PlayerEntry { name, properties, show_hat: true, ..Default::default() });
                changed = true;
            }
            let Some(p) = self.map.get_mut(&e.uuid) else { continue };
            if let Some(v) = e.chat_session {
                p.chat_session = v;
                changed = true;
            }
            if let Some(v) = e.game_mode {
                p.game_mode = v;
                changed = true;
            }
            if let Some(v) = e.listed {
                p.listed = v;
                changed = true;
            }
            if let Some(v) = e.latency {
                p.latency = v;
                latency_changed = true;
            }
            if let Some(v) = e.display_name {
                p.display_name = v;
                changed = true;
            }
            if let Some(v) = e.list_order {
                p.list_order = v;
                changed = true;
            }
            if let Some(v) = e.show_hat {
                p.show_hat = v;
                changed = true;
            }
        }
        if changed {
            self.revision.bump();
        }
        if latency_changed {
            self.latency_revision.bump();
        }
    }

    pub fn remove(&mut self, uuids: &[u128]) {
        let before = self.map.len();
        for u in uuids {
            self.map.remove(u);
        }
        if self.map.len() != before {
            self.revision.bump();
        }
    }

    pub fn name(&self, uuid: u128) -> Option<&str> {
        self.map.get(&uuid).map(|p| p.name.as_str())
    }

    /// Revision of everything but the latencies.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Revision of the latencies.
    pub fn latency_revision(&self) -> Revision {
        self.latency_revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_proto_v777::players::InfoEntry;

    fn update(entries: Vec<InfoEntry>) -> InfoUpdate {
        InfoUpdate { actions: 0xff, entries }
    }

    /// Every write moves its revision, the latency alone moves only its own, an ignored entry
    /// (unknown player) or a removal of nobody moves nothing; two fresh lists never share a stamp.
    #[test]
    fn revisions_follow_the_writes() {
        let mut list = PlayerList::default();
        assert_ne!(list.revision(), PlayerList::default().revision());
        let (r0, l0) = (list.revision(), list.latency_revision());
        list.apply(update(vec![InfoEntry { uuid: 1, profile: Some(("a".into(), Vec::new())), ..Default::default() }]));
        let (r1, l1) = (list.revision(), list.latency_revision());
        assert!(r1 != r0 && l1 == l0, "addition");
        list.apply(update(vec![InfoEntry { uuid: 1, latency: Some(30), ..Default::default() }]));
        assert!(list.revision() == r1 && list.latency_revision() != l1, "latency alone");
        let l2 = list.latency_revision();
        list.apply(update(vec![InfoEntry { uuid: 2, listed: Some(true), ..Default::default() }]));
        assert!(list.revision() == r1 && list.latency_revision() == l2, "unknown player ignored");
        for (i, e) in [
            InfoEntry { game_mode: Some(3), ..Default::default() },
            InfoEntry { listed: Some(true), ..Default::default() },
            InfoEntry { display_name: Some(None), ..Default::default() },
            InfoEntry { list_order: Some(2), ..Default::default() },
            InfoEntry { show_hat: Some(false), ..Default::default() },
            InfoEntry { chat_session: Some(true), ..Default::default() },
        ]
        .into_iter()
        .enumerate()
        {
            let before = list.revision();
            list.apply(update(vec![InfoEntry { uuid: 1, ..e }]));
            assert_ne!(list.revision(), before, "field {i}");
        }
        let before = list.revision();
        list.remove(&[7]);
        assert_eq!(list.revision(), before, "removal of nobody");
        list.remove(&[1]);
        assert_ne!(list.revision(), before, "removal");
    }
}
