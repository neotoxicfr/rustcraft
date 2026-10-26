//! Cache of decoded sounds (PCM i16 per path), bounded in bytes (`RUSTCRAFT_OPT_PCMBUDGET`).
//!
//! The game keeps every sound played until the resources reload; a long survival game
//! accumulates ~94 MB of them, of which ~67 MB are sounds of 2 s or more (cave, underwater and
//! desert ambiences, explosions; bench `examples/pcm_census.rs`). Beyond the budget, the least
//! recently played sounds are forgotten, except:
//! - a buffer held elsewhere (mixer voice, sound being loaded: more than one reference);
//! - a short sound (under [`SHORT_MS`]): steps, hits, clicks, mob cries, almost everything
//!   that repeats (zombie, cow, sheep, deepslate last 1 to 2 s);
//! - a frequent sound (repetition score at least [`HOT_SCORE`], half-life [`HALF_LIFE`] ticks);
//! - a `preload` variant (kept like the game).
//!
//! A forgotten sound requested again goes back through the loading threads: PCM identical bit for
//! bit, only its start is delayed by the decoding time (rare: only long, rarely played sounds).
//! The budget only counts to decide what to forget: protected sounds may exceed it.

use std::sync::Arc;

use hashbrown::HashMap;

use crate::decode::Pcm;

/// Cache budget, in bytes of PCM.
pub(crate) const BUDGET: usize = 32 << 20;
/// Sounds shorter than this threshold are never forgotten (ms).
pub(crate) const SHORT_MS: u64 = 2000;
/// Repetition score from which a sound is "frequent" (each playback adds 1).
pub(crate) const HOT_SCORE: f32 = 3.0;
/// Half-life of the repetition score, in client ticks (30 s).
pub(crate) const HALF_LIFE: f32 = 600.0;

struct Entry {
    pcm: Arc<Pcm>,
    bytes: usize,
    /// Rank of the last access (forgetting order: the smallest first).
    seq: u64,
    /// Tick of the last access and repetition score at that tick.
    tick: u64,
    score: f32,
    /// Short or `preload`: never forgotten.
    keep: bool,
}

impl Entry {
    fn score_at(&self, now: u64) -> f32 {
        self.score * 0.5f32.powf(now.saturating_sub(self.tick) as f32 / HALF_LIFE)
    }

    fn touch(&mut self, seq: u64, now: u64) {
        self.score = self.score_at(now) + 1.0;
        self.tick = now;
        self.seq = seq;
    }
}

/// Sounds decoded by path (key shared with the table: lookup by `&str`, no copy).
pub(crate) struct PcmCache {
    map: HashMap<Arc<str>, Entry>,
    /// Bytes of PCM in cache (kept up to date at every entry and exit).
    bytes: usize,
    /// `None`: unbounded (old path, `PCMBUDGET=0`).
    pub(crate) budget: Option<usize>,
    seq: u64,
    /// Sounds forgotten since creation (debug screen, proofs).
    pub(crate) evicted: u64,
}

fn pcm_bytes(p: &Pcm) -> usize {
    p.data.len() * 2
}

fn is_short(p: &Pcm) -> bool {
    (p.frames() as u64).saturating_mul(1000) < SHORT_MS * u64::from(p.rate.max(1))
}

impl PcmCache {
    pub(crate) fn new(budget: Option<usize>) -> Self {
        PcmCache { map: HashMap::new(), bytes: 0, budget, seq: 0, evicted: 0 }
    }

    /// Cached sound, counted as a playback at tick `now`.
    pub(crate) fn get(&mut self, path: &str, now: u64) -> Option<Arc<Pcm>> {
        self.seq += 1;
        let seq = self.seq;
        let e = self.map.get_mut(path)?;
        e.touch(seq, now);
        Some(e.pcm.clone())
    }

    /// Adds (or replaces) a decoded sound, counted as a playback, then forgets what exceeds the
    /// budget. `pin`: `preload` variant, never forgotten.
    pub(crate) fn put(&mut self, key: Arc<str>, pcm: Arc<Pcm>, now: u64, pin: bool) {
        self.seq += 1;
        let bytes = pcm_bytes(&pcm);
        let keep = pin || is_short(&pcm);
        let e = Entry { pcm, bytes, seq: self.seq, tick: now, score: 1.0, keep };
        if let Some(old) = self.map.insert(key.clone(), e) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.evict(now, &key);
    }

