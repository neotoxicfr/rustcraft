//! Replay of the official client's reference capture (outside the repository) through the
//! handlers of the network thread: the dense packet table and the comparisons in order must give
//! the same events and the same replies, packet for packet, and both the digest recorded with the
//! code before the table (`RUSTCRAFT_OPT_CLIENT_PACKET_TABLE`).

use super::*;
use crate::link::Sink;

#[path = "conn_replay_tail.rs"]
mod tail;

/// Size of the capture the frozen digests were recorded on; another capture skips that check.
const REFERENCE_CAPTURE_LEN: usize = 53_361_964;

/// Recorded with the code before the table (the comparison chain of `run`, fed the capture then
/// the tail through a loopback socket: compressed frames, heavy packets decoded on the pool):
/// events, their kinds, replies. The capture alone gave 39 374 events and 118 replies.
const FROZEN: Digests = Digests {
    events: 0xc5a9_23ba_bdbf_396e,
    kinds: 0x4a24_44c5_d0c9_389f,
    replies: 0x2bc8_4ac0_9c65_9150,
    event_count: 39_413,
    reply_count: 128,
};

/// End of the session given by the tail's last packet, recorded with the same code.
const FROZEN_END: &str = "disconnected by the server: Some(String(\"bye\"))";

/// FNV-1a 64: a digest that does not depend on the Rust version, unlike the standard hasher.
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    const START: Fnv = Fnv(0xcbf2_9ce4_8422_2325);

    fn bytes(mut self, b: &[u8]) -> Fnv {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
        }
        self
    }
}

/// Digest and first characters of an event's `Debug` text, which is never held (a chunk's is
/// megabytes long).
struct EventText {
    digest: Fnv,
    head: String,
}

impl std::fmt::Write for EventText {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.digest = self.digest.bytes(s.as_bytes());
        if self.head.len() < 48 {
            self.head.extend(s.chars().take(48 - self.head.len()));
        }
        Ok(())
    }
}

/// Kind of an event (variant, and the inner one for wrappers: `Entity(Move`) and digest of its
/// full `Debug` text.
fn describe(e: &Event) -> (String, u64) {
    use std::fmt::Write;
    let mut t = EventText { digest: Fnv::START, head: String::new() };
    write!(t, "{e:?}").expect("debug text");
    let mut parens = 0;
    let kind = t
        .head
        .chars()
        .take_while(|&c| {
            parens += usize::from(c == '(');
            c.is_alphanumeric() || c == '_' || (c == '(' && parens < 2)
        })
        .collect();
    (kind, t.digest.0)
}

