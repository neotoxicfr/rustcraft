//! Iteration order of the game's open-addressing hash table with linear probing (tables of
//! objectives, score holders and teams): the game lists them in the order of their
//! slots, from the last to the first. The table is replayed key by key (insertions, removals
//! with backward shift of the following keys, growth and shrinking), only the keys are kept.

/// Hash code mixing (golden ratio constant, upper half folded).
fn mix(x: i32) -> i32 {
    let h = x.wrapping_mul(0x9E37_79B9_u32 as i32);
    h ^ ((h as u32) >> 16) as i32
}

/// Hash code of a string (UTF-16 units, h = 31·h + c).
pub fn string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
}

/// Table size: power of two ≥ ⌈expected / load⌉, at least 2.
fn array_size(expected: usize, f: f32) -> usize {
    ((f64::from(expected as u32) / f64::from(f)).ceil() as usize).next_power_of_two().max(2)
}

/// Maximum fill before growth: min(⌈n·load⌉, n − 1).
fn max_fill(n: usize, f: f32) -> usize {
    ((n as f64 * f64::from(f)).ceil() as usize).min(n - 1)
}

#[derive(Debug, Clone)]
pub struct HashOrder {
    keys: Vec<Option<String>>,
    size: usize,
    max_fill: usize,
    f: f32,
    min_n: usize,
}

impl HashOrder {
    /// Empty table sized for `expected` keys at load `f` (16 and 0.75 by default in the game).
    pub fn new(expected: usize, f: f32) -> HashOrder {
        let n = array_size(expected, f);
        HashOrder { keys: vec![None; n], size: 0, max_fill: max_fill(n, f), f, min_n: n }
    }

    fn mask(&self) -> usize {
        self.keys.len() - 1
    }

    fn home(&self, k: &str) -> usize {
        mix(string_hash(k)) as u32 as usize & self.mask()
    }

    fn find(&self, k: &str) -> Result<usize, usize> {
        let mut pos = self.home(k);
        loop {
            match &self.keys[pos] {
                None => return Err(pos),
                Some(x) if x == k => return Ok(pos),
                Some(_) => pos = (pos + 1) & self.mask(),
            }
        }
    }

    pub fn insert(&mut self, k: &str) {
        let Err(pos) = self.find(k) else { return };
        self.keys[pos] = Some(k.to_owned());
        let before = self.size;
        self.size += 1;
        if before >= self.max_fill {
            self.rehash(array_size(self.size + 1, self.f));
        }
    }

    pub fn remove(&mut self, k: &str) {
        let Ok(pos) = self.find(k) else { return };
        self.size -= 1;
        self.shift_keys(pos);
        let n = self.keys.len();
        if n > self.min_n && self.size < self.max_fill / 4 && n > 16 {
            self.rehash(n / 2);
        }
    }

    /// Removal by shifting back the following keys of the same group.
    fn shift_keys(&mut self, mut pos: usize) {
        let mask = self.mask();
        loop {
            let last = pos;
            pos = (last + 1) & mask;
            loop {
                let Some(cur) = &self.keys[pos] else {
                    self.keys[last] = None;
                    return;
                };
                let slot = self.home(cur);
                let stop = if last <= pos { last >= slot || slot > pos } else { last >= slot && slot > pos };
                if stop {
                    break;
                }
                pos = (pos + 1) & mask;
            }
            self.keys[last] = self.keys[pos].take();
        }
    }

    /// New table: keys taken from the last slot to the first.
    fn rehash(&mut self, new_n: usize) {
        let mask = new_n - 1;
        let mut fresh: Vec<Option<String>> = vec![None; new_n];
        for k in self.keys.iter_mut().rev().filter_map(Option::take) {
            let mut pos = mix(string_hash(&k)) as u32 as usize & mask;
            while fresh[pos].is_some() {
                pos = (pos + 1) & mask;
            }
            fresh[pos] = Some(k);
        }
        self.keys = fresh;
        self.max_fill = max_fill(new_n, self.f);
    }