    /// Direct insertion (tests): not pinned, at tick 0.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, key: Arc<str>, pcm: Arc<Pcm>) {
        self.put(key, pcm, 0, false);
    }

    fn evict(&mut self, now: u64, fresh: &str) {
        let Some(budget) = self.budget else { return };
        if self.bytes <= budget {
            return;
        }
        // Candidates: neither protected, nor held elsewhere, nor the sound that just came in.
        // Forgetting order: lowest repetition score first (a replayed sound comes after
        // sounds played once), then the least recent.
        let mut cand: Vec<(f32, u64, Arc<str>)> = self
            .map
            .iter()
            .filter(|(k, e)| !e.keep && &***k != fresh && Arc::strong_count(&e.pcm) == 1)
            .map(|(k, e)| (e.score_at(now), e.seq, k.clone()))
            .filter(|c| c.0 < HOT_SCORE)
            .collect();
        cand.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, _, k) in cand {
            if self.bytes <= budget {
                break;
            }
            if let Some(e) = self.map.remove(&k) {
                self.bytes -= e.bytes;
                self.evicted += 1;
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, path: &str) -> bool {
        self.map.contains_key(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(seconds: f32, k: usize) -> Arc<Pcm> {
        let frames = (seconds * 1000.0) as usize;
        let data = (0..frames).map(|i| ((i * k) % 3000) as i16 - 1500).collect();
        Arc::new(Pcm { channels: 1, rate: 1000, data })
    }

    fn recount(c: &PcmCache) -> usize {
        c.map.values().map(|e| pcm_bytes(&e.pcm)).sum()
    }

    /// Forgetting order: least recently played first; budget respected as soon as a candidate exists;
    /// byte counter equal to the recount.
    #[test]
    fn evicts_least_recent_long_sound() {
        let mut c = PcmCache::new(Some(3 * 4000));
        c.put("a".into(), pcm(2.0, 3), 0, false);
        c.put("b".into(), pcm(2.0, 5), 1000, false);
        c.put("c".into(), pcm(2.0, 7), 2000, false);
        assert!(c.get("a", 3000).is_some());
        c.put("d".into(), pcm(2.0, 11), 4000, false);
        assert!(!c.contains("b"), "b, the least recent, forgotten");
        assert!(c.contains("a") && c.contains("c") && c.contains("d"));
        assert_eq!(c.bytes(), recount(&c));
        assert!(c.bytes() <= 3 * 4000);
        assert_eq!(c.evicted, 1);
    }

    /// Never forgotten: buffer held by a voice, short sound, frequent sound, `preload` variant.
    #[test]
    fn protected_sounds_stay() {
        let mut c = PcmCache::new(Some(1));
        c.put("voice".into(), pcm(2.0, 3), 0, false);
        let held = c.get("voice", 0).expect("in cache");
        c.put("short".into(), pcm(0.5, 5), 0, false);
        c.put("pin".into(), pcm(3.0, 7), 0, true);
        c.put("hot".into(), pcm(2.0, 9), 0, false);
        for t in 1..4 {
            c.get("hot", t * 20);
        }
        c.put("x".into(), pcm(2.0, 13), 100, false);
        c.put("y".into(), pcm(2.0, 17), 110, false);
        assert!(c.contains("voice"), "buffer held by a voice forgotten");
        assert!(c.contains("short") && c.contains("pin") && c.contains("hot"));
        assert!(!c.contains("x"), "x forgotten when y came in");
        drop(held);
        // Voice finished, score dropped (10 half-lives): candidate again, like "hot".
        c.put("z".into(), pcm(2.0, 19), 100 + 6000, false);
        assert!(!c.contains("voice") && !c.contains("hot") && !c.contains("y"));
        assert!(c.contains("short") && c.contains("pin") && c.contains("z"));
        assert_eq!(c.bytes(), recount(&c));
    }

    /// Old path (`PCMBUDGET=0`): nothing is ever forgotten.
    #[test]
    fn unbounded_keeps_everything() {
        let mut c = PcmCache::new(None);
        for i in 0..50 {
            c.put(format!("s{i}").into(), pcm(5.0, i + 1), i as u64, false);
        }
        assert_eq!((c.len(), c.evicted), (50, 0));
        assert_eq!(c.bytes(), recount(&c));
    }
}
