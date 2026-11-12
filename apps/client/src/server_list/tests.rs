//! Proofs and micro-benches of the server list (`docs/perf.md`, C4-M31): icons decoded on change,
//! row texts kept, icons of one frame written together, status threads kept for the session, LAN
//! discovery that survives a refresh.
//!
//! The drawing proofs render on the real GPU interface of `gui/tests_opt.rs`: without an installed
//! game or a GPU they skip themselves (`RUSTCRAFT_OPT_REQUIRE=1` makes that a failure).

use std::cell::Cell;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use base64::Engine;

use super::*;
use crate::gui::tests_opt;

mod reference;

thread_local! {
    /// Icon decodings, row texts built and status threads started by the current thread (each
    /// test has its own).
    static ICON_DECODINGS: Cell<usize> = const { Cell::new(0) };
    static ROW_TEXTS_BUILT: Cell<usize> = const { Cell::new(0) };
    static STATUS_THREADS_STARTED: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn count_icon_decoding() {
    ICON_DECODINGS.with(|count| count.set(count.get() + 1));
}

pub(super) fn count_row_texts_built() {
    ROW_TEXTS_BUILT.with(|count| count.set(count.get() + 1));
}

fn row_texts_built() -> usize {
    ROW_TEXTS_BUILT.with(Cell::get)
}

pub(super) fn count_status_thread() {
    STATUS_THREADS_STARTED.with(|count| count.set(count.get() + 1));
}

fn icon_decodings() -> usize {
    ICON_DECODINGS.with(Cell::get)
}

fn status_threads_started() -> usize {
    STATUS_THREADS_STARTED.with(Cell::get)
}

const ALL_OFF: Switches = Switches {
    decode_icon_on_change: false,
    keep_row_texts: false,
    batched_icon_save: false,
    session_status_threads: false,
};
const ALL_ON: Switches = Switches {
    decode_icon_on_change: true,
    keep_row_texts: true,
    batched_icon_save: true,
    session_status_threads: true,
};

fn scratch(name: &str) -> PathBuf {
    rc_testdata::scratch(std::env::temp_dir().join(format!("rc-server-list-{name}-{}", std::process::id())))
}

/// A square RGBA PNG whose pixels depend on `seed`.
fn png_icon(side: u32, seed: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, side, side);
    encoder.set_color(png::ColorType::Rgba);
    let mut writer = encoder.write_header().expect("png header");
    let pixels: Vec<u8> = (0..side * side * 4).map(|k| (k as u8).wrapping_mul(seed | 1).wrapping_add(seed)).collect();
    writer.write_image_data(&pixels).expect("png pixels");
    drop(writer);
    bytes
}

fn write_list(dir: &Path, entries: Vec<ServerEntry>) {
    ServerList { visible: entries, hidden: Vec::new() }.save(dir).expect("servers.dat");
}

fn servers_dat(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("servers.dat")).expect("servers.dat")
}

/// Screen over `dir` that asks nothing on the network: its queries wait in the returned queue,
/// which nobody answers, and it does not listen for LAN worlds.
fn silent_screen(dir: &Path, switches: Switches) -> (Multiplayer, Receiver<PingJob>) {
    let (queue, unanswered) = crossbeam_channel::unbounded();
    let pinger = Pinger::on(StatusThreads::Own(queue));
    (Multiplayer::with_parts(dir.to_owned(), switches, pinger, None), unanswered)
}

/// What the first drawing of every row does: its query sent.
fn query_all_rows(screen: &mut Multiplayer) {
    let Multiplayer { rows, next_id, pinger, .. } = screen;
    for row in rows.iter_mut().filter(|row| matches!(row.ping, Ping::Initial)) {
        row.start_query(next_id, pinger);
    }
}

/// Status answer of a modern server.
struct Reply<'a> {
    motd: serde_json::Value,
    /// Online, maximum and sample names; `None`: no `players` field.
    players: Option<(i32, i32, &'a [&'a str])>,
    version: &'a str,
    protocol: i32,
    favicon: Option<&'a [u8]>,
}

impl Reply<'_> {
    fn json(&self) -> String {
        let mut reply = serde_json::json!({
            "version": { "name": self.version, "protocol": self.protocol },
            "description": self.motd,
        });
        if let Some((online, max, names)) = self.players {
            let sample: Vec<serde_json::Value> = names
                .iter()
                .enumerate()
                .map(|(k, name)| {
                    // The game's anonymous player: this name with the nil id.
                    let id = if *name == "Anonymous Player" { 0 } else { k + 1 };
                    serde_json::json!({ "id": format!("00000000-0000-0000-0000-{id:012x}"), "name": name })
                })
                .collect();
            reply["players"] = serde_json::json!({ "online": online, "max": max, "sample": sample });
        }
        if let Some(png) = self.favicon {
            let encoded = base64::engine::general_purpose::STANDARD.encode(png);
            reply["favicon"] = format!("data:image/png;base64,{encoded}").into();
        }
        reply.to_string()
    }

    /// The answer as the query threads hand it to the screen.
    fn answer(&self, latency_ms: u64) -> Result<PingOutcome, StatusError> {
        let status = status::parse_status_json(&self.json());
        let compatible = self.protocol == rc_proto_v777::PROTOCOL;
        Ok(PingOutcome::Modern(status::Pinged { status, latency_ms, compatible }))
    }
}

/// A coloured MOTD that needs the two lines of a row.
fn two_line_motd() -> serde_json::Value {
    serde_json::json!({
        "text": "A long message of the day, ",
        "color": "gold",
        "extra": [{ "text": "bold part that wraps onto the second line of the row", "bold": true }],
    })
}

fn deliver(screen: &Multiplayer, id: u64, answer: Result<PingOutcome, StatusError>) {
    screen.pinger.answers.send((id, answer)).expect("answer channel");
}

