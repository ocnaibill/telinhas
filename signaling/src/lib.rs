//! Telinhas signaling server.

/// Version of the signaling protocol spoken by this build (see `docs/PROTOCOLO.md`).
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_starts_at_one() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
