// Direct read allowed (tool outside the game, single read at startup); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Capture proxy: the client connects to `--listen`, the proxy relays byte for byte to
//! `--target` and, on the side, decodes the frames to log and record each packet. Only works
//! in offline mode (no encryption to intercept).
//!
//! Usage: `rc-capture [--listen 127.0.0.1:25566] [--target 127.0.0.1:25565]
//!                     [--packets <packets.json>] [--out <folder>] [--no-save] [-v]`

mod catalog;
mod session;

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use catalog::{Catalog, Dir};
use rc_net::FrameDecoder;
use session::Session;

struct Args {
    listen: String,
    target: String,
    packets: PathBuf,
    out: Option<PathBuf>,
    verbose: bool,
}

fn re_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("rustcraft-re")
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        listen: "127.0.0.1:25566".into(),
        target: "127.0.0.1:25565".into(),
        packets: re_dir().join("26.3").join("reports").join("reports").join("packets.json"),
        out: Some(re_dir().join("captures")),
        verbose: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("missing value after {arg}"));
        match arg.as_str() {
            "--listen" => a.listen = value()?,
            "--target" => a.target = value()?,
            "--packets" => a.packets = value()?.into(),
            "--out" => a.out = Some(value()?.into()),
            "--no-save" => a.out = None,
            "-v" | "--verbose" => a.verbose = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(a)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let catalog = match Catalog::load(&args.packets) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("packet catalog: {e}");
            std::process::exit(1);
        }
    };
    let listener = TcpListener::bind(&args.listen).unwrap_or_else(|e| {
        eprintln!("cannot listen on {}: {e}", args.listen);
        std::process::exit(1);
    });
    eprintln!("{} known packets; listening {} → {}", catalog.len(), args.listen, args.target);
    let args = Arc::new(args);
    for client in listener.incoming() {
        let Ok(client) = client else { continue };
        let (catalog, args) = (catalog.clone(), args.clone());
        thread::spawn(move || {
            if let Err(e) = handle(client, &catalog, &args) {
                eprintln!("connection: {e}");
            }
        });
    }
}

fn handle(client: TcpStream, catalog: &Arc<Catalog>, args: &Args) -> std::io::Result<()> {
    let peer = client.peer_addr()?;
    let server = TcpStream::connect(&args.target)?;
    client.set_nodelay(true)?;
    server.set_nodelay(true)?;
    let file = match &args.out {
        Some(dir) => {
            std::fs::create_dir_all(dir)?;
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
            let path = dir.join(format!("{stamp}.rccap"));
            eprintln!("{peer}: recording to {}", path.display());
            Some(std::fs::File::create(path)?)
        }
        None => None,
    };
    let session = session::new_session(file);

    let up = {
        let (src, dst) = (client.try_clone()?, server.try_clone()?);
        let (session, catalog, verbose) = (session.clone(), catalog.clone(), args.verbose);
        thread::spawn(move || pump(src, dst, Dir::ToServer, &session, &catalog, verbose))
    };
    pump(server, client, Dir::ToClient, &session, catalog, args.verbose);
    let _ = up.join();

    let mut s = session.lock().unwrap_or_else(|p| p.into_inner());
    s.flush();
    eprintln!("{peer} finished: {}", s.report(catalog));
    Ok(())
}

/// Relays `src` to `dst`. Each batch read is first decoded (updating the phase and the
/// compression), then forwarded unchanged.
fn pump(mut src: TcpStream, mut dst: TcpStream, dir: Dir, session: &Session, catalog: &Catalog, verbose: bool) {
    let mut decoder = FrameDecoder::new();
    let mut threshold = None;
    let mut decoding = true;
    let mut buf = vec![0u8; 64 * 1024];
    let mut packet = Vec::new();
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if decoding {
            decoder.push(&buf[..n]);
            let mut s = session.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if s.threshold != threshold {
                    threshold = s.threshold;
                    if let Some(t) = threshold {
                        decoder.set_compression(t);
                    }
                }
                match decoder.next_packet(&mut packet) {
                    Ok(true) => {
                        if let Err(e) = s.on_packet(catalog, dir, &packet, verbose) {
                            eprintln!("{}: decoding stopped ({e}), relay kept", dir.arrow());
                            decoding = false;
                            break;
                        }
                    }
                    Ok(false) => break,
                    Err(e) => {
                        eprintln!("{}: unreadable frame ({e}), relay kept", dir.arrow());
                        decoding = false;
                        break;
                    }
                }
            }
        }
        if dst.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = dst.shutdown(Shutdown::Write);
}