/// Anti-hang guard of the waits on the machine's sockets and threads: never the normal path of a
/// test, which waits for its condition.
const GUARD: Duration = Duration::from_secs(120);

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let guard = Instant::now() + GUARD;
    while !done() {
        assert!(Instant::now() < guard, "{what}: not reached within the {GUARD:?} anti-hang guard");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Game alert: title 70, message 90, button clamped between h/6 + 96 and h − 24.
#[test]
fn alert_rows_follow_the_game() {
    assert_eq!(Column::alert_rows(240, 1), (70, 90, 136));
    assert_eq!(Column::alert_rows(240, 10), (70, 90, 192));
    assert_eq!(Column::alert_rows(240, 20), (70, 90, 216));
    // Tiny window: lower bound above the upper one, the upper one wins.
    assert_eq!(Column::alert_rows(100, 1), (70, 90, 76));
}

#[test]
fn lan_announcements() {
    assert_eq!(parse_lan("[MOTD]A world[/MOTD][AD]51234[/AD]"), Some(("A world".into(), "51234".into())));
    assert_eq!(parse_lan("[/MOTD][AD]1[/AD]"), Some(("missing no".into(), "1".into())));
    assert_eq!(parse_lan("[MOTD]x[/MOTD]"), None);
    assert_eq!(parse_lan("[MOTD]x[/MOTD][/MOTD][AD]1[/AD]"), None);
    assert_eq!(parse_lan("[MOTD]x[AD]1[/AD]"), None);
}

// --- Icons ---

/// Rows of a short list keep the slot of their rank (what the screen did before); a longer list
/// shares the slots among the rows on screen; a row a whole turn below the first one gets none.
#[test]
fn icon_slots_follow_the_rows_on_screen() {
    for index in 0..32 {
        assert_eq!(icon_slot(index, 0, 32), Some(index));
        assert_eq!(icon_slot(index, index.min(5), 32), Some(index));
    }
    assert_eq!(icon_slot(40, 30, 32), Some(8));
    assert_eq!(icon_slot(61, 30, 32), Some(29));
    assert_eq!(icon_slot(62, 30, 32), None, "would overwrite the slot of row 30, drawn this frame");
    assert_eq!(icon_slot(3, 3, 0), None, "no slot at all");
    // No two rows of one frame share a slot.
    let taken: Vec<usize> = (100..140).filter_map(|index| icon_slot(index, 100, 32)).collect();
    let mut distinct = taken.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!((taken.len(), distinct.len()), (32, 32));
}

/// One way of showing the icons of a list, frame after frame, with the uploads it asked for.
struct IconPath {
    decode_on_change: bool,
    rows: Vec<Row>,
    slots: Vec<Option<Vec<u8>>>,
    uploads: Vec<(usize, Vec<u8>)>,
    decodings: usize,
}

impl IconPath {
    fn new(decode_on_change: bool, icons: &[Option<Vec<u8>>], slot_count: usize) -> IconPath {
        let rows = icons
            .iter()
            .map(|icon| {
                let mut entry = ServerEntry::new("s", "s.example");
                entry.icon = icon.clone();
                Row::new(entry)
            })
            .collect();
        IconPath { decode_on_change, rows, slots: vec![None; slot_count], uploads: Vec::new(), decodings: 0 }
    }

    /// One frame with the rows from `first_visible` on screen: what each row shows.
    fn frame(&mut self, first_visible: usize) -> Vec<IconShown> {
        let before = icon_decodings();
        let IconPath { decode_on_change, rows, slots, uploads, .. } = self;
        let mut upload = |slot: usize, pixels: &[u8]| {
            uploads.push((slot, pixels.to_vec()));
            true
        };
        let mut shown = Vec::new();
        for (index, row) in rows.iter_mut().enumerate().skip(first_visible) {
            let slot = icon_slot(index, first_visible, slots.len());
            let icon = if *decode_on_change {
                show_icon_decoding_on_change(row, slot, slots, &mut upload)
            } else {
                show_icon_decoding_every_frame(row, slot, slots, &mut upload)
            };
            if icon == IconShown::Rejected {
                row.entry.icon = None;
            }
            shown.push(icon);
        }
        self.decodings += icon_decodings() - before;
        shown
    }
}

/// Decoding on change shows, uploads and rejects exactly what decoding on every frame does, over
/// 60 frames of a list of 7 rows on 4 slots (rows without a slot included): valid icons, a
/// 32 × 32 picture and bytes that are no PNG (rejected), no icon, an icon replaced by an answer,
/// two rows swapped, the list scrolled. And it decodes on changes only.
#[test]
fn icon_decoded_on_change_shows_what_decoding_every_frame_shows() {
    let icons = [
        Some(png_icon(64, 1)),
        Some(png_icon(64, 2)),
        Some(png_icon(32, 3)),
        None,
        Some(vec![1, 2, 3]),
        Some(png_icon(64, 5)),
        Some(png_icon(64, 6)),
    ];
    let mut every_frame = IconPath::new(false, &icons, 4);
    let mut on_change = IconPath::new(true, &icons, 4);
    let (answered_icon, wrong_size_icon) = (png_icon(64, 9), png_icon(48, 7));
    let mut frames_with_a_slot = 0;
    for frame in 0..60 {
        for path in [&mut every_frame, &mut on_change] {
            match frame {
                // An answer brings another icon to row 1; later, one of another size to row 5.
                15 => {
                    let reply = Reply {
                        motd: "m".into(),
                        players: None,
                        version: "26.3",
                        protocol: 777,
                        favicon: Some(&answered_icon),
                    };
                    assert!(path.rows[1].take_answer(reply.answer(10)));
                }
                25 => path.rows.swap(0, 1),
                55 => {
                    let reply = Reply {
                        motd: "m".into(),
                        players: None,
                        version: "26.3",
                        protocol: 777,
                        favicon: Some(&wrong_size_icon),
                    };
                    assert!(path.rows[5].take_answer(reply.answer(10)));
                }
                _ => {}
            }
        }
        // Frames 40 to 49: the list scrolled by two rows (row 5 takes the slot 1, held by row 1).
        let first_visible = if (40..50).contains(&frame) { 2 } else { 0 };
        let (want, got) = (every_frame.frame(first_visible), on_change.frame(first_visible));
        assert_eq!(got, want, "frame {frame}: icons shown");
        assert_eq!(on_change.slots, every_frame.slots, "frame {frame}: slots");
        frames_with_a_slot += want.iter().filter(|icon| matches!(icon, IconShown::InSlot(_))).count();
        let icons_left = |path: &IconPath| path.rows.iter().map(|row| row.entry.icon.clone()).collect::<Vec<_>>();
        assert_eq!(icons_left(&on_change), icons_left(&every_frame), "frame {frame}: icons kept");
    }
    assert_eq!(on_change.uploads, every_frame.uploads, "same pixels sent to the same slots, in the same order");
    assert!(frames_with_a_slot > 100, "{frames_with_a_slot} icons shown from a slot");
    assert_eq!(every_frame.uploads.len(), 7, "two icons, one answered, two swapped, one scrolled in and out");
    // Every frame: one decoding per row with an icon. On change: one per upload, one per
    // rejection (rows 2 and 4, then row 5), one for each valid icon first met without a slot
    // (rows 5 and 6).
    assert!(every_frame.decodings > 200, "{} decodings on the old path", every_frame.decodings);
    let (rejections, valid_without_slot) = (3, 2);
    assert_eq!(on_change.decodings, on_change.uploads.len() + rejections + valid_without_slot);
}

// --- Row texts ---

/// Fixed-width text metrics (6 per character).
struct Fixed;

impl rc_text::Metrics for Fixed {
    fn advance(&self, _font: &str, _c: char) -> (f32, f32) {
        (6.0, 1.0)
    }
}

/// Kept texts are the texts built on every frame, over 40 frames of five rows: queries sent,
/// answers (players, no players, incompatible, failure, legacy), another language, another row
/// width, an entry renamed and queried again. And they are built on changes only.
#[test]
fn kept_row_texts_are_the_texts_built_every_frame() {
    let lang = rc_text::Lang::default();
    let new_rows =
        || (0..5).map(|k| Row::new(ServerEntry::new(format!("Server {k} §6gold"), "s.example"))).collect::<Vec<_>>();
    let (mut built_every_frame, mut kept) = (new_rows(), new_rows());
    let long_motd = serde_json::json!({
        "text": "A message of the day long enough to need a second line, ",
        "extra": [{ "text": "and even a third one that the row does not show", "color": "red" }],
    });
    let (mut every_frame_builds, mut kept_builds) = (0, 0);
    for frame in 0..40u64 {
        for rows in [&mut built_every_frame, &mut kept] {
            match frame {
                2 => rows.iter_mut().for_each(|row| row.set_ping(Ping::Pinging)),
                5 => {
                    let players = Reply {
                        motd: long_motd.clone(),
                        players: Some((7, 20, &["Zoé"])),
                        version: "26.3",
                        protocol: 777,
                        favicon: None,
                    };
                    rows[0].take_answer(players.answer(30));
                    let no_players =
                        Reply { motd: "§aShort".into(), players: None, version: "26.3", protocol: 777, favicon: None };
                    rows[1].take_answer(no_players.answer(30));
                    let incompatible = Reply {
                        motd: "Old".into(),
                        players: Some((1, 2, &[])),
                        version: "1.8.9",
                        protocol: 47,
                        favicon: None,
                    };
                    rows[2].take_answer(incompatible.answer(30));
                    rows[3].take_answer(Err(StatusError::UnknownHost));
                    let legacy = status::LegacyStatus {
                        protocol: 60,
                        version: "1.6.4".into(),
                        motd: "Legacy".into(),
                        online: 3,
                        max: 8,
                    };
                    rows[4].take_answer(Ok(PingOutcome::Legacy(legacy)));
                }
                // An entry renamed and queried again (what an edit does), then answered.
                25 => {
                    rows[1].entry.name = "Renamed".into();
                    rows[1].set_ping(Ping::Initial);
                }
                30 => {
                    rows[1].take_answer(Err(StatusError::Invalid("no answer")));
                }
                _ => {}
            }
        }
        // Another language at frame 12, a narrower row from frame 18.
        let key = RowTextKey {
            context: TextContext::for_tests(if frame < 12 { 1 } else { 2 }),
            content_width: if frame < 18 { ROW_W - 4 } else { 200 },
        };
        let before = row_texts_built();
        built_every_frame.iter_mut().for_each(|row| row.refresh_texts(key, false, &Fixed, &lang));
        every_frame_builds += row_texts_built() - before;
        let before = row_texts_built();
        kept.iter_mut().for_each(|row| row.refresh_texts(key, true, &Fixed, &lang));
        kept_builds += row_texts_built() - before;
        for (index, (want, got)) in built_every_frame.iter().zip(&kept).enumerate() {
            assert_eq!(got.texts, want.texts, "frame {frame}, row {index}");
        }
    }
    let texts = |row: &Row| row.texts.clone().expect("texts").1;
    assert_eq!(texts(&kept[0]).motd_lines.len(), 2, "MOTD cut to the two lines shown");
    assert!(texts(&kept[0]).status_width > 0 && texts(&kept[2]).status_width > 0 && texts(&kept[3]).status.is_empty());
    assert_eq!(every_frame_builds, 40 * 5);
    // First frame, queries sent, answers, language, width: five rows each; then the renamed row
    // twice (edited, answered).
    assert_eq!(kept_builds, 5 * 5 + 2);
}

// --- Writes of servers.dat ---

/// Eight servers answer in the same frame with a new icon: one write of `servers.dat` instead of
/// eight, the same file to the byte. `servers.dat_old` is then the list as it was before the
/// frame (the old path leaves the list with all the icons but the last one there).
#[test]
fn icons_of_one_frame_are_written_once() {
    let entries: Vec<ServerEntry> =
        (0..8).map(|k| ServerEntry::new(format!("Server {k}"), format!("s{k}.example"))).collect();
    let icons: Vec<Vec<u8>> = (0..8).map(|k| png_icon(64, k + 1)).collect();
    let run = |name: &str, switches: Switches| {
        let dir = scratch(name);
        write_list(&dir, entries.clone());
        let before_frame = servers_dat(&dir);
        let (mut screen, _unanswered) = silent_screen(&dir, switches);
        query_all_rows(&mut screen);
        for (k, icon) in icons.iter().enumerate() {
            let reply = Reply { motd: "m".into(), players: None, version: "26.3", protocol: 777, favicon: Some(icon) };
            deliver(&screen, k as u64 + 1, reply.answer(20));
        }
        let saves = crate::servers::tests::saves();
        screen.poll();
        let writes = crate::servers::tests::saves() - saves;
        assert!(screen.rows.iter().zip(&icons).all(|(row, icon)| row.entry.icon.as_ref() == Some(icon)));
        // A second frame without a new icon writes nothing.
        screen.poll();
        assert_eq!(crate::servers::tests::saves() - saves, writes);
        let previous = std::fs::read(dir.join("servers.dat_old")).expect("servers.dat_old");
        (writes, servers_dat(&dir), previous == before_frame)
    };
    let (old_writes, old_file, old_keeps_the_list_before) = run("writes-old", ALL_OFF);
    let (new_writes, new_file, new_keeps_the_list_before) = run("writes-batched", ALL_ON);
    assert_eq!((old_writes, new_writes), (8, 1));
    assert_eq!(new_file, old_file, "same servers.dat");
    assert_eq!(ServerList::decode(&new_file).visible.iter().filter(|e| e.icon.is_some()).count(), 8);
    assert!(new_keeps_the_list_before && !old_keeps_the_list_before);
}

// --- Status threads ---

fn read_frame(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut length = 0usize;
    for shift in 0..5 {
        let mut byte = [0u8];
        stream.read_exact(&mut byte)?;
        length |= usize::from(byte[0] & 0x7f) << (7 * shift);
        if byte[0] & 0x80 == 0 {
            break;
        }
    }
    let mut frame = vec![0; length];
    stream.read_exact(&mut frame)?;
    Ok(frame)
}

fn varint(mut value: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        bytes.push(if value == 0 { low } else { low | 0x80 });
        if value == 0 {
            return bytes;
        }
    }
}

