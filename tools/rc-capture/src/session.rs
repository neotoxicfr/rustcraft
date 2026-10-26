//! Tracking of a connection: current phase, compression threshold, recording and statistics.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rc_proto::varint::write_varint;
use rc_proto::{Decode, Reader, VarInt};

use crate::catalog::{Catalog, Dir, Phase, Transition};

/// Header of the capture files.
pub const MAGIC: &[u8; 8] = b"RCCAP\x00\x00\x01";

pub struct Shared {
    pub phase: Phase,
    /// Threshold received from the server; `None` while compression is not enabled.
    pub threshold: Option<i32>,
    /// Count and decompressed bytes per packet.
    stats: HashMap<(Phase, Dir, i32), (u64, u64)>,
    out: Option<BufWriter<File>>,
    start: Instant,
    pub protocol: Option<i32>,
}

pub type Session = Arc<Mutex<Shared>>;

pub fn new_session(out: Option<File>) -> Session {
    let mut out = out.map(BufWriter::new);
    if let Some(w) = &mut out {
        // A write error stops the recording, not the connection.
        if w.write_all(MAGIC).is_err() {
            out = None;
        }
    }
    Arc::new(Mutex::new(Shared {
        phase: Phase::Handshake,
        threshold: None,
        stats: HashMap::new(),
        out,
        start: Instant::now(),
        protocol: None,
    }))
}

fn phase_code(p: Phase) -> u8 {
    match p {
        Phase::Handshake => 0,
        Phase::Status => 1,
        Phase::Login => 2,
        Phase::Configuration => 3,
        Phase::Play => 4,
    }
}

impl Shared {
    /// Handles a complete packet (id + body) received in direction `dir`.
    /// Updates the state BEFORE the bytes are forwarded: the other end therefore cannot
    /// react before the proxy has changed phase.
    pub fn on_packet(&mut self, catalog: &Catalog, dir: Dir, packet: &[u8], verbose: bool) -> Result<(), String> {
        let mut r = Reader::new(packet);
        let id = VarInt::decode(&mut r).map_err(|e| e.to_string())?.0;
        let phase = self.phase;
        let entry = self.stats.entry((phase, dir, id)).or_default();
        entry.0 += 1;
        entry.1 += packet.len() as u64;

        if let Some(w) = &mut self.out {
            let mut rec = Vec::with_capacity(packet.len() + 16);
            rec.extend_from_slice(&(self.start.elapsed().as_nanos() as u64).to_le_bytes());
            rec.push(u8::from(dir == Dir::ToServer));
            rec.push(phase_code(phase));
            write_varint(&mut rec, packet.len() as i32);
            rec.extend_from_slice(packet);
            if w.write_all(&rec).is_err() {
                self.out = None;
            }
        }

        if verbose {
            let name = catalog.name(phase, dir, id).unwrap_or("?");
            eprintln!(
                "{:>10.3} ms {} {:<13} 0x{id:02x} {name} ({} B)",
                self.start.elapsed().as_secs_f64() * 1e3,
                dir.arrow(),
                phase.report_key(),
                packet.len()
            );
        }

        match catalog.transition(phase, dir, id) {
            Some(Transition::Intention) => {
                let protocol = VarInt::decode(&mut r).map_err(|e| e.to_string())?.0;
                let _address: &str = rc_proto::read_str(&mut r, 255).map_err(|e| e.to_string())?;
                let _port = u16::decode(&mut r).map_err(|e| e.to_string())?;
                let intent = VarInt::decode(&mut r).map_err(|e| e.to_string())?.0;
                self.protocol = Some(protocol);
                // 1 = status, 2 = login, 3 = transfer (connection coming from another server).
                self.phase = match intent {
                    1 => Phase::Status,
                    2 | 3 => Phase::Login,
                    other => return Err(format!("unknown intent: {other}")),
                };
            }
            Some(Transition::LoginCompression) => {
                self.threshold = Some(VarInt::decode(&mut r).map_err(|e| e.to_string())?.0);
            }
            Some(Transition::LoginAcknowledged | Transition::ConfigurationAcknowledged) => {
                self.phase = Phase::Configuration;
            }
            Some(Transition::FinishConfigurationToServer) => self.phase = Phase::Play,
            // The change only happens at the client's acknowledgement.
            Some(
                Transition::LoginFinished | Transition::FinishConfigurationToClient | Transition::StartConfiguration,
            )
            | None => {}
        }
        Ok(())
    }

    pub fn flush(&mut self) {
        if let Some(w) = &mut self.out {
            let _ = w.flush();
        }
    }

    pub fn report(&self, catalog: &Catalog) -> String {
        let mut rows: Vec<_> = self.stats.iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1.1));
        let total_bytes: u64 = rows.iter().map(|r| r.1.1).sum();
        let total_count: u64 = rows.iter().map(|r| r.1.0).sum();
        let mut s = format!(
            "{total_count} packets, {:.2} MiB decompressed in {:.1} s (protocol {})\n",
            total_bytes as f64 / 1_048_576.0,
            self.start.elapsed().as_secs_f64(),
            self.protocol.map_or_else(|| "?".into(), |p| p.to_string()),
        );
        for ((phase, dir, id), (count, bytes)) in rows.iter().take(25) {
            let name = catalog.name(*phase, *dir, *id).unwrap_or("?");
            s += &format!(
                "  {} {:<13} {:<40} {count:>7} × {:>10.1} KiB\n",
                dir.arrow(),
                phase.report_key(),
                name,
                *bytes as f64 / 1024.0
            );
        }
        s
    }
}
