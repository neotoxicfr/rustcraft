use libdeflater::{CompressionLvl, Compressor, DecompressionError, Decompressor};
use rc_proto::varint::{peek_varint, varint_len, write_varint};

use crate::{Cfb8, Error, Result};

/// Maximum length of a frame's content (3-byte VarInt prefix).
pub const MAX_FRAME_LEN: usize = (1 << 21) - 1;
/// Maximum decompressed size of a packet.
pub const MAX_UNCOMPRESSED_LEN: usize = 1 << 23;

/// Threshold received from the server: negative = compression disabled.
fn threshold_from_wire(t: i32) -> Option<usize> {
    usize::try_from(t).ok()
}

/// Produces the bytes to write to the socket.
pub struct FrameEncoder {
    threshold: Option<usize>,
    cipher: Option<Cfb8>,
    deflate: Compressor,
    scratch: Vec<u8>,
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameEncoder {
    pub fn new() -> Self {
        Self { threshold: None, cipher: None, deflate: Compressor::new(CompressionLvl::default()), scratch: Vec::new() }
    }

    pub fn set_compression(&mut self, wire_threshold: i32) {
        self.threshold = threshold_from_wire(wire_threshold);
    }

    pub fn enable_encryption(&mut self, shared_secret: &[u8; 16]) {
        self.cipher = Some(Cfb8::new(shared_secret));
    }

    /// Appends to `out` the frame of a packet (`packet` = VarInt identifier + body).
    pub fn encode(&mut self, packet: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let start = out.len();
        match self.threshold {
            None => write_frame(out, packet)?,
            Some(threshold) => {
                if packet.len() > MAX_UNCOMPRESSED_LEN {
                    return Err(Error::PacketTooLarge { len: packet.len(), max: MAX_UNCOMPRESSED_LEN });
                }
                self.scratch.clear();
                if packet.len() < threshold {
                    write_varint(&mut self.scratch, 0);
                    self.scratch.extend_from_slice(packet);
                } else {
                    write_varint(&mut self.scratch, packet.len() as i32);
                    deflate_into(&mut self.deflate, packet, &mut self.scratch)?;
                }
                write_frame(out, &self.scratch)?;
            }
        }
        if let Some(c) = &mut self.cipher {
            c.encrypt(&mut out[start..]);
        }
        Ok(())
    }
}

fn write_frame(out: &mut Vec<u8>, content: &[u8]) -> Result<()> {
    if content.len() > MAX_FRAME_LEN {
        return Err(Error::PacketTooLarge { len: content.len(), max: MAX_FRAME_LEN });
    }
    out.reserve(varint_len(content.len() as i32) + content.len());
    write_varint(out, content.len() as i32);
    out.extend_from_slice(content);
    Ok(())
}

/// Compression of a block in one call (libdeflate): faster than streaming zlib, and any
/// conforming zlib decompressor reads the result back.
fn deflate_into(z: &mut Compressor, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let start = out.len();
    out.resize(start + z.zlib_compress_bound(input.len()), 0);
    let n = z.zlib_compress(input, &mut out[start..]).map_err(|e| Error::Zlib(format!("{e:?}")))?;
    out.truncate(start + n);
    Ok(())
}

/// Receives the raw bytes of the socket and returns complete packets.
pub struct FrameDecoder {
    threshold: Option<usize>,
    cipher: Option<Cfb8>,
    inflate: Decompressor,
    buf: Vec<u8>,
    /// Start of the unconsumed bytes in `buf`.
    head: usize,
    /// Check of the announced sizes (threshold, 8 MiB): true on the server side; the game's client
    /// enables compression WITHOUT this check and therefore accepts a packet compressed below the
    /// threshold or announced beyond 8 MiB.
    validate: bool,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Maximum expansion ratio of a deflate stream (block of maximum lengths): an announced size
/// beyond it can never be reached.
const MAX_DEFLATE_RATIO: usize = 1032;

impl FrameDecoder {
    pub fn new() -> Self {
        Self { threshold: None, cipher: None, inflate: Decompressor::new(), buf: Vec::new(), head: 0, validate: true }
    }

