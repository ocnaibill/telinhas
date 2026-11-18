//! Telinhas signaling server.
//!
//! Keeps the ephemeral state of rooms in memory and relays SDP/ICE between
//! peers. No media goes through it. The wire format is described in
//! `docs/PROTOCOLO.md`.

pub mod code;
pub mod hub;
pub mod limiter;
pub mod server;

pub use hub::{Config, Hub, parse_ip_list};
pub use protocol::PROTOCOL_VERSION;
pub use server::{router, router_with_downloads, serve, serve_with_downloads};
pub use telinhas_protocol as protocol;
