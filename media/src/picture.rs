/// A decoded picture, as three tightly packed planes (I420).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}
