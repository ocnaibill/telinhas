//! Wire messages (see `docs/PROTOCOLO.md`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencySample {
    pub peer_id: PeerId,
    pub rtt_ms: f64,
}

/// Messages sent by clients.
#[derive(Debug, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaveReason {
    Left,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub nickname: String,
    pub state: ConnectionState,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamInfo {
    pub from: PeerId,
    pub stream_id: String,
    pub kind: StreamKind,
    pub meta: Value,
}

/// Messages sent by the server.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    RoomJoined {
        code: String,
        you: PeerInfo,
        peers: Vec<PeerInfo>,
        admin: PeerId,
        require_approval: bool,
        streams: Vec<StreamInfo>,
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
        #[serde(skip_serializing_if = "Option::is_none")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_create_room_with_default_approval() {
        let value = json!({
            "v": 1, "type": "create_room",
            "nickname": "Bianco", "session_token": "abc"
        });
        let message: ClientMessage = serde_json::from_value(value).unwrap();
        assert!(matches!(
            message,
            ClientMessage::CreateRoom {
                require_approval: false,
                ..
            }
        ));
    }

    #[test]
    fn decodes_unit_variant() {
        let message: ClientMessage =
            serde_json::from_value(json!({ "v": 1, "type": "leave_room" })).unwrap();
        assert!(matches!(message, ClientMessage::LeaveRoom));
    }

    #[test]
    fn rejects_unknown_type() {
        let result = serde_json::from_value::<ClientMessage>(json!({ "v": 1, "type": "nope" }));
        assert!(result.is_err());
    }

    #[test]
    fn encodes_with_version_and_type() {
        let text = encode(&ServerMessage::ApprovalPending);
        let value: Value = serde_json::from_str(&text).unwrap();
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
}
