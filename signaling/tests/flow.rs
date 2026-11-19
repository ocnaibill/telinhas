//! End-to-end tests of the flow in `docs/PROTOCOLO.md`, against a real server
//! on a random local port.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use telinhas_signaling::{Config, Hub, serve};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

const WAIT: Duration = Duration::from_secs(3);

fn fast() -> Config {
    Config {
        reconnect_grace: Duration::from_millis(300),
        empty_room_grace: Duration::from_millis(600),
        approval_timeout: Duration::from_millis(300),
        ..Config::default()
    }
}

async fn start(config: Config) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Hub::new(config)));
    addr
}

fn token(n: u8) -> String {
    format!("session-token-{n:03}")
}

struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Client {
    async fn connect(addr: SocketAddr) -> Self {
        Self::connect_as(addr, None).await
    }

    /// Connects pretending to come through the tunnel from `ip`.
    async fn connect_as(addr: SocketAddr, ip: Option<&str>) -> Self {
        let mut request = format!("ws://{addr}/ws").into_client_request().unwrap();
        if let Some(ip) = ip {
            request
                .headers_mut()
                .insert("cf-connecting-ip", ip.parse().unwrap());
        }
        let (ws, _) = connect_async(request).await.unwrap();
        Self { ws }
    }

    async fn send(&mut self, mut message: Value) {
        message["v"] = json!(1);
        self.send_raw(&message.to_string()).await;
    }

    async fn send_raw(&mut self, text: &str) {
        self.ws
            .send(Message::Text(text.to_owned().into()))
            .await
            .unwrap();
    }

    async fn recv(&mut self) -> Value {
        loop {
            let frame = timeout(WAIT, self.ws.next())
                .await
                .expect("timed out waiting for a message")
                .expect("connection closed")
                .expect("websocket error");
            match frame {
                Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
                Message::Close(_) => panic!("connection closed by the server"),
                _ => {}
            }
        }
    }

    async fn recv_type(&mut self, kind: &str) -> Value {
        let message = self.recv().await;
        assert_eq!(message["type"], kind, "unexpected message: {message}");
        message
    }

    async fn recv_error(&mut self, code: &str) -> Value {
        let message = self.recv_type("error").await;
        assert_eq!(message["code"], code, "unexpected error: {message}");
        message
    }

    async fn expect_closed(&mut self) {
        loop {
            match timeout(WAIT, self.ws.next())
                .await
                .expect("not closed in time")
            {
                None | Some(Err(_) | Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    }

    async fn expect_silence(&mut self, wait: Duration) {
        if let Ok(Some(Ok(Message::Text(text)))) = timeout(wait, self.ws.next()).await {
            panic!("unexpected message: {text}");
        }
    }
}

struct Joined {
    client: Client,
    code: String,
    peer_id: String,
}

async fn create(addr: SocketAddr, nickname: &str, n: u8, approval: bool) -> Joined {
    let mut client = Client::connect(addr).await;
    client
        .send(json!({
            "type": "create_room", "nickname": nickname,
            "session_token": token(n), "require_approval": approval
        }))
        .await;
    let joined = client.recv_type("room_joined").await;
    Joined {
        client,
        code: joined["code"].as_str().unwrap().to_owned(),
        peer_id: joined["you"]["peer_id"].as_str().unwrap().to_owned(),
    }
}

async fn join(addr: SocketAddr, code: &str, nickname: &str, n: u8) -> (Client, Value) {
    let mut client = Client::connect(addr).await;
    client
        .send(json!({
            "type": "join_room", "code": code, "nickname": nickname,
            "session_token": token(n)
        }))
        .await;
    let joined = client.recv_type("room_joined").await;
    (client, joined)
}

#[tokio::test]
async fn create_then_join_by_code() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;

    assert_eq!(a.code.len(), 9);
    assert_eq!(a.code.as_bytes()[4], b'-');

    let typed = a.code.replace('-', "").to_lowercase();
    let (_b, joined) = join(addr, &typed, "Beto", 2).await;

    assert_eq!(joined["admin"], a.peer_id);
    assert_eq!(joined["require_approval"], false);
    assert_eq!(joined["peers"].as_array().unwrap().len(), 1);
    assert_eq!(joined["peers"][0]["nickname"], "Ana");
    assert_eq!(joined["peers"][0]["state"], "connected");

    let announced = a.client.recv_type("peer_joined").await;
    assert_eq!(announced["peer"]["nickname"], "Beto");
}

#[tokio::test]
async fn people_who_join_get_the_relays_the_server_has() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hub = Hub::new(fast());
    tokio::spawn(serve(listener, std::sync::Arc::clone(&hub)));

    // Before the server has any relay the list is empty.
    let a = create(addr, "Ana", 1, false).await;
    let (_b, before) = join(addr, &a.code, "Beto", 2).await;
    assert_eq!(before["ice_servers"], json!([]));

    hub.set_ice_servers(vec![telinhas_signaling::protocol::IceServer {
        urls: vec!["turn:relay.example:3478?transport=udp".into()],
        username: "user".into(),
        credential: "secret".into(),
    }]);
    let (_c, after) = join(addr, &a.code, "Caio", 3).await;
    assert_eq!(
        after["ice_servers"],
        json!([{
            "urls": ["turn:relay.example:3478?transport=udp"],
            "username": "user",
            "credential": "secret"
        }])
    );
}

#[tokio::test]
async fn repeated_nicknames_get_a_suffix() {
    let addr = start(fast()).await;
    let a = create(addr, "Bianco", 1, false).await;
    let (_b, second) = join(addr, &a.code, "bianco", 2).await;
    let (_c, third) = join(addr, &a.code, "Bianco", 3).await;
    assert_eq!(second["you"]["nickname"], "bianco (2)");
    assert_eq!(third["you"]["nickname"], "Bianco (3)");
}

#[tokio::test]
async fn invalid_nicknames_are_rejected() {
    let addr = start(fast()).await;
    let mut client = Client::connect(addr).await;
    client
        .send(json!({ "type": "create_room", "nickname": "   ", "session_token": token(1) }))
        .await;
    client.recv_error("invalid_nickname").await;
    client
        .send(json!({
            "type": "create_room", "nickname": "x".repeat(25), "session_token": token(1)
        }))
        .await;
    client.recv_error("invalid_nickname").await;
}

#[tokio::test]
async fn unknown_and_malformed_codes_look_the_same() {
    let addr = start(fast()).await;
    let mut client = Client::connect(addr).await;
    for code in ["ZZZZ-ZZZZ", "nope", ""] {
        client
            .send(json!({
                "type": "join_room", "code": code, "nickname": "Ana",
                "session_token": token(1)
            }))
            .await;
        let error = client.recv_error("invalid_code").await;
        assert!(error.get("retry_after_s").is_none());
    }
}

#[tokio::test]
async fn too_many_wrong_codes_block_the_ip() {
    let addr = start(fast()).await;
    let owner = create(addr, "Ana", 1, false).await;
    let mut attacker = Client::connect_as(addr, Some("203.0.113.9")).await;
    let attempt = |code: String| {
        json!({
            "type": "join_room", "code": code, "nickname": "Eve",
            "session_token": token(9)
        })
    };
    for _ in 0..10 {
        attacker.send(attempt("ZZZZ-ZZZZ".into())).await;
        attacker.recv_error("invalid_code").await;
    }
    // Even the right code is refused while blocked.
    attacker.send(attempt(owner.code.clone())).await;
    let blocked = attacker.recv_error("rate_limited").await;
    assert!(blocked["retry_after_s"].as_u64().unwrap() > 0);

    // Another address is not affected.
    let mut other = Client::connect_as(addr, Some("203.0.113.10")).await;
    other.send(attempt("ZZZZ-ZZZZ".into())).await;
    other.recv_error("invalid_code").await;
}

#[tokio::test]
async fn forwarded_ip_header_is_ignored_from_untrusted_peers() {
    // The test client connects from 127.0.0.1; with no trusted proxies the
    // header must not let anyone pick their own IP.
    let config = Config {
        trusted_proxies: vec![],
        ..fast()
    };
    let addr = start(config).await;
    let mut client = Client::connect_as(addr, Some("203.0.113.1")).await;
    for i in 0..10 {
        let ip = format!("198.51.100.{i}");
        let mut spoof = Client::connect_as(addr, Some(&ip)).await;
        spoof
            .send(json!({
                "type": "join_room", "code": "ZZZZ-ZZZZ", "nickname": "Eve",
                "session_token": token(9)
            }))
            .await;
        spoof.recv_error("invalid_code").await;
    }
    // All attempts came from 127.0.0.1, so this one is blocked.
    client
        .send(json!({
            "type": "join_room", "code": "ZZZZ-ZZZZ", "nickname": "Eve",
            "session_token": token(9)
        }))
        .await;
    client.recv_error("rate_limited").await;
}

#[tokio::test]
async fn approval_accepted() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, true).await;

    let mut b = Client::connect(addr).await;
    b.send(json!({
        "type": "join_room", "code": a.code, "nickname": "Beto", "session_token": token(2)
    }))
    .await;
    b.recv_type("approval_pending").await;

    let request = a.client.recv_type("approval_request").await;
    assert_eq!(request["nickname"], "Beto");
    a.client
        .send(json!({
            "type": "approval_decision", "request_id": request["request_id"], "accept": true
        }))
        .await;

    let joined = b.recv_type("room_joined").await;
    assert_eq!(joined["admin"], a.peer_id);
    let announced = a.client.recv_type("peer_joined").await;
    assert_eq!(announced["peer"]["nickname"], "Beto");
}

