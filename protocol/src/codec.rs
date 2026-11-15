use std::marker::PhantomData;

use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use tokio_util::codec::{Decoder, Encoder};

use crate::error::CodecError;
use crate::message::{Command, MessageBody, MessageType, Outcome, RequestId};
use crate::version::ProtocolVersion;

/// Number of bytes in the big-endian frame length prefix.
pub const LENGTH_PREFIX_BYTES: usize = 4;

/// Maximum size of one encoded frame payload.
pub const MAX_FRAME_PAYLOAD_BYTES: usize = 1024 * 1024;

/// A versioned protocol message with a request identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope<B> {
    pub version: ProtocolVersion,
    pub request_id: RequestId,
    pub body: B,
}

impl<B> Envelope<B> {
    pub const fn new(version: ProtocolVersion, request_id: RequestId, body: B) -> Self {
        Self {
            version,
            request_id,
            body,
        }
    }

    pub const fn current(request_id: RequestId, body: B) -> Self {
        Self::new(ProtocolVersion::CURRENT, request_id, body)
    }

    /// Whether this envelope uses a protocol version compatible with this build.
    pub fn is_compatible(&self) -> bool {
        self.version.is_compatible(ProtocolVersion::CURRENT)
    }
}

impl<B: MessageBody> Envelope<B> {
    /// Encode this envelope into a complete length-prefixed frame.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let payload = self.body.encode_payload()?;
        let wire = WireFrame {
            version: self.version,
            request_id: self.request_id,
            message_type: self.body.message_type(),
            payload,
        };
        encode_wire_frame(&wire)
    }

    /// Decode a complete length-prefixed frame into an envelope.
    pub fn decode(frame: &[u8]) -> Result<Self, CodecError> {
        let wire = decode_wire_frame(frame)?;
        let body = B::decode_payload(wire.message_type, &wire.payload)?;
        Ok(Self {
            version: wire.version,
            request_id: wire.request_id,
            body,
        })
    }
}

/// A control plane request.
pub type Request = Envelope<Command>;

/// A server response.
pub type Response = Envelope<Outcome>;

/// A tokio codec for control plane requests.
pub type RequestCodec = ProtocolCodec<Command>;

/// A tokio codec for server responses.
pub type ResponseCodec = ProtocolCodec<Outcome>;

#[derive(Debug, Serialize, Deserialize)]
struct WireFrame {
    version: ProtocolVersion,
    request_id: RequestId,
    message_type: MessageType,
    payload: Vec<u8>,
}

fn encode_wire_frame(frame: &WireFrame) -> Result<Vec<u8>, CodecError> {
    let encoded = postcard::to_allocvec(frame).map_err(CodecError::Encode)?;
    if encoded.len() > MAX_FRAME_PAYLOAD_BYTES {
        return Err(CodecError::FrameTooLarge {
            actual: encoded.len(),
            max: MAX_FRAME_PAYLOAD_BYTES,
        });
    }

    let length = u32::try_from(encoded.len()).map_err(|_| CodecError::FrameTooLarge {
        actual: encoded.len(),
        max: MAX_FRAME_PAYLOAD_BYTES,
    })?;

    let mut bytes = Vec::with_capacity(LENGTH_PREFIX_BYTES + encoded.len());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

fn decode_wire_frame(frame: &[u8]) -> Result<WireFrame, CodecError> {
    let payload = frame_payload(frame)?;
    postcard::from_bytes(payload).map_err(CodecError::Decode)
}

fn frame_payload(frame: &[u8]) -> Result<&[u8], CodecError> {
    if frame.len() < LENGTH_PREFIX_BYTES {
        return Err(CodecError::FrameTooShort {
            actual: frame.len(),
            prefix: LENGTH_PREFIX_BYTES,
        });
    }

    let declared = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    if declared > MAX_FRAME_PAYLOAD_BYTES {
        return Err(CodecError::FrameTooLarge {
            actual: declared,
            max: MAX_FRAME_PAYLOAD_BYTES,
        });
    }

    let payload = &frame[LENGTH_PREFIX_BYTES..];
    if payload.len() != declared {
        return Err(CodecError::FrameLengthMismatch {
            declared,
            actual: payload.len(),
        });
    }
    Ok(payload)
}

fn frame_length(source: &[u8]) -> Result<Option<usize>, CodecError> {
    if source.len() < LENGTH_PREFIX_BYTES {
        return Ok(None);
    }

    let length = u32::from_be_bytes([source[0], source[1], source[2], source[3]]) as usize;
    if length > MAX_FRAME_PAYLOAD_BYTES {
        return Err(CodecError::FrameTooLarge {
            actual: length,
            max: MAX_FRAME_PAYLOAD_BYTES,
        });
    }
    Ok(Some(length))
}

/// A tokio codec for length-prefixed protocol frames.
pub struct ProtocolCodec<B> {
    marker: PhantomData<B>,
}

impl<B> ProtocolCodec<B> {
    pub const fn new() -> Self {
        Self {
            marker: PhantomData,
        }
    }
}

impl<B> Default for ProtocolCodec<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: MessageBody> Encoder<Envelope<B>> for ProtocolCodec<B> {
    type Error = CodecError;

    fn encode(&mut self, item: Envelope<B>, destination: &mut BytesMut) -> Result<(), Self::Error> {
        let frame = item.encode()?;
        destination.extend_from_slice(&frame);
        Ok(())
    }
}

impl<B: MessageBody> Decoder for ProtocolCodec<B> {
    type Item = Envelope<B>;
    type Error = CodecError;

    fn decode(&mut self, source: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(length) = frame_length(source)? else {
            return Ok(None);
        };

        let total = LENGTH_PREFIX_BYTES + length;
        if source.len() < total {
            source.reserve(total - source.len());
            return Ok(None);
        }

        let frame = source.split_to(total);
        Envelope::<B>::decode(&frame[..]).map(Some)
    }
}
