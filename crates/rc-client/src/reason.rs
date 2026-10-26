//! Local reasons for the end of a connection, rendered like the game: translation key
//! (`disconnect.timeout`, `disconnect.endOfStream`…) or exception text displayed in
//! `disconnect.genericReason` ("Internal Exception: …" when reading, raw message when
//! opening the connection). The exception messages follow those of the game running on
//! Windows (Java/Netty network stack), the only texts a player sees there.
//!
//! Encoding in the `Event::Disconnected` string: a reserved leading character indicates the
//! kind; a string without that character is plain text (old reasons, singleplayer).

use crate::Error;

const KEY: char = '\u{1}';
const INTERNAL: char = '\u{2}';
const CONNECT: char = '\u{3}';

/// Decoded reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason<'a> {
    /// Translation key only.
    Key(&'a str),
    /// Error during the session: `disconnect.genericReason` with "Internal Exception:" + text.
    Internal(&'a str),
    /// Failure when opening the connection: `disconnect.genericReason` with the message.
    Connect(&'a str),
    /// Text as is.
    Plain(&'a str),
}

pub fn key(k: &str) -> String {
    format!("{KEY}{k}")
}

pub fn internal(exception: &str) -> String {
    format!("{INTERNAL}{exception}")
}

pub fn connect(message: &str) -> String {
    format!("{CONNECT}{message}")
}

pub fn parse(s: &str) -> Reason<'_> {
    if let Some(k) = s.strip_prefix(KEY) {
        Reason::Key(k)
    } else if let Some(e) = s.strip_prefix(INTERNAL) {
        Reason::Internal(e)
    } else if let Some(m) = s.strip_prefix(CONNECT) {
        Reason::Connect(m)
    } else {
        Reason::Plain(s)
    }
}

/// End of stream: the server closed the connection.
pub fn end_of_stream() -> String {
    key("disconnect.endOfStream")
}

/// Decoding exception of a named packet (`clientbound/minecraft:…`).
pub fn decode_failed(packet: &str) -> String {
    internal(&format!("io.netty.handler.codec.DecoderException: Failed to decode packet 'clientbound/{packet}'"))
}

/// Socket read error during the session.
pub fn from_io(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    match e.kind() {
        // Read timeout of 30 s exceeded (the game's timeout handler).
        K::TimedOut | K::WouldBlock => key("disconnect.timeout"),
        K::UnexpectedEof => end_of_stream(),
        K::ConnectionReset | K::ConnectionAborted => internal("java.net.SocketException: Connection reset"),
        _ => internal(&format!("java.io.IOException: {e}")),
    }
}

/// Transport error (frames, compression) outside the decoding of a packet.
pub fn from_net(e: &rc_net::Error) -> String {
    use rc_net::Error as N;
    let corrupted = |m: &str| internal(&format!("io.netty.handler.codec.CorruptedFrameException: {m}"));
    let decoder = |m: &str| internal(&format!("io.netty.handler.codec.DecoderException: {m}"));
    match e {
        N::FrameLengthTooWide => corrupted("length wider than 21-bit"),
        N::EmptyFrame => corrupted("Frame length cannot be zero"),
        N::SizeMismatch { announced, actual } => decoder(&format!(
            "Badly compressed packet - actual length of uncompressed payload {actual} is does not match declared size {announced}"
        )),
        N::BelowThreshold { len, threshold } => {
            decoder(&format!("Badly compressed packet - size of {len} is below server threshold of {threshold}"))
        }
        N::PacketTooLarge { len, .. } => {
            decoder(&format!("Badly compressed packet - size of {len} is larger than protocol maximum of 8388608"))
        }
        N::Zlib(_) => decoder("java.util.zip.DataFormatException: invalid stored block lengths"),
        N::Proto(_) => decoder("Failed to decode packet"),
    }
}

/// Reason for a session error; `packet`: packet being decoded, if there is one.
pub fn from_error(e: &Error, packet: Option<&str>) -> String {
    match (e, packet) {
        (Error::Io(io), _) => from_io(io),
        (Error::Net(rc_net::Error::Proto(_)) | Error::Proto(_) | Error::Invalid(_), Some(p)) => decode_failed(p),
        (Error::Net(n), _) => from_net(n),
        (Error::Proto(p), None) => internal(&format!("io.netty.handler.codec.DecoderException: {p}")),
        (Error::Invalid(m), None) => internal(&format!("io.netty.handler.codec.DecoderException: {m}")),
        (Error::MissingPacket(m), _) => internal(&format!("java.lang.IllegalStateException: {m}")),
    }
}

/// Failure to open the TCP connection, message as the game's screen displays it (address
/// removed from the exception message).
pub fn from_connect(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::ConnectionRefused => connect("Connection refused: no further information"),
        K::TimedOut | K::WouldBlock => connect("connection timed out after 30000 ms: "),
        K::NetworkUnreachable => connect("Network is unreachable: no further information"),
        K::HostUnreachable => connect("No route to host: no further information"),
        _ => connect(&e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_mapping() {
        assert_eq!(parse(&key("disconnect.timeout")), Reason::Key("disconnect.timeout"));
        assert_eq!(parse("text"), Reason::Plain("text"));
        let timeout = std::io::Error::from(std::io::ErrorKind::TimedOut);
        assert_eq!(parse(&from_io(&timeout)), Reason::Key("disconnect.timeout"));
        let reset = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
        assert_eq!(parse(&from_io(&reset)), Reason::Internal("java.net.SocketException: Connection reset"));
        let e = Error::Proto(rc_proto::Error::UnexpectedEof { needed: 1 });
        assert_eq!(
            parse(&from_error(&e, Some("minecraft:set_entity_data"))),
            Reason::Internal(
                "io.netty.handler.codec.DecoderException: Failed to decode packet 'clientbound/minecraft:set_entity_data'"
            )
        );
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(parse(&from_connect(&refused)), Reason::Connect("Connection refused: no further information"));
    }
}
