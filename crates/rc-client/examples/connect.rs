//! Connects to the local server, receives the world for a few seconds, and measures.
//! `cargo run -p rc-client --example connect --release -- [host] [port] [seconds] [name]`
//! (distinct name per guest for the bench's `lan` scene, `tools/bench/scenes.py`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_client::{Config, Event};

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let mut args = std::env::args().skip(1);
    let host = args.next().unwrap_or_else(|| "127.0.0.1".into());
    let port = args.next().and_then(|p| p.parse().ok()).unwrap_or(25565);
    let secs = args.next().and_then(|s| s.parse().ok()).unwrap_or(5u64);
    let name = args.next().unwrap_or_else(|| "Rustcraft".into());
    let t0 = Instant::now();
    let data = Arc::new(rc_data::load_or_prepare("26.3", |s| eprintln!("{s}")).expect("data"));
    let cfg = Config { host, port, name, ..Default::default() };
    let session = rc_client::connect(cfg, data).expect("connection");
    let (mut chunks, mut sections, mut first_chunk, mut joined) = (0usize, 0usize, None, None);
    let deadline = Instant::now() + Duration::from_secs(secs);
    while let Ok(ev) = session.events.recv_deadline(deadline) {
        match ev {
            Event::Joined { login, height, .. } => {
                joined = Some(t0.elapsed());
                println!(
                    "in game ({:.0} ms): {} , height {:?}, view {}",
                    t0.elapsed().as_secs_f64() * 1e3,
                    login.spawn.dimension,
                    height,
                    login.view_distance
                );
            }
            Event::Chunk(c) => {
                first_chunk.get_or_insert_with(|| t0.elapsed());
                chunks += 1;
                sections += c.sections.iter().filter(|s| !s.is_empty()).count();
            }
            Event::Teleport(p) => println!("teleport {:?}", p.pos),
            Event::Disconnected(r) => {
                println!("end: {r}");
                break;
            }
            _ => {}
        }
    }
    println!(
        "{chunks} chunks received ({sections} non-empty sections) in {secs} s; entered game {:?}, first chunk {:?}",
        joined, first_chunk
    );
}