/// What a replay produced.
struct Replay {
    events: Vec<(String, u64)>,
    /// Packets sent (identifier then body); the chunk rate reply depends on the clock: identifier only.
    sent: Vec<Vec<u8>>,
    /// End of the session before the end of the capture.
    end: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Digests {
    events: u64,
    kinds: u64,
    replies: u64,
    event_count: usize,
    reply_count: usize,
}

impl Replay {
    fn digests(&self) -> Digests {
        let (mut events, mut kinds, mut replies) = (Fnv::START, Fnv::START, Fnv::START);
        for (kind, digest) in &self.events {
            events = events.bytes(&digest.to_le_bytes());
            kinds = kinds.bytes(kind.as_bytes()).bytes(b"\n");
        }
        for p in &self.sent {
            replies = replies.bytes(&(p.len() as u64).to_le_bytes()).bytes(p);
        }
        Digests {
            events: events.0,
            kinds: kinds.0,
            replies: replies.0,
            event_count: self.events.len(),
            reply_count: self.sent.len(),
        }
    }
}

/// No services in a test: the chat sessions are checked against an empty key set.
fn no_services_keys() -> Arc<Vec<Vec<u8>>> {
    Arc::new(Vec::new())
}

/// Every server → client packet of the capture, then the synthetic tail, through `handle`,
/// inline. `sabotage`: two identifiers of the play phase swapped in the dispatch (checks that the
/// comparison sees it).
fn replay(data: &VersionData, capture: &[u8], use_table: bool, sabotage: Option<(i32, i32)>) -> Replay {
    let ids = Ids::resolve(data).expect("packet identifiers");
    let (sink, sent) = crossbeam_channel::unbounded();
    let names = crate::protolog::Names::new(data);
    let link = Arc::new(Link::with_sink(Some(Sink::Memory(sink)), ids, names, Default::default()));
    let (tx, rx) = crossbeam_channel::unbounded();
    let seq = Arc::new(Sequencer::new(tx, |e| matches!(e, Event::Disconnected(_))));
    let cfg = Config { name: "Replay".into(), ..Default::default() };
    let mut out = Emit::new(&seq);
    let mut conn = Connection::new(&cfg, &link, data, use_table, no_services_keys);
    if let Some((a, b)) = sabotage {
        conn.dispatch.play.swap_for_test(a, b);
    }
    conn.greet().expect("greeting");
    let (mut dec, mut decoding, mut end) = (FrameDecoder::new(), None, None);
    let tail = tail::synthetic_tail(data);
    let records = rc_testdata::records(capture);
    let captured = records.iter().filter(|r| !r.to_server).map(|r| (r.phase, r.packet));
    for (phase_number, packet) in captured.chain(tail.iter().map(|(p, b)| (*p, b.as_slice()))) {
        let phase = match conn.phase {
            Phase::Login => 2,
            Phase::Configuration => 3,
            Phase::Play => 4,
        };
        assert_eq!(phase, phase_number, "phase of the client against the capture");
        match conn.handle(packet, &mut out, &mut dec, &mut decoding) {
            Ok(Flow::Next) => {}
            Ok(Flow::End(reason)) => {
                end = Some(reason);
                break;
            }
            Err(e) => {
                end = Some(format!("error {e} while reading {decoding:?}"));
                break;
            }
        }
    }
    drop(out);
    drop(seq);
    let rate_reply = link.ids.chunk_batch_received;
    Replay {
        events: rx.try_iter().map(|e| describe(&e)).collect(),
        sent: sent.try_iter().map(|p| clock_free(p, rate_reply)).collect(),
        end,
    }
}

/// Body of the chunk rate reply dropped: it carries a measured duration.
fn clock_free(mut packet: Vec<u8>, rate_reply: i32) -> Vec<u8> {
    let mut id = Vec::new();
    write_varint(&mut id, rate_reply);
    if packet.starts_with(&id) {
        packet.truncate(id.len());
    }
    packet
}

fn first_difference(a: &Replay, b: &Replay) -> Option<String> {
    if let Some(i) = (0..a.events.len().min(b.events.len())).find(|&i| a.events[i] != b.events[i]) {
        return Some(format!("event {i}: {:?} against {:?}", a.events[i], b.events[i]));
    }
    if a.events.len() != b.events.len() {
        return Some(format!("{} events against {}", a.events.len(), b.events.len()));
    }
    if let Some(i) = (0..a.sent.len().min(b.sent.len())).find(|&i| a.sent[i] != b.sent[i]) {
        return Some(format!("reply {i}: {:?} against {:?}", a.sent[i], b.sent[i]));
    }
    if a.sent.len() != b.sent.len() || a.end != b.end {
        return Some(format!(
            "{} replies, end {:?} against {} replies, end {:?}",
            a.sent.len(),
            a.end,
            b.sent.len(),
            b.end
        ));
    }
    None
}

fn reference() -> Option<(VersionData, Vec<u8>)> {
    Some((rc_testdata::version_data()?, rc_testdata::reference_capture()?))
}

#[test]
fn packet_table_replays_the_capture_like_the_comparisons() {
    let Some((data, capture)) = reference() else { return };
    let table = replay(&data, &capture, true, None);
    let compared = replay(&data, &capture, false, None);
    if let Some(d) = first_difference(&table, &compared) {
        panic!("table against comparisons: {d}");
    }
    let digests = table.digests();
    eprintln!("{digests:?}, end {:?}", table.end);
    assert_eq!(table.end.as_deref(), Some(FROZEN_END), "the capture and the tail are read to their end");
    assert!(digests.event_count > 30_000, "{digests:?}");
    if capture.len() != REFERENCE_CAPTURE_LEN {
        eprintln!("capture of {} bytes, not the reference one: frozen digests not compared", capture.len());
        return;
    }
    assert_eq!(digests, FROZEN, "events or replies changed since the code before the table");
}

/// Micro-bench (loaded machine figures in `docs/perf.md`):
/// `cargo test --release -p rc-client network_thread_handle_bench -- --ignored --nocapture`.
/// The capture's light play packets (no chunk, light or recipes: their decoding hides the rest)
/// read again and again by `handle`, with the comparisons then with the table.
#[test]
#[ignore]
fn network_thread_handle_bench() {
    let Some((data, capture)) = reference() else { return };
    let ids = Ids::resolve(&data).expect("packet identifiers");
    let records = rc_testdata::records(&capture);
    let server: Vec<_> = records.iter().filter(|r| !r.to_server).collect();
    let first_play = server.iter().position(|r| r.phase == 4).expect("play phase");
    let heavy = [ids.chunk, ids.light_update, ids.recipe_book_add, ids.login, ids.commands];
    let mut light: Vec<&[u8]> = server[first_play + 1..]
        .iter()
        .map(|r| r.packet)
        .filter(|p| !heavy.contains(&read_varint(&mut Reader::new(p)).expect("identifier")))
        .collect();
    // Every round must close the bundles it opens.
    let is_delimiter = |p: &[u8]| read_varint(&mut Reader::new(p)).ok() == Some(ids.bundle_delimiter);
    if light.iter().filter(|p| is_delimiter(p)).count() % 2 == 1 {
        let last = light.iter().rposition(|p| is_delimiter(p)).expect("delimiter");
        light.truncate(last);
    }
    for trial in 0..3 {
        for (name, use_table) in [("comparisons", false), ("table", true)] {
            let (sink, sent) = crossbeam_channel::unbounded();
            let names = crate::protolog::Names::new(&data);
            let link = Arc::new(Link::with_sink(Some(Sink::Memory(sink)), ids, names, Default::default()));
            let (tx, rx) = crossbeam_channel::unbounded();
            let seq = Arc::new(Sequencer::new(tx, |e| matches!(e, Event::Disconnected(_))));
            let cfg = Config { name: "Replay".into(), ..Default::default() };
            let mut out = Emit::new(&seq);
            let mut conn = Connection::new(&cfg, &link, &data, use_table, no_services_keys);
            let (mut dec, mut decoding) = (FrameDecoder::new(), None);
            // Into the play phase with the capture's own beginning.
            for r in &server[..=first_play] {
                conn.handle(r.packet, &mut out, &mut dec, &mut decoding).expect("beginning");
            }
            let mut elapsed = std::time::Duration::ZERO;
            for _ in 0..20 {
                let t = std::time::Instant::now();
                for p in &light {
                    conn.handle(p, &mut out, &mut dec, &mut decoding).expect("light packet");
                }
                elapsed += t.elapsed();
                rx.try_iter().for_each(drop);
                sent.try_iter().for_each(drop);
            }
            let ns = elapsed.as_nanos() as f64 / (20 * light.len()) as f64;
            eprintln!("trial {trial}: {name} {ns:.1} ns per packet ({} packets)", light.len());
        }
    }
}

#[test]
fn replay_comparison_sees_two_swapped_packets() {
    let Some((data, capture)) = reference() else { return };
    let ids = Ids::resolve(&data).expect("packet identifiers");
    let good = replay(&data, &capture, true, None);
    let swapped = replay(&data, &capture, true, Some((ids.move_entity_pos, ids.move_entity_rot)));
    assert!(first_difference(&good, &swapped).is_some(), "a swapped handler went unnoticed");
}
