//! A broadcaster and a viewer on this machine, negotiating by hand the way the
//! signaling server would relay it.

use std::time::{Duration, Instant};

use telinhas_media::{BroadcastSource, BroadcastSpec, Command, Event, LinkConfig, Media, PeerId};

const ANA: PeerId = PeerId::from_u128(1);
const BETO: PeerId = PeerId::from_u128(2);

/// Relays one side's negotiation to the other.
fn relay(from: &Media, from_id: PeerId, to: &Media, log: &mut Vec<String>) {
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

fn pair() -> (Media, Media) {
    (
        Media::spawn(LinkConfig::loopback(), || {}),
        Media::spawn(LinkConfig::loopback(), || {}),
    )
}

fn spec() -> BroadcastSpec {
    BroadcastSpec::new(
        BroadcastSource::Synthetic {
            width: 640,
            height: 360,
        },
        15,
        2000,
    )
}

#[test]
fn the_viewer_decodes_what_the_broadcaster_sends() {
    let (ana, beto) = pair();
    ana.send(Command::StartBroadcast {
        spec: spec(),
        viewers: vec![BETO],
    });
    let mut log = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        relay(&ana, ANA, &beto, &mut log);
        relay(&beto, BETO, &ana, &mut log);
        if let Some(picture) = beto.latest(ANA)
            && picture.seq >= 5
        {
            assert_eq!((picture.width, picture.height), (640, 360));
            assert_eq!(picture.y.len(), 640 * 360);
            assert_eq!(picture.u.len(), 320 * 180);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no pictures arrived; events: {log:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(log.iter().any(|e| e.contains("Encoder")), "events: {log:?}");

    // The viewer confirms what it shows, and both sides learn where the time went.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        relay(&ana, ANA, &beto, &mut log);
        relay(&beto, BETO, &ana, &mut log);
        let sent = ana.stats();
        let seen = beto.stats();
        let path = sent.viewers.get(&BETO);
        let feedback = seen.receiving.get(&ANA).map(|r| r.path.total_ms > 0.0);
        if path.is_some_and(|p| p.total_ms > 0.0) && feedback == Some(true) {
            let path = path.unwrap();
            assert!(path.total_ms >= path.encode_ms, "{path:?}");
            assert!(
                sent.sending.as_ref().is_some_and(|s| s.width == 640),
                "{sent:?}"
            );
            assert!(seen.receiving[&ANA].decode_ms > 0.0, "{seen:?}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no measurements: {sent:?} {seen:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_viewer_who_joins_later_gets_the_broadcast() {
    let (ana, beto) = pair();
    ana.send(Command::StartBroadcast {
        spec: spec(),
        viewers: vec![],
    });
    std::thread::sleep(Duration::from_millis(500));
    ana.send(Command::PeerJoined(BETO));
    let mut log = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        relay(&ana, ANA, &beto, &mut log);
        relay(&beto, BETO, &ana, &mut log);
        if beto.latest(ANA).is_some_and(|p| p.seq >= 3) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no pictures arrived; events: {log:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Stopping ends the stream at the viewer once told so.
    beto.send(Command::StreamEnded(ANA));
    ana.send(Command::StopBroadcast);
    std::thread::sleep(Duration::from_millis(300));
    assert!(beto.latest(ANA).is_none());
}

fn wait_for(
    ana: &Media,
    beto: &Media,
    log: &mut Vec<String>,
    what: &str,
    done: impl Fn(&Media) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        relay(ana, ANA, beto, log);
        relay(beto, BETO, ana, log);
        if done(beto) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; events: {log:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_broadcast_is_shrunk_to_what_the_broadcaster_chose_and_never_enlarged() {
    let (ana, beto) = pair();
    let mut spec = BroadcastSpec::new(
        BroadcastSource::Synthetic {
            width: 1280,
            height: 720,
        },
        15,
        2000,
    );
    spec.max_height = Some(360);
    ana.send(Command::StartBroadcast {
        spec: spec.clone(),
        viewers: vec![BETO],
    });
    let mut log = Vec::new();
    wait_for(&ana, &beto, &mut log, "a shrunk picture", |b| {
        b.latest(ANA).is_some_and(|p| p.seq >= 3 && p.height == 360)
    });
    assert_eq!(beto.latest(ANA).unwrap().width, 640);

    // A limit above the source changes nothing: no enlarging.
    spec.max_height = Some(1080);
    ana.send(Command::StartBroadcast {
        spec,
        viewers: vec![BETO],
    });
    wait_for(&ana, &beto, &mut log, "the original size", |b| {
        b.latest(ANA).is_some_and(|p| p.height == 720)
    });
}

#[test]
fn changing_the_source_keeps_the_connection() {
    let (ana, beto) = pair();
    let small =
        |width, height| BroadcastSpec::new(BroadcastSource::Synthetic { width, height }, 15, 2000);
    ana.send(Command::StartBroadcast {
        spec: small(640, 360),
        viewers: vec![BETO],
    });
    let mut log = Vec::new();
    wait_for(&ana, &beto, &mut log, "the first source", |b| {
        b.latest(ANA).is_some_and(|p| p.seq >= 3 && p.width == 640)
    });
    ana.send(Command::StartBroadcast {
        spec: small(320, 180),
        viewers: vec![BETO],
    });
    wait_for(&ana, &beto, &mut log, "the second source", |b| {
        b.latest(ANA).is_some_and(|p| p.width == 320)
    });
    // One offer for the whole time: the connection was not renegotiated.
    let offers = log.iter().filter(|e| *e == "signal Offer").count();
    assert_eq!(offers, 1, "events: {log:?}");
    ana.send(Command::StopBroadcast);
}

/// Counts how many times a stereo tone crosses zero going up: its frequency.
fn rising_crossings(samples: &[f32]) -> usize {
    let left: Vec<f32> = samples.chunks(2).map(|c| c[0]).collect();
    left.windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count()
}

#[test]
fn sound_travels_with_the_picture_and_comes_out_as_the_same_tone() {
    let (ana, beto) = pair();
    let mut spec = BroadcastSpec::new(
        BroadcastSource::Synthetic {
            width: 640,
            height: 360,
        },
        15,
        2000,
    );
    spec.audio = Some(telinhas_media::AudioInput::Synthetic);
    ana.send(Command::StartBroadcast {
        spec,
        viewers: vec![BETO],
    });
    let mut log = Vec::new();
    wait_for(&ana, &beto, &mut log, "decoded sound", |b| {
        b.stats()
            .receiving
            .get(&ANA)
            .is_some_and(|r| r.audio.decoded >= 50)
    });
    let buffer = beto.audio(ANA).expect("a sound buffer for the broadcaster");
    // Let half a second pile up, then read it all.
    std::thread::sleep(Duration::from_millis(500));
    let mut samples = vec![0f32; 48_000 * 2 / 2];
    let real = buffer.pull(&mut samples);
    assert!(real > 48_000 / 4, "only {real} samples were waiting");
    let heard = &samples[..real];
    let peak = heard.iter().fold(0f32, |p, s| p.max(s.abs()));
    assert!((0.2..0.4).contains(&peak), "a 0.3 tone came out at {peak}");
    // 440 Hz: about 440 rising crossings per second of sound.
    let seconds = real as f32 / 2.0 / 48_000.0;
    let hz = rising_crossings(heard) as f32 / seconds;
    assert!((400.0..480.0).contains(&hz), "the tone came out at {hz} Hz");
    let stats = beto.stats();
    assert_eq!(stats.receiving[&ANA].audio.lost, 0, "{stats:?}");
}
