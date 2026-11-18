//! Sound: Opus on the wire, 20 ms frames of 48 kHz stereo.
//!
//! The broadcaster captures (or generates) sound, encodes each 20 ms frame
//! once and hands it to every viewer's link. A viewer decodes into a small
//! buffer that the playback pulls from.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bytes::Bytes;
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use telinhas_capture::audio::{self as capture_audio, AudioSource, CHANNELS, FRAME_SAMPLES};
use telinhas_protocol::PeerId;
use tokio::sync::broadcast;

use crate::Event;
use crate::stats::Stats;

pub const SAMPLE_RATE: u32 = capture_audio::SAMPLE_RATE;
/// Interleaved samples in one 20 ms frame.
pub const FRAME_LEN: usize = FRAME_SAMPLES * CHANNELS;
pub const FRAME_PERIOD: Duration = Duration::from_millis(20);
/// 128 kbps stereo music-grade Opus: small next to the video.
const BITRATE: i32 = 128_000;
/// The most Opus ever produces for one frame is far below this.
const MAX_PACKET: usize = 1500;

/// Where the broadcast's sound comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioInput {
    /// A steady 440 Hz tone, for tests and for machines without audio capture.
    Synthetic,
    /// Everything the system plays, except this app and the named programs.
    System { exclude: Vec<String> },
    /// One application and the processes it started.
    Process { pid: u32 },
}

/// One encoded 20 ms frame on its way to every viewer.
pub(crate) struct AudioPacket {
    pub data: Bytes,
}

/// What the receive path hands to the audio decoder.
pub(crate) enum AudioUnit {
    Packet(Vec<u8>),
    /// Packets missing before the next one: the decoder conceals them.
    Lost(u16),
}

/// Decoded sound waiting to be played. The playback pulls from it, and what
/// the network delivers a little early or late is absorbed here.
#[derive(Default)]
pub struct AudioBuffer {
    samples: Mutex<VecDeque<f32>>,
}

/// More than this waiting means playback runs slower than the network: the
/// oldest is dropped, so delay does not grow.
const MAX_BUFFERED: usize = SAMPLE_RATE as usize * CHANNELS / 5; // 200 ms

impl AudioBuffer {
    pub(crate) fn push(&self, frame: &[f32]) {
        let mut samples = self.samples.lock().unwrap();
        samples.extend(frame.iter().copied());
        if samples.len() > MAX_BUFFERED {
            let excess = samples.len() - MAX_BUFFERED * 3 / 4;
            samples.drain(..excess);
        }
    }

    /// Fills `out` (interleaved stereo) with what is waiting and returns how
    /// many samples were real; the rest of `out` is silence.
    pub fn pull(&self, out: &mut [f32]) -> usize {
        let mut samples = self.samples.lock().unwrap();
        let take = samples.len().min(out.len());
        for (slot, sample) in out.iter_mut().zip(samples.drain(..take)) {
            *slot = sample;
        }
        out[take..].fill(0.0);
        take
    }

    /// How much sound waits, in milliseconds.
    pub fn buffered_ms(&self) -> f32 {
        self.samples.lock().unwrap().len() as f32 * 1000.0 / (SAMPLE_RATE as f32 * CHANNELS as f32)
    }
}

fn encoder() -> Result<Encoder, String> {
    let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Stereo, Application::Audio)
        .map_err(|e| e.to_string())?;
    encoder
        .set_bitrate(Bitrate::Bits(BITRATE))
        .map_err(|e| e.to_string())?;
    Ok(encoder)
}

/// Starts producing encoded frames into `out` until `stop` is set. The thread
/// owns the capture, so dropping it ends the capture too.
pub(crate) fn spawn_audio(
    input: AudioInput,
    out: broadcast::Sender<Arc<AudioPacket>>,
    stop: Arc<AtomicBool>,
    tell: Arc<dyn Fn(Event) + Send + Sync>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(reason) = run_audio(input, &out, &stop) {
            tell(Event::AudioFailed(reason));
        }
    })
}

fn run_audio(
    input: AudioInput,
    out: &broadcast::Sender<Arc<AudioPacket>>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let mut encoder = encoder()?;
    let mut packet = vec![0u8; MAX_PACKET];
    let out = out.clone();
    let mut send = move |frame: &[f32]| {
        if let Ok(len) = encoder.encode_float(frame, &mut packet) {
            // Nobody listening yet is fine: the frame is dropped.
            let _ = out.send(Arc::new(AudioPacket {
                data: Bytes::copy_from_slice(&packet[..len]),
            }));
        }
    };
    match input {
        AudioInput::Synthetic => {
            let mut frame = vec![0f32; FRAME_LEN];
            let mut tick = 0u64;
            let mut next = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                for i in 0..FRAME_SAMPLES {
                    let t = (tick * FRAME_SAMPLES as u64 + i as u64) as f32 / SAMPLE_RATE as f32;
                    let value = 0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                    frame[i * CHANNELS] = value;
                    frame[i * CHANNELS + 1] = value;
                }
                send(&frame);
                tick += 1;
                next += FRAME_PERIOD;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else if now - next > FRAME_PERIOD * 5 {
                    next = now;
                }
            }
            Ok(())
        }
        AudioInput::System { exclude } => capture(AudioSource::System { exclude }, send, stop),
        AudioInput::Process { pid } => capture(AudioSource::Process { pid }, send, stop),
    }
}

fn capture(
    source: AudioSource,
    send: impl FnMut(&[f32]) + Send + 'static,
    stop: &AtomicBool,
) -> Result<(), String> {
    let capture = capture_audio::start_audio(source, send).map_err(|e| e.to_string())?;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(capture);
    Ok(())
}

/// Decodes what arrives into `buffer`, and keeps the audio numbers.
pub(crate) fn spawn_audio_decoder(
    units: Receiver<AudioUnit>,
    peer: PeerId,
    stats: Arc<Mutex<Stats>>,
    buffer: Arc<AudioBuffer>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let Ok(mut decoder) = Decoder::new(SAMPLE_RATE, Channels::Stereo) else {
            return;
        };
        let mut pcm = vec![0f32; FRAME_LEN * 6];
        while let Ok(unit) = units.recv() {
            let (data, lost): (&[u8], u16) = match &unit {
                AudioUnit::Packet(data) => (data, 0),
                AudioUnit::Lost(n) => (&[], *n),
            };
            // Conceal what went missing, then decode what arrived.
            let mut peak = 0f32;
            let mut decoded = 0u64;
            for step in 0..=lost {
                let bytes: &[u8] = if step == lost { data } else { &[] };
                if bytes.is_empty() && step == lost {
                    break;
                }
                if let Ok(frames) = decoder.decode_float(bytes, &mut pcm, false) {
                    let samples = &pcm[..frames * CHANNELS];
                    peak = samples.iter().fold(peak, |p, s| p.max(s.abs()));
                    buffer.push(samples);
                    decoded += 1;
                }
            }
            let mut stats = stats.lock().unwrap();
            let entry = &mut stats.receiving.entry(peer).or_default().audio;
            entry.packets += u64::from(matches!(unit, AudioUnit::Packet(_)));
            entry.lost += u64::from(lost);
            entry.decoded += decoded;
            entry.peak = entry.peak.max(peak);
            entry.buffered_ms = buffer.buffered_ms();
        }
    })
}