fn write_frame(stream: &mut TcpStream, id: u8, body: &[u8]) -> std::io::Result<()> {
    let mut frame = varint(body.len() + 1);
    frame.push(id);
    frame.extend_from_slice(body);
    stream.write_all(&frame)
}

/// One status exchange of a modern server: handshake and request read, `json` sent once `gate`
/// can be read (a test holding its write guard holds the answers back), ping echoed.
fn serve_status(mut stream: TcpStream, json: &str, gate: &RwLock<()>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(GUARD))?;
    read_frame(&mut stream)?;
    read_frame(&mut stream)?;
    drop(gate.read());
    let mut text = varint(json.len());
    text.extend_from_slice(json.as_bytes());
    write_frame(&mut stream, 0x00, &text)?;
    let ping = read_frame(&mut stream)?;
    write_frame(&mut stream, 0x01, ping.get(1..).unwrap_or_default())
}

/// A status server on the machine, for the tests: answers every connection with `json` once
/// `gate` is open (`Arc::default()`: at once), and counts the connections it accepted. Stops
/// accepting when dropped.
struct FakeStatusServer {
    address: String,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl FakeStatusServer {
    fn start(json: String, gate: Arc<RwLock<()>>) -> FakeStatusServer {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address").to_string();
        let (connections, stop) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let (counted, stopped) = (Arc::clone(&connections), Arc::clone(&stop));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                counted.fetch_add(1, Ordering::Relaxed);
                let (json, gate) = (json.clone(), Arc::clone(&gate));
                std::thread::spawn(move || serve_status(stream, &json, &gate));
            }
        });
        FakeStatusServer { address, connections, stop }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for FakeStatusServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wakes the accept loop so that it sees the flag.
        let _ = TcpStream::connect(&self.address);
    }
}

