//! M2 spike: synthetic H.264 over WebRTC between two peers.
//!
//! Exists to answer one question for DEC-38: is `webrtc-rs` good enough?
//! It is throwaway code, not the app.

pub mod link;
pub mod media;
pub mod signaling;
pub mod source;
pub mod stats;
