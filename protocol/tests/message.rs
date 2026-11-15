use protocol::{Capability, CodecError, Command, HelloRequest, MessageBody, MessageType, Outcome};

#[test]
fn rejects_unknown_request_types() {
    let result = Command::decode_payload(MessageType::new(0x7fff), &[]);

    assert!(matches!(
        result,
        Err(CodecError::UnknownMessageType(0x7fff))
    ));
}

#[test]
fn rejects_unknown_response_types() {
    let result = Outcome::decode_payload(MessageType::new(0xfffe), &[]);

    assert!(matches!(
        result,
        Err(CodecError::UnknownMessageType(0xfffe))
    ));
}

#[test]
fn rejects_mismatched_message_types() -> Result<(), CodecError> {
    let payload = Command::Status.encode_payload()?;
    let result = Command::decode_payload(MessageType::HELLO_REQUEST, &payload);

    assert!(matches!(
        result,
        Err(CodecError::MessageTypeMismatch { .. })
    ));
    Ok(())
}

#[test]
fn preserves_unknown_capabilities_on_the_wire() -> Result<(), CodecError> {
    let request = HelloRequest {
        capabilities: vec![Capability::SIGNED_DEPLOYMENTS, Capability::new(0x1234)],
    };

    let payload = Command::Hello(request.clone()).encode_payload()?;
    let decoded = Command::decode_payload(MessageType::HELLO_REQUEST, &payload)?;

    assert_eq!(decoded, Command::Hello(request));
    Ok(())
}