    /// Enables compression with a check of the announced sizes (server side).
    pub fn set_compression(&mut self, wire_threshold: i32) {
        self.threshold = threshold_from_wire(wire_threshold);
        self.validate = true;
    }

    /// Enables compression like the game's client: without the threshold or ceiling check
    /// of 8 MiB (only the size obtained must equal the announced size).
    pub fn set_compression_unvalidated(&mut self, wire_threshold: i32) {
        self.threshold = threshold_from_wire(wire_threshold);
        self.validate = false;
    }

    /// To be called right after the packet that enables encryption; the bytes already received
    /// and not consumed are decrypted at that point.
    pub fn enable_encryption(&mut self, shared_secret: &[u8; 16]) {
        let mut c = Cfb8::new(shared_secret);
        c.decrypt(&mut self.buf[self.head..]);
        self.cipher = Some(c);
    }

    pub fn push(&mut self, data: &[u8]) {
        // Compacts when more than half the buffer is already consumed.
        if self.head > 0 && self.head * 2 >= self.buf.len() {
            self.buf.drain(..self.head);
            self.head = 0;
        }
        let start = self.buf.len();
        self.buf.extend_from_slice(data);
        if let Some(c) = &mut self.cipher {
            c.decrypt(&mut self.buf[start..]);
        }
    }

    /// Bytes received not yet consumed into frames.
    pub fn pending(&self) -> usize {
        self.buf.len() - self.head
    }

    /// Fills `packet` (identifier + body) with the next complete packet.
    /// `Ok(false)` if the next frame is not yet fully received.
    pub fn next_packet(&mut self, packet: &mut Vec<u8>) -> Result<bool> {
        Ok(matches!(self.next_or_deferred(packet, usize::MAX)?, Next::Ready))
    }

