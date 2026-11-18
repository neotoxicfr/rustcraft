//! Team of every member, kept up to date by each membership change
//! (`RUSTCRAFT_OPT_TEAMMEMBERINDEX`, default 1): the team of a holder (decorated names, `team=`
//! filters, friendly fire) and the push rules of the mobs are one lookup instead of a scan of
//! every team's members, and joining or leaving no longer throws a rebuilt index away (a rebuild
//! on demand costs O(members) at each read that follows a write, O(N²) when they alternate).
//! `0`: teams scanned, push index rebuilt on demand (the former way).

use hashbrown::HashMap;

use super::scores::Team;
use crate::nbtio::canonical_uuid;

/// Member → rank of its team in the board's team list. A member registered in several teams (a
/// save, a team created already filled) belongs to the first one of the list, like a scan.
#[derive(Debug, Clone, Default)]
pub struct TeamMembers {
    /// Members whose name is an entity UUID in canonical text: looked up by UUID, without text.
    by_uuid: HashMap<u128, u32>,
    /// The other members (players, plain names, UUIDs in another text form).
    by_name: HashMap<String, u32>,
    /// A member was registered in a second team (or twice in one): leaving must then look for
    /// the team it still belongs to. Never set by joins alone (joining leaves the old team first).
    shared: bool,
}

impl TeamMembers {
    /// Index of `teams`, members in list order (the first registration wins).
    pub fn of(teams: &[Team]) -> TeamMembers {
        let mut ix = TeamMembers::default();
        for (rank, t) in teams.iter().enumerate() {
            for p in &t.players {
                ix.add(p, rank as u32);
            }
        }
        ix
    }

    pub fn is_empty(&self) -> bool {
        self.by_uuid.is_empty() && self.by_name.is_empty()
    }

    /// Rank of `member`'s team.
    pub fn rank(&self, member: &str) -> Option<u32> {
        match canonical_uuid(member) {
            Some(u) => self.by_uuid.get(&u).copied(),
            None => self.by_name.get(member).copied(),
        }
    }

    /// Rank of the team of the member named by `uuid`'s canonical text.
    pub fn rank_by_uuid(&self, uuid: u128) -> Option<u32> {
        self.by_uuid.get(&uuid).copied()
    }

    /// Rank of the team of a member whose name is not a canonical UUID text (`None` for one that
    /// is: it is found by [`TeamMembers::rank_by_uuid`]).
    pub fn rank_by_plain_name(&self, name: &str) -> Option<u32> {
        self.by_name.get(name).copied()
    }

    /// `member` registered in the team of rank `rank`; an earlier team it is still in keeps it.
    pub fn add(&mut self, member: &str, rank: u32) {
        // `NEW` marks an entry this call creates (no team has that many ranks).
        const NEW: u32 = u32::MAX;
        let slot = match canonical_uuid(member) {
            Some(u) => self.by_uuid.entry(u).or_insert(NEW),
            None => self.by_name.entry_ref(member).or_insert(NEW),
        };
        if *slot != NEW {
            self.shared = true;
        }
        *slot = (*slot).min(rank);
    }

    /// `member` left its team (all its registrations in it are gone from `teams`): forgotten,
    /// unless it is still registered in another team.
    pub fn remove(&mut self, member: &str, teams: &[Team]) {
        let next = if self.shared { teams.iter().position(|t| t.players.iter().any(|p| p == member)) } else { None };
        match (canonical_uuid(member), next) {
            (Some(u), Some(r)) => {
                self.by_uuid.insert(u, r as u32);
            }
            (Some(u), None) => {
                self.by_uuid.remove(&u);
            }
            (None, Some(r)) => {
                if let Some(slot) = self.by_name.get_mut(member) {
                    *slot = r as u32;
                }
            }
            (None, None) => {
                self.by_name.remove(member);
            }
        }
    }

    /// The index matches a scan of `teams` for every member.
    #[cfg(test)]
    pub fn check_consistent(&self, teams: &[Team]) {
        let rebuilt = TeamMembers::of(teams);
        assert_eq!(self.by_uuid, rebuilt.by_uuid, "UUID members differ from the teams");
        assert_eq!(self.by_name, rebuilt.by_name, "named members differ from the teams");
    }
}
