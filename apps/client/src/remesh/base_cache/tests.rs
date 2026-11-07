use super::*;

/// Xorshift draws in `0..n` (fixed seed: the same sequence on every run).
fn draws(mut seed: u64) -> impl FnMut(u64) -> u64 {
    move |n| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    }
}

/// Mesh of `faces` faces and `fluids` fluid faces in one section (starts as the mesher writes
/// them); face words drawn from the column and the version, so that two meshes differ.
fn kept_mesh(x: i32, version: u64, faces: usize, fluids: usize) -> BlockMesh {
    let q = rc_mesh::fluid::FluidQuad { corners: [[0.0; 5]; 4], texture: 1, colors: [0; 4], light: [0; 4] };
    let salt = (version as u32).wrapping_mul(0x9e37_79b9) ^ x as u32;
    BlockMesh {
        x,
        z: 0,
        version,
        layers: [(0..faces as u32).map(|i| [salt, i, salt ^ i, 0, i << 3, 1]).collect(), Vec::new(), Vec::new()],
        fluids: [vec![q; fluids], Vec::new(), Vec::new()],
        starts: [vec![0, faces as u32], vec![0, 0], vec![0, 0]],
        fluid_starts: [vec![0, fluids as u32], vec![0, 0], vec![0, 0]],
        vis: vec![0],
    }
}

fn old_policy(budget: usize) -> CachePolicy {
    CachePolicy {
        budget,
        adaptive: false,
        frequency_eviction: false,
        cold_budget: None,
        expiry: None,
        second_admission: false,
    }
}

/// Fluid bytes read from the starts alone = 27 words per fluid face of the flat format, the
/// faces and fluids themselves freed (as the pool leaves them).
#[test]
fn fluid_bytes_from_the_starts() {
    let q = rc_mesh::fluid::FluidQuad { corners: [[0.0; 5]; 4], texture: 1, colors: [0; 4], light: [0; 4] };
    let mut m = BlockMesh {
        x: 0,
        z: 0,
        version: 1,
        layers: [vec![[1; 6]; 3], Vec::new(), vec![[7; 6]]],
        fluids: [vec![q; 2], Vec::new(), vec![q]],
        starts: [vec![0, 1, 3], vec![0, 0, 0], vec![0, 1, 1]],
        fluid_starts: [vec![0, 2], vec![0, 0], vec![0, 1]],
        vis: vec![1, 2],
    };
    let words = rc_mesh::blocks::packed_len(&m);
    m.layers = Default::default();
    m.fluids = Default::default();
    assert_eq!(packed_fluid_bytes(&m, words), 3 * 27 * 4);
}

/// The cache before C2-M03 (pure least recently used, one budget), as a reference.
#[derive(Default)]
struct OldCache {
    kept: HashMap<Key, (u64, u64, usize)>,
    order: BTreeMap<u64, Key>,
    bytes: usize,
    clock: u64,
}

impl OldCache {
    fn take(&mut self, key: Key) {
        if let Some((_, stamp, size)) = self.kept.remove(&key) {
            self.order.remove(&stamp);
            self.bytes -= size;
        }
    }

    fn put(&mut self, key: Key, version: u64, size: usize, live: bool, budget: usize) {
        self.take(key);
        if live && size <= budget {
            self.clock += 1;
            self.kept.insert(key, (version, self.clock, size));
            self.order.insert(self.clock, key);
            self.bytes += size;
            while self.bytes > budget {
                let Some((_, old)) = self.order.pop_first() else { break };
                let (_, _, size) = self.kept.remove(&old).expect("ordered entry kept");
                self.bytes -= size;
            }
        }
    }

    fn base(&mut self, key: Key) -> Option<u64> {
        let (version, stamp, _) = self.kept.get_mut(&key)?;
        self.order.remove(stamp);
        self.clock += 1;
        *stamp = self.clock;
        self.order.insert(self.clock, key);
        Some(*version)
    }
}