#[tokio::test]
async fn approval_denied_closes_the_connection() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, true).await;
    let mut b = Client::connect(addr).await;
    b.send(json!({
        "type": "join_room", "code": a.code, "nickname": "Beto", "session_token": token(2)
    }))
    .await;
    b.recv_type("approval_pending").await;
    let request = a.client.recv_type("approval_request").await;
    a.client
        .send(json!({
            "type": "approval_decision", "request_id": request["request_id"], "accept": false
        }))
        .await;
    b.recv_error("join_denied").await;
    b.expect_closed().await;
}

#[tokio::test]
async fn approval_times_out_as_a_refusal() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, true).await;
    let mut b = Client::connect(addr).await;
    b.send(json!({
        "type": "join_room", "code": a.code, "nickname": "Beto", "session_token": token(2)
    }))
    .await;
    b.recv_type("approval_pending").await;
    a.client.recv_type("approval_request").await;
    b.recv_error("join_denied").await;
    b.expect_closed().await;
}

#[tokio::test]
async fn only_the_admin_changes_approval_and_turning_it_off_admits_the_queue() {
    let config = Config {
        approval_timeout: Duration::from_secs(5),
        ..fast()
    };
    let addr = start(config).await;
    let mut a = create(addr, "Ana", 1, false).await;
    let (mut b, _) = join(addr, &a.code, "Beto", 2).await;
    a.client.recv_type("peer_joined").await;

    b.send(json!({ "type": "set_approval", "require_approval": true }))
        .await;
    b.recv_error("not_admin").await;

    a.client
        .send(json!({ "type": "set_approval", "require_approval": true }))
        .await;
    let changed = a.client.recv_type("approval_changed").await;
    assert_eq!(changed["require_approval"], true);
    b.recv_type("approval_changed").await;

    let mut c = Client::connect(addr).await;
    c.send(json!({
        "type": "join_room", "code": a.code, "nickname": "Caio", "session_token": token(3)
    }))
    .await;
    c.recv_type("approval_pending").await;
    a.client.recv_type("approval_request").await;

    a.client
        .send(json!({ "type": "set_approval", "require_approval": false }))
        .await;
    a.client.recv_type("approval_changed").await;
    c.recv_type("room_joined").await;
}

