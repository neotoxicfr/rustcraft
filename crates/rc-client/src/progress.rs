//! Progress and statistics (`docs/specs/advancements-stats.md` § 1–2): decoded on the network
//! thread, applied by the game (`rc-app/src/advancements.rs`, `stats.rs`).

use rc_proto::Reader;
use rc_proto_v777::advancements as adv;
use rc_proto_v777::item::ItemCodec;

use crate::ids::Ids;
use crate::{Error, Event};

/// Events of this module (a single `Event::Progress` variant on the session side).
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Advancements(Box<adv::UpdateAdvancements>),
    /// Tab imposed by the server (root), `None`: none.
    SelectTab(Option<String>),
    /// Statistics: (type, registry value, count).
    Stats(Vec<(i32, i32, i32)>),
}

/// Identifiers, fixed per version: also read by the decoding of heavy packets (pool).
static IDS: std::sync::OnceLock<ProgressIds> = std::sync::OnceLock::new();

/// Heavy packet (compressed, decoded on the pool): `None` if it is not from this module; an
/// unreadable packet ends the session, like the game.
pub(crate) fn heavy(id: i32, r: &mut Reader<'_>, items: &ItemCodec) -> Option<Event> {
    let ids = IDS.get()?;
    match play(ids, id, r, items) {
        Ok(e) => e,
        Err(e) => Some(Event::Disconnected(format!("unreadable progress: {e}"))),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ProgressIds {
    update: i32,
    select: i32,
    stats: i32,
}

pub(crate) fn ids(i: &Ids) -> ProgressIds {
    let p = ProgressIds { update: i.update_advancements, select: i.select_advancements_tab, stats: i.award_stats };
    let _ = IDS.set(p);
    p
}

/// `None` if the packet is not from this module.
pub(crate) fn play(ids: &ProgressIds, id: i32, r: &mut Reader<'_>, items: &ItemCodec) -> Result<Option<Event>, Error> {
    let p = if id == ids.update {
        Progress::Advancements(Box::new(adv::read_update_advancements(r, items)?))
    } else if id == ids.select {
        Progress::SelectTab(adv::read_select_tab(r)?)
    } else if id == ids.stats {
        Progress::Stats(adv::read_award_stats(r)?)
    } else {
        return Ok(None);
    };
    r.finish()?;
    Ok(Some(Event::Progress(p)))
}