fn motd_reply(motd: &str) -> String {
    Reply { motd: motd.into(), players: Some((1, 20, &["Zoé"])), version: "26.3", protocol: 777, favicon: None }.json()
}

fn motd_of(answer: &PingResult) -> String {
    match &answer.1 {
        Ok(PingOutcome::Modern(pinged)) => pinged.status.description.as_str().unwrap_or_default().to_owned(),
        other => panic!("modern answer expected, got {other:?}"),
    }
}

/// Eleven openings of the screen (ten refreshes) over three servers of the machine: every
/// opening gets its three answers. With the session's threads no opening starts a thread beyond
/// the first five; the old path starts five per opening.
#[test]
fn ten_refreshes_keep_the_status_threads_of_the_session() {
    rc_sim::lan::announce::LOCAL_ONLY.store(true, Ordering::Relaxed);
    let server = FakeStatusServer::start(motd_reply("Hello from the machine"), Arc::default());
    let entries: Vec<ServerEntry> =
        (0..3).map(|k| ServerEntry::new(format!("Local {k}"), server.address.clone())).collect();
    let threads_started = |name: &str, switches: Switches| {
        let dir = scratch(name);
        write_list(&dir, entries.clone());
        let started = status_threads_started();
        let mut screen = Multiplayer::open(dir, switches);
        for opening in 0..=10 {
            query_all_rows(&mut screen);
            wait_until("three answers", || {
                screen.poll();
                screen.rows.iter().all(|row| matches!(&row.ping, Ping::Done(status) if status.players == Some((1, 20))))
            });
            assert_eq!(screen.rows.len(), 3, "opening {opening}");
            screen.refresh();
        }
        status_threads_started() - started
    };
    let connections = server.connections();
    let with_session_threads = threads_started("threads-session", ALL_ON);
    assert_eq!(server.connections() - connections, 33, "one query per row and per opening");
    // Five, or none if another test of this process started the session's threads first.
    assert!(with_session_threads <= PING_THREADS, "{with_session_threads} threads started");
    assert_eq!(threads_started("threads-per-opening", ALL_OFF), 12 * PING_THREADS, "old path: five per opening");
}

