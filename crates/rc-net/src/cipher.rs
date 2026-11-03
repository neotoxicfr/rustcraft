use aes::Aes128;
use aes::cipher::{Array, BlockCipherEncrypt, KeyInit};

/// AES-128 in CFB8 mode on a continuous stream. One state per direction of communication.
/// CFB8 only uses the block encryption, in both directions.
pub struct Cfb8 {
    aes: Aes128,
    register: [u8; 16],
}

impl Cfb8 {
    /// Key and initialisation vector are both equal to the shared secret.
    pub fn new(shared_secret: &[u8; 16]) -> Self {
        Self { aes: Aes128::new(&Array::from(*shared_secret)), register: *shared_secret }
    }

    #[inline]
    fn keystream_byte(&self) -> u8 {
        let mut block = Array::from(self.register);
        self.aes.encrypt_block(&mut block);
        block[0]
    }

    #[inline]
    fn shift_in(&mut self, c: u8) {
        self.register.copy_within(1.., 0);
        self.register[15] = c;
    }

    /// Encrypts in place. Sequential by nature: each byte needs the previous encrypted one.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        match encrypt_path() {
            EncryptPath::Shifting => self.encrypt_shifting(data),
            EncryptPath::SlidingWindow => self.encrypt_sliding_window(data),
            #[cfg(target_arch = "x86_64")]
            EncryptPath::VectorRegister => self.encrypt_vector_register(data),
        }
    }

    /// Old path: the 16-byte register is shifted in memory by one byte after every byte.
    fn encrypt_shifting(&mut self, data: &mut [u8]) {
        for b in data {
            let c = *b ^ self.keystream_byte();
            self.shift_in(c);
            *b = c;
        }
    }

    /// The register is a 16-byte view sliding over a window of `16 + ENCRYPT_WINDOW` bytes: each
    /// encrypted byte is appended after the view, which then advances by one. The 15-byte shift
    /// per byte becomes one 16-byte copy every `ENCRYPT_WINDOW` bytes.
    fn encrypt_sliding_window(&mut self, data: &mut [u8]) {
        let mut window = [0u8; 16 + ENCRYPT_WINDOW];
        window[..16].copy_from_slice(&self.register);
        for chunk in data.chunks_mut(ENCRYPT_WINDOW) {
            for (i, b) in chunk.iter_mut().enumerate() {
                let mut block = aes::Block::default();
                block.copy_from_slice(&window[i..i + 16]);
                self.aes.encrypt_block(&mut block);
                *b ^= block[0];
                window[16 + i] = *b;
            }
            let n = chunk.len();
            window.copy_within(n..n + 16, 0);
        }
        self.register.copy_from_slice(&window[..16]);
    }

    /// The register stays in a vector register between bytes and reaches the AES input with ONE
    /// 16-byte write. The other paths build that input from several smaller writes, which the
    /// processor cannot forward to the 16-byte read that follows: it waits for them to reach the
    /// cache, on every byte of a chain that is sequential anyway.
    #[cfg(target_arch = "x86_64")]
    fn encrypt_vector_register(&mut self, data: &mut [u8]) {
        let mut register = vector::load(&self.register);
        let mut block = aes::Block::default();
        for b in data {
            vector::store(register, &mut block.0);
            self.aes.encrypt_block(&mut block);
            *b ^= block[0];
            register = vector::shift_in(register, *b);
        }
        vector::store(register, &mut self.register);
    }

    /// Decrypts in place; beyond [`PARALLEL_MIN`] bytes, spread over the pool.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        if data.len() >= PARALLEL_MIN && rayon::current_num_threads() > 1 {
            self.decrypt_parallel(data);
        } else {
            self.decrypt_batched(data);
        }
    }

    /// Each byte depends only on the 16 encrypted bytes that precede it: the buffer is
    /// cut into chunks whose initial state is taken BEFORE the in-place decryption,
    /// then the chunks are decrypted in parallel. Result identical to the sequential one.
    pub fn decrypt_parallel(&mut self, data: &mut [u8]) {
        use rayon::prelude::*;
        if data.len() < 16 {
            return self.decrypt_batched(data);
        }
        let piece = data.len().div_ceil(rayon::current_num_threads()).max(PIECE_MIN);
        let registers: Vec<[u8; 16]> = (0..data.len().div_ceil(piece))
            .map(|k| match k {
                0 => self.register,
                _ => data[k * piece - 16..k * piece].try_into().expect("16 bytes"),
            })
            .collect();
        let last: [u8; 16] = data[data.len() - 16..].try_into().expect("16 bytes");
        let aes = &self.aes;
        data.par_chunks_mut(piece).zip(registers).for_each(|(chunk, register)| {
            Cfb8 { aes: aes.clone(), register }.decrypt_batched(chunk);
        });
        self.register = last;
    }

    /// Decryption depends only on encrypted bytes already known: the AES blocks of a batch
    /// are independent and computed together (AES-NI pipeline), instead of one by one.
    pub fn decrypt_batched(&mut self, data: &mut [u8]) {
        let mut window = [0u8; 16 + DECRYPT_LANES];
        let mut blocks = [aes::Block::default(); DECRYPT_LANES];
        for chunk in data.chunks_mut(DECRYPT_LANES) {
            let n = chunk.len();
            window[..16].copy_from_slice(&self.register);
            window[16..16 + n].copy_from_slice(chunk);
            for (i, block) in blocks[..n].iter_mut().enumerate() {
                block.copy_from_slice(&window[i..i + 16]);
            }
            self.aes.encrypt_blocks(&mut blocks[..n]);
            for (b, k) in chunk.iter_mut().zip(&blocks[..n]) {
                *b ^= k[0];
            }
            self.register.copy_from_slice(&window[n..n + 16]);
        }
    }

    /// Byte-by-byte version, reference of the equivalence tests.
    #[cfg(test)]
    fn decrypt_scalar(&mut self, data: &mut [u8]) {
        for b in data {
            let c = *b;
            *b = c ^ self.keystream_byte();
            self.shift_in(c);
        }
    }
}

