//! Putting events back in order: the network thread numbers each event in packet order;
//! heavy decodings finish on the pool out of order; the
//! sequencer delivers event n only once all the earlier ones have been delivered.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crossbeam_channel::Sender;

pub struct Sequencer<T> {
    state: Mutex<(u64, BTreeMap<u64, T>)>,
    out: Sender<T>,
    /// Final event (disconnection): nothing is delivered after it.
    is_final: fn(&T) -> bool,
}

impl<T> Sequencer<T> {
    pub fn new(out: Sender<T>, is_final: fn(&T) -> bool) -> Self {
        Sequencer { state: Mutex::new((0, BTreeMap::new())), out, is_final }
    }

    /// Deposits event `seq`; delivers all the consecutive events available.
    pub fn complete(&self, seq: u64, item: T) {
        let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let (next, pending) = &mut *guard;
        if *next == u64::MAX {
            return;
        }
        if seq != *next {
            pending.insert(seq, item);
            return;
        }
        let mut item = Some(item);
        while let Some(it) = item {
            let last = (self.is_final)(&it);
            let _ = self.out.send(it);
            if last {
                *next = u64::MAX;
                pending.clear();
                return;
            }
            *next += 1;
            item = pending.remove(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivers_in_order() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let s = Sequencer::new(tx, |_| false);
        s.complete(2, 'c');
        s.complete(1, 'b');
        assert!(rx.try_recv().is_err());
        s.complete(0, 'a');
        s.complete(3, 'd');
        assert_eq!(rx.try_iter().collect::<String>(), "abcd");
    }

    #[test]
    fn nothing_after_final() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let s = Sequencer::new(tx, |&c| c == 'x');
        s.complete(2, 'c');
        s.complete(1, 'x');
        s.complete(0, 'a');
        s.complete(3, 'd');
        assert_eq!(rx.try_iter().collect::<String>(), "ax");
    }
}
