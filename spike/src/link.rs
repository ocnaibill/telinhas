//! WebRTC peer connections for the spike, on `webrtc` 0.21 (`rtc` core).
//!
//! Signaling here is non-trickle: each side waits for ICE gathering to
//! finish and exchanges one complete session description as JSON.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender as StdSender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use rtc::data_channel::RTCDataChannelInit;
use rtc::interceptor::{BandwidthEstimator, EstimatorStats, Gcc, PacketReport, Registry};
use rtc::media::Sample;
use rtc::media::io::sample_builder::SampleBuilder;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::configuration::interceptor_registry::{
    CongestionFeedback, configure_congestion_control, register_default_interceptors,
};
use rtc::peer_connection::configuration::media_engine::{MIME_TYPE_H264, MediaEngine};
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceServer;
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
    RtpCodecKind,
};
use rtc::rtp_transceiver::{PayloadType, RTCRtpTransceiverDirection, RTCRtpTransceiverInit};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::media_stream::Track;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState,
    RTCPeerConnectionState,
};
use webrtc::rtp_transceiver::RtpSender;
use webrtc::runtime::{Runtime, default_runtime};

use crate::media::{SendCounters, StreamConfig, Unit, spawn_decoder, spawn_encoder};
use crate::stats::Stats;

const H264_FMTP: &str = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f";
const H264_PAYLOAD_TYPE: PayloadType = 102;

/// Frames sent but not yet acknowledged beyond which new frames are dropped
/// (one second at 30 fps).
const MAX_IN_FLIGHT: usize = 30;
/// An unacknowledged frame is given up on after this long.
const LOST_AFTER: Duration = Duration::from_secs(2);

const BWE_INITIAL: f64 = 1_500_000.0;
const BWE_MIN: f64 = 150_000.0;
const BWE_MAX: f64 = 8_000_000.0;

#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// Local address to bind UDP on, such as `127.0.0.1:0` or `0.0.0.0:0`.
    pub udp_addr: String,
    /// STUN URLs, such as `stun:stun.l.google.com:19302`.
    pub stun: Vec<String>,
    /// Optional TURN relay as `(url, username, credential)`.
    pub turn: Option<(String, String, String)>,
}

impl LinkConfig {
    pub fn loopback() -> Self {
        Self {
            udp_addr: "127.0.0.1:0".into(),
            stun: Vec::new(),
            turn: None,
        }
    }

    /// The first non-loopback interface, with public STUN.
    pub fn internet() -> Self {
        Self {
            udp_addr: format!("{}:0", rtc_signal::get_local_ip()),
            stun: vec!["stun:stun.l.google.com:19302".into()],
            turn: None,
        }
    }
}

/// Wraps the GCC estimator to publish its target where the encoder can read it.
struct ReportingEstimator<E: BandwidthEstimator> {
    inner: E,
    target: Arc<AtomicU64>,
}

impl<E: BandwidthEstimator> ReportingEstimator<E> {
    fn publish(&self) {
        self.target
            .store(self.inner.target_bitrate().to_bits(), Ordering::Relaxed);
    }
}

impl<E: BandwidthEstimator> BandwidthEstimator for ReportingEstimator<E> {
    fn on_reports(&mut self, now: Instant, reports: &[PacketReport]) {
        self.inner.on_reports(now, reports);
        self.publish();
    }

    fn target_bitrate(&self) -> f64 {
        self.inner.target_bitrate()
    }

    fn handle_timeout(&mut self, now: Instant) {
        self.inner.handle_timeout(now);
        self.publish();
    }

    fn poll_timeout(&self) -> Option<Instant> {
        self.inner.poll_timeout()
    }

    fn stats(&self) -> EstimatorStats {
        self.inner.stats()
    }
}

struct RxContext {
    runtime: Arc<dyn Runtime>,
    units: StdSender<Unit>,
    /// Frame counters to echo back to the sender over the data channel.
    acks: Mutex<Option<UnboundedReceiver<u32>>>,
    stats: Arc<Stats>,
}

