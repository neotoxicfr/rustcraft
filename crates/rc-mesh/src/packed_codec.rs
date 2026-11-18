//! Lossless compression of flat meshes ([`crate::blocks::pack`]), for the cold stage of the
//! client's base cache: a base dropped from the plain cache is kept compressed, and decoded only
//! when a partial remesh needs it again (much cheaper than meshing the whole column).
//!
//! Plain deflate at level 1 (libdeflate, already used by the network and the region
//! files): the flat words are compressed as their little-endian bytes, without any prefilter
//! (measured in `tests/packed_codec.rs`, `docs/perf.md`).

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use libdeflater::{CompressionLvl, Compressor, Decompressor};

use crate::blocks::Packed;

/// Level 1: the fastest that compresses (`CompressionLvl::fastest()` is level 0, stored blocks).
const FASTEST: CompressionLvl = match CompressionLvl::new(1) {
    Ok(level) => level,
    Err(_) => panic!("deflate level 1"),
};

/// A flat mesh, compressed.
pub struct CompressedMesh {
    bytes: Box<[u8]>,
    /// Number of words of the flat mesh (exact size of the decoded buffer).
    words: usize,
}

impl CompressedMesh {
    /// Bytes held in memory.
    pub fn bytes_held(&self) -> usize {
        self.bytes.len()
    }

    /// Bytes of the flat mesh once decoded.
    pub fn plain_bytes(&self) -> usize {
        self.words * 4
    }
}

thread_local! {
    // One compressor and one output buffer per thread: libdeflate allocates about 300 KB of
    // tables per compressor, and the output buffer would otherwise be allocated per mesh.
    static COMPRESSOR: RefCell<(Compressor, Vec<u8>)> = RefCell::new((Compressor::new(FASTEST), Vec::new()));
    static DECOMPRESSOR: RefCell<Decompressor> = RefCell::new(Decompressor::new());
}

/// Compresses a flat mesh (deflate, level 1).
pub fn compress(words: &[u32]) -> CompressedMesh {
    let plain: &[u8] = bytemuck::cast_slice(words);
    let bytes = COMPRESSOR.with_borrow_mut(|(c, out)| {
        out.resize(c.deflate_compress_bound(plain.len()), 0);
        let n = c.deflate_compress(plain, out).expect("buffer sized by the compression bound");
        Box::<[u8]>::from(&out[..n])
    });
    CompressedMesh { bytes, words: words.len() }
}

/// Decodes a compressed flat mesh; `None` if the bytes are damaged (never expected).
pub fn decompress(c: &CompressedMesh) -> Option<Vec<u32>> {
    let mut words = vec![0u32; c.words];
    let n = DECOMPRESSOR.with_borrow_mut(|d| d.deflate_decompress(&c.bytes, bytemuck::cast_slice_mut(&mut words)));
    (n.ok()? == c.words * 4).then_some(words)
}

/// Base decoded on its first read: the main thread hands the compressed mesh to the meshing task,
/// which pays for the decoding on the pool. A damaged mesh reads as empty, which the mesher
/// takes as "no base" (whole column).
struct DecodedOnFirstRead {
    compressed: Arc<CompressedMesh>,
    words: OnceLock<Vec<u32>>,
}

impl std::ops::Deref for DecodedOnFirstRead {
    type Target = [u32];

    fn deref(&self) -> &[u32] {
        self.words.get_or_init(|| decompress(&self.compressed).unwrap_or_default())
    }
}

/// Base of a partial remesh read from a compressed mesh (decoded where it is first read).
pub fn decoded_on_first_read(compressed: Arc<CompressedMesh>) -> Packed {
    Arc::new(DecodedOnFirstRead { compressed, words: OnceLock::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_damage() {
        let words: Vec<u32> = (0..10_000u32).map(|i| i.wrapping_mul(2_654_435_761) >> (i % 29)).collect();
        let c = compress(&words);
        assert_eq!(decompress(&c).as_deref(), Some(&words[..]));
        assert_eq!(&**decoded_on_first_read(Arc::new(compress(&words))), &words[..]);
        // Really compressed (level 0 would store the bytes as they are).
        assert!(compress(&[7u32; 10_000]).bytes_held() < 1_000);
        let empty = compress(&[]);
        assert_eq!(decompress(&empty).as_deref(), Some(&[][..]));
        // Damaged bytes: no base rather than a wrong one.
        let mut bad = compress(&words);
        bad.words += 1;
        assert!(decompress(&bad).is_none());
        assert!(decoded_on_first_read(Arc::new(bad)).is_empty());
    }
}
