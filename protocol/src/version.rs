use std::fmt;

use serde::{Deserialize, Serialize};

/// The version of the management protocol.
///
/// Peers with the same major version are compatible.
/// The minor version is informative and may grow without breaking the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const CURRENT: Self = Self::new(1, 0);

    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Whether two peers may communicate with each other.
    pub const fn is_compatible(self, other: Self) -> bool {
        self.major == other.major
    }
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_requires_a_matching_major_version() {
        assert!(ProtocolVersion::new(1, 0).is_compatible(ProtocolVersion::new(1, 7)));
        assert!(!ProtocolVersion::new(1, 0).is_compatible(ProtocolVersion::new(2, 0)));
    }
}