struct Handler {
    gathered: watch::Sender<bool>,
    state: watch::Sender<RTCPeerConnectionState>,
    rx: Option<RxContext>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.send(true);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state.send(state);
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        let Some(rx) = &self.rx else { return };
        let Some(mut acks) = rx.acks.lock().unwrap().take() else {
            return;
        };
        rx.runtime.spawn(Box::pin(async move {
            let mut open = false;
            loop {
                tokio::select! {
                    event = channel.poll() => match event {
                        Some(DataChannelEvent::OnOpen) => open = true,
                        Some(DataChannelEvent::OnClose) | None => break,
                        _ => {}
                    },
                    counter = acks.recv() => {
                        let Some(counter) = counter else { break };
                        if open {
                            let _ = channel.send_text(&counter.to_string()).await;
                        }
                    }
                }
            }
        }));
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let Some(rx) = &self.rx else { return };
        let units = rx.units.clone();
        let stats = Arc::clone(&rx.stats);
        rx.runtime.spawn(Box::pin(async move {
            // A 1080p keyframe is hundreds of packets: the builder must be
            // willing to wait for all of them before it gives up on a unit.
            let mut builder = SampleBuilder::new(1024, H264Packet::default(), 90_000);
            while let Some(event) = track.poll().await {
                let TrackRemoteEvent::OnRtpPacket(packet) = event else {
                    continue;
                };
                stats.diag.packets.fetch_add(1, Ordering::Relaxed);
                builder.push(Instant::now(), packet);
                while let Some(sample) = builder.pop(Instant::now()) {
                    stats.diag.note_sample(sample.data.len());
                    let unit = Unit {
                        data: sample.data.to_vec(),
                        ready_at: Instant::now(),
                    };
                    if units.send(unit).is_err() {
                        return;
                    }
                }
            }
        }));
    }
}

fn h264_codec() -> RTCRtpCodecParameters {
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: H264_FMTP.to_owned(),
            rtcp_feedback: vec![],
        },
        payload_type: H264_PAYLOAD_TYPE,
    }
}

struct Built {
    pc: Arc<dyn PeerConnection>,
    gathered: watch::Receiver<bool>,
    state: watch::Receiver<RTCPeerConnectionState>,
}

async fn build(
    link: &LinkConfig,
    runtime: Arc<dyn Runtime>,
    rx: Option<RxContext>,
    estimator: Option<ReportingEstimator<Gcc>>,
) -> Result<Built> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_codec(h264_codec(), RtpCodecKind::Video)?;

    let registry = match estimator {
        Some(estimator) => configure_congestion_control(
            Registry::new(),
            estimator,
            CongestionFeedback::Twcc,
            &mut media_engine,
        )?,
        None => Registry::new(),
    };
    let registry = register_default_interceptors(registry, &mut media_engine)?;

    let mut config = RTCConfigurationBuilder::new();
    let mut servers = Vec::new();
    if !link.stun.is_empty() {
        servers.push(RTCIceServer {
            urls: link.stun.clone(),
            ..Default::default()
        });
    }
    if let Some((url, username, credential)) = &link.turn {
        servers.push(RTCIceServer {
            urls: vec![url.clone()],
            username: username.clone(),
            credential: credential.clone(),
        });
    }
    if !servers.is_empty() {
        config = config.with_ice_servers(servers);
    }

    let (gathered_tx, gathered) = watch::channel(false);
    let (state_tx, state) = watch::channel(RTCPeerConnectionState::New);
    let handler = Arc::new(Handler {
        gathered: gathered_tx,
        state: state_tx,
        rx,
    });

    let pc = PeerConnectionBuilder::new()
        .with_configuration(config.build())
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .with_handler(handler)
        .with_runtime(runtime)
        .with_udp_addrs(vec![link.udp_addr.clone()])
        .build()
        .await?;
    Ok(Built {
        pc: Arc::new(pc),
        gathered,
        state,
    })
}

async fn wait_gathered(gathered: &mut watch::Receiver<bool>) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), gathered.wait_for(|done| *done))
        .await
        .context("ICE gathering did not finish in 10 s")??;
    Ok(())
}

async fn wait_connected(state: &mut watch::Receiver<RTCPeerConnectionState>) -> Result<()> {
    tokio::time::timeout(
        Duration::from_secs(30),
        state.wait_for(|s| *s == RTCPeerConnectionState::Connected),
    )
    .await
    .context("peer connection did not reach Connected in 30 s")??;
    Ok(())
}