/// Every switch off: the same bases as the cache of before, step by step; kept and evicted bases
/// are exactly the columns whose last mesh was a live one that fits the budget; the fluid bytes
/// are those of the kept bases.
#[test]
fn switches_off_keep_the_old_cache() {
    let budget = 40_000;
    let mut next = draws(0x2545_f491_4f6c_dd1d);
    let mut old = OldCache::default();
    let mut cache = BaseCache::new(old_policy(budget));
    let now = Instant::now();
    let mut live_last: HashMap<Key, bool> = HashMap::new();
    let mut fluid_faces: HashMap<Key, usize> = HashMap::new();
    for version in 0..3000u64 {
        let key = (next(40) as i32, 0);
        match next(6) {
            0 => {
                old.take(key);
                cache.forget(key);
                live_last.remove(&key);
            }
            1 | 2 => assert_eq!(old.base(key), cache.base(key, now).map(|(_, v)| v), "base of {key:?}"),
            _ => {
                let (live, fluids) = (next(3) != 0, next(60) as usize);
                let m = kept_mesh(key.0, version, next(1200) as usize, fluids);
                let size = rc_mesh::blocks::packed_len(&m) * 4;
                old.put(key, m.version, size, live, budget);
                cache.put(key, &m, live, None, now);
                live_last.insert(key, live && size <= budget);
                fluid_faces.insert(key, fluids);
            }
        }
        let mut kept: Vec<_> = cache.hot.iter().map(|(k, e)| (*k, e.version, e.packed.len() * 4)).collect();
        kept.sort_unstable();
        let mut want: Vec<_> = old.kept.iter().map(|(k, &(v, _, size))| (*k, v, size)).collect();
        want.sort_unstable();
        assert_eq!(kept, want, "kept bases at step {version}");
        assert_eq!(cache.hot_bytes, old.bytes);
        let mut live: Vec<_> = cache.hot.keys().chain(&cache.evicted).copied().collect();
        live.sort_unstable();
        let mut want: Vec<_> = live_last.iter().filter(|(_, l)| **l).map(|(k, _)| *k).collect();
        want.sort_unstable();
        assert_eq!(live, want, "kept or evicted bases at step {version}");
        assert!(cache.evicted.iter().all(|k| !cache.hot.contains_key(k)));
        let fluids: usize = cache.hot.keys().map(|k| fluid_faces[k] * 27 * 4).sum();
        assert_eq!(cache.fluid_bytes, fluids, "fluid bytes of the kept bases at step {version}");
    }
    assert!(!cache.evicted.is_empty(), "the budget never evicted: test without object");
}

/// Every policy: a base served is the last mesh received for its column, word for word, with its
/// version (so the partial remesh gives the whole mesh, `rc-mesh` `tests/partial.rs`); never a base
/// for a column whose last mesh was whole or that was forgotten; hot and cold stages within their
/// budgets. Worlds change on the way (versions start again from 0, as after a new world).
#[test]
fn served_bases_are_the_last_mesh_received() {
    let budget = 40_000;
    let adaptive = CachePolicy { adaptive: true, ..old_policy(budget) };
    let cold = CachePolicy { cold_budget: Some(12_000), ..old_policy(budget) };
    let frequency = CachePolicy { frequency_eviction: true, ..old_policy(budget) };
    let expiry = CachePolicy { expiry: Some(Duration::from_secs(20)), ..old_policy(budget) };
    let admission = CachePolicy { second_admission: true, ..old_policy(budget) };
    let all = CachePolicy {
        adaptive: true,
        frequency_eviction: true,
        cold_budget: Some(12_000),
        expiry: Some(Duration::from_secs(20)),
        second_admission: true,
        budget,
    };
    let policies = [
        ("no cache", old_policy(0)),
        ("fixed budget", old_policy(budget)),
        ("adaptive", adaptive),
        ("frequency", frequency),
        ("cold", cold),
        ("expiry", expiry),
        ("second admission", admission),
        ("all", all),
    ];
    for (name, policy) in policies {
        let served = serve_random_bases(policy.clone());
        match name {
            "no cache" => assert_eq!(served, (0, 0), "{name}: a base without a cache"),
            "cold" | "all" => assert!(served.1 > 0, "{name}: the cold stage never served"),
            _ => assert!(served.0 > 0, "{name}: no base ever served"),
        }
        eprintln!("{name}: {} bases served, {} by the cold stage", served.0, served.1);
    }
}

