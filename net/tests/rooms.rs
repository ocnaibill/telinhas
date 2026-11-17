//! The client against the real signaling server, in the same process.

use std::time::Duration;

use telinhas_net::{Command, Event, Failure, Handle, Settings, spawn};
use telinhas_protocol::{ConnectionState, LeaveReason};
use telinhas_signaling::{Config, Hub, serve};
use tokio::time::{Instant, sleep};

async fn server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Hub::new(Config::default())));
    format!("ws://{addr}/ws")
}

fn client(server: &str, nickname: &str) -> Handle {
    spawn(
        Settings {
            server: server.to_owned(),
            nickname: nickname.to_owned(),
        },
        || {},
    )
}

/// The next event, or a panic after a few seconds.
async fn next(handle: &Handle) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = handle.try_event() {
            return event;
        }
        assert!(Instant::now() < deadline, "no event arrived in time");
        sleep(Duration::from_millis(10)).await;
    }
}

/// The next event that is not the one the closure skips.
async fn next_matching(handle: &Handle, mut keep: impl FnMut(&Event) -> bool) -> Event {
    loop {
        let event = next(handle).await;
        if keep(&event) {
            return event;
        }
    }
}

async fn joined(handle: &Handle) -> telinhas_net::RoomSnapshot {
    match next_matching(handle, |e| matches!(e, Event::Joined(_))).await {
        Event::Joined(snapshot) => snapshot,
        _ => unreachable!(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn creating_and_joining_a_room() {
    let url = server().await;
    let ana = client(&url, "Ana");
    ana.send(Command::CreateRoom {
        require_approval: false,
    });
    assert_eq!(next(&ana).await, Event::Connecting);
    let room = joined(&ana).await;
    assert_eq!(room.code.len(), 9);
    assert_eq!(room.you.nickname, "Ana");
    assert_eq!(room.admin, room.you.peer_id);
    assert!(room.peers.is_empty());

    let beto = client(&url, "Beto");
    beto.send(Command::JoinRoom {
        code: room.code.to_lowercase(),
    });
    let in_room = joined(&beto).await;
    assert_eq!(in_room.peers.len(), 1);
    assert_eq!(in_room.peers[0].nickname, "Ana");
    assert_eq!(in_room.admin, room.you.peer_id);

    match next_matching(&ana, |e| matches!(e, Event::PeerJoined(_))).await {
        Event::PeerJoined(peer) => assert_eq!(peer.nickname, "Beto"),
        _ => unreachable!(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_code_fails_and_the_client_can_try_again() {
    let url = server().await;
    let ana = client(&url, "Ana");
    ana.send(Command::JoinRoom {
        code: "ZZZZ-ZZZZ".into(),
    });
    match next_matching(&ana, |e| matches!(e, Event::Failure { .. })).await {
        Event::Failure { kind, fatal, .. } => {
            assert_eq!(kind, Failure::InvalidCode);
            assert!(fatal);
        }
        _ => unreachable!(),
    }
    // Idle again: creating a room works.
    ana.send(Command::CreateRoom {
        require_approval: false,
    });
    joined(&ana).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_server_is_reported() {
    // A port nothing listens on.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    drop(listener);
    let ana = client(&url, "Ana");
    ana.send(Command::CreateRoom {
        require_approval: false,
    });
    match next_matching(&ana, |e| matches!(e, Event::Failure { .. })).await {
        Event::Failure { kind, fatal, .. } => {
            assert_eq!(kind, Failure::Unreachable);
            assert!(fatal);
        }
        _ => unreachable!(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_accepted_and_denied() {
    let url = server().await;
    let admin = client(&url, "Ana");
    admin.send(Command::CreateRoom {
        require_approval: true,
    });
    let room = joined(&admin).await;

    let beto = client(&url, "Beto");
    beto.send(Command::JoinRoom {
        code: room.code.clone(),
    });
    next_matching(&beto, |e| *e == Event::ApprovalPending).await;
    let request = match next_matching(&admin, |e| matches!(e, Event::ApprovalRequest { .. })).await
    {
        Event::ApprovalRequest {
            request_id,
            nickname,
        } => {
            assert_eq!(nickname, "Beto");
            request_id
        }
        _ => unreachable!(),
    };
    admin.send(Command::ApprovalDecision {
        request_id: request,
        accept: true,
    });
    joined(&beto).await;

    let caio = client(&url, "Caio");
    caio.send(Command::JoinRoom { code: room.code });
    next_matching(&caio, |e| *e == Event::ApprovalPending).await;
    let request = match next_matching(&admin, |e| matches!(e, Event::ApprovalRequest { .. })).await
    {
        Event::ApprovalRequest { request_id, .. } => request_id,
        _ => unreachable!(),
    };
    admin.send(Command::ApprovalDecision {
        request_id: request,
        accept: false,
    });
    next_matching(&caio, |e| *e == Event::JoinDenied).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_connection_comes_back_as_the_same_participant() {
    let url = server().await;
    let ana = client(&url, "Ana");
    ana.send(Command::CreateRoom {
        require_approval: false,
    });
    let room = joined(&ana).await;
    let beto = client(&url, "Beto");
    beto.send(Command::JoinRoom {
        code: room.code.clone(),
    });
    joined(&beto).await;

    ana.send(Command::DropConnectionForTesting);
    match next_matching(&ana, |e| matches!(e, Event::Reconnecting { .. })).await {
        Event::Reconnecting { attempt } => assert_eq!(attempt, 1),
        _ => unreachable!(),
    }
    // The other side sees her go quiet, then come back.
    match next_matching(&beto, |e| matches!(e, Event::PeerConnection { .. })).await {
        Event::PeerConnection { state, peer_id } => {
            assert_eq!(state, ConnectionState::Reconnecting);
            assert_eq!(peer_id, room.you.peer_id);
        }
        _ => unreachable!(),
    }
    let back = joined(&ana).await;
    assert_eq!(back.you.peer_id, room.you.peer_id, "same participant");
    assert_eq!(back.admin, room.you.peer_id, "still the admin");
    assert_eq!(back.code, room.code);
    match next_matching(&beto, |e| matches!(e, Event::PeerConnection { .. })).await {
        Event::PeerConnection { state, .. } => assert_eq!(state, ConnectionState::Connected),
        _ => unreachable!(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn leaving_and_admin_hand_over() {
    let url = server().await;
    let ana = client(&url, "Ana");
    ana.send(Command::CreateRoom {
        require_approval: false,
    });
    let room = joined(&ana).await;
    let beto = client(&url, "Beto");
    beto.send(Command::JoinRoom { code: room.code });
    let beto_room = joined(&beto).await;

    ana.send(Command::Leave);
    next_matching(&ana, |e| *e == Event::Left).await;
    match next_matching(&beto, |e| matches!(e, Event::PeerLeft { .. })).await {
        Event::PeerLeft { reason, .. } => assert_eq!(reason, LeaveReason::Left),
        _ => unreachable!(),
    }
    match next_matching(&beto, |e| matches!(e, Event::AdminChanged(_))).await {
        Event::AdminChanged(peer) => assert_eq!(peer, beto_room.you.peer_id),
        _ => unreachable!(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_and_the_approval_switch_reach_the_others() {
    let url = server().await;
    let ana = client(&url, "Ana");
    ana.send(Command::CreateRoom {
        require_approval: false,
    });
    let room = joined(&ana).await;
    let beto = client(&url, "Beto");
    beto.send(Command::JoinRoom { code: room.code });
    joined(&beto).await;

    beto.send(Command::SetNickname("Beto Silva".into()));
    match next_matching(&ana, |e| matches!(e, Event::PeerRenamed { .. })).await {
        Event::PeerRenamed { nickname, .. } => assert_eq!(nickname, "Beto Silva"),
        _ => unreachable!(),
    }
    ana.send(Command::SetApproval(true));
    next_matching(&beto, |e| *e == Event::ApprovalChanged(true)).await;
    // A person who is not the admin is told so.
    beto.send(Command::SetApproval(false));
    match next_matching(&beto, |e| matches!(e, Event::Failure { .. })).await {
        Event::Failure { kind, fatal, .. } => {
            assert_eq!(kind, Failure::NotAdmin);
            assert!(!fatal, "stays in the room");
        }
        _ => unreachable!(),
    }
}