    /// Like [`next_packet`](Self::next_packet), but a compressed frame whose announced
    /// size is at least `defer_from` bytes is returned **compressed** (size checks
    /// done): its decompression can then go onto the pool, via [`inflate`].
    pub fn next_or_deferred(&mut self, packet: &mut Vec<u8>, defer_from: usize) -> Result<Next> {
        let avail = &self.buf[self.head..];
        let Some((len, hdr)) = peek_frame_len(avail)? else {
            return Ok(Next::Pending);
        };
        if avail.len() < hdr + len {
            return Ok(Next::Pending);
        }
        let content = &avail[hdr..hdr + len];
        packet.clear();
        let mut result = Next::Ready;
        match self.threshold {
            None => packet.extend_from_slice(content),
            Some(threshold) => {
                let Some((announced, n)) = peek_varint(content)? else {
                    return Err(rc_proto::Error::UnexpectedEof { needed: 1 }.into());
                };
                let data = &content[n..];
                if announced == 0 {
                    packet.extend_from_slice(data);
                } else {
                    let announced = usize::try_from(announced)
                        .map_err(|_| Error::PacketTooLarge { len: usize::MAX, max: MAX_UNCOMPRESSED_LEN })?;
                    if self.validate {
                        if announced < threshold {
                            return Err(Error::BelowThreshold { len: announced, threshold });
                        }
                        if announced > MAX_UNCOMPRESSED_LEN {
                            return Err(Error::PacketTooLarge { len: announced, max: MAX_UNCOMPRESSED_LEN });
                        }
                    } else if announced > MAX_UNCOMPRESSED_LEN
                        && announced > data.len().saturating_mul(MAX_DEFLATE_RATIO).saturating_add(64)
                    {
                        // Unreachable size: the game would fail the same way at the final comparison;
                        // we only avoid allocating for nothing.
                        return Err(Error::SizeMismatch { announced, actual: 0 });
                    }
                    if announced >= defer_from {
                        packet.extend_from_slice(data);
                        result = Next::Deferred { size: announced };
                    } else {
                        inflate_exact(&mut self.inflate, data, announced, packet)?;
                    }
                }
            }
        }
        self.head += hdr + len;
        Ok(result)
    }
}

/// Result of [`FrameDecoder::next_or_deferred`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Incomplete frame: wait for more bytes.
    Pending,
    /// Decompressed packet ready.
    Ready,
    /// Packet left compressed; `size` = announced decompressed size (already validated).
    Deferred { size: usize },
}

thread_local! {
    static INFLATE: std::cell::RefCell<Decompressor> = std::cell::RefCell::new(Decompressor::new());
}

/// Decompresses a packet returned by [`Next::Deferred`] (any thread).
pub fn inflate(compressed: &[u8], size: usize, out: &mut Vec<u8>) -> Result<()> {
    INFLATE.with(|d| inflate_exact(&mut d.borrow_mut(), compressed, size, out))
}

fn peek_frame_len(buf: &[u8]) -> Result<Option<(usize, usize)>> {
    match peek_varint(&buf[..buf.len().min(3)]) {
        Ok(Some((0, _))) => Err(Error::EmptyFrame),
        Ok(Some((len, hdr))) if len > 0 => Ok(Some((len as usize, hdr))),
        Ok(Some(_)) => Err(Error::FrameLengthTooWide),
        // Three bytes all with the continuation bit: prefix too wide.
        Ok(None) if buf.len() >= 3 => Err(Error::FrameLengthTooWide),
        Ok(None) => Ok(None),
        Err(_) => Err(Error::FrameLengthTooWide),
    }
}

/// Decompression in one call into a buffer of the announced size (libdeflate): the size
/// being known in advance, no loop or reallocation.
fn inflate_exact(z: &mut Decompressor, input: &[u8], announced: usize, out: &mut Vec<u8>) -> Result<()> {
    out.clear();
    out.resize(announced, 0);
    match z.zlib_decompress(input, out) {
        Ok(n) if n == announced => Ok(()),
        Ok(n) => Err(Error::SizeMismatch { announced, actual: n }),
        Err(DecompressionError::InsufficientSpace) => Err(Error::Zlib("stream longer than announced".into())),
        Err(DecompressionError::BadData) => Err(Error::Zlib("invalid or truncated data".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(id: i32, body_len: usize) -> Vec<u8> {
        let mut p = Vec::new();
        write_varint(&mut p, id);
        p.extend((0..body_len).map(|i| (i % 13) as u8));
        p
    }

    fn pipe(enc: &mut FrameEncoder, dec: &mut FrameDecoder, packets: &[Vec<u8>], chunk: usize) {
        let mut wire = Vec::new();
        for p in packets {
            enc.encode(p, &mut wire).unwrap();
        }
        let mut got = Vec::new();
        let mut buf = Vec::new();
        for c in wire.chunks(chunk) {
            dec.push(c);
            while dec.next_packet(&mut buf).unwrap() {
                got.push(buf.clone());
            }
        }
        assert_eq!(got, packets);
        assert_eq!(dec.pending(), 0);
    }

    #[test]
    fn plain_frames() {
        let packets = [packet(0, 0), packet(0x2b, 300), packet(1, 70_000)];
        for chunk in [1, 7, 4096, usize::MAX] {
            pipe(&mut FrameEncoder::new(), &mut FrameDecoder::new(), &packets, chunk);
        }
        let mut wire = Vec::new();
        FrameEncoder::new().encode(&[0x00, 0x01], &mut wire).unwrap();
        assert_eq!(wire, [2, 0, 1]);
    }

    #[test]
    fn compressed_frames_both_sides_of_threshold() {
        let packets = [packet(3, 10), packet(4, 255), packet(5, 256), packet(6, 1 << 20)];
        let (mut e, mut d) = (FrameEncoder::new(), FrameDecoder::new());
        e.set_compression(256);
        d.set_compression(256);
        pipe(&mut e, &mut d, &packets, 1000);
        // Below the threshold: size 0 then raw data.
        let mut wire = Vec::new();
        e.encode(&[9, 9], &mut wire).unwrap();
        assert_eq!(wire, [3, 0, 9, 9]);
    }

    #[test]
    fn encrypted_and_compressed() {
        let secret = [0x5a; 16];
        let packets = [packet(1, 5), packet(2, 5000), packet(3, 0)];
        let (mut e, mut d) = (FrameEncoder::new(), FrameDecoder::new());
        e.set_compression(64);
        d.set_compression(64);
        e.enable_encryption(&secret);
        d.enable_encryption(&secret);
        pipe(&mut e, &mut d, &packets, 3);
    }

    #[test]
    fn encryption_enabled_with_bytes_already_buffered() {
        let secret = [1; 16];
        let mut e = FrameEncoder::new();
        let mut wire = Vec::new();
        e.encode(&packet(7, 3), &mut wire).unwrap();
        e.enable_encryption(&secret);
        e.encode(&packet(8, 3), &mut wire).unwrap();
        let mut d = FrameDecoder::new();
        d.push(&wire);
        let mut buf = Vec::new();
        assert!(d.next_packet(&mut buf).unwrap());
        assert_eq!(buf, packet(7, 3));
        d.enable_encryption(&secret);
        assert!(d.next_packet(&mut buf).unwrap());
        assert_eq!(buf, packet(8, 3));
    }

    #[test]
    fn deferred_frames_inflate_to_the_same_packets() {
        let packets = [packet(1, 10), packet(2, 300), packet(3, 70_000)];
        let mut e = FrameEncoder::new();
        e.set_compression(256);
        let mut wire = Vec::new();
        for p in &packets {
            e.encode(p, &mut wire).unwrap();
        }
        let mut d = FrameDecoder::new();
        d.set_compression(256);
        d.push(&wire);
        let mut buf = Vec::new();
        let mut got = Vec::new();
        loop {
            match d.next_or_deferred(&mut buf, 1000).unwrap() {
                Next::Pending => break,
                Next::Ready => got.push(buf.clone()),
                Next::Deferred { size } => {
                    let mut out = Vec::new();
                    inflate(&buf, size, &mut out).unwrap();
                    assert!(size >= 1000);
                    got.push(out);
                }
            }
        }
        assert_eq!(got, packets);
    }

    #[test]
    fn negative_threshold_disables_compression() {
        let mut e = FrameEncoder::new();
        e.set_compression(-1);
        let mut wire = Vec::new();
        e.encode(&[1, 2, 3], &mut wire).unwrap();
        assert_eq!(wire, [3, 1, 2, 3]);
    }

    /// Frame compressed "by hand" (announced size = actual size), whatever the threshold.
    fn compressed_frame(packet: &[u8]) -> Vec<u8> {
        let mut content = Vec::new();
        write_varint(&mut content, packet.len() as i32);
        deflate_into(&mut Compressor::new(CompressionLvl::default()), packet, &mut content).unwrap();
        let mut frame = Vec::new();
        write_frame(&mut frame, &content).unwrap();
        frame
    }

    #[test]
    fn client_mode_accepts_sizes_the_server_mode_rejects() {
        let mut buf = Vec::new();
        // Below the threshold: the game's client accepts it.
        let small = packet(5, 10);
        let mut d = FrameDecoder::new();
        d.set_compression_unvalidated(256);
        d.push(&compressed_frame(&small));
        assert!(d.next_packet(&mut buf).unwrap());
        assert_eq!(buf, small);
        let mut d = FrameDecoder::new();
        d.set_compression(256);
        d.push(&compressed_frame(&small));
        assert!(matches!(d.next_packet(&mut buf), Err(Error::BelowThreshold { .. })));

        // Beyond 8 MiB (zeros: highly compressible).
        let big = vec![0u8; MAX_UNCOMPRESSED_LEN + 7];
        let mut d = FrameDecoder::new();
        d.set_compression_unvalidated(256);
        d.push(&compressed_frame(&big));
        assert!(d.next_packet(&mut buf).unwrap());
        assert_eq!(buf.len(), big.len());
        let mut d = FrameDecoder::new();
        d.set_compression(256);
        d.push(&compressed_frame(&big));
        assert!(matches!(d.next_packet(&mut buf), Err(Error::PacketTooLarge { .. })));

        // Unreachable announcement (empty data): error without a giant allocation.
        let mut content = Vec::new();
        write_varint(&mut content, i32::MAX);
        let mut frame = Vec::new();
        write_frame(&mut frame, &content).unwrap();
        let mut d = FrameDecoder::new();
        d.set_compression_unvalidated(256);
        d.push(&frame);
        assert!(matches!(d.next_packet(&mut buf), Err(Error::SizeMismatch { .. })));
    }

    #[test]
    fn rejects_bad_frames() {
        let mut buf = Vec::new();
        let mut d = FrameDecoder::new();
        d.push(&[0x00]);
        assert!(matches!(d.next_packet(&mut buf), Err(Error::EmptyFrame)));

        let mut d = FrameDecoder::new();
        d.push(&[0x80, 0x80, 0x80, 0x01]);
        assert!(matches!(d.next_packet(&mut buf), Err(Error::FrameLengthTooWide)));

        let mut d = FrameDecoder::new();
        d.push(&[0x80, 0x80]);
        assert!(!d.next_packet(&mut buf).unwrap());

        // Compressed announcement below the threshold.
        let mut d = FrameDecoder::new();
        d.set_compression(256);
        d.push(&[2, 10, 0]);
        assert!(matches!(d.next_packet(&mut buf), Err(Error::BelowThreshold { len: 10, threshold: 256 })));

        // Announcement beyond 8 MiB.
        let mut content = Vec::new();
        write_varint(&mut content, (MAX_UNCOMPRESSED_LEN + 1) as i32);
        let mut frame = Vec::new();
        write_frame(&mut frame, &content).unwrap();
        let mut d = FrameDecoder::new();
        d.set_compression(256);
        d.push(&frame);
        assert!(matches!(d.next_packet(&mut buf), Err(Error::PacketTooLarge { .. })));

        assert!(matches!(
            FrameEncoder::new().encode(&vec![0; MAX_FRAME_LEN + 1], &mut Vec::new()),
            Err(Error::PacketTooLarge { .. })
        ));
    }

    #[test]
    fn size_mismatch_detected() {
        // 300-byte zlib stream announced as 400.
        let data = vec![1u8; 300];
        let mut z = Vec::new();
        deflate_into(&mut Compressor::new(CompressionLvl::default()), &data, &mut z).unwrap();
        let mut content = Vec::new();
        write_varint(&mut content, 400);
        content.extend_from_slice(&z);
        let mut frame = Vec::new();
        write_frame(&mut frame, &content).unwrap();
        let mut d = FrameDecoder::new();
        d.set_compression(256);
        d.push(&frame);
        let mut buf = Vec::new();
        assert!(matches!(d.next_packet(&mut buf), Err(Error::SizeMismatch { announced: 400, actual: 300 })));
    }
}

/// The official server compresses with zlib: our frames must read its streams back, and
/// ours must remain readable by any zlib decompressor.
#[cfg(test)]
mod interop {
    use std::io::{Read, Write};

    use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};

    use super::*;

    fn sample() -> Vec<u8> {
        (0..100_000u32).map(|i| if i % 7 == 0 { (i >> 5) as u8 } else { 3 }).collect()
    }

    #[test]
    fn reads_zlib_streams() {
        let data = sample();
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(&data).unwrap();
        let z = e.finish().unwrap();
        let mut out = Vec::new();
        inflate_exact(&mut Decompressor::new(), &z, data.len(), &mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn zlib_reads_our_streams() {
        let data = sample();
        let mut z = Vec::new();
        deflate_into(&mut Compressor::new(CompressionLvl::default()), &data, &mut z).unwrap();
        let mut out = Vec::new();
        ZlibDecoder::new(&z[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn longer_than_announced_is_rejected() {
        let data = sample();
        let mut z = Vec::new();
        deflate_into(&mut Compressor::new(CompressionLvl::default()), &data, &mut z).unwrap();
        let mut out = Vec::new();
        assert!(inflate_exact(&mut Decompressor::new(), &z, data.len() - 1, &mut out).is_err());
    }
}
