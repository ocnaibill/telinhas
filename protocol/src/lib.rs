//! Wire messages (see `docs/PROTOCOLO.md`), serializable in both directions so
//! the server and the clients share one definition.

use serde::{Deserialize, Serialize};
pub use serde_json::{Map, Value};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;

pub type PeerId = Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Offer,
    Answer,
    Ice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Screen,
    Webcam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Original,
    Economy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencySample {
    pub peer_id: PeerId,
    pub rtt_ms: f64,
}

/// Messages sent by clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    CreateRoom {
        nickname: String,
        session_token: String,
        #[serde(default)]
        require_approval: bool,
    },
    JoinRoom {
        code: String,
        nickname: String,
        session_token: String,
    },
    SetNickname {
        nickname: String,
    },
    SetApproval {
        require_approval: bool,
    },
    ApprovalDecision {
        request_id: String,
        accept: bool,
    },
    LeaveRoom,
    Signal {
        to: PeerId,
        kind: SignalKind,
        payload: Value,
    },
    StreamState {
        stream_id: String,
        kind: StreamKind,
        live: bool,
        #[serde(default)]
        meta: Value,
    },
    LayerRequest {
        to: PeerId,
        stream_id: String,
        layer: Layer,
    },
    LatencyReport {
        samples: Vec<LatencySample>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaveReason {
    Left,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedVersion,
    InvalidCode,
    RateLimited,
    JoinDenied,
    NotAdmin,
    InvalidNickname,
    BadMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub nickname: String,
    pub state: ConnectionState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    pub from: PeerId,
    pub stream_id: String,
    pub kind: StreamKind,
    pub meta: Value,
}

/// A relay (TURN) the clients may use when a direct connection is impossible,
/// with credentials that expire. The server hands them out in `room_joined`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}

/// Messages sent by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    RoomJoined {
        code: String,
        you: PeerInfo,
        peers: Vec<PeerInfo>,
        admin: PeerId,
        require_approval: bool,
        streams: Vec<StreamInfo>,
        /// Relays for connections that cannot go direct. Missing from older servers.
        #[serde(default)]
        ice_servers: Vec<IceServer>,
    },
    ApprovalPending,
    ApprovalRequest {
        request_id: String,
        nickname: String,
    },
    PeerJoined {
        peer: PeerInfo,
    },
    PeerLeft {
        peer_id: PeerId,
        reason: LeaveReason,
    },
    PeerConnection {
        peer_id: PeerId,
        state: ConnectionState,
    },
    PeerRenamed {
        peer_id: PeerId,
        nickname: String,
    },
    AdminChanged {
        peer_id: PeerId,
    },
    ApprovalChanged {
        require_approval: bool,
    },
    Signal {
        from: PeerId,
        kind: SignalKind,
        payload: Value,
    },
    StreamState {
        from: PeerId,
        stream_id: String,
        kind: StreamKind,
        live: bool,
        meta: Value,
    },
    LayerRequest {
        from: PeerId,
        stream_id: String,
        layer: Layer,
    },
    LatencyReport {
        from: PeerId,
        samples: Vec<LatencySample>,
    },
    RoomOverCapacity {
        count: usize,
    },
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_s: Option<u64>,
    },
}

/// Serializes a server message with the protocol version attached.
pub fn encode(message: &ServerMessage) -> String {
    #[derive(Serialize)]
    struct Envelope<'a> {
        v: u32,
        #[serde(flatten)]
        message: &'a ServerMessage,
    }
    serde_json::to_string(&Envelope {
        v: PROTOCOL_VERSION,
        message,
    })
    .expect("server messages always serialize")
}

/// Serializes a client message with the protocol version attached.
pub fn encode_client(message: &ClientMessage) -> String {
    #[derive(Serialize)]
    struct Envelope<'a> {
        v: u32,
        #[serde(flatten)]
        message: &'a ClientMessage,
    }
    serde_json::to_string(&Envelope {
        v: PROTOCOL_VERSION,
        message,
    })
    .expect("client messages always serialize")
}