async fn local_json(pc: &Arc<dyn PeerConnection>) -> Result<String> {
    let description = pc
        .local_description()
        .await
        .ok_or_else(|| anyhow!("no local description"))?;
    Ok(serde_json::to_string(&description)?)
}

/// The sending peer: owns the track and the congestion estimate.
pub struct SendLink {
    pc: Arc<dyn PeerConnection>,
    gathered: watch::Receiver<bool>,
    state: watch::Receiver<RTCPeerConnectionState>,
    track: Arc<TrackLocalStaticSample>,
    sender: Arc<dyn RtpSender>,
    estimate: Arc<AtomicU64>,
    /// When each frame was captured, until its ack comes back.
    sent_at: Arc<Mutex<HashMap<u32, Instant>>>,
    /// Round trip from capture to "decoded at the other side" (see `Stats`).
    pub round_trip: Arc<Stats>,
    pub counters: Arc<SendCounters>,
}

impl SendLink {
    pub async fn new(link: &LinkConfig) -> Result<Self> {
        let runtime = default_runtime().context("no runtime")?;
        let estimate = Arc::new(AtomicU64::new(BWE_INITIAL.to_bits()));
        let estimator = ReportingEstimator {
            inner: Gcc::new(BWE_INITIAL, BWE_MIN, BWE_MAX),
            target: Arc::clone(&estimate),
        };
        let built = build(link, runtime, None, Some(estimator)).await?;

        let ssrc = rand::random::<u32>();
        let track = Arc::new(TrackLocalStaticSample::new(
            Instant::now(),
            MediaStreamTrack::new(
                "telinhas-screen".to_owned(),
                "telinhas-screen-video".to_owned(),
                "screen".to_owned(),
                RtpCodecKind::Video,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters {
                        ssrc: Some(ssrc),
                        ..Default::default()
                    },
                    codec: h264_codec().rtp_codec,
                    ..Default::default()
                }],
            ),
        )?);
        let sender = built
            .pc
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal>)
            .await?;
        let sent_at: Arc<Mutex<HashMap<u32, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let round_trip = Arc::new(Stats::default());
        // Acks must not wait behind a retransmission: unordered and
        // unreliable, so a late ack never holds back the ones after it.
        let channel = built
            .pc
            .create_data_channel(
                "acks",
                Some(RTCDataChannelInit {
                    ordered: false,
                    max_retransmits: Some(0),
                    ..Default::default()
                }),
            )
            .await?;
        {
            let sent_at = Arc::clone(&sent_at);
            let round_trip = Arc::clone(&round_trip);
            let runtime = default_runtime().context("no runtime")?;
            runtime.spawn(Box::pin(async move {
                while let Some(event) = channel.poll().await {
                    let DataChannelEvent::OnMessage(message) = event else {
                        continue;
                    };
                    let Ok(counter) = String::from_utf8_lossy(&message.data).parse::<u32>() else {
                        continue;
                    };
                    let sent = sent_at.lock().unwrap().remove(&counter);
                    if let Some(sent) = sent {
                        round_trip.record(counter, sent.elapsed().as_secs_f64() * 1000.0, 0);
                    }
                }
            }));
        }
        Ok(Self {
            pc: built.pc,
            gathered: built.gathered,
            state: built.state,
            track,
            sender,
            estimate,
            sent_at,
            round_trip,
            counters: Arc::new(SendCounters::default()),
        })
    }

    /// Offer as JSON, once ICE gathering is done.
    pub async fn create_offer(&mut self) -> Result<String> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer).await?;
        wait_gathered(&mut self.gathered).await?;
        local_json(&self.pc).await
    }

    pub async fn accept_answer(&self, answer_json: &str) -> Result<()> {
        let answer: RTCSessionDescription = serde_json::from_str(answer_json)?;
        self.pc.set_remote_description(answer).await?;
        Ok(())
    }

    pub async fn wait_connected(&mut self) -> Result<()> {
        wait_connected(&mut self.state).await
    }

    /// Congestion controller's current estimate, in kbps.
    pub fn estimate_kbps(&self) -> f64 {
        f64::from_bits(self.estimate.load(Ordering::Relaxed)) / 1000.0
    }

    /// Encodes and sends the synthetic source until `stop` is set.
    pub async fn stream(&self, cfg: StreamConfig, stop: Arc<AtomicBool>) -> Result<()> {
        let ssrc = *self
            .track
            .ssrcs()
            .await
            .first()
            .ok_or_else(|| anyhow!("track has no SSRC"))?;
        let payload_type = self
            .sender
            .get_parameters()
            .await?
            .rtp_parameters
            .codecs
            .first()
            .map(|codec| codec.payload_type)
            .ok_or_else(|| anyhow!("no negotiated codec"))?;

        let (frames_tx, mut frames) = tokio::sync::mpsc::unbounded_channel();
        let duration = Duration::from_secs_f64(1.0 / f64::from(cfg.fps));
        let need_key = Arc::new(AtomicBool::new(false));
        let encoder = spawn_encoder(
            cfg,
            frames_tx,
            Arc::clone(&stop),
            Arc::clone(&self.estimate),
            Arc::clone(&need_key),
            Arc::clone(&self.counters),
        );
        while let Some(frame) = frames.recv().await {
            {
                let mut sent_at = self.sent_at.lock().unwrap();
                // A frame whose ack has not come back in this long is lost.
                sent_at.retain(|_, sent| sent.elapsed() < LOST_AFTER);
                // Too many frames still in flight means the network cannot
                // keep up. Sending more only adds delay, so drop this one
                // and ask for a keyframe to resume from.
                if sent_at.len() > MAX_IN_FLIGHT {
                    self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                    need_key.store(true, Ordering::Relaxed);
                    continue;
                }
                sent_at.insert(frame.counter, frame.captured_at);
            }
            self.counters.written.fetch_add(1, Ordering::Relaxed);
            self.track
                .sample_writer(ssrc, payload_type)
                .write_sample(&Sample {
                    data: Bytes::from(frame.data),
                    duration,
                    ..Sample::new(Instant::now())
                })
                .await?;
        }
        encoder
            .join()
            .map_err(|_| anyhow!("encoder thread panicked"))??;
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        Ok(self.pc.close().await?)
    }
}