/// Two openings share the same threads and both number their queries from 1: the answer to the
/// closed opening never reaches the next one, and its queries that had not started are skipped.
/// The slow server holds its answers until the closed screen is gone (a gate, not a delay), and
/// the five answers of the queries it had started are awaited on the closed screen's own channel
/// before the open one is checked for a stray answer. The queries get the anti-hang guard as their
/// time limit instead of the game's 5 s: this thread held up longer than 5 s cannot end a held
/// query and let a sixth start. No step depends on the machine's speed.
#[test]
fn answers_to_a_closed_screen_are_dropped() {
    let gate = Arc::new(RwLock::new(()));
    let slow = FakeStatusServer::start(motd_reply("slow, asked by the closed screen"), Arc::clone(&gate));
    let fast = FakeStatusServer::start(motd_reply("fast, asked by the open screen"), Arc::default());
    let held = gate.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    let queue = start_status_threads(GUARD);
    let closed = Pinger::on(StatusThreads::Own(queue.clone()));
    // Five queries occupy the five threads, fifteen wait behind them.
    for id in 1..=20 {
        closed.submit(id, &slow.address);
    }
    wait_until("five queries running", || slow.connections() == PING_THREADS);
    let closed_answers = closed.results.clone();
    drop(closed);
    let open = Pinger::on(StatusThreads::Own(queue));
    open.submit(1, &fast.address);
    // The screen is gone: the five running queries may end now.
    drop(held);
    let answer = open.results.recv_timeout(GUARD).expect("answer to the open screen (anti-hang guard)");
    assert_eq!((answer.0, motd_of(&answer).as_str()), (1, "fast, asked by the open screen"));
    // The five queries that had started (the first five, in arrival order) answer the closed
    // screen, on its own channel...
    let mut started: Vec<u64> = (0..PING_THREADS)
        .map(|k| closed_answers.recv_timeout(GUARD).unwrap_or_else(|e| panic!("answer {k} of a started query: {e:?}")).0)
        .collect();
    started.sort_unstable();
    assert_eq!(started, (1..=PING_THREADS as u64).collect::<Vec<_>>(), "answers of the started queries");
    // ...so every stale answer has been sent by now, and none came here.
    assert!(open.results.try_recv().is_err(), "stale answer delivered");
    assert_eq!(slow.connections(), PING_THREADS, "the fifteen waiting queries were skipped");
    assert_eq!(fast.connections(), 1);
}

// --- LAN ---

