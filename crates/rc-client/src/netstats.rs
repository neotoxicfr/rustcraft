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
    /// Tick time samples (ns: full, method, tasks, wait), not yet read. Fixed-size entries: the
    /// graph only reads these four values, and no sample needs an allocation of its own.
    pub samples: Mutex<Vec<TickSample>>,
}

/// Tick time sample: full, method, tasks, wait (ns).
pub type TickSample = [i64; 4];

/// Samples kept until the graph reads them (it does every client tick).
const SAMPLES_MAX: usize = 1024;

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

    pub fn take_samples(&self) -> Vec<TickSample> {
        std::mem::take(&mut *lock(&self.samples))
    }

    fn push_sample(&self, s: TickSample) {
        let mut v = lock(&self.samples);
        if v.len() < SAMPLES_MAX {
            v.push(s);
        }
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
    pub fn push_tick_sample(&self, s: TickSample) {
        self.push_sample(s);
    }

    /// Debug sample: array of longs then type (0 = tick time). Only its first four values are
    /// kept: the graph ignores a shorter sample and reads nothing beyond the fourth.
    pub(crate) fn debug_sample(&self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let n = read_varint(r)?.clamp(0, 64) as usize;
        let mut first = [0i64; 4];
        for i in 0..n {
            let v = i64::decode(r)?;
            if let Some(slot) = first.get_mut(i) {
                *slot = v;
            }
        }
        if read_varint(r)? == 0 && n >= first.len() {
            self.push_sample(first);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_proto::Encode;
    use rc_proto::varint::write_varint;

    fn debug_sample(values: &[i64], kind: i32) -> Vec<u8> {
        let mut w = Vec::new();
        write_varint(&mut w, values.len() as i32);
        values.iter().for_each(|v| v.encode(&mut w));
        write_varint(&mut w, kind);
        w
    }

    /// What the frame-time graph (F3+2) reads, written out: the integrated server's samples,
    /// then the remote server's with 3, 4 and 6 values and one of another type. The graph only
    /// draws samples of at least four values, from their first four.
    #[test]
    fn frame_time_graph_reads_the_same_samples() {
        let s = NetStats::default();
        s.push_tick_sample([1, 2, 3, 4]);
        for (values, kind) in
            [(&[5, 6, 7][..], 0), (&[8, 9, 10, 11], 0), (&[12, 13, 14, 15, 16, 17], 0), (&[20, 21, 22, 23], 1)]
        {
            let packet = debug_sample(values, kind);
            let mut r = Reader::new(&packet);
            s.debug_sample(&mut r).unwrap();
            assert!(r.is_empty(), "the whole packet is read");
        }
        assert_eq!(s.take_samples(), vec![[1, 2, 3, 4], [8, 9, 10, 11], [12, 13, 14, 15]]);
        assert!(s.take_samples().is_empty());
        // At most 1024 samples wait for the graph; the newest ones are dropped.
        for i in 0..1030 {
            s.push_tick_sample([i, 0, 0, 0]);
        }
        let kept = s.take_samples();
        assert_eq!((kept.len(), kept[0][0], kept[1023][0]), (1024, 0, 1023));
    }
}
