//! A frame number carried inside the H.264 itself, as an SEI "user data
//! unregistered" NAL unit in front of each access unit. It survives the
//! network untouched and the decoder ignores it, so the viewer can tell the
//! broadcaster which picture it just showed.
//!
//! The number is written in ASCII digits so the payload never holds a zero
//! byte, which would need escaping in Annex B.

const UUID: &[u8; 16] = b"telinhas-stamp-0";

/// `unit` with the stamp for `counter` in front.
pub(crate) fn prepend(counter: u32, unit: &[u8]) -> Vec<u8> {
    let digits = counter.to_string();
    let size = UUID.len() + digits.len();
    let mut out = Vec::with_capacity(unit.len() + size + 8);
    out.extend_from_slice(&[0, 0, 0, 1, 0x06, 0x05, size as u8]);
    out.extend_from_slice(UUID);
    out.extend_from_slice(digits.as_bytes());
    out.push(0x80);
    out.extend_from_slice(unit);
    out
}

/// The frame number of an access unit, if it carries one.
pub(crate) fn read(unit: &[u8]) -> Option<u32> {
    let at = unit.windows(UUID.len()).position(|w| w == UUID)? + UUID.len();
    let digits: Vec<u8> = unit[at..]
        .iter()
        .copied()
        .take_while(u8::is_ascii_digit)
        .collect();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_number_survives_the_round_trip() {
        let unit = [0, 0, 0, 1, 0x65, 1, 2, 3];
        for counter in [0, 7, 59, 123_456, u32::MAX] {
            let stamped = prepend(counter, &unit);
            assert_eq!(read(&stamped), Some(counter));
            assert!(stamped.ends_with(&unit));
            assert!(
                !stamped[..stamped.len() - unit.len()][4..].contains(&0),
                "no zero byte inside the stamp"
            );
        }
        assert_eq!(read(&unit), None);
    }
}
