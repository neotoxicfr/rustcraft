//! Equivalence of the optimised NBT writer with the reference one ([`WriteOptions`]), and of the
//! string reader with and without its redundant scans. Inputs come from a fixed seed (no clock).

use super::*;

/// Small xorshift generator: deterministic inputs.
struct TestRng(u64);

impl TestRng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn in_range(&mut self, low: u32, high: u32) -> u32 {
        low + self.below(u64::from(high - low) + 1) as u32
    }
}

/// Character families of the generated strings; the last one mixes all the others.
const CHARACTER_FAMILIES: u64 = 7;

fn random_char(rng: &mut TestRng, family: u64) -> char {
    loop {
        let code = match family {
            // ASCII without null, then with nulls.
            0 => rng.in_range(0x01, 0x7f),
            1 if rng.below(4) == 0 => 0,
            1 => rng.in_range(0x01, 0x7f),
            // Latin-1, hangul (UTF-8 lead byte 0xED), base plane, outside the base plane.
            2 => rng.in_range(0x80, 0xff),
            3 => rng.in_range(0xd000, 0xd7a3),
            4 => rng.in_range(0, 0xffff),
            5 => rng.in_range(0x1_0000, 0x10_ffff),
            _ => {
                let any = rng.below(CHARACTER_FAMILIES - 1);
                return random_char(rng, any);
            }
        };
        // Surrogate codes are not characters: drawn again.
        if let Some(c) = char::from_u32(code) {
            return c;
        }
    }
}

fn random_string(rng: &mut TestRng) -> String {
    let family = rng.below(CHARACTER_FAMILIES);
    let len = rng.below(48);
    (0..len).map(|_| random_char(rng, family)).collect()
}

/// Strings around the 65,535-byte limit, in UTF-8 and in modified UTF-8.
fn boundary_strings() -> Vec<String> {
    let a = |n: usize| "a".repeat(n);
    vec![
        String::new(),
        a(65_535),
        a(65_536),
        a(70_000),
        a(65_531) + "𝄞",           // 65,535 bytes in UTF-8, 65,537 in modified UTF-8: too long
        a(65_529) + "𝄞",           // 65,535 bytes in modified UTF-8: fits exactly
        a(65_534) + "\0",          // 65,536 in modified UTF-8: too long
        a(65_533) + "\0",          // 65,535: fits
        "\u{d55c}".repeat(21_845), // 65,535 bytes, same in both encodings
        "\u{d55c}".repeat(21_846), // 65,538
        "é".repeat(40_000),
    ]
}

/// The string given back by the reader: empty when its modified UTF-8 form is too long.
fn expected_after_round_trip(s: &str) -> &str {
    let mutf8_len: usize = s
        .encode_utf16()
        .map(|u| match u {
            0x01..=0x7f => 1,
            0 | 0x80..=0x7ff => 2,
            _ => 3,
        })
        .sum();
    if mutf8_len > MAX_STRING_BYTES { "" } else { s }
}

fn describe(s: &str) -> String {
    format!("{} bytes, starting {:?}", s.len(), s.chars().take(16).collect::<String>())
}

/// 100,000 generated strings plus the boundary ones: the in-place writer gives the bytes of the
/// reference writer (after a byte already in the buffer, to check where the length is patched)
/// and the reader gives the string back. Fails if the null check of the copy path is removed.
#[test]
fn string_writers_agree() {
    let mut rng = TestRng(0x6e62_7473_7472_696e);
    let strings = boundary_strings().into_iter().chain((0..100_000).map(|_| random_string(&mut rng)));
    let mut reference = Vec::new();
    let mut optimised = Vec::new();
    let mut checked = 0;
    for s in strings {
        reference.clear();
        optimised.clear();
        reference.push(0xaa);
        optimised.push(0xaa);
        write_string_through_buffer(&mut reference, &s);
        write_string_in_place(&mut optimised, &s);
        // `assert!` rather than `assert_eq!`: a failure names the string without dumping 64 KiB.
        assert!(optimised == reference, "different bytes for {}", describe(&s));
        let mut r = Reader::new(&optimised[1..]);
        assert_eq!(read_string(&mut r).unwrap(), expected_after_round_trip(&s), "{}", describe(&s));
        assert!(r.rest().is_empty());
        checked += 1;
    }
    assert_eq!(checked, 100_011);
}

