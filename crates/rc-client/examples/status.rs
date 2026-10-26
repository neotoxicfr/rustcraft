//! Queries the status of a server like the server list does.
//! `cargo run -p rc-client --example status --release -- [typed address]` (default 127.0.0.1:25570)

use std::time::Duration;

use rc_client::status::{self, PingOutcome, ServerAddress};

fn main() {
    let input = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:25570".into());
    let typed = ServerAddress::parse(&input);
    println!("typed: {input:?} → host {:?}, port {}", typed.host, typed.port);
    match status::resolve(&typed) {
        Some(r) => println!("resolved: announced host {:?}, port {}, sockets {:?}", r.host, r.port, r.addrs),
        None => println!("resolved: unknown host"),
    }
    match status::ping(&input, Duration::from_secs(5)) {
        Ok(PingOutcome::Modern(p)) => {
            let s = &p.status;
            println!("version: {:?}; compatible: {}", s.version, p.compatible);
            println!("latency: {} ms", p.latency_ms);
            match &s.players {
                Some(pl) => {
                    println!("players: {}/{}", pl.online, pl.max);
                    for j in &pl.sample {
                        println!("  - {} ({:032x})", j.name, j.id);
                    }
                }
                None => println!("players: ???"),
            }
            println!("raw description: {}", s.description);
            println!("text description: {:?}", status::plain_text(&s.description));
            match &s.favicon {
                Some(f) => println!(
                    "icon: {} bytes, {}×{}, displayable {}, RGBA {:?} bytes",
                    f.png.len(),
                    f.width,
                    f.height,
                    f.is_displayable(),
                    f.rgba().map(|v| v.len())
                ),
                None => println!("icon: none"),
            }
            println!("secure chat enforced: {}", s.enforces_secure_chat);
        }
        Ok(PingOutcome::Legacy(l)) => println!("legacy ping: {l:?}"),
        Err(e) => println!("failure: {e}"),
    }
}
