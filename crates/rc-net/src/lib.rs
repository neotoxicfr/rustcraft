//! Protocol transport, independent of sockets: frame delimiting, zlib compression,
//! AES-128/CFB8 encryption. Behaviour described in `docs/specs/transport.md`.
//!
//! Synchronous and with no per-byte allocation: the network thread pushes what it reads into a
//! `FrameDecoder` and collects complete packets; on sending, `FrameEncoder` produces
//! bytes ready to write to the socket.

mod cipher;
mod frame;

pub use cipher::Cfb8;
pub use frame::{FrameDecoder, FrameEncoder, MAX_FRAME_LEN, MAX_UNCOMPRESSED_LEN, Next, inflate};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("length prefix over 3 bytes")]
    FrameLengthTooWide,
    #[error("zero-length frame")]
    EmptyFrame,
    #[error("packet too large: {len} bytes (max {max})")]
    PacketTooLarge { len: usize, max: usize },
    #[error("badly compressed packet: size {len} below threshold {threshold}")]
    BelowThreshold { len: usize, threshold: usize },
    #[error("announced decompressed size {announced}, got {actual}")]
    SizeMismatch { announced: usize, actual: usize },
    #[error("invalid zlib data: {0}")]
    Zlib(String),
    #[error(transparent)]
    Proto(#[from] rc_proto::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
