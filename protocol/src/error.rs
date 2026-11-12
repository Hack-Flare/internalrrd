use std::io;

/// Errors raised while framing or encoding protocol messages.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("transport I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("failed to encode message: {0}")]
    Encode(#[source] postcard::Error),
    #[error("failed to decode message: {0}")]
    Decode(#[source] postcard::Error),
    #[error("frame payload is {actual} bytes, exceeding the {max} byte limit")]
    FrameTooLarge { actual: usize, max: usize },
    #[error("frame is {actual} bytes and cannot contain a {prefix} byte length prefix")]
    FrameTooShort { actual: usize, prefix: usize },
    #[error("frame declares {declared} payload bytes but contains {actual}")]
    FrameLengthMismatch { declared: usize, actual: usize },
    #[error("unknown message type 0x{0:04x}")]
    UnknownMessageType(u16),
    #[error("message type 0x{declared:04x} does not match payload type 0x{actual:04x}")]
    MessageTypeMismatch { declared: u16, actual: u16 },
}