/// Why a frame could not be read as a protocol message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Not JSON, or not a message this version knows.
    Malformed(String),
    /// The `v` field is missing.
    MissingVersion,
    /// The peer speaks another protocol version.
    UnsupportedVersion(u64),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Malformed(text) => write!(f, "malformed message: {text}"),
            DecodeError::MissingVersion => write!(f, "missing protocol version"),
            DecodeError::UnsupportedVersion(v) => write!(f, "unsupported protocol version {v}"),
        }
    }
}

impl std::error::Error for DecodeError {}

fn decode<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, DecodeError> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| DecodeError::Malformed(error.to_string()))?;
    match value.get("v").and_then(Value::as_u64) {
        Some(v) if v == u64::from(PROTOCOL_VERSION) => {}
        Some(v) => return Err(DecodeError::UnsupportedVersion(v)),
        None => return Err(DecodeError::MissingVersion),
    }
    serde_json::from_value(value).map_err(|error| DecodeError::Malformed(error.to_string()))
}

/// Reads a frame sent by the server.
pub fn decode_server(text: &str) -> Result<ServerMessage, DecodeError> {
    decode(text)
}

/// Reads a frame sent by a client.
pub fn decode_client(text: &str) -> Result<ClientMessage, DecodeError> {
    decode(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn peer(name: &str) -> PeerInfo {
        PeerInfo {
            peer_id: Uuid::from_u128(7),
            nickname: name.to_owned(),
            state: ConnectionState::Connected,
        }
    }

    #[test]
    fn decodes_create_room_with_default_approval() {
        let text = json!({
            "v": 1, "type": "create_room",
            "nickname": "Bianco", "session_token": "abc"
        })
        .to_string();
        assert!(matches!(
            decode_client(&text),
            Ok(ClientMessage::CreateRoom {
                require_approval: false,
                ..
            })
        ));
    }

    #[test]
    fn decodes_unit_variant() {
        let message = decode_client(&json!({ "v": 1, "type": "leave_room" }).to_string());
        assert_eq!(message, Ok(ClientMessage::LeaveRoom));
    }

    #[test]
    fn rejects_unknown_type_missing_and_wrong_version() {
        assert!(matches!(
            decode_client(&json!({ "v": 1, "type": "nope" }).to_string()),
            Err(DecodeError::Malformed(_))
        ));
        assert_eq!(
            decode_client(&json!({ "type": "leave_room" }).to_string()),
            Err(DecodeError::MissingVersion)
        );
        assert_eq!(
            decode_client(&json!({ "v": 2, "type": "leave_room" }).to_string()),
            Err(DecodeError::UnsupportedVersion(2))
        );
        assert!(matches!(
            decode_client("not json"),
            Err(DecodeError::Malformed(_))
        ));
    }

    #[test]
    fn every_client_message_survives_a_round_trip() {
        let messages = vec![
            ClientMessage::CreateRoom {
                nickname: "A".into(),
                session_token: "t".repeat(20),
                require_approval: true,
            },
            ClientMessage::JoinRoom {
                code: "ABCD-2345".into(),
                nickname: "A".into(),
                session_token: "t".repeat(20),
            },
            ClientMessage::SetNickname {
                nickname: "B".into(),
            },
            ClientMessage::SetApproval {
                require_approval: false,
            },
            ClientMessage::ApprovalDecision {
                request_id: "req-1".into(),
                accept: true,
            },
            ClientMessage::LeaveRoom,
            ClientMessage::Signal {
                to: Uuid::from_u128(1),
                kind: SignalKind::Offer,
                payload: json!({ "sdp": "v=0" }),
            },
            ClientMessage::StreamState {
                stream_id: "s".into(),
                kind: StreamKind::Screen,
                live: true,
                meta: json!({ "fps": 60 }),
            },
            ClientMessage::LayerRequest {
                to: Uuid::from_u128(2),
                stream_id: "s".into(),
                layer: Layer::Economy,
            },
            ClientMessage::LatencyReport {
                samples: vec![LatencySample {
                    peer_id: Uuid::from_u128(3),
                    rtt_ms: 12.5,
                }],
            },
        ];
        for message in messages {
            assert_eq!(decode_client(&encode_client(&message)), Ok(message));
        }
    }

    #[test]
    fn every_server_message_survives_a_round_trip() {
        let messages = vec![
            ServerMessage::RoomJoined {
                code: "ABCD-2345".into(),
                you: peer("A"),
                peers: vec![peer("B")],
                admin: Uuid::from_u128(7),
                require_approval: false,
                streams: vec![StreamInfo {
                    from: Uuid::from_u128(7),
                    stream_id: "s".into(),
                    kind: StreamKind::Webcam,
                    meta: json!(null),
                }],
                ice_servers: vec![IceServer {
                    urls: vec!["turn:relay.example:3478?transport=udp".into()],
                    username: "u".into(),
                    credential: "c".into(),
                }],
            },
            ServerMessage::ApprovalPending,
            ServerMessage::ApprovalRequest {
                request_id: "req-1".into(),
                nickname: "C".into(),
            },
            ServerMessage::PeerJoined { peer: peer("D") },
            ServerMessage::PeerLeft {
                peer_id: Uuid::from_u128(4),
                reason: LeaveReason::Timeout,
            },
            ServerMessage::PeerConnection {
                peer_id: Uuid::from_u128(4),
                state: ConnectionState::Reconnecting,
            },
            ServerMessage::PeerRenamed {
                peer_id: Uuid::from_u128(4),
                nickname: "E".into(),
            },
            ServerMessage::AdminChanged {
                peer_id: Uuid::from_u128(4),
            },
            ServerMessage::ApprovalChanged {
                require_approval: true,
            },
            ServerMessage::Signal {
                from: Uuid::from_u128(5),
                kind: SignalKind::Ice,
                payload: json!({}),
            },
            ServerMessage::StreamState {
                from: Uuid::from_u128(5),
                stream_id: "s".into(),
                kind: StreamKind::Screen,
                live: false,
                meta: json!(null),
            },
            ServerMessage::LayerRequest {
                from: Uuid::from_u128(5),
                stream_id: "s".into(),
                layer: Layer::Original,
            },
            ServerMessage::LatencyReport {
                from: Uuid::from_u128(5),
                samples: vec![],
            },
            ServerMessage::RoomOverCapacity { count: 13 },
            ServerMessage::Error {
                code: ErrorCode::RateLimited,
                message: "x".into(),
                retry_after_s: Some(30),
            },
            ServerMessage::Error {
                code: ErrorCode::InvalidCode,
                message: "y".into(),
                retry_after_s: None,
            },
        ];
        for message in messages {
            assert_eq!(decode_server(&encode(&message)), Ok(message));
        }
    }

    #[test]
    fn encodes_with_version_and_type() {
        let value: Value = serde_json::from_str(&encode(&ServerMessage::ApprovalPending)).unwrap();
        assert_eq!(value, json!({ "v": 1, "type": "approval_pending" }));
    }

    #[test]
    fn error_omits_missing_retry_after() {
        let text = encode(&ServerMessage::Error {
            code: ErrorCode::InvalidCode,
            message: "x".into(),
            retry_after_s: None,
        });
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["code"], "invalid_code");
        assert!(value.get("retry_after_s").is_none());
    }

    #[test]
    fn a_room_joined_from_an_older_server_has_no_relays() {
        let old = r#"{"v":1,"type":"room_joined","code":"X","you":{"peer_id":"00000000-0000-0000-0000-000000000001","nickname":"A","state":"connected"},"peers":[],"admin":"00000000-0000-0000-0000-000000000001","require_approval":false,"streams":[]}"#;
        match decode_server(old) {
            Ok(ServerMessage::RoomJoined { ice_servers, .. }) => assert!(ice_servers.is_empty()),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
