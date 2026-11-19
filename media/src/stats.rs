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

/// Where one connection to another person stands, in words for the diagnostics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkStatus {
    /// `criando`, `oferta enviada`, `connecting`, `connected`, `failed`...
    pub stage: String,
    /// The addresses this side offered, by kind: `host 192.168.0.5, srflx 203.0.113.9`.
    pub local: String,
    /// The addresses the other side offered.
    pub remote: String,
    /// Why it stopped, if it did.
    pub error: Option<String>,
}

/// The broadcaster's own numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sending {
    pub encoder: String,
    pub hardware: bool,
    pub width: u32,
    pub height: u32,
    /// Pictures leaving the encoder loop per second.
    pub fps: f32,
    /// New pictures the capture delivered per second: a still screen, or a
    /// game locked to a low rate, delivers fewer than `fps`.
    pub capture_fps: f32,
    pub kbps: f32,
    /// Taking the current picture from the capture, per picture.
    pub copy_ms: f32,
    /// Time inside the encoder per picture.
    pub encode_ms: f32,
}

/// The sound a viewer receives from one broadcaster.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioReceiving {
    pub packets: u64,
    /// Packets that never arrived, concealed by the decoder.
    pub lost: u64,
    pub decoded: u64,
    /// Loudest sample so far, 0 to 1.
    pub peak: f32,
    /// Sound waiting to be played, in milliseconds.
    pub buffered_ms: f32,
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
    pub audio: AudioReceiving,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// How each connection to another person is going, by peer and whether we
    /// are the broadcaster (`true`) or the viewer (`false`). It is what to read
    /// when nothing arrives.
    pub links: HashMap<(PeerId, bool), LinkStatus>,
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
