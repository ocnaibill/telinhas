//! One WebRTC connection to one peer, on `webrtc` 0.21 (the `rtc` core).
//!
//! Descriptions are complete: each side waits for ICE gathering to finish
//! before handing its description over.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
use telinhas_protocol::{PeerId, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
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

use crate::picture::Picture;
use crate::stamp;
use crate::stats::{Stats, smooth};

const H264_FMTP: &str = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f";
const H264_PAYLOAD_TYPE: PayloadType = 102;

// Starting low made the first seconds blocky and the climb slow (GCC adds a
// few percent per second), so start where a home connection usually is and
// let the estimator pull down if the path cannot take it.
pub(crate) const BWE_INITIAL: f64 = 6_000_000.0;
const BWE_MIN: f64 = 150_000.0;
const BWE_MAX: f64 = 50_000_000.0;

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

/// An access unit and when it was complete, on its way to the decoder.
pub(crate) type Unit = (Vec<u8>, Instant);

/// Where a viewer's access units go to be decoded, and how it talks back.
struct RxContext {
    runtime: Arc<dyn Runtime>,
    units: StdSender<Unit>,
    backlog: Arc<AtomicUsize>,
    /// Confirmations of shown pictures, to send to the broadcaster.
    acks: Mutex<Option<UnboundedReceiver<String>>>,
    stats: Arc<Mutex<Stats>>,
    peer: PeerId,
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
        let stats = Arc::clone(&rx.stats);
        let peer = rx.peer;
        rx.runtime.spawn(Box::pin(async move {
            let mut open = false;
            loop {
                tokio::select! {
                    event = channel.poll() => match event {
                        Some(DataChannelEvent::OnOpen) => open = true,
                        Some(DataChannelEvent::OnMessage(message)) => {
                            // "L,total,encode,network,viewer,estimate" from the broadcaster.
                            let text = String::from_utf8_lossy(&message.data).into_owned();
                            if let Some(path) = parse_path(&text)
                                && let Some(entry) = stats.lock().unwrap().receiving.get_mut(&peer)
                            {
                                entry.path = path;
                            }
                        }
                        Some(DataChannelEvent::OnClose) | None => break,
                        _ => {}
                    },
                    ack = acks.recv() => {
                        let Some(ack) = ack else { break };
                        if open {
                            let _ = channel.send_text(&ack).await;
                        }
                    }
                }
            }
        }));
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let Some(rx) = &self.rx else { return };
        let units = rx.units.clone();
        let backlog = Arc::clone(&rx.backlog);
        rx.runtime.spawn(Box::pin(async move {
            // A 1080p keyframe is hundreds of packets: the builder must be
            // willing to wait for all of them before it gives up on a unit.
            let mut builder = SampleBuilder::new(1024, H264Packet::default(), 90_000);
            while let Some(event) = track.poll().await {
                let TrackRemoteEvent::OnRtpPacket(packet) = event else {
                    continue;
                };
                builder.push(Instant::now(), packet);
                while let Some(sample) = builder.pop(Instant::now()) {
                    backlog.fetch_add(1, Ordering::Relaxed);
                    if units.send((sample.data.to_vec(), Instant::now())).is_err() {
                        return;
                    }
                }
            }
        }));
    }
}

pub(crate) fn parse_path(text: &str) -> Option<crate::stats::Path> {
    let mut parts = text.strip_prefix("L,")?.split(',');
    let mut next = || parts.next()?.parse::<f32>().ok();
    Some(crate::stats::Path {
        total_ms: next()?,
        encode_ms: next()?,
        network_ms: next()?,
        viewer_ms: next()?,
        estimate_kbps: next()?,
    })
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
    target: Arc<AtomicU64>,
) -> Result<Built> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_codec(h264_codec(), RtpCodecKind::Video)?;

    let estimator = ReportingEstimator {
        inner: Gcc::new(BWE_INITIAL, BWE_MIN, BWE_MAX),
        target,
    };
    let registry = configure_congestion_control(
        Registry::new(),
        estimator,
        CongestionFeedback::Twcc,
        &mut media_engine,
    )?;
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

