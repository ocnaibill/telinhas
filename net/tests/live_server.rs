//! Checks against a running server, named by `TELINHAS_SERVER`. Ignored by
//! default: `TELINHAS_SERVER=wss://host/ws cargo test -p telinhas-net --test live_server -- --ignored --nocapture`.

use std::time::Duration;

use telinhas_net::{Command, Event, Settings, spawn};
use tokio::time::{Instant, sleep};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a server in TELINHAS_SERVER"]
async fn a_new_room_comes_with_relays() {
    let Ok(server) = std::env::var("TELINHAS_SERVER") else {
        eprintln!("TELINHAS_SERVER is not set: nothing to check");
        return;
    };
    let handle = spawn(
        Settings {
            server,
            nickname: "check".to_owned(),
        },
        || {},
    );
    handle.send(Command::CreateRoom {
        require_approval: false,
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match handle.try_event() {
            Some(Event::Joined(room)) => {
                // Only shapes are printed: the credentials are not for logs.
                for server in &room.ice_servers {
                    println!(
                        "relay: {:?}, username {} chars, credential {} chars",
                        server.urls,
                        server.username.len(),
                        server.credential.len()
                    );
                }
                assert!(!room.ice_servers.is_empty(), "the server sent no relays");
                handle.send(Command::Leave);
                return;
            }
            Some(_) => {}
            None => {
                assert!(Instant::now() < deadline, "no room in time");
                sleep(Duration::from_millis(20)).await;
            }
        }
    }
}