/// The receiving peer: decodes and measures.
pub struct RecvLink {
    pc: Arc<dyn PeerConnection>,
    gathered: watch::Receiver<bool>,
    state: watch::Receiver<RTCPeerConnectionState>,
    pub stats: Arc<Stats>,
}

impl RecvLink {
    pub async fn new(link: &LinkConfig) -> Result<Self> {
        let runtime = default_runtime().context("no runtime")?;
        let stats = Arc::new(Stats::default());
        let (units_tx, units_rx) = std::sync::mpsc::channel();
        let (acks_tx, acks_rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_decoder(units_rx, Arc::clone(&stats), Some(acks_tx));
        let rx = RxContext {
            runtime: Arc::clone(&runtime),
            units: units_tx,
            acks: Mutex::new(Some(acks_rx)),
            stats: Arc::clone(&stats),
        };
        // The receiver uses the same interceptors so TWCC feedback flows back.
        let estimator = ReportingEstimator {
            inner: Gcc::new(BWE_INITIAL, BWE_MIN, BWE_MAX),
            target: Arc::new(AtomicU64::new(0)),
        };
        let built = build(link, runtime, Some(rx), Some(estimator)).await?;
        built
            .pc
            .add_transceiver_from_kind(
                RtpCodecKind::Video,
                Some(RTCRtpTransceiverInit {
                    direction: RTCRtpTransceiverDirection::Recvonly,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(Self {
            pc: built.pc,
            gathered: built.gathered,
            state: built.state,
            stats,
        })
    }

    /// Takes the offer, returns the answer as JSON.
    pub async fn accept_offer(&mut self, offer_json: &str) -> Result<String> {
        let offer: RTCSessionDescription = serde_json::from_str(offer_json)?;
        self.pc.set_remote_description(offer).await?;
        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer).await?;
        wait_gathered(&mut self.gathered).await?;
        local_json(&self.pc).await
    }

    pub async fn wait_connected(&mut self) -> Result<()> {
        wait_connected(&mut self.state).await
    }

    pub async fn close(&self) -> Result<()> {
        Ok(self.pc.close().await?)
    }
}