async fn local_description(pc: &Arc<dyn PeerConnection>) -> Result<Value> {
    let description = pc
        .local_description()
        .await
        .ok_or_else(|| anyhow!("no local description"))?;
    Ok(serde_json::to_value(&description)?)
}

/// The broadcaster's side of one viewer: owns the track and the congestion
/// estimate for this path.
pub(crate) struct SendLink {
    pc: Arc<dyn PeerConnection>,
    gathered: watch::Receiver<bool>,
    state: watch::Receiver<RTCPeerConnectionState>,
    track: Arc<TrackLocalStaticSample>,
    sender: Arc<dyn RtpSender>,
    /// Confirmations come back on it, and measurements go out on it.
    pub channel: Arc<dyn DataChannel>,
    /// The congestion controller's target for this path, in bits per second
    /// (as `f64` bits).
    pub estimate: Arc<AtomicU64>,
    writer: Option<(u32, PayloadType)>,
}

impl SendLink {
    pub async fn new(link: &LinkConfig) -> Result<Self> {
        let runtime = default_runtime().context("no runtime")?;
        let estimate = Arc::new(AtomicU64::new(BWE_INITIAL.to_bits()));
        let built = build(link, runtime, None, Arc::clone(&estimate)).await?;
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
        // Confirmations must not wait behind a retransmission: unordered and
        // unreliable, so a late one never holds back the ones after it.
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
        Ok(Self {
            pc: built.pc,
            gathered: built.gathered,
            state: built.state,
            track,
            sender,
            channel,
            estimate,
            writer: None,
        })
    }

    /// The offer, once ICE gathering is done.
    pub async fn create_offer(&mut self) -> Result<Value> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer).await?;
        wait_gathered(&mut self.gathered).await?;
        local_description(&self.pc).await
    }

    pub async fn accept_answer(&self, answer: Value) -> Result<()> {
        let answer: RTCSessionDescription = serde_json::from_value(answer)?;
        self.pc.set_remote_description(answer).await?;
        Ok(())
    }

    /// Waits for the connection, then readies the track for writing.
    pub async fn wait_connected(&mut self) -> Result<()> {
        wait_connected(&mut self.state).await?;
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
        self.writer = Some((ssrc, payload_type));
        Ok(())
    }

    /// Whether the connection is still alive.
    pub fn failed(&self) -> bool {
        matches!(
            *self.state.borrow(),
            RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected
        )
    }

    pub async fn write(&self, data: Bytes, duration: Duration) -> Result<()> {
        let (ssrc, payload_type) = self.writer.ok_or_else(|| anyhow!("not connected"))?;
        self.track
            .sample_writer(ssrc, payload_type)
            .write_sample(&Sample {
                data,
                duration,
                ..Sample::new(Instant::now())
            })
            .await?;
        Ok(())
    }

    pub async fn close(&self) {
        let _ = self.pc.close().await;
    }
}

/// The viewer's side: access units go to `units` for the decoder.
pub(crate) struct RecvLink {
    pc: Arc<dyn PeerConnection>,
    gathered: watch::Receiver<bool>,
}

impl RecvLink {
    pub async fn new(
        link: &LinkConfig,
        peer: PeerId,
        stats: Arc<Mutex<Stats>>,
        feed: DecoderFeed,
    ) -> Result<Self> {
        let runtime = default_runtime().context("no runtime")?;
        let rx = RxContext {
            runtime: Arc::clone(&runtime),
            units: feed.units,
            backlog: feed.backlog,
            acks: Mutex::new(Some(feed.acks)),
            stats,
            peer,
        };
        // The viewer runs the same interceptors so congestion feedback flows
        // back to the broadcaster.
        let built = build(link, runtime, Some(rx), Arc::new(AtomicU64::new(0))).await?;
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
        })
    }

    /// Takes the offer, returns the answer.
    pub async fn accept_offer(&mut self, offer: Value) -> Result<Value> {
        let offer: RTCSessionDescription = serde_json::from_value(offer)?;
        self.pc.set_remote_description(offer).await?;
        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer).await?;
        wait_gathered(&mut self.gathered).await?;
        local_description(&self.pc).await
    }

    pub async fn close(&self) {
        let _ = self.pc.close().await;
    }
}