#[tokio::test]
async fn admin_passes_to_the_oldest_when_the_admin_leaves() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;
    let (mut b, joined_b) = join(addr, &a.code, "Beto", 2).await;
    let (mut c, _) = join(addr, &a.code, "Caio", 3).await;
    let b_id = joined_b["you"]["peer_id"].as_str().unwrap().to_owned();
    b.recv_type("peer_joined").await; // Caio

    a.client.send(json!({ "type": "leave_room" })).await;

    for client in [&mut b, &mut c] {
        let left = client.recv_type("peer_left").await;
        assert_eq!(left["peer_id"], a.peer_id);
        assert_eq!(left["reason"], "left");
        let changed = client.recv_type("admin_changed").await;
        assert_eq!(changed["peer_id"], b_id);
    }
}

#[tokio::test]
async fn reconnecting_with_the_token_keeps_the_same_participant() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;
    let (b, joined_b) = join(addr, &a.code, "Beto", 2).await;
    let b_id = joined_b["you"]["peer_id"].as_str().unwrap().to_owned();
    a.client.recv_type("peer_joined").await;

    drop(b);
    let dropped = a.client.recv_type("peer_connection").await;
    assert_eq!(dropped["peer_id"], b_id);
    assert_eq!(dropped["state"], "reconnecting");

    // A different nickname on return is ignored: the stored one is restored.
    let (_b2, back) = join(addr, &a.code, "Outro", 2).await;
    assert_eq!(back["you"]["peer_id"], b_id);
    assert_eq!(back["you"]["nickname"], "Beto");
    let restored = a.client.recv_type("peer_connection").await;
    assert_eq!(restored["state"], "connected");

    // And the pending removal no longer fires.
    a.client.expect_silence(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn reconnecting_admin_keeps_the_role() {
    let addr = start(fast()).await;
    let a = create(addr, "Ana", 1, false).await;
    let (mut b, _) = join(addr, &a.code, "Beto", 2).await;

    drop(a.client);
    b.recv_type("peer_connection").await;
    let (_a2, back) = join(addr, &a.code, "Ana", 1).await;
    assert_eq!(back["admin"], a.peer_id);
    b.recv_type("peer_connection").await;
    b.expect_silence(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn dropped_participant_is_removed_after_the_grace_and_admin_moves() {
    let addr = start(fast()).await;
    let a = create(addr, "Ana", 1, false).await;
    let (mut b, joined_b) = join(addr, &a.code, "Beto", 2).await;
    let b_id = joined_b["you"]["peer_id"].as_str().unwrap().to_owned();

    drop(a.client);
    b.recv_type("peer_connection").await;
    let left = b.recv_type("peer_left").await;
    assert_eq!(left["peer_id"], a.peer_id);
    assert_eq!(left["reason"], "timeout");
    let changed = b.recv_type("admin_changed").await;
    assert_eq!(changed["peer_id"], b_id);
}

#[tokio::test]
async fn empty_room_keeps_its_code_and_restores_the_admin() {
    let addr = start(fast()).await;
    let a = create(addr, "Ana", 1, false).await;
    drop(a.client);

    // Grace is 300 ms, then the room stays for 600 ms more.
    sleep(Duration::from_millis(450)).await;
    let (mut a2, back) = join(addr, &a.code, "Ana", 1).await;
    assert_eq!(back["you"]["peer_id"], a.peer_id);
    assert_eq!(back["admin"], a.peer_id);

    // Coming back cancelled the expiry.
    sleep(Duration::from_millis(900)).await;
    let (_b, _) = join(addr, &a.code, "Beto", 2).await;
    a2.recv_type("peer_joined").await;
}

#[tokio::test]
async fn new_joiner_of_an_empty_room_becomes_admin() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, true).await;
    a.client.send(json!({ "type": "leave_room" })).await;
    sleep(Duration::from_millis(100)).await;

    let (_b, joined) = join(addr, &a.code, "Beto", 2).await;
    assert_eq!(joined["admin"], joined["you"]["peer_id"]);
    assert_eq!(joined["peers"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn empty_room_expires_and_the_code_is_invalid() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;
    a.client.send(json!({ "type": "leave_room" })).await;
    sleep(Duration::from_millis(900)).await;

    let mut b = Client::connect(addr).await;
    b.send(json!({
        "type": "join_room", "code": a.code, "nickname": "Beto", "session_token": token(2)
    }))
    .await;
    b.recv_error("invalid_code").await;
}

#[tokio::test]
async fn signals_are_relayed_to_the_target_only() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;
    let (mut b, joined_b) = join(addr, &a.code, "Beto", 2).await;
    let (mut c, _) = join(addr, &a.code, "Caio", 3).await;
    let b_id = joined_b["you"]["peer_id"].as_str().unwrap().to_owned();
    a.client.recv_type("peer_joined").await;
    a.client.recv_type("peer_joined").await;
    b.recv_type("peer_joined").await;

    a.client
        .send(json!({
            "type": "signal", "to": b_id, "kind": "offer", "payload": { "sdp": "v=0" }
        }))
        .await;
    let relayed = b.recv_type("signal").await;
    assert_eq!(relayed["from"], a.peer_id);
    assert_eq!(relayed["kind"], "offer");
    assert_eq!(relayed["payload"]["sdp"], "v=0");
    c.expect_silence(Duration::from_millis(200)).await;

    a.client
        .send(json!({
            "type": "signal", "to": "00000000-0000-4000-8000-000000000000",
            "kind": "ice", "payload": {}
        }))
        .await;
    a.client.recv_error("bad_message").await;
}

#[tokio::test]
async fn stream_state_reaches_peers_and_late_joiners() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;
    let (mut b, _) = join(addr, &a.code, "Beto", 2).await;
    a.client.recv_type("peer_joined").await;

    a.client
        .send(json!({
            "type": "stream_state", "stream_id": "screen-1", "kind": "screen",
            "live": true, "meta": { "width": 1920, "height": 1080, "fps": 60 }
        }))
        .await;
    let seen = b.recv_type("stream_state").await;
    assert_eq!(seen["from"], a.peer_id);
    assert_eq!(seen["live"], true);
    assert_eq!(seen["meta"]["fps"], 60);

    let (_c, joined) = join(addr, &a.code, "Caio", 3).await;
    let streams = joined["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["stream_id"], "screen-1");
    assert_eq!(streams[0]["kind"], "screen");

    // When the streamer leaves, the stream is ended before the departure.
    b.recv_type("peer_joined").await;
    a.client.send(json!({ "type": "leave_room" })).await;
    let ended = b.recv_type("stream_state").await;
    assert_eq!(ended["live"], false);
    b.recv_type("peer_left").await;
}

#[tokio::test]
async fn layer_requests_and_latency_reports_are_relayed() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;
    let (mut b, joined_b) = join(addr, &a.code, "Beto", 2).await;
    let b_id = joined_b["you"]["peer_id"].as_str().unwrap().to_owned();
    a.client.recv_type("peer_joined").await;

    b.send(json!({
        "type": "layer_request", "to": a.peer_id, "stream_id": "screen-1", "layer": "economy"
    }))
    .await;
    let request = a.client.recv_type("layer_request").await;
    assert_eq!(request["from"], b_id);
    assert_eq!(request["layer"], "economy");

    b.send(json!({
        "type": "latency_report",
        "samples": [{ "peer_id": a.peer_id, "rtt_ms": 42.5 }]
    }))
    .await;
    let report = a.client.recv_type("latency_report").await;
    assert_eq!(report["from"], b_id);
    assert_eq!(report["samples"][0]["rtt_ms"], 42.5);
}

#[tokio::test]
async fn renaming_dedupes_and_notifies_everyone() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;
    let (mut b, joined_b) = join(addr, &a.code, "Beto", 2).await;
    let b_id = joined_b["you"]["peer_id"].as_str().unwrap().to_owned();
    a.client.recv_type("peer_joined").await;

    b.send(json!({ "type": "set_nickname", "nickname": "ana" }))
        .await;
    for client in [&mut a.client, &mut b] {
        let renamed = client.recv_type("peer_renamed").await;
        assert_eq!(renamed["peer_id"], b_id);
        assert_eq!(renamed["nickname"], "ana (2)");
    }
}

