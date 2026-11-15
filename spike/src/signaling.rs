//! Minimal client for the signaling server (`docs/PROTOCOLO.md`).

use std::sync::Once;

use anyhow::{Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

pub struct Signaling {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

/// A fresh 128-bit session token, hex encoded.
pub fn new_token() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}

fn install_tls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

impl Signaling {
    pub async fn connect(url: &str) -> Result<Self> {
        install_tls_provider();
        let (ws, _) = connect_async(url).await?;
        Ok(Self { ws })
    }

    pub async fn send(&mut self, mut message: Value) -> Result<()> {
        message["v"] = json!(1);
        self.ws
            .send(Message::Text(message.to_string().into()))
            .await?;
        Ok(())
    }

    /// Next protocol message; `error` messages become errors.
    pub async fn recv(&mut self) -> Result<Value> {
        loop {
            match self.ws.next().await {
                None => bail!("signaling connection closed"),
                Some(Err(error)) => return Err(error.into()),
                Some(Ok(Message::Text(text))) => {
                    let message: Value = serde_json::from_str(text.as_str())?;
                    if message["type"] == "error" {
                        bail!("server error: {message}");
                    }
                    return Ok(message);
                }
                Some(Ok(Message::Close(_))) => bail!("signaling connection closed"),
                Some(Ok(_)) => {}
            }
        }
    }

    /// Creates a room, or joins `code`, and returns the `room_joined` message.
    pub async fn enter(&mut self, code: Option<&str>, nickname: &str) -> Result<Value> {
        let token = new_token();
        match code {
            Some(code) => {
                self.send(json!({
                    "type": "join_room", "code": code,
                    "nickname": nickname, "session_token": token
                }))
                .await?;
            }
            None => {
                self.send(json!({
                    "type": "create_room", "nickname": nickname, "session_token": token
                }))
                .await?;
            }
        }
        loop {
            let message = self.recv().await?;
            if message["type"] == "room_joined" {
                return Ok(message);
            }
        }
    }

    /// Waits for a `signal` of the given kind and returns `(from, sdp json)`.
    pub async fn wait_signal(&mut self, kind: &str) -> Result<(String, String)> {
        loop {
            let message = self.recv().await?;
            if message["type"] == "signal" && message["kind"] == kind {
                let from = message["from"]
                    .as_str()
                    .ok_or_else(|| anyhow!("signal without sender"))?
                    .to_owned();
                let sdp = message["payload"]["sdp"]
                    .as_str()
                    .ok_or_else(|| anyhow!("signal without sdp"))?
                    .to_owned();
                return Ok((from, sdp));
            }
        }
    }

    /// Waits until another participant is in the room and returns their id.
    pub async fn wait_for_peer(&mut self, joined: &Value) -> Result<String> {
        if let Some(peer) = joined["peers"].as_array().and_then(|p| p.first()) {
            return Ok(peer["peer_id"].as_str().unwrap_or_default().to_owned());
        }
        loop {
            let message = self.recv().await?;
            if message["type"] == "peer_joined" {
                return Ok(message["peer"]["peer_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned());
            }
        }
    }
}