/// Size of the decryption batches (in bytes, hence in AES blocks).
const DECRYPT_LANES: usize = 64;
/// Bytes encrypted between two rewinds of the sliding register (see `encrypt_sliding_window`).
const ENCRYPT_WINDOW: usize = 256;

/// How [`Cfb8::encrypt`] keeps its register between bytes (same output on every path).
#[derive(Clone, Copy)]
enum EncryptPath {
    /// Shifted in memory after every byte (old path).
    Shifting,
    /// Sliding view over a window (lost 3 to 5 % at the micro-bench, see docs/perf.md).
    SlidingWindow,
    /// Kept in a vector register (default on x86-64).
    #[cfg(target_arch = "x86_64")]
    VectorRegister,
}

/// Chosen once: `RUSTCRAFT_OPT_CFB8_SLIDING_WINDOW=1` takes the sliding window;
/// `RUSTCRAFT_OPT_CFB8_VECTOR_REGISTER=0` restores the in-memory shift.
fn encrypt_path() -> EncryptPath {
    static PATH: std::sync::OnceLock<EncryptPath> = std::sync::OnceLock::new();
    *PATH.get_or_init(|| {
        if rc_data::env_str!("RUSTCRAFT_OPT_CFB8_SLIDING_WINDOW") == Some("1") {
            return EncryptPath::SlidingWindow;
        }
        #[cfg(target_arch = "x86_64")]
        if rc_data::env_str!("RUSTCRAFT_OPT_CFB8_VECTOR_REGISTER") != Some("0") {
            return EncryptPath::VectorRegister;
        }
        EncryptPath::Shifting
    })
}

/// 16 bytes to and from an SSE2 register (part of the x86-64 baseline, no detection needed).
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
mod vector {
    use core::arch::x86_64::{
        __m128i, _mm_cvtsi32_si128, _mm_loadu_si128, _mm_or_si128, _mm_slli_si128, _mm_srli_si128, _mm_storeu_si128,
    };

    /// Drops the oldest byte (lowest address) and appends `byte` at the top.
    pub fn shift_in(register: __m128i, byte: u8) -> __m128i {
        // SAFETY: SSE2 only, which every x86-64 processor has.
        unsafe {
            let incoming = _mm_slli_si128::<15>(_mm_cvtsi32_si128(i32::from(byte)));
            _mm_or_si128(_mm_srli_si128::<1>(register), incoming)
        }
    }

    pub fn load(bytes: &[u8; 16]) -> __m128i {
        // SAFETY: unaligned 16-byte read from a 16-byte array.
        unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
    }