#[tokio::test]
async fn going_over_capacity_only_warns() {
    let config = Config {
        capacity_warning: 2,
        ..fast()
    };
    let addr = start(config).await;
    let mut a = create(addr, "Ana", 1, false).await;
    let (mut b, _) = join(addr, &a.code, "Beto", 2).await;
    a.client.recv_type("peer_joined").await;

    let (mut c, joined) = join(addr, &a.code, "Caio", 3).await;
    assert_eq!(joined["type"], "room_joined");
    for client in [&mut a.client, &mut b] {
        client.recv_type("peer_joined").await;
    }
    for client in [&mut a.client, &mut b, &mut c] {
        let warning = client.recv_type("room_over_capacity").await;
        assert_eq!(warning["count"], 3);
    }
}

#[tokio::test]
async fn a_second_connection_with_the_same_token_replaces_the_first() {
    let addr = start(fast()).await;
    let a = create(addr, "Ana", 1, false).await;
    let mut first = a.client;
    let (_second, back) = join(addr, &a.code, "Ana", 1).await;
    assert_eq!(back["you"]["peer_id"], a.peer_id);
    first.expect_closed().await;
}

#[tokio::test]
async fn bad_input_gets_clear_errors() {
    let addr = start(fast()).await;
    let mut client = Client::connect(addr).await;

    client.send_raw(r#"{"v":2,"type":"leave_room"}"#).await;
    client.recv_error("unsupported_version").await;
    client.send_raw("not json").await;
    client.recv_error("bad_message").await;
    client.send_raw(r#"{"type":"leave_room"}"#).await;
    client.recv_error("bad_message").await;
    client.send(json!({ "type": "does_not_exist" })).await;
    client.recv_error("bad_message").await;
    client
        .send(json!({ "type": "signal", "to": "x", "kind": "offer", "payload": {} }))
        .await;
    client.recv_error("bad_message").await;
    // Acting before being in a room.
    client
        .send(json!({ "type": "set_nickname", "nickname": "Ana" }))
        .await;
    client.recv_error("bad_message").await;
}

#[tokio::test]
async fn cannot_join_twice_on_one_connection() {
    let addr = start(fast()).await;
    let mut a = create(addr, "Ana", 1, false).await;
    a.client
        .send(json!({
            "type": "join_room", "code": a.code, "nickname": "Ana", "session_token": token(2)
        }))
        .await;
    a.client.recv_error("bad_message").await;
}

#[tokio::test]
async fn oversized_messages_close_the_connection() {
    let addr = start(fast()).await;
    let mut client = Client::connect(addr).await;
    let huge = json!({ "v": 1, "type": "set_nickname", "nickname": "x".repeat(20_000) });
    client.send_raw(&huge.to_string()).await;
    client.expect_closed().await;
}

#[tokio::test]
async fn health_endpoint_answers() {
    let addr = start(fast()).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    tokio::io::AsyncWriteExt::write_all(
        &mut stream,
        b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut body = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut body)
        .await
        .unwrap();
    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    assert!(body.ends_with("ok"), "{body}");
}
