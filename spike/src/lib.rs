//! M2 spike: synthetic H.264 over WebRTC between two peers.
//!
//! Exists to answer one question for DEC-38: is `webrtc-rs` good enough?
//! It is throwaway code, not the app.

pub mod audio_cmd;
pub mod capture_cmd;
pub mod encode_cmd;
pub mod frames;
pub mod link;
pub mod measure;
pub mod media;
pub mod probe;
pub mod signaling;
pub mod source;
pub mod stats;