/// Valid UTF-8, modified UTF-8 and random bytes: the reader without the two extra scans gives
/// the same result, except valid UTF-8 holding both a 0xED lead and a 4-byte character, refused
/// before and taken as it is now (see `decode_mutf8`).
#[test]
fn reader_without_redundant_scans_agrees() {
    let mut rng = TestRng(0x6d75_7466_385f_7264);
    let mut inputs: Vec<Vec<u8>> = Vec::new();
    for _ in 0..50_000 {
        let s = random_string(&mut rng);
        inputs.push(s.as_bytes().to_vec());
        let mut modified = Vec::new();
        write_string_through_buffer(&mut modified, &s);
        inputs.push(modified[2..].to_vec());
        let noise_len = rng.below(12);
        inputs.push((0..noise_len).map(|_| rng.next() as u8).collect());
    }
    let mut accepted_only_now = 0;
    for bytes in &inputs {
        match (decode_mutf8(bytes, false), decode_mutf8(bytes, true)) {
            (Ok(old), Ok(new)) => assert_eq!(old, new, "{bytes:x?}"),
            (Err(_), Err(_)) => {}
            (Err(_), Ok(new)) => {
                assert!(bytes.contains(&0xed) && bytes.iter().any(|&b| b >= 0xf0), "{bytes:x?}");
                assert_eq!(new.as_bytes(), &bytes[..]);
                accepted_only_now += 1;
            }
            (Ok(old), Err(_)) => panic!("refused, accepted before as {old:?}: {bytes:x?}"),
        }
    }
    eprintln!("{} inputs, {accepted_only_now} valid UTF-8 with hangul and 4-byte forms now accepted", inputs.len());
}

/// Int and long arrays: expected bytes frozen, then 4,000 random arrays written in one block
/// give the bytes of the element-by-element writer.
#[test]
fn array_writers_agree() {
    let ints = Nbt::IntArray(vec![1, -1, 0x1234_5678]);
    let longs = Nbt::LongArray(vec![0x0102_0304_0506_0708, -2]);
    #[rustfmt::skip]
    let frozen: [(&Nbt, &[u8]); 2] = [
        (&ints, &[0, 0, 0, 3, 0, 0, 0, 1, 0xff, 0xff, 0xff, 0xff, 0x12, 0x34, 0x56, 0x78]),
        (&longs, &[0, 0, 0, 2, 1, 2, 3, 4, 5, 6, 7, 8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe]),
    ];
    for options in [WriteOptions::REFERENCE, WriteOptions::OPTIMISED] {
        for (v, want) in frozen {
            let mut w = vec![0xaa];
            write_payload(&mut w, v, options);
            assert_eq!(w[1..], *want, "{options:?}");
        }
    }
    let mut rng = TestRng(0x6172_7261_7973_3031);
    for i in 0..4_000 {
        let len = rng.below(600) as usize;
        let v = if i % 2 == 0 {
            Nbt::IntArray((0..len).map(|_| rng.next() as i32).collect())
        } else {
            Nbt::LongArray((0..len).map(|_| rng.next() as i64).collect())
        };
        let (mut reference, mut optimised) = (vec![0xaa], vec![0xaa]);
        write_payload(&mut reference, &v, WriteOptions::REFERENCE);
        write_payload(&mut optimised, &v, WriteOptions::OPTIMISED);
        assert_eq!(optimised, reference, "array {i}, {len} elements");
    }
}

/// Bench (loaded machine): long and int arrays alone, element by element against one block.
/// `cargo test --release -p rc-proto -- --ignored array_write_bench --nocapture`.
#[test]
#[ignore = "bench"]
fn array_write_bench() {
    let longs = Nbt::LongArray((0..4096).map(|i| i * 0x0101_0101_0101).collect());
    let ints = Nbt::IntArray((0..4096).map(|i| i * 0x0101_0101).collect());
    let mut w = Vec::with_capacity(64 * 1024);
    let rounds = 20_000;
    for pass in 0..3 {
        let mut line = format!("pass {pass}:");
        for (name, v) in [("4,096 longs", &longs), ("4,096 ints", &ints)] {
            for options in [WriteOptions::REFERENCE, WriteOptions::OPTIMISED] {
                let t = std::time::Instant::now();
                for _ in 0..rounds {
                    w.clear();
                    write_payload(&mut w, v, options);
                    std::hint::black_box(&w);
                }
                line += &format!(" {name} bulk={} {:?};", options.array_bulk, t.elapsed() / rounds);
            }
        }
        println!("{line}");
    }
}

/// Whole trees: the typical column and the sample of the codec tests give the same bytes with
/// every writer option.
#[test]
fn tree_writers_agree() {
    for v in [super::tests::typical_chunk(), super::tests::sample()] {
        let mut reference = Vec::new();
        write_named_with(&mut reference, "", &v, WriteOptions::REFERENCE);
        for string_fast_path in [false, true] {
            for array_bulk in [false, true] {
                let mut w = Vec::new();
                write_named_with(&mut w, "", &v, WriteOptions { string_fast_path, array_bulk });
                assert_eq!(w, reference, "strings {string_fast_path}, arrays {array_bulk}");
            }
        }
    }
}
