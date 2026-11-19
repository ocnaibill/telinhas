//! Two peers that may only talk through the relay (Cloudflare TURN), as when
//! one of them sits behind a symmetric NAT.
//!
//! Needs the internet and a TURN key, so it is ignored by default. The key is
//! read from the environment and never written down:
//!
//! ```text
//! TELINHAS_TURN_KEY_ID=... TELINHAS_TURN_TOKEN=... \
//!   cargo test -p telinhas-media --test relay -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use telinhas_media::{BroadcastSource, BroadcastSpec, Command, Event, LinkConfig, Media, PeerId};

const ANA: PeerId = PeerId::from_u128(1);
const BETO: PeerId = PeerId::from_u128(2);

fn pump(from: &Media, from_id: PeerId, to: &Media, log: &mut Vec<String>) {
    while let Some(event) = from.try_event() {
        match event {
            Event::Signal { kind, payload, .. } => {
                log.push(format!("signal {kind:?}"));
                to.send(Command::Signal {
                    from: from_id,
                    kind,
                    payload,
                });
            }
            other => log.push(format!("{other:?}")),
        }
    }
}

#[test]
#[ignore = "needs the internet and a Cloudflare TURN key in the environment"]
fn pictures_arrive_through_the_relay_alone() {
    let (Ok(key_id), Ok(token)) = (
        std::env::var("TELINHAS_TURN_KEY_ID"),
        std::env::var("TELINHAS_TURN_TOKEN"),
    ) else {
        eprintln!("TELINHAS_TURN_KEY_ID and TELINHAS_TURN_TOKEN are not set: nothing to test");
        return;
    };
    let relays = telinhas_signaling::relay::fetch(&key_id, &token).expect("credentials");
    let link = || LinkConfig {
        relays: relays.clone(),
        force_relay: true,
        ..LinkConfig::internet()
    };
    let (ana, beto) = (Media::spawn(link(), || {}), Media::spawn(link(), || {}));
    ana.send(Command::StartBroadcast {
        spec: BroadcastSpec::new(
            BroadcastSource::Synthetic {
                width: 640,
                height: 360,
            },
            15,
            2000,
        ),
        viewers: vec![BETO],
    });
    let mut log = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        pump(&ana, ANA, &beto, &mut log);
        pump(&beto, BETO, &ana, &mut log);
        if beto.latest(ANA).is_some_and(|p| p.seq >= 5) {
            break;
        }
        let (sent, seen) = (ana.stats(), beto.stats());
        assert!(
            Instant::now() < deadline,
            "no pictures through the relay; events: {log:?}; links: {:?} {:?}",
            sent.links,
            seen.links
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Only relay addresses may have been offered.
    let seen = beto.stats();
    let link = seen.links.values().next().expect("a link");
    println!("viewer's addresses: {}", link.local);
    assert!(link.local.contains("relay"), "{}", link.local);
    assert!(!link.local.contains("host"), "{}", link.local);
}