    pub fn store(value: __m128i, bytes: &mut [u8; 16]) {
        // SAFETY: unaligned 16-byte write into a 16-byte array.
        unsafe { _mm_storeu_si128(bytes.as_mut_ptr().cast(), value) }
    }
}
/// Split threshold onto the pool (measured at the `cfb8` bench, see docs/perf.md).
pub const PARALLEL_MIN: usize = 128 * 1024;
/// Minimum size of a chunk handed to a thread.
const PIECE_MIN: usize = 4 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    /// NIST SP 800-38A test vector, F.3.7 (CFB8-AES128.Encrypt).
    #[test]
    fn nist_vector() {
        let key = [0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
        let iv: [u8; 16] = core::array::from_fn(|i| i as u8);
        let plain = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a, 0xae, 0x2d,
        ];
        let expected = [
            0x3b, 0x79, 0x42, 0x4c, 0x9c, 0x0d, 0xd4, 0x36, 0xba, 0xce, 0x9e, 0x0e, 0xd4, 0x58, 0x6a, 0x4f, 0x32, 0xb9,
        ];
        let mut c = Cfb8 { aes: Aes128::new(&Array::from(key)), register: iv };
        let mut data = plain;
        c.encrypt(&mut data);
        assert_eq!(data, expected);
        let mut d = Cfb8 { aes: Aes128::new(&Array::from(key)), register: iv };
        d.decrypt(&mut data);
        assert_eq!(data, plain);
    }

    #[test]
    fn batched_decrypt_matches_scalar() {
        let secret: [u8; 16] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let msg: Vec<u8> = (0..5000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let mut wire = msg.clone();
        Cfb8::new(&secret).encrypt(&mut wire);
        // Various splits, including around the batch size.
        for split in [1, 15, 16, 17, 63, 64, 65, 1000, 5000] {
            let mut a = wire.clone();
            let mut b = wire.clone();
            let (mut ca, mut cb) = (Cfb8::new(&secret), Cfb8::new(&secret));
            for (x, y) in a.chunks_mut(split).zip(b.chunks_mut(split)) {
                ca.decrypt(x);
                cb.decrypt_scalar(y);
            }
            assert_eq!(a, b, "split {split}");
            assert_eq!(a, msg);
        }
    }

    #[test]
    fn parallel_decrypt_matches_scalar() {
        let secret = [0x42u8; 16];
        for len in [17usize, 4096, 300_000, 1 << 20] {
            let msg: Vec<u8> = (0..len as u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8).collect();
            let mut wire = msg.clone();
            Cfb8::new(&secret).encrypt(&mut wire);
            // Two successive calls: the final state must chain correctly.
            let (a, b) = wire.split_at(len / 3);
            let (mut pa, mut pb) = (a.to_vec(), b.to_vec());
            let mut c = Cfb8::new(&secret);
            c.decrypt_parallel(&mut pa);
            c.decrypt_parallel(&mut pb);
            let mut s = wire.clone();
            Cfb8::new(&secret).decrypt_scalar(&mut s);
            assert_eq!([pa, pb].concat(), s, "length {len}");
            assert_eq!(s, msg);
        }
    }

    use crate::test_bytes::pseudo_random;

    /// Chunk sizes from 1 to 70 000 bytes: the window edges first, then pseudo-random sizes.
    fn chunk_sizes(total: usize) -> Vec<usize> {
        let edges = [1, 15, 16, 17, 255, 256, 257, 511, 512, 4096, 70_000];
        let noise = pseudo_random(3 * 1024, 99);
        let random = noise
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[a, b, c]| 1 + (usize::from(a) | usize::from(b) << 8 | usize::from(c) << 16) % 70_000);
        let mut sizes = Vec::new();
        let mut left = total;
        for size in edges.into_iter().chain(random).cycle() {
            if left == 0 {
                break;
            }
            let size = size.min(left);
            sizes.push(size);
            left -= size;
        }
        sizes
    }

    type EncryptFn = fn(&mut Cfb8, &mut [u8]);

    /// Every encryption path (switches) produces the in-memory shift's stream on 1 MiB cut into
    /// chunks of 1 to 70 000 bytes, and leaves the same register behind.
    #[test]
    fn encrypt_paths_match_shifting() {
        let msg = pseudo_random(1 << 20, 0x9e37_79b9_7f4a_7c15);
        let secret: [u8; 16] = core::array::from_fn(|i| (i * 11 + 5) as u8);
        let mut reference = msg.clone();
        let mut whole = Cfb8::new(&secret);
        whole.encrypt_shifting(&mut reference);
        let mut paths: Vec<(&str, EncryptFn)> = vec![("sliding window", Cfb8::encrypt_sliding_window)];
        #[cfg(target_arch = "x86_64")]
        paths.push(("vector register", Cfb8::encrypt_vector_register));
        for (label, encrypt) in paths {
            let mut data = msg.clone();
            let mut cipher = Cfb8::new(&secret);
            let mut offset = 0;
            for size in chunk_sizes(msg.len()) {
                encrypt(&mut cipher, &mut data[offset..offset + size]);
                offset += size;
            }
            assert_eq!(offset, msg.len());
            assert!(data == reference, "{label}: stream differs from the in-memory shift");
            assert_eq!(cipher.register, whole.register, "{label}: final register differs");
            Cfb8::new(&secret).decrypt(&mut data);
            assert!(data == msg, "{label}: does not decrypt back");
        }
    }

    /// Bench-only variant: register kept in a `u128`, shifted by one byte per byte.
    fn encrypt_u128_register(cipher: &mut Cfb8, data: &mut [u8]) {
        let mut register = u128::from_le_bytes(cipher.register);
        for b in data {
            let mut block = aes::Block::from(register.to_le_bytes());
            cipher.aes.encrypt_block(&mut block);
            *b ^= block[0];
            register = (register >> 8) | (u128::from(*b) << 120);
        }
        cipher.register = register.to_le_bytes();
    }

    /// Bench-only variant: the next register is rebuilt as a whole value (a byte shuffle the
    /// compiler can keep in a vector register) instead of being shifted in memory.
    fn encrypt_rebuilt_register(cipher: &mut Cfb8, data: &mut [u8]) {
        let mut register = cipher.register;
        for b in data {
            let mut block = aes::Block::from(register);
            cipher.aes.encrypt_block(&mut block);
            *b ^= block[0];
            let c = *b;
            register = core::array::from_fn(|i| if i < 15 { register[i + 1] } else { c });
        }
        cipher.register = register;
    }

    /// The two bench-only variants produce the reference stream too (else their figures mean nothing).
    #[test]
    fn bench_variants_match_shifting() {
        let msg = pseudo_random(5000, 17);
        let mut reference = msg.clone();
        Cfb8::new(&[9; 16]).encrypt_shifting(&mut reference);
        for variant in [encrypt_u128_register, encrypt_rebuilt_register] {
            let mut data = msg.clone();
            let mut cipher = Cfb8::new(&[9; 16]);
            for chunk in data.chunks_mut(333) {
                variant(&mut cipher, chunk);
            }
            assert!(data == reference);
        }
    }

    /// Micro-bench: `cargo test --release -p rc-net cfb8_encrypt_bench -- --ignored --nocapture`.
    /// One stream per core at most (sending is sequential by nature; several cores = several
    /// guests, each with its own writer thread).
    #[test]
    #[ignore]
    #[cfg(target_arch = "x86_64")]
    fn cfb8_encrypt_bench() {
        use std::hint::black_box;
        use std::time::Instant;
        let variants: [(&str, EncryptFn); 5] = [
            ("shifting", Cfb8::encrypt_shifting),
            ("vector register", Cfb8::encrypt_vector_register),
            ("sliding window", Cfb8::encrypt_sliding_window),
            ("u128 register", encrypt_u128_register),
            ("rebuilt array", encrypt_rebuilt_register),
        ];
        for (packet, rounds) in [(16usize, 50_000usize), (300, 3_000), (8 * 1024, 100), (1 << 20, 1)] {
            let mut data = pseudo_random(packet, 3);
            let mut best = [f64::MAX; 5];
            // Variants alternate inside each trial, so that a load spike hits all of them.
            for _ in 0..15 {
                for ((_, encrypt), best) in variants.iter().zip(&mut best) {
                    let mut cipher = Cfb8::new(&[3; 16]);
                    let t = Instant::now();
                    for _ in 0..rounds {
                        encrypt(&mut cipher, black_box(&mut data));
                    }
                    *best = best.min(t.elapsed().as_secs_f64());
                }
            }
            for ((label, _), best) in variants.iter().zip(best) {
                let mib_s = (packet * rounds) as f64 / best / (1 << 20) as f64;
                println!("1 core, {packet:>7} B packets, {label:15}: {mib_s:7.1} MiB/s");
            }
        }
        let variants = &variants[..2];
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut threads = 1;
        while threads <= cores {
            for (label, encrypt) in variants {
                let t = Instant::now();
                std::thread::scope(|s| {
                    for k in 0..threads {
                        s.spawn(move || {
                            let mut data = pseudo_random(1 << 20, k as u64 + 1);
                            let mut cipher = Cfb8::new(&[k as u8; 16]);
                            for _ in 0..4 {
                                encrypt(&mut cipher, black_box(&mut data));
                            }
                        });
                    }
                });
                let mib_s = (threads * 4) as f64 / t.elapsed().as_secs_f64();
                println!("{threads:>2} threads, 1 MiB streams, {label:15}: {mib_s:7.1} MiB/s in total");
            }
            threads *= 2;
        }
    }

    #[test]
    fn streaming_is_chunk_independent() {
        let secret = [7u8; 16];
        let msg: Vec<u8> = (0..1000u32).map(|i| (i * 31 % 251) as u8).collect();
        let mut whole = msg.clone();
        Cfb8::new(&secret).encrypt(&mut whole);
        let mut parts = msg.clone();
        let mut enc = Cfb8::new(&secret);
        for chunk in parts.chunks_mut(37) {
            enc.encrypt(chunk);
        }
        assert_eq!(whole, parts);
        let mut dec = Cfb8::new(&secret);
        for chunk in parts.chunks_mut(5) {
            dec.decrypt(chunk);
        }
        assert_eq!(parts, msg);
    }
}
