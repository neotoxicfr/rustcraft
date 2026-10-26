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

    pub fn encrypt(&mut self, data: &mut [u8]) {
        for b in data {
            let c = *b ^ self.keystream_byte();
            self.shift_in(c);
            *b = c;
        }
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
