//! Announcement of the world opened to the local network: datagram `[MOTD]…[/MOTD][AD]port[/AD]` to the
//! multicast group 224.0.2.60:4445 every 1.5 s, like the game.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 2, 60);
pub const PORT: u16 = 4445;

/// Listening limited to the machine (tests).
pub static LOCAL_ONLY: AtomicBool = AtomicBool::new(false);

/// Announced text.
pub fn ping_string(motd: &str, port: u16) -> String {
    format!("[MOTD]{motd}[/MOTD][AD]{port}[/AD]")
}

/// Listening address: all interfaces like the game; `RUSTCRAFT_LAN_BIND=127.0.0.1`
/// limits it to the machine (trials), like `LOCAL_ONLY` (tests).
pub fn bind_addr() -> Ipv4Addr {
    if LOCAL_ONLY.load(Ordering::Relaxed) {
        return Ipv4Addr::LOCALHOST;
    }
    rc_data::env_str!("RUSTCRAFT_LAN_BIND").and_then(|s| s.parse().ok()).unwrap_or(Ipv4Addr::UNSPECIFIED)
}

pub fn start(motd: String, port: u16, stop: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new().name("lan-announce".into()).spawn(move || {
        let Ok(sock) = UdpSocket::bind((bind_addr(), 0)) else { return };
        let _ = sock.set_multicast_ttl_v4(1);
        let msg = ping_string(&motd, port);
        // Limited to the machine: direct send to the listening port of local clients (they listen on
        // port 4445 on all interfaces), nothing leaves on the network.
        let group = if bind_addr().is_loopback() { Ipv4Addr::LOCALHOST } else { GROUP };
        let to = SocketAddrV4::new(group, PORT);
        let mut logged = false;
        while !stop.load(Ordering::Acquire) {
            if let Err(e) = sock.send_to(msg.as_bytes(), to)
                && !std::mem::replace(&mut logged, true)
            {
                eprintln!("lan: announcement: {e}");
            }
            for _ in 0..15 {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    });
}
