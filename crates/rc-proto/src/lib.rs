//! Basic protocol types and codecs, shared by all versions.
//!
//! Zero-copy reading (`Reader` over a borrowed slice), writing into a `Vec<u8>`.
//! Encoding and decoding in both directions: the same crate serves the client, the capture
//! proxy and a possible server.

mod codec;
mod error;
pub mod nbt;
mod reader;
mod types;
pub mod varint;

pub use codec::{Decode, Encode, read_count, read_len, read_str, write_len, write_str};
pub use error::{Error, Result};
pub use reader::Reader;
pub use types::{Angle, BlockPos, Bounded, VarInt, VarLong, check_enum};

/// Default maximum length of a string, in UTF-16 units.
pub const STRING_MAX_DEFAULT: usize = 32767;