/// A world announces itself on the machine while the screen is refreshed twice: it is found
/// again after each refresh. (A refresh used to bind the port while the previous listener still
/// held it, without address reuse: the bind failed and the screen found no LAN world.)
#[test]
fn lan_worlds_are_found_again_after_each_refresh() {
    rc_sim::lan::announce::LOCAL_ONLY.store(true, Ordering::Relaxed);
    let motd = format!("LAN world of test {}", std::process::id());
    let announcement = rc_sim::lan::announce::ping_string(&motd, 25565);
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let announcer = std::thread::spawn(move || {
        let socket = UdpSocket::bind(("127.0.0.1", 0)).expect("announcer socket");
        while !stopped.load(Ordering::Relaxed) {
            let _ = socket.send_to(announcement.as_bytes(), ("127.0.0.1", rc_sim::lan::announce::PORT));
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let mut screen = Multiplayer::open(scratch("lan"), ALL_ON);
    if rc_testdata::need(screen.lan.as_ref(), "LAN listener on 127.0.0.1:4445").is_some() {
        for refresh in 0..3 {
            assert!(screen.lan.is_some(), "after {refresh} refreshes: not listening");
            assert!(screen.lan_worlds.is_empty(), "a refresh starts from an empty list");
            wait_until("LAN world found", || {
                screen.poll();
                screen.lan_worlds.iter().any(|(found, address)| *found == motd && address == "127.0.0.1:25565")
            });
            screen.refresh();
        }
    }
    stop.store(true, Ordering::Relaxed);
    announcer.join().expect("announcer");
}

// --- Whole screen, on the GPU interface ---

const WINDOW: (u32, u32) = (1600, 900);

fn interface(env: &tests_opt::Env) -> Gui {
    let mut gui = tests_opt::gui(env, tests_opt::ALL);
    // Scale 2: ten rows of the list on screen.
    gui.scale_option = 2;
    gui
}

/// The screen drawn three ways over 36 frames: the code in service before (kept in
/// `tests/reference.rs`), the new code with every switch at 0, and with every switch on. Four
/// servers (icon and players, icon without players, failure, incompatible) and two LAN worlds;
/// answers at frame 2, tooltips of the status picture and of the players, a hovered row, the two
/// rows with an icon swapped, an entry edited and queried again (another icon), a language
/// change, a selection. On every frame: layers equal to the byte and same icon slots; at the end,
/// same `servers.dat`.
#[test]
fn screen_draws_identical_layers() {
    let Some(env) = tests_opt::env() else { return };
    let (icon_a, icon_b, icon_c) = (png_icon(64, 11), png_icon(64, 12), png_icon(64, 13));
    let mut known = ServerEntry::new("Known icon §asurvival", "known.example");
    known.icon = Some(icon_a.clone());
    let entries = vec![
        known,
        ServerEntry::new("No player list", "quiet.example"),
        ServerEntry::new("Unreachable", "down.example:25570"),
        ServerEntry::new("Old version", "old.example"),
    ];
    let names = ["reference", "switches at 0", "switches on"];
    let mut screens = Vec::new();
    for (name, switches) in names.iter().zip([ALL_OFF, ALL_OFF, ALL_ON]) {
        let dir = scratch(&format!("draw-{}", name.replace(' ', "-")));
        write_list(&dir, entries.clone());
        let (mut screen, unanswered) = silent_screen(&dir, switches);
        screen.lan_worlds = vec![
            ("A LAN world".into(), "192.168.1.20:51234".into()),
            ("§bAnother".into(), "192.168.1.21:25565".into()),
        ];
        screens.push((dir, screen, unanswered, interface(&env)));
    }
    let answers = |screen: &Multiplayer| {
        let full = Reply {
            motd: two_line_motd(),
            players: Some((12, 100, &["Zoé", "Anonymous Player"])),
            version: "26.3",
            protocol: 777,
            favicon: Some(&icon_a),
        };
        deliver(screen, 1, full.answer(42));
        let quiet =
            Reply { motd: "§cQuiet".into(), players: None, version: "26.3", protocol: 777, favicon: Some(&icon_b) };
        deliver(screen, 2, quiet.answer(700));
        deliver(screen, 3, Err(StatusError::Io(std::io::ErrorKind::ConnectionRefused.into())));
        let old = Reply {
            motd: "Legacy lobby".into(),
            players: Some((3, 50, &[])),
            version: "1.8.9",
            protocol: 47,
            favicon: None,
        };
        deliver(screen, 4, old.answer(250));
    };
    let options = Options::default();
    let base = env.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
    let (mut frames_changed, mut previous) = (0, None);
    let mut decodings = [0usize; 3];
    for frame in 0..36u32 {
        // Rectangle of a row, the same in the three screens once laid out.
        let row = |index: usize| screens[0].1.panel.list.as_ref().and_then(|list| list.entry_rect(index));
        let mouse = match frame {
            5..8 => row(0).map(|(left, top, width, _)| (left + width - 2 - 15 + 4, top + 2 + 3)),
            8..11 => row(0).map(|(left, top, width, _)| (left + width - 2 - 15 - 12, top + 2 + 3)),
            11..14 => row(1).map(|(left, top, _, _)| (left + 2 + 6, top + 2 + 20)),
            14..16 => row(2).map(|(left, top, width, _)| (left + width - 2 - 15 + 4, top + 2 + 3)),
            16..18 => row(3).map(|(left, top, width, _)| (left + width - 2 - 15 + 4, top + 2 + 3)),
            _ => None,
        }
        .map(|(x, y)| (x as f32 + 0.5, y as f32 + 0.5));
        let mut layers = Vec::new();
        for (path, (_, screen, _, gui)) in screens.iter_mut().enumerate() {
            match frame {
                2 => answers(screen),
                // The two rows with an icon change places: each slot takes the other's picture.
                18 => screen.swap(0, 1),
                22 => {
                    let edited = screen.rows[1].entry.clone();
                    let mut renamed = edited.clone();
                    renamed.name = "Renamed, queried again".into();
                    screen.replace(&edited, renamed);
                }
                // The edited row was queried again at frame 22: fifth query of the screen. Its
                // answer brings another icon, into a slot that holds the previous one.
                24 => {
                    let again = Reply {
                        motd: "Back".into(),
                        players: Some((1, 2, &["Épée"])),
                        version: "26.3",
                        protocol: 777,
                        favicon: Some(&icon_c),
                    };
                    deliver(screen, 5, again.answer(1200));
                }
                28 => {
                    let french = serde_json::json!({
                        "multiplayer.status.cannot_connect": "Connexion impossible",
                        "multiplayer.status.player_count": "%s sur %s",
                        "lanServer.title": "Monde en LAN",
                    });
                    gui.lang = Arc::new(rc_text::Lang::load(&base, Some(french.to_string().as_bytes()), None));
                }
                31 => {
                    if let Some(list) = &mut screen.panel.list {
                        list.selected = Some(1);
                    }
                }
                _ => {}
            }
            gui.begin(WINDOW.0, WINDOW.1);
            let time = f64::from(frame) * 0.11;
            let before = icon_decodings();
            if path == 0 {
                reference::draw(screen, gui, &env.queue, &options, mouse, time);
            } else {
                screen.draw(gui, &env.queue, &options, mouse, time);
            }
            decodings[path] += icon_decodings() - before;
            layers.push(tests_opt::layer_bytes(gui));
        }
        for path in 1..3 {
            let first = layers[0].iter().zip(&layers[path]).position(|(a, b)| a != b);
            assert!(
                layers[path].len() == layers[0].len() && first.is_none(),
                "frame {frame}, {}: layers differ from the reference (byte {first:?})",
                names[path]
            );
            assert!(screens[path].1.slots == screens[0].1.slots, "frame {frame}, {}: icon slots differ", names[path]);
        }
        frames_changed += usize::from(previous.as_ref() != Some(&layers[0]));
        previous = layers.into_iter().next();
    }
    assert!(frames_changed >= 20, "{frames_changed} frames changed");
    for path in 1..3 {
        assert_eq!(servers_dat(&screens[path].0), servers_dat(&screens[0].0), "{}: servers.dat", names[path]);
    }
    // Slot 0: the known icon, then the received one after the swap. Slot 1: the received one,
    // the known one after the swap, then the one the edited entry received.
    assert_eq!(screens[0].1.slots[..3], [Some(icon_b), Some(icon_c), None]);
    // Two rows with an icon on 34 frames, against one decoding per icon written into a slot.
    assert!(decodings[1] >= 60, "{} decodings with the switch at 0", decodings[1]);
    assert_eq!(decodings[2], 5, "decodings with the switch on");
}

/// An icon that is not a 64 × 64 picture is removed from the entry and the list is saved in the
/// same frame, like the game (it used to stay in `servers.dat`): one write for the two rejected
/// icons, the valid one kept.
#[test]
fn rejected_icons_are_removed_and_saved() {
    let Some(env) = tests_opt::env() else { return };
    let mut gui = interface(&env);
    let dir = scratch("rejected");
    let mut entries: Vec<ServerEntry> =
        (0..3).map(|k| ServerEntry::new(format!("Server {k}"), format!("s{k}.example"))).collect();
    entries[0].icon = Some(png_icon(32, 1));
    entries[1].icon = Some(png_icon(64, 2));
    entries[2].icon = Some(vec![1, 2, 3]);
    write_list(&dir, entries.clone());
    let (mut screen, _unanswered) = silent_screen(&dir, ALL_ON);
    let saves = crate::servers::tests::saves();
    for frame in 0..3 {
        gui.begin(WINDOW.0, WINDOW.1);
        screen.draw(&mut gui, &env.queue, &Options::default(), None, f64::from(frame));
    }
    assert_eq!(crate::servers::tests::saves() - saves, 1, "one write, in the frame that rejects");
    let saved = ServerList::load(&dir);
    let icons: Vec<Option<&Vec<u8>>> = saved.visible.iter().map(|entry| entry.icon.as_ref()).collect();
    assert_eq!(icons, [None, entries[1].icon.as_ref(), None]);
    assert!(screen.rows.iter().zip(&saved.visible).all(|(row, entry)| row.entry == *entry));
}

/// Forty servers with an icon: scrolled to the end, the rows on screen beyond the 32nd show their
/// icon (they used to show the "unknown server" picture), each in the slot of its rank modulo 32.
#[test]
fn rows_beyond_the_icon_slots_show_their_icon() {
    let Some(env) = tests_opt::env() else { return };
    let mut gui = interface(&env);
    let dir = scratch("forty");
    let icons: Vec<Vec<u8>> = (0..40).map(|k| png_icon(64, k + 1)).collect();
    let entries = icons
        .iter()
        .enumerate()
        .map(|(k, icon)| {
            let mut entry = ServerEntry::new(format!("Server {k}"), format!("s{k}.example"));
            entry.icon = Some(icon.clone());
            entry
        })
        .collect();
    write_list(&dir, entries);
    let (mut screen, _unanswered) = silent_screen(&dir, ALL_ON);
    let mut draw = |screen: &mut Multiplayer| {
        gui.begin(WINDOW.0, WINDOW.1);
        screen.draw(&mut gui, &env.queue, &Options::default(), None, 0.0);
    };
    draw(&mut screen);
    let list = screen.panel.list.as_mut().expect("list");
    list.set_scroll(f64::MAX);
    draw(&mut screen);
    let list = screen.panel.list.as_ref().expect("list");
    let on_screen: Vec<usize> = (0..40)
        .filter(|&index| {
            list.entry_rect(index).is_some_and(|(_, top, _, height)| top + height >= list.y && top <= list.y + list.h)
        })
        .collect();
    assert!(
        on_screen.len() >= 5 && on_screen.contains(&39) && on_screen.iter().all(|&index| index >= 28),
        "{on_screen:?}"
    );
    for &index in &on_screen {
        assert_eq!(screen.slots[index % 32].as_ref(), Some(&icons[index]), "row {index}");
    }
}

// --- Micro-benches (loaded machine unless said otherwise) ---

fn microseconds_per_call(calls: u32, mut call: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..calls {
        call();
    }
    start.elapsed().as_secs_f64() * 1e6 / f64::from(calls)
}

/// Micro-bench, per row and per frame: the icon (copied and decoded on every frame against
/// compared with its slot) and the texts (built on every frame against kept), fixed-width
/// metrics standing for the font. Main thread by nature. 3 passes.
/// `cargo test --release -p rc-app bench_server_row -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_server_row() {
    let lang = rc_text::Lang::default();
    let reply = Reply {
        motd: two_line_motd(),
        players: Some((12, 100, &["Zoé"])),
        version: "26.3",
        protocol: 777,
        favicon: None,
    };
    // A regular picture (small PNG) and noise (largest PNG): a real icon lies between the two.
    for (kind, icon) in [("regular icon", png_icon(64, 0)), ("noisy icon", noisy_icon())] {
        let mut entry = ServerEntry::new("A server name", "s.example");
        entry.icon = Some(icon.clone());
        let mut row = Row::new(entry);
        row.take_answer(reply.answer(42));
        let mut slots = vec![None; 32];
        let mut upload = |_: usize, _: &[u8]| true;
        assert_eq!(show_icon_decoding_on_change(&mut row, Some(0), &mut slots, &mut upload), IconShown::InSlot(0));
        for pass in 0..3 {
            let decoded = microseconds_per_call(20_000, || {
                std::hint::black_box(show_icon_decoding_every_frame(&row, Some(0), &mut slots, &mut upload));
            });
            let compared = microseconds_per_call(2_000_000, || {
                std::hint::black_box(show_icon_decoding_on_change(&mut row, Some(0), &mut slots, &mut upload));
            });
            println!(
                "pass {pass}, {kind} ({} bytes), µs per row and per frame: decoded {decoded:.2}, compared with its \
                 slot {compared:.4}",
                icon.len()
            );
        }
    }
    let mut row = Row::new(ServerEntry::new("A server name", "s.example"));
    row.take_answer(reply.answer(42));
    for pass in 0..3 {
        let built = microseconds_per_call(200_000, || {
            std::hint::black_box(RowTexts::new(&Fixed, &lang, &row.entry.name, &row.ping, ROW_W - 4));
        });
        let mut kept: Option<RowTexts> = None;
        let kept_texts = microseconds_per_call(2_000_000, || {
            std::hint::black_box(
                kept.get_or_insert_with(|| RowTexts::new(&Fixed, &lang, &row.entry.name, &row.ping, ROW_W - 4)),
            );
        });
        // What the old code also built for every row and frame, and the new one never does.
        let names_and_tooltip = microseconds_per_call(2_000_000, || {
            std::hint::black_box((format!("dyn/server_icon/{}", 7), format!("server_list/ping_{}", 5)));
            std::hint::black_box(tr(&lang, "multiplayer.status.ping", &["42".to_owned()]));
        });
        // Left as it is: the label of the LAN scan row, built on every frame.
        let scan_row = microseconds_per_call(2_000_000, || {
            let label = chars(&tr(&lang, "lanServer.scanning", &[]));
            std::hint::black_box((rc_text::width(&Fixed, &label), chars("O o o")));
        });
        println!(
            "pass {pass}, µs per row and per frame: texts built {built:.2}, kept {kept_texts:.4}; sprite names and \
             tooltip {names_and_tooltip:.3} (no longer built); LAN scan row label {scan_row:.3} per frame (left as is)"
        );
    }
}

/// A 64 × 64 PNG of noise: nothing to compress, the largest file an icon of this size can be.
fn noisy_icon() -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let pixels: Vec<u8> = (0..64 * 64 * 4)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 64, 64);
    encoder.set_color(png::ColorType::Rgba);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&pixels).expect("png pixels");
    drop(writer);
    bytes
}

