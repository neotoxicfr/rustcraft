//! In-memory link with the singleplayer integrated server (`integrated-server.md` § 0): the server
//! publishes the `Event`s directly (no decoding), the client sends its packets through the same
//! `Link` as over the network, delivered to the server as "identifier + body" (no framing, no
//! compression, no encryption: a few dozen bytes per packet).

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};

use crate::link::{Link, Sink};
use crate::{Cookies, Error, Event, Session};

/// Server side of the link.
pub struct Peer {
    events: Sender<Event>,
    link: Arc<Link>,
    uuid: u128,
    /// Client packets: VarInt identifier then body. Closed when the client quits.
    pub packets: Receiver<Vec<u8>>,
}

impl Peer {
    /// Publishes an event; false if the client is gone.
    pub fn send(&self, e: Event) -> bool {
        if let Event::Joined { login, .. } = &e {
            // What the network thread does on receiving `login`.
            self.link.set_player_id(login.player_id);
            self.link.chat_reset(self.uuid, false, false);
        }
        self.events.send(e).is_ok()
    }

    pub fn ids(&self) -> &crate::ids::Ids {
        &self.link.ids
    }

    /// Client readings (integrated server tick graph, F3+2).
    pub fn stats(&self) -> &crate::netstats::NetStats {
        &self.link.stats
    }
}

/// Opens an in-memory session for player `uuid`.
pub fn session(data: Arc<rc_data::VersionData>, uuid: u128) -> Result<(Session, Peer), Error> {
    let ids = crate::ids::Ids::resolve(&data)?;
    let (etx, erx) = crossbeam_channel::unbounded();
    let (ptx, prx) = crossbeam_channel::unbounded();
    let names = crate::protolog::Names::new(&data);
    let link = Arc::new(Link::with_sink(Some(Sink::Memory(ptx)), ids, names, Cookies::default()));
    let peer = Peer { events: etx, link: link.clone(), uuid, packets: prx };
    Ok((Session { events: erx, data, link }, peer))
}
