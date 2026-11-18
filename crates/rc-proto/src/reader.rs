use crate::{Error, Result};

/// Read cursor over a borrowed slice. Decoded strings and byte arrays
/// point into the original slice, without copying.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// The unread remainder, without advancing.
    pub fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let rem = self.remaining();
        if n > rem {
            return Err(Error::UnexpectedEof { needed: n - rem });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn take_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(s);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        let rem = self.remaining();
        if rem == 0 {
            return Err(Error::UnexpectedEof { needed: 1 });
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        Ok(b)
    }

    /// Consumes the whole remainder ("remaining bytes" field at the end of a packet).
    pub fn take_rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }

    /// Error if bytes remain: a packet must be read exactly.
    pub fn finish(&self) -> Result<()> {
        match self.remaining() {
            0 => Ok(()),
            remaining => Err(Error::TrailingBytes { remaining }),
        }
    }
}
