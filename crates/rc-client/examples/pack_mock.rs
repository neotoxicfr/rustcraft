//! Fake local server for the server's resource packs (`docs/specs/resource-packs.md`
//! § 5): drives the connection up to the configuration (offline, no compression), pushes
//! packs and prints the `resource_pack` states received. A single client (the game).
//!
//! `cargo run -p rc-client --example pack_mock --release -- <port> <scenario> <url> <sha1>`
//! Scenarios: `optional` (prompt, server message), `required` (required pack),
//! `optional-pop` (optional then removal), `badurl` (URL refused), `badhash` (wrong fingerprint).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use rc_client::ids::Ids;
use rc_net::{FrameDecoder, FrameEncoder};
use rc_proto::Reader;
use rc_proto::nbt::Nbt;
use rc_proto::varint::{read_varint, write_varint};

fn read_packet(s: &mut TcpStream, dec: &mut FrameDecoder) -> Result<Vec<u8>, String> {
    let mut packet = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if dec.next_packet(&mut packet).map_err(|e| e.to_string())? {
            return Ok(packet);
        }
        let n = s.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("client gone".into());
        }
        dec.push(&buf[..n]);
    }
}

fn send(s: &mut TcpStream, enc: &mut FrameEncoder, id: i32, body: &[u8]) {
    let mut p = Vec::new();
    write_varint(&mut p, id);
    p.extend_from_slice(body);
    let mut wire = Vec::new();
    enc.encode(&p, &mut wire).expect("encoding");
    s.write_all(&wire).expect("write");
}

fn push(id: u128, url: &str, hash: &str, required: bool, prompt: Option<&str>) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&id.to_be_bytes());
    rc_proto::write_str(&mut b, url);
    rc_proto::write_str(&mut b, hash);
    b.push(u8::from(required));
    b.push(u8::from(prompt.is_some()));
    if let Some(p) = prompt {
        rc_proto::nbt::write(&mut b, Some(&Nbt::String(p.into())));
    }
    b
}

const NAMES: [&str; 8] = [
    "loaded",
    "declined",
    "download failed",
    "accepted",
    "downloaded",
    "invalid URL",
    "reload failed",
    "discarded",
];

/// Reads the client's packets up to a terminal state for `id` (or the end); returns the states.
fn statuses(s: &mut TcpStream, dec: &mut FrameDecoder, ids: &Ids, id: u128, t0: Instant) -> Vec<i32> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    s.set_read_timeout(Some(Duration::from_secs(60))).ok();
    while Instant::now() < deadline {
        let p = match read_packet(s, dec) {
            Ok(p) => p,
            Err(e) => {
                println!("[{:5.1} s] server: {e}", t0.elapsed().as_secs_f32());
                break;
            }
        };
        let mut r = Reader::new(&p);
        let Ok(pid) = read_varint(&mut r) else { continue };
        if pid != ids.cfg_resource_pack_out {
            continue;
        }
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(r.take(16).expect("uuid"));
        let st = read_varint(&mut r).expect("state");
        let who = u128::from_be_bytes(uuid);
        println!(
            "[{:5.1} s] server: resource_pack {who:032x} → {st} ({})",
            t0.elapsed().as_secs_f32(),
            NAMES.get(st as usize).unwrap_or(&"?")
        );
        if who == id {
            out.push(st);
            if !matches!(st, 3 | 4) {
                break;
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [port, scenario, url, hash, ..] = args.as_slice() else {
        eprintln!("usage: pack_mock <port> <scenario> <url> <sha1>");
        std::process::exit(2);
    };
    let data = rc_data::load_or_prepare("26.3", |s| eprintln!("{s}")).expect("data");
    let ids = Ids::resolve(&data).expect("identifiers");
    let listener = TcpListener::bind(("127.0.0.1", port.parse::<u16>().expect("port"))).expect("listen");
    println!("server: listening on {port}, scenario {scenario}");
    // Status requests (server list, intent 1) are closed without a reply.
    let (mut s, mut dec) = loop {
        let (mut s, _) = listener.accept().expect("client");
        let mut dec = FrameDecoder::new();
        let Ok(hs) = read_packet(&mut s, &mut dec) else { continue };
        let mut r = Reader::new(&hs);
        let _ = read_varint(&mut r);
        let _protocol = read_varint(&mut r);
        let _host = rc_proto::read_str(&mut r, 255);
        let _port = r.take(2);
        if read_varint(&mut r).ok() == Some(1) {
            println!("server: status request ignored");
            continue;
        }
        break (s, dec);
    };
    let t0 = Instant::now();
    let mut enc = FrameEncoder::new();
    read_packet(&mut s, &mut dec).expect("hello");
    let mut fin = Vec::new();
    fin.extend_from_slice(&0x1234u128.to_be_bytes());
    rc_proto::write_str(&mut fin, "AgentPacks");
    write_varint(&mut fin, 0);
    send(&mut s, &mut enc, ids.login_finished, &fin);
    // Acknowledgement, brand, client information.
    let ack = read_packet(&mut s, &mut dec).expect("acknowledgement");
    assert_eq!(read_varint(&mut Reader::new(&ack)).ok(), Some(ids.login_acknowledged));
    println!("[{:5.1} s] server: configuration", t0.elapsed().as_secs_f32());
    std::thread::sleep(Duration::from_millis(500));
    let a = 0xA11C_E000_0000_0000_0000_0000_0000_0001u128;
    let b = 0xB0B0_0000_0000_0000_0000_0000_0000_0002u128;
    let result = match scenario.as_str() {
        "optional" | "optional-pop" => {
            send(
                &mut s,
                &mut enc,
                ids.cfg_resource_pack_push,
                &push(a, url, hash, false, Some("Rustcraft test pack")),
            );
            let st = statuses(&mut s, &mut dec, &ids, a, t0);
            if scenario == "optional-pop" && st.last() == Some(&0) {
                std::thread::sleep(Duration::from_secs(4));
                println!("[{:5.1} s] server: resource_pack_pop (all)", t0.elapsed().as_secs_f32());
                send(&mut s, &mut enc, ids.cfg_resource_pack_pop, &[0]);
            }
            st
        }
        "required" => {
            send(&mut s, &mut enc, ids.cfg_resource_pack_push, &push(b, url, hash, true, None));
            statuses(&mut s, &mut dec, &ids, b, t0)
        }
        "badurl" => {
            send(&mut s, &mut enc, ids.cfg_resource_pack_push, &push(a, "ftp://127.0.0.1/x.zip", hash, false, None));
            statuses(&mut s, &mut dec, &ids, a, t0)
        }
        "badhash" => {
            let wrong = "0".repeat(40);
            send(&mut s, &mut enc, ids.cfg_resource_pack_push, &push(a, url, &wrong, false, None));
            statuses(&mut s, &mut dec, &ids, a, t0)
        }
        other => panic!("unknown scenario: {other}"),
    };
    println!("[{:5.1} s] server: states received {result:?}", t0.elapsed().as_secs_f32());
    // Lets the client display the current screen, then closes.
    std::thread::sleep(Duration::from_secs(8));
}
