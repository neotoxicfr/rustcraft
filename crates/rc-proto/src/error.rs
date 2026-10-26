use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("unexpected end of data: {needed} byte(s) missing")]
    UnexpectedEof { needed: usize },
    #[error("variable-length integer too long (more than {max} bytes)")]
    VarIntTooLong { max: usize },
    #[error("invalid length: {len} (max {max})")]
    LengthOutOfRange { len: i64, max: usize },
    #[error("invalid UTF-8 string")]
    InvalidUtf8,
    #[error("invalid boolean: {0:#04x}")]
    InvalidBool(u8),
    #[error("{remaining} byte(s) unread at the end of the packet")]
    TrailingBytes { remaining: usize },
    #[error("invalid value for {what}: {value}")]
    InvalidValue { what: &'static str, value: i64 },
}