/// Random puts, bases, forgets, new worlds and frames on `policy`; (bases served, of which by the
/// cold stage).
fn serve_random_bases(policy: CachePolicy) -> (usize, usize) {
    let mut next = draws(0x9e37_79b9_7f4a_7c15);
    let mut cache = BaseCache::new(policy.clone());
    let t0 = Instant::now();
    let mut last: HashMap<Key, Option<(Vec<u32>, u64)>> = HashMap::new();
    let (mut version, mut served, mut cold_served) = (0u64, 0, 0);
    for step in 0..6000u64 {
        let now = t0 + Duration::from_millis(step * 40);
        // Few columns: most are remeshed again before their base leaves the cache.
        let key = (next(24) as i32, 0);
        match next(40) {
            0 => {
                cache.forget(key);
                last.remove(&key);
            }
            1 if next(10) == 0 => {
                // New world: versions start again.
                cache.clear();
                last.clear();
                version = 0;
            }
            2..=4 => {
                if let Some(cold) = &mut cache.cold {
                    cold.wait_compressions(&mut cache.clock, &mut cache.evicted);
                }
            }
            5..=17 => {
                let from_cold = !cache.hot.contains_key(&key);
                let Some((packed, v)) = cache.base(key, now) else { continue };
                let (words, want_version) = last.get(&key).cloned().flatten().expect("base of a column without one");
                assert_eq!(&**packed, &words[..], "base served differs from the last mesh received");
                assert_eq!(v, want_version);
                served += 1;
                cold_served += usize::from(from_cold);
            }
            _ => {
                version += 1;
                let live = next(4) != 0;
                let m = kept_mesh(key.0, version, next(700) as usize, next(30) as usize);
                let packed = (next(2) == 0).then(|| Arc::new(rc_mesh::blocks::pack_exact(&m)) as Packed);
                cache.put(key, &m, live, packed, now);
                last.insert(key, live.then(|| (rc_mesh::blocks::pack_exact(&m), m.version)));
            }
        }
        cache.tick(now);
        assert!(cache.hot_bytes <= cache.budget, "hot stage over its budget");
        if let Some(cold) = &cache.cold {
            assert!(cold.bytes <= cold.budget, "cold stage over its budget");
        }
        assert!(cache.budget >= policy.budget && cache.budget <= 3 * policy.budget, "budget out of its bounds");
    }
    (served, cold_served)
}

/// Regulator: one step up per window with enough misses, up to twice the floor, three times while
/// they stay twice as many, never above; down only after a long rest, never below the floor.
#[test]
fn regulator_raises_caps_and_lowers() {
    let floor = 64 << 20;
    let mut r = BudgetRegulator::new(floor);
    let raise = BudgetRegulator::RAISE_AT;
    assert_eq!(r.end_window(raise - 1, 100), floor, "few misses: no change");
    assert_eq!(r.end_window(raise, 100), 96 << 20);
    assert_eq!(r.end_window(raise, 100), 128 << 20);
    assert_eq!(r.end_window(raise, 100), 128 << 20, "first cap");
    assert_eq!(r.end_window(2 * raise, 100), 160 << 20);
    assert_eq!(r.end_window(2 * raise, 100), 192 << 20);
    assert_eq!(r.end_window(100 * raise, 100), 192 << 20, "cap");
    // Busy but without misses: kept.
    for _ in 0..100 {
        assert_eq!(r.end_window(0, 50), 192 << 20);
    }
    // Rest: one step down every 15 windows, down to the floor and no further.
    let mut budgets = Vec::new();
    for _ in 0..15 * 6 {
        budgets.push(r.end_window(0, 0));
    }
    assert_eq!(budgets[13], 192 << 20);
    assert_eq!(budgets[14], 160 << 20);
    assert_eq!(budgets[29], 128 << 20);
    assert_eq!(*budgets.last().expect("windows"), floor);
    // A busy window restarts the rest count.
    for _ in 0..14 {
        r.end_window(raise, 100);
    }
    let up = r.end_window(0, 0);
    for _ in 0..13 {
        assert_eq!(r.end_window(0, 0), up);
    }
    assert_eq!(r.end_window(0, 50), up);
    for _ in 0..14 {
        assert_eq!(r.end_window(0, 0), up);
    }
    assert!(r.end_window(0, 0) < up);
}

