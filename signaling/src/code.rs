//! Room codes (DEC-07): 8 symbols from a 32-symbol Crockford base32 alphabet.

use std::fmt;

/// Crockford base32 without `I`, `L`, `O` and `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 8;

/// A normalized room code, stored as 8 uppercase alphabet symbols.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoomCode([u8; CODE_LEN]);

impl RoomCode {
    /// Draws a fresh code from the thread-local cryptographically secure RNG.
    pub fn generate() -> Self {
        let mut symbols = [0u8; CODE_LEN];
        for symbol in &mut symbols {
            *symbol = ALPHABET[rand::random_range(0..ALPHABET.len())];
        }
        Self(symbols)
    }

    /// Parses user input: case-insensitive, hyphens and spaces ignored.
    /// Follows Crockford's rules and reads `O` as `0` and `I`/`L` as `1`.
    pub fn parse(input: &str) -> Option<Self> {
        let mut symbols = [0u8; CODE_LEN];
        let mut count = 0;
        for ch in input.chars() {
            if ch == '-' || ch.is_whitespace() {
                continue;
            }
            let upper = ch.to_ascii_uppercase();
            let mapped = match upper {
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            };
            if !mapped.is_ascii() || count == CODE_LEN {
                return None;
            }
            let byte = mapped as u8;
            if !ALPHABET.contains(&byte) {
                return None;
            }
            symbols[count] = byte;
            count += 1;
        }
        (count == CODE_LEN).then_some(Self(symbols))
    }
}

impl fmt::Display for RoomCode {
    /// Formats as `XXXX-XXXX`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: String = self.0.iter().map(|&b| b as char).collect();
        write!(f, "{}-{}", &text[..4], &text[4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_use_the_alphabet_and_display_with_a_hyphen() {
        for _ in 0..200 {
            let code = RoomCode::generate();
            let shown = code.to_string();
            assert_eq!(shown.len(), 9);
            assert_eq!(shown.as_bytes()[4], b'-');
            assert!(
                shown
                    .bytes()
                    .filter(|&b| b != b'-')
                    .all(|b| ALPHABET.contains(&b))
            );
            assert_eq!(RoomCode::parse(&shown), Some(code));
        }
    }

    #[test]
    fn parse_accepts_lowercase_and_missing_hyphen() {
        let a = RoomCode::parse("abcd-2345").unwrap();
        let b = RoomCode::parse("ABCD2345").unwrap();
        let c = RoomCode::parse(" abcd 2345 ").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a.to_string(), "ABCD-2345");
    }

    #[test]
    fn parse_maps_ambiguous_letters_like_crockford() {
        assert_eq!(RoomCode::parse("OOII-LL00"), RoomCode::parse("0011-1100"));
    }

    #[test]
    fn parse_rejects_malformed_input() {
        assert!(RoomCode::parse("").is_none());
        assert!(RoomCode::parse("ABCD-234").is_none());
        assert!(RoomCode::parse("ABCD-23456").is_none());
        assert!(RoomCode::parse("ABCD-234U").is_none());
        assert!(RoomCode::parse("ABCD-234!").is_none());
        assert!(RoomCode::parse("ABCD-23é5").is_none());
    }
}
