//! Two peers in one process, over loopback UDP.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use telinhas_spike::link::{LinkConfig, RecvLink, SendLink};
use telinhas_spike::media::StreamConfig;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn synthetic_h264_reaches_the_other_peer() {
    let link = LinkConfig::loopback();
    let mut sender = SendLink::new(&link).await.unwrap();
    let mut receiver = RecvLink::new(&link).await.unwrap();

    let offer = sender.create_offer().await.unwrap();
    let answer = receiver.accept_offer(&offer).await.unwrap();
    sender.accept_answer(&answer).await.unwrap();
    sender.wait_connected().await.unwrap();
    receiver.wait_connected().await.unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let cfg = StreamConfig::default();
    let fps = cfg.fps;
    let streaming = {
        let stop = Arc::clone(&stop);
        let sender = Arc::new(sender);
        let task = {
            let sender = Arc::clone(&sender);
            tokio::spawn(async move { sender.stream(cfg, stop).await })
        };
        (sender, task)
    };

    tokio::time::sleep(Duration::from_secs(6)).await;
    stop.store(true, Ordering::Relaxed);
    streaming.1.await.unwrap().unwrap();

    let report = receiver.stats.report();
    println!("{report}");
    println!("estimate: {:.0} kbps", streaming.0.estimate_kbps());
    let round_trip = streaming.0.round_trip.report();
    println!("round trip to decode (sender's clock only): {round_trip}");

    // 6 s at 30 fps is 180 frames; allow for startup until the first keyframe.
    assert!(
        report.decoded >= u64::from(fps) * 3,
        "decoded too few frames: {report}"
    );
    assert!(report.latency_p95_ms < 500.0, "latency too high: {report}");
    assert!(report.latency_p50_ms >= 0.0, "negative latency: {report}");
    assert!(
        round_trip.decoded >= u64::from(fps) * 3,
        "acks did not come back over the data channel: {round_trip}"
    );
    assert!(round_trip.latency_p50_ms > 0.0);
}