/// Micro-bench of the whole screen on the GPU interface (real fonts and translations), twelve
/// servers answered, per frame: the code in service before, the new code with the switches at
/// 0, and on. Without a GPU or an installed game: skipped.
/// `cargo test --release -p rc-app bench_server_list_frame -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_server_list_frame() {
    let Some(env) = tests_opt::env() else { return };
    let icons: Vec<Vec<u8>> = (0..12).map(|k| png_icon(64, k + 1)).collect();
    let entries: Vec<ServerEntry> =
        (0..12).map(|k| ServerEntry::new(format!("Server number {k}"), format!("s{k}.example"))).collect();
    let options = Options::default();
    let mut paths = Vec::new();
    for (name, switches) in [("before", ALL_OFF), ("switches at 0", ALL_OFF), ("switches on", ALL_ON)] {
        let dir = scratch(&format!("bench-{}", name.replace(' ', "-")));
        write_list(&dir, entries.clone());
        let (mut screen, unanswered) = silent_screen(&dir, switches);
        let mut gui = interface(&env);
        gui.scale_option = 1;
        gui.begin(WINDOW.0, WINDOW.1);
        screen.draw(&mut gui, &env.queue, &options, None, 0.0);
        for (k, icon) in icons.iter().enumerate() {
            let reply = Reply {
                motd: two_line_motd(),
                players: Some((12, 100, &["Zoé"])),
                version: "26.3",
                protocol: 777,
                favicon: Some(icon),
            };
            deliver(&screen, k as u64 + 1, reply.answer(42));
        }
        paths.push((name, screen, unanswered, gui));
    }
    for pass in 0..3 {
        for (name, screen, _, gui) in &mut paths {
            let mut frame = 0u32;
            let per_frame = microseconds_per_call(1500, || {
                gui.begin(WINDOW.0, WINDOW.1);
                let time = f64::from(frame) / 60.0;
                if *name == "before" {
                    reference::draw(screen, gui, &env.queue, &options, None, time);
                } else {
                    screen.draw(gui, &env.queue, &options, None, time);
                }
                frame += 1;
            });
            println!("pass {pass}, {name}: {per_frame:.1} µs per frame (12 server rows on screen)");
        }
    }
}

