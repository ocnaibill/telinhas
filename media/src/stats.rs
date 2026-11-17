//! What the media layer measures, for the interface to show.

use std::collections::HashMap;

use telinhas_protocol::PeerId;

/// Where the time goes between the broadcaster's screen and a viewer's, in
/// milliseconds. Measured by the broadcaster from the viewer's confirmations,
/// so it needs no synchronized clocks: the network part includes the way back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    /// From capturing a picture until its confirmation is back.
    pub total_ms: f32,
    /// Capture, encode and the wait to be sent.
    pub encode_ms: f32,
    /// Network both ways, including the sender's pacing queue and jitter.
    pub network_ms: f32,
    /// From the picture arriving whole to being decoded, at the viewer.
    pub viewer_ms: f32,
    /// What the congestion controller thinks this path carries.
    pub estimate_kbps: f32,
}

/// The broadcaster's own numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sending {
    pub encoder: String,
    pub hardware: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub kbps: f32,
    /// Time inside the encoder per picture.
    pub encode_ms: f32,
}

/// A viewer's numbers about one broadcaster.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Receiving {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub kbps: f32,
    /// Time inside the decoder per picture.
    pub decode_ms: f32,
    /// Pictures waiting to be decoded: above a few, the decoder is too slow.
    pub backlog: usize,
    /// The path as the broadcaster measured it, sent back to us.
    pub path: Path,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub sending: Option<Sending>,
    pub viewers: HashMap<PeerId, Path>,
    pub receiving: HashMap<PeerId, Receiving>,
}

/// A running average that follows quickly but does not jump on one sample.
pub(crate) fn smooth(old: f32, new: f32) -> f32 {
    if old == 0.0 {
        new
    } else {
        old * 0.8 + new * 0.2
    }
}