/// Frequency-aware eviction: a column remeshed again and again keeps its base through a burst of
/// one-off remeshes that the pure least recently used order lets push it out.
#[test]
fn frequency_eviction_protects_columns_remeshed_with_a_base() {
    let now = Instant::now();
    let size = rc_mesh::blocks::packed_len(&kept_mesh(0, 0, 400, 0)) * 4;
    let mut kept_after_burst = Vec::new();
    for frequency_eviction in [false, true] {
        let mut cache = BaseCache::new(CachePolicy { frequency_eviction, ..old_policy(size * 4) });
        let mut version = 0;
        let mut put = |cache: &mut BaseCache, x: i32| {
            version += 1;
            cache.put((x, 0), &kept_mesh(x, version, 400, 0), true, None, now);
        };
        // Column 0 remeshed twice with a base.
        put(&mut cache, 0);
        assert!(cache.base((0, 0), now).is_some());
        put(&mut cache, 0);
        // Burst of columns changed once.
        for x in 1..=6 {
            put(&mut cache, x);
        }
        kept_after_burst.push(cache.base((0, 0), now).is_some());
    }
    assert_eq!(kept_after_burst, [false, true]);
}

/// Second admission: the first live mesh of a column is not kept, the second is; a whole mesh
/// starts the count again.
#[test]
fn second_admission_keeps_from_the_second_live_mesh() {
    let now = Instant::now();
    let mut cache = BaseCache::new(CachePolicy { second_admission: true, ..old_policy(1 << 20) });
    cache.put((0, 0), &kept_mesh(0, 1, 10, 0), true, None, now);
    assert!(cache.base((0, 0), now).is_none(), "first live mesh kept");
    assert!(!cache.was_evicted((0, 0)));
    cache.put((0, 0), &kept_mesh(0, 2, 10, 0), true, None, now);
    assert_eq!(cache.base((0, 0), now).map(|(_, v)| v), Some(2));
    cache.put((0, 0), &kept_mesh(0, 3, 10, 0), false, None, now);
    cache.put((0, 0), &kept_mesh(0, 4, 10, 0), true, None, now);
    assert!(cache.base((0, 0), now).is_none(), "first live mesh after a whole one kept");
}

/// Expiry: a base unused for the idle time leaves both stages (and counts as evicted); a base
/// used in time stays.
#[test]
fn expiry_drops_idle_bases() {
    let t0 = Instant::now();
    let idle = Duration::from_secs(20);
    let policy = CachePolicy { expiry: Some(idle), cold_budget: Some(1 << 20), ..old_policy(1 << 20) };
    let mut cache = BaseCache::new(policy);
    cache.put((0, 0), &kept_mesh(0, 1, 10, 0), true, None, t0);
    cache.put((1, 0), &kept_mesh(1, 2, 10, 0), true, None, t0);
    let mut t = t0;
    while t < t0 + idle + WINDOW * 2 {
        t += Duration::from_millis(500);
        assert!(cache.base((1, 0), t).is_some(), "base used in time expired");
        cache.tick(t);
    }
    assert!(cache.base((0, 0), t).is_none(), "idle base kept");
    assert!(cache.was_evicted((0, 0)));
}

/// Cold stage: a base pushed out of the hot stage is served again, decoded from the cold one.
#[test]
fn cold_stage_serves_evicted_bases() {
    let now = Instant::now();
    let size = rc_mesh::blocks::packed_len(&kept_mesh(0, 0, 400, 0)) * 4;
    let mut cache = BaseCache::new(CachePolicy { cold_budget: Some(1 << 20), ..old_policy(size * 2) });
    let meshes: Vec<BlockMesh> = (0..6).map(|x| kept_mesh(x, x as u64 + 1, 400, 0)).collect();
    for m in &meshes {
        cache.put((m.x, 0), m, true, None, now);
    }
    let cold = cache.cold.as_mut().expect("cold stage");
    cold.wait_compressions(&mut cache.clock, &mut cache.evicted);
    assert_eq!(cold.entries.len(), 4, "evicted bases not compressed");
    assert!(cold.bytes < 4 * size, "cold stage not smaller than the plain bases");
    for m in &meshes {
        let (packed, version) = cache.base((m.x, 0), now).expect("base kept in one stage");
        assert_eq!((&**packed, version), (&rc_mesh::blocks::pack_exact(m)[..], m.version));
    }
}