/// Micro-bench of the writes: eight icons received in one frame, one write per icon against one
/// write for all; and one save alone (what a background writer would take off the main thread).
/// `cargo test --release -p rc-app bench_server_list_writes -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_server_list_writes() {
    let entries: Vec<ServerEntry> =
        (0..8).map(|k| ServerEntry::new(format!("Server {k}"), format!("s{k}.example"))).collect();
    let icons: Vec<Vec<u8>> = (0..8).map(|k| png_icon(64, k + 1)).collect();
    for pass in 0..3 {
        let mut timings = Vec::new();
        for (name, switches) in [("one write per icon", ALL_OFF), ("one write", ALL_ON)] {
            let dir = scratch("bench-writes");
            let rounds = 40;
            let mut total = Duration::ZERO;
            let saves = crate::servers::tests::saves();
            for _ in 0..rounds {
                write_list(&dir, entries.clone());
                let (mut screen, _unanswered) = silent_screen(&dir, switches);
                query_all_rows(&mut screen);
                for (k, icon) in icons.iter().enumerate() {
                    let reply =
                        Reply { motd: "m".into(), players: None, version: "26.3", protocol: 777, favicon: Some(icon) };
                    deliver(&screen, k as u64 + 1, reply.answer(20));
                }
                let start = Instant::now();
                screen.poll();
                total += start.elapsed();
            }
            let writes = (crate::servers::tests::saves() - saves) / rounds - 1;
            timings.push(format!("{name}: {:.2} ms, {writes} writes", total.as_secs_f64() * 1e3 / rounds as f64));
        }
        let dir = scratch("bench-one-save");
        let mut list = ServerList { visible: entries.clone(), hidden: Vec::new() };
        for (entry, icon) in list.visible.iter_mut().zip(&icons) {
            entry.icon = Some(icon.clone());
        }
        let one_save = microseconds_per_call(200, || list.save(&dir).expect("save")) / 1e3;
        println!(
            "pass {pass}, frame that receives 8 icons: {}; one save of the list alone: {one_save:.2} ms",
            timings.join("; ")
        );
    }
}

/// Micro-bench of the status threads over ten refreshes: what the main thread pays to open the
/// queries of a screen (five threads started per opening against none), and the threads started.
/// `cargo test --release -p rc-app bench_status_threads -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_status_threads() {
    // The session's threads start with the first query: started here, outside the measure.
    let warm = Pinger::start(true);
    warm.submit(1, "127.0.0.1:1");
    let _ = warm.results.recv_timeout(Duration::from_secs(15));
    for pass in 0..3 {
        for (name, session_threads) in [("five threads per opening", false), ("threads of the session", true)] {
            let started = status_threads_started();
            let per_opening = microseconds_per_call(10, || {
                std::hint::black_box(Pinger::start(session_threads));
            });
            let threads = status_threads_started() - started;
            println!(
                "pass {pass}, {name}: {per_opening:.1} µs per opening on the main thread, {threads} threads started in \
                 10 openings"
            );
        }
    }
}
