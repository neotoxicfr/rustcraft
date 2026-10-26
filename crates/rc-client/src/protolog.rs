//! Packet log (`RUSTCRAFT_PROTOLOG`): every packet received or sent, on the error
//! output, with its phase, name and size. `1`: all; otherwise a comma-separated list of name
//! fragments (`explode,bundle`): only the packets whose name contains one of them.
//! Disabled, each call costs one boolean test.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use rc_data::{Bound, Phase, VersionData};

/// Filter read once: `None` disabled, empty list = everything.
fn filter() -> Option<&'static [String]> {
    static F: OnceLock<Option<Vec<String>>> = OnceLock::new();
    F.get_or_init(|| {
        let v = rc_data::env_str!("RUSTCRAFT_PROTOLOG")?;
        if v.is_empty() || v == "0" {
            return None;
        }
        Some(if v == "1" { Vec::new() } else { v.split(',').map(|s| s.trim().to_owned()).collect() })
    })
    .as_deref()
}

pub fn enabled() -> bool {
    filter().is_some()
}

/// Packet names per phase and direction, and current phase of the connection (for sends).
pub struct Names {
    table: Vec<(Phase, Bound, i32, String)>,
    phase: AtomicU8,
}

const PHASES: [Phase; 5] = [Phase::Handshake, Phase::Status, Phase::Login, Phase::Configuration, Phase::Play];

fn phase_label(p: Phase) -> &'static str {
    match p {
        Phase::Handshake => "handshake",
        Phase::Status => "status",
        Phase::Login => "login",
        Phase::Configuration => "config",
        Phase::Play => "play",
    }
}

impl Names {
    pub fn new(data: &VersionData) -> Names {
        let table = if enabled() {
            data.packets.iter().map(|p| (p.phase, p.bound, p.id, p.name.clone())).collect()
        } else {
            Vec::new()
        };
        Names { table, phase: AtomicU8::new(0) }
    }

    pub fn set_phase(&self, p: Phase) {
        self.phase.store(PHASES.iter().position(|&x| x == p).unwrap_or(0) as u8, Ordering::Relaxed);
    }

    pub fn phase(&self) -> Phase {
        PHASES[self.phase.load(Ordering::Relaxed) as usize % PHASES.len()]
    }

    fn name(&self, phase: Phase, bound: Bound, id: i32) -> &str {
        self.table
            .iter()
            .find(|t| t.0 == phase && t.1 == bound && t.2 == id)
            .map_or("?", |t| t.3.strip_prefix("minecraft:").unwrap_or(&t.3))
    }

    /// Packet received (`id`, size of the decompressed body; 0 if unknown).
    pub fn received(&self, id: i32, len: usize) {
        self.log(Bound::ToClient, self.phase(), id, len, "");
    }

    /// Packet sent in the current phase.
    pub fn sent(&self, id: i32, len: usize) {
        self.log(Bound::ToServer, self.phase(), id, len, "");
    }

    /// Detail line attached to a packet (decoded content).
    pub fn detail(&self, bound: Bound, id: i32, text: &str) {
        self.log(bound, self.phase(), id, 0, text);
    }

    fn log(&self, bound: Bound, phase: Phase, id: i32, len: usize, text: &str) {
        let Some(f) = filter() else { return };
        let name = self.name(phase, bound, id);
        if !f.is_empty() && !f.iter().any(|w| name.contains(w.as_str())) {
            return;
        }
        let arrow = if bound == Bound::ToClient { "←" } else { "→" };
        if text.is_empty() {
            eprintln!("proto {arrow} {} 0x{id:02X} {name} ({len} B)", phase_label(phase));
        } else {
            eprintln!("proto {arrow} {} 0x{id:02X} {name}: {text}", phase_label(phase));
        }
    }
}
