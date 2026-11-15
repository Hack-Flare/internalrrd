use protocol::ProtocolVersion;

#[test]
fn compatibility_requires_a_matching_major_version() {
    assert!(ProtocolVersion::new(1, 0).is_compatible(ProtocolVersion::new(1, 7)));
    assert!(!ProtocolVersion::new(1, 0).is_compatible(ProtocolVersion::new(2, 0)));
}