/// What the receive path hands to the decoder thread.
pub(crate) struct DecoderFeed {
    pub units: StdSender<Unit>,
    pub backlog: Arc<AtomicUsize>,
    pub acks: UnboundedReceiver<String>,
}

/// The decoder thread's side of a `DecoderFeed`.
pub(crate) struct DecoderInput {
    pub units: std::sync::mpsc::Receiver<Unit>,
    pub backlog: Arc<AtomicUsize>,
    pub acks: UnboundedSender<String>,
}

pub(crate) fn decoder_channels() -> (DecoderFeed, DecoderInput) {
    let (units_tx, units_rx) = std::sync::mpsc::channel();
    let (acks_tx, acks_rx) = tokio::sync::mpsc::unbounded_channel();
    let backlog = Arc::new(AtomicUsize::new(0));
    (
        DecoderFeed {
            units: units_tx,
            backlog: Arc::clone(&backlog),
            acks: acks_rx,
        },
        DecoderInput {
            units: units_rx,
            backlog,
            acks: acks_tx,
        },
    )
}

/// Decodes access units into pictures on its own thread, publishes the newest
/// one and confirms each to the broadcaster. Ends when the feed is dropped.
pub(crate) fn spawn_decoder(
    input: DecoderInput,
    peer: PeerId,
    stats: Arc<Mutex<Stats>>,
    publish: impl Fn(Arc<Picture>) + Send + 'static,
) -> std::thread::JoinHandle<()> {
    use openh264::decoder::Decoder;
    use openh264::formats::YUVSource;
    std::thread::spawn(move || {
        let Ok(mut decoder) = Decoder::new() else {
            return;
        };
        let mut seq = 0u64;
        let (mut window, mut frames, mut bytes) = (Instant::now(), 0u32, 0usize);
        while let Ok((data, ready_at)) = input.units.recv() {
            input.backlog.fetch_sub(1, Ordering::Relaxed);
            let started = Instant::now();
            // The first units may fail until a keyframe arrives.
            let Ok(Some(yuv)) = decoder.decode(&data) else {
                continue;
            };
            let decode_ms = started.elapsed().as_secs_f32() * 1000.0;
            let (width, height) = yuv.dimensions();
            let (ys, us, vs) = yuv.strides();
            let copy = |plane: &[u8], stride: usize, w: usize, h: usize| -> Vec<u8> {
                let mut out = Vec::with_capacity(w * h);
                for row in plane.chunks(stride).take(h) {
                    out.extend_from_slice(&row[..w.min(row.len())]);
                }
                out
            };
            seq += 1;
            publish(Arc::new(Picture {
                seq,
                width: width as u32,
                height: height as u32,
                y: copy(yuv.y(), ys, width, height),
                u: copy(yuv.u(), us, width / 2, height / 2),
                v: copy(yuv.v(), vs, width / 2, height / 2),
            }));
            if let Some(counter) = stamp::read(&data) {
                let hold = ready_at.elapsed().as_secs_f32() * 1000.0;
                let _ = input.acks.send(format!("{counter},{hold:.1}"));
            }
            frames += 1;
            bytes += data.len();
            let backlog = input.backlog.load(Ordering::Relaxed);
            let mut stats = stats.lock().unwrap();
            let entry = stats.receiving.entry(peer).or_default();
            entry.width = width as u32;
            entry.height = height as u32;
            entry.decode_ms = smooth(entry.decode_ms, decode_ms);
            entry.backlog = backlog;
            let elapsed = window.elapsed().as_secs_f32();
            if elapsed >= 1.0 {
                entry.fps = frames as f32 / elapsed;
                entry.kbps = bytes as f32 * 8.0 / 1000.0 / elapsed;
                (window, frames, bytes) = (Instant::now(), 0, 0);
            }
        }
    })
}