    /// Keys in the order of a direct walk of the table (loop over the collection): from the
    /// last slot to the first.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().rev().filter_map(Option::as_deref)
    }

    /// Keys in the order of a stream walk (`stream`): from the first slot to the last,
    /// the reverse of the direct walk.
    pub fn iter_stream(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().filter_map(Option::as_deref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_string_hash() {
        assert_eq!(string_hash(""), 0);
        assert_eq!(string_hash("Alex"), 2_043_454);
        assert_eq!(string_hash("hello"), 99_162_322);
    }

    /// Orders recorded from the game's table (server 26.3 library, same sequence of operations).
    #[test]
    fn same_order_as_the_game_table() {
        let show = |h: &HashOrder| h.iter().collect::<Vec<_>>().join(",");
        let mut h = HashOrder::new(16, 0.5);
        let names = [
            "Steve",
            "Alex",
            "#fake",
            "Notch",
            "jeb_",
            "Dinnerbone",
            "Grumm",
            "a",
            "b",
            "c",
            "d",
            "e",
            "f",
            "g",
            "h",
            "i",
            "j",
            "k",
            "l",
        ];
        for n in names {
            h.insert(n);
        }
        assert_eq!(show(&h), "c,g,k,Alex,d,b,f,l,jeb_,h,Grumm,j,i,e,a,#fake,Steve,Dinnerbone,Notch");
        for n in ["Alex", "c", "g"] {
            h.remove(n);
        }
        assert_eq!(show(&h), "k,d,b,f,l,jeb_,h,Grumm,j,i,e,a,#fake,Steve,Dinnerbone,Notch");
        h.insert("Alex");
        assert_eq!(show(&h), "k,Alex,d,b,f,l,jeb_,h,Grumm,j,i,e,a,#fake,Steve,Dinnerbone,Notch");
        for i in 0..40 {
            h.insert(&format!("p{i}"));
        }
        for i in 0..40 {
            h.remove(&format!("p{i}"));
        }
        assert_eq!(show(&h), "k,Alex,d,b,l,j,#fake,Steve,f,jeb_,Grumm,i,h,e,a,Dinnerbone,Notch");
        let mut t = HashOrder::new(16, 0.75);
        for n in ["red", "blue", "green", "yellow", "Équipe"] {
            t.insert(n);
        }
        assert_eq!(show(&t), "yellow,blue,green,Équipe,red");
        // Recorded from the official server: objectives "o" (read from disk), then "t", "u".
        let mut o = HashOrder::new(16, 0.5);
        for n in ["o", "t", "u"] {
            o.insert(n);
        }
        assert_eq!(show(&o), "o,u,t");
        assert_eq!(o.iter_stream().collect::<Vec<_>>().join(","), "t,u,o");
        // Official `team list`: [yellow], [blue], [green], [red].
        let mut t = HashOrder::new(16, 0.75);
        for n in ["red", "blue", "green", "yellow"] {
            t.insert(n);
        }
        assert_eq!(show(&t), "yellow,blue,green,red");
    }

    #[test]
    fn order_follows_slots_from_the_end_and_survives_growth_and_removal() {
        let mut h = HashOrder::new(16, 0.5);
        let names: Vec<String> = (0..40).map(|i| format!("p{i}")).collect();
        for n in &names {
            h.insert(n);
        }
        assert_eq!(h.iter().count(), 40);
        assert_eq!(h.keys.len(), 128, "grown twice (16 then 32 keys)");
        // Order = decreasing slots: each following key has a lower slot.
        let slots: Vec<usize> = h.iter().map(|k| h.find(k).unwrap()).collect();
        assert!(slots.windows(2).all(|w| w[0] > w[1]));
        for n in &names[..38] {
            h.remove(n);
        }
        assert_eq!(h.iter().collect::<Vec<_>>().len(), 2);
        assert!(h.keys.len() < 128, "shrunk after the removals");
        assert!(h.find("p38").is_ok() && h.find("p39").is_ok());
    }
}
