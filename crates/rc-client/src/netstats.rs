//! Connection readings for the debug screen (`debug-f3.md` § 3.3 and § 7): packets
//! sent and received, raw bytes received, server brand, tick-rate state, ping replies and
//! tick samples. Everything is read by the app at its own pace; nothing is costly to maintain.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use rc_proto::Decode;
use rc_proto::Reader;
use rc_proto::varint::read_varint;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TickState {
    /// Ticks per second of the current rate (20 by default).
    pub rate: f32,
    pub frozen: bool,
    /// Steps remaining when the world advances step by step.
    pub steps: i32,
}

#[derive(Debug, Default)]
pub struct NetStats {
    pub sent: AtomicU64,
    pub received: AtomicU64,
    /// Bytes read from the socket (compressed, encrypted: as received).
    pub bytes_in: AtomicU64,
    /// Last value of the `minecraft:brand` channel.
    pub brand: Mutex<Option<String>>,
    pub tick: Mutex<Option<TickState>>,
    /// Delays (ms) of the ping replies, measured on arrival, not yet read.
    pub pongs: Mutex<Vec<i64>>,
    /// Tick time samples (ns: full, method, tasks, wait), not yet read.
    pub samples: Mutex<Vec<Vec<i64>>>,
}

/// Monotonic clock in milliseconds (time carried by the ping requests).
pub fn millis() -> i64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as i64
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl NetStats {
    pub fn brand(&self) -> Option<String> {
        lock(&self.brand).clone()
    }

    pub fn tick_state(&self) -> Option<TickState> {
        *lock(&self.tick)
    }

    pub fn take_pongs(&self) -> Vec<i64> {
        std::mem::take(&mut *lock(&self.pongs))
    }

    pub fn take_samples(&self) -> Vec<Vec<i64>> {
        std::mem::take(&mut *lock(&self.samples))
    }

    /// New connection or new configuration: server state forgotten.
    pub(crate) fn reset_server(&self) {
        *lock(&self.tick) = None;
    }

    pub(crate) fn count_in(&self, bytes: usize) {
        self.bytes_in.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Payload packet (configuration or play): only the brand is kept.
    pub(crate) fn custom_payload(&self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let channel = rc_proto::read_str(r, 32_767)?;
        if channel == "minecraft:brand" {
            let brand = rc_proto::read_str(r, 32_767)?.to_owned();
            *lock(&self.brand) = Some(brand);
        }
        Ok(())
    }

    pub(crate) fn ticking_state(&self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let rate = f32::decode(r)?;
        let frozen = bool::decode(r)?;
        let mut t = lock(&self.tick);
        let steps = t.map_or(0, |t| t.steps);
        *t = Some(TickState { rate, frozen, steps });
        Ok(())
    }

    pub(crate) fn ticking_step(&self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let steps = read_varint(r)?;
        let mut t = lock(&self.tick);
        let mut s = t.unwrap_or(TickState { rate: 20.0, frozen: false, steps: 0 });
        s.steps = steps;
        *t = Some(s);
        Ok(())
    }

    pub(crate) fn pong(&self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let t = i64::decode(r)?;
        self.push_pong(t);
        Ok(())
    }

    /// Ping reply carrying the time `t` of the request (clock of `millis`). As in the
    /// game, the delay is measured right away, on the thread that receives the reply, without waiting for
    /// the next frame.
    pub fn push_pong(&self, t: i64) {
        let delay = millis() - t;
        let mut p = lock(&self.pongs);
        if p.len() < 1024 {
            p.push(delay);
        }
    }

    /// Tick time sample produced locally (singleplayer integrated server), as if it
    /// had come from the server: full, method, tasks, wait (ns).
    pub fn push_tick_sample(&self, s: [i64; 4]) {
        let mut v = lock(&self.samples);
        if v.len() < 1024 {
            v.push(s.to_vec());
        }
    }

    /// Debug sample: array of longs then type (0 = tick time).
    pub(crate) fn debug_sample(&self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let n = read_varint(r)?.clamp(0, 64) as usize;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(i64::decode(r)?);
        }
        if read_varint(r)? == 0 {
            let mut s = lock(&self.samples);
            if s.len() < 1024 {
                s.push(v);
            }
        }
        Ok(())
    }
}
