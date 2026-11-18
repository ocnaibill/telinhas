//! Plays the sound of the streams in the room: each broadcaster's decoded
//! sound is mixed with its own volume and sent to the default output device.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use telinhas_media::{AudioBuffer, PeerId};

/// The rate the decoded sound has.
const SOURCE_RATE: u32 = 48_000;
/// A source starts playing once this much is waiting, so a late packet does not
/// make it stutter; and starts over when it runs dry.
const PREFILL_MS: f32 = 60.0;
/// How much sound should be waiting, in steady state.
const TARGET_MS: f32 = 70.0;
/// The sender's clock and the sound card's never run at exactly the same
/// speed. Instead of letting the buffer grow until it must be cut (a click, and
/// a delay that keeps getting longer), the playback speeds up or slows down by
/// up to this fraction, which is too little to hear.
const MAX_SPEED_ADJUST: f32 = 0.005;

/// One broadcaster's sound as the mixer plays it.
struct Source {
    buffer: Arc<AudioBuffer>,
    volume: f32,
    primed: bool,
    /// Decoded stereo frames not played yet, for resampling.
    pending: Vec<f32>,
    /// Position inside `pending`, in frames, between 0 and 1 after each call.
    position: f64,
}

/// Mixes sources into stereo frames at the output's rate. It knows nothing of
/// the sound card, so it can be tested.
#[derive(Default)]
pub struct Mixer {
    sources: HashMap<PeerId, Source>,
}

impl Mixer {
    /// Makes the playing sources exactly these, keeping the state of the ones
    /// that stay.
    pub fn set_sources(&mut self, wanted: Vec<(PeerId, Arc<AudioBuffer>, f32)>) {
        self.sources
            .retain(|peer, _| wanted.iter().any(|(id, ..)| id == peer));
        for (peer, buffer, volume) in wanted {
            let source = self.sources.entry(peer).or_insert_with(|| Source {
                buffer: Arc::clone(&buffer),
                volume,
                primed: false,
                pending: Vec::new(),
                position: 0.0,
            });
            // A new decoder (the broadcaster came back) has a new buffer.
            if !Arc::ptr_eq(&source.buffer, &buffer) {
                source.buffer = buffer;
                source.primed = false;
                source.pending.clear();
                source.position = 0.0;
            }
            source.volume = volume;
        }
    }

    /// Fills `out` (interleaved stereo at `rate`) with the sum of every source.
    pub fn render(&mut self, out: &mut [f32], rate: u32) {
        out.fill(0.0);
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        let base_step = f64::from(SOURCE_RATE) / f64::from(rate.max(1));
        for source in self.sources.values_mut() {
            if source.volume <= 0.0 {
                // Keep the buffer from filling up and then jumping ahead.
                let mut sink = vec![0f32; frames * 2];
                source.buffer.pull(&mut sink);
                continue;
            }
            if !source.primed {
                if source.buffer.buffered_ms() < PREFILL_MS {
                    continue;
                }
                source.primed = true;
            }
            // Follow the sender's clock: consume a little faster when too much
            // waits, a little slower when too little does.
            let waiting_ms = source.buffer.buffered_ms()
                + (source.pending.len() / 2) as f32 * 1000.0 / SOURCE_RATE as f32;
            let adjust = ((waiting_ms - TARGET_MS) / TARGET_MS * 0.01)
                .clamp(-MAX_SPEED_ADJUST, MAX_SPEED_ADJUST);
            let step = base_step * (1.0 + f64::from(adjust));
            // Input frames needed to produce `frames` output frames.
            let needed = (source.position + frames as f64 * step).ceil() as usize + 2;
            let have = source.pending.len() / 2;
            if have < needed {
                let mut chunk = vec![0f32; (needed - have) * 2];
                let real = source.buffer.pull(&mut chunk);
                source.pending.extend_from_slice(&chunk[..real]);
                if real < chunk.len() {
                    // Ran dry: play what there is, then wait to refill.
                    source.primed = false;
                }
            }
            let available = source.pending.len() / 2;
            let mut position = source.position;
            for frame in 0..frames {
                let index = position as usize;
                if index + 1 >= available {
                    break;
                }
                let fraction = (position - index as f64) as f32;
                for channel in 0..2 {
                    let a = source.pending[index * 2 + channel];
                    let b = source.pending[(index + 1) * 2 + channel];
                    out[frame * 2 + channel] += (a + (b - a) * fraction) * source.volume;
                }
                position += step;
            }
            let used = (position as usize).min(available);
            source.pending.drain(..used * 2);
            source.position = position - used as f64;
        }
        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

/// The running output stream. Dropping it stops the sound.
pub struct Playback {
    mixer: Arc<Mutex<Mixer>>,
    _stream: cpal::Stream,
}

impl Playback {
    /// Opens the default output device.
    pub fn start() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "nenhuma saída de áudio encontrada".to_owned())?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config = supported.config();
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, &mixer),
            SampleFormat::I16 => build::<i16>(&device, config, &mixer),
            SampleFormat::U16 => build::<u16>(&device, config, &mixer),
            SampleFormat::I32 => build::<i32>(&device, config, &mixer),
            other => Err(format!("formato de áudio sem suporte: {other}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            mixer,
            _stream: stream,
        })
    }

    pub fn set_sources(&self, wanted: Vec<(PeerId, Arc<AudioBuffer>, f32)>) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.set_sources(wanted);
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mixer: &Arc<Mutex<Mixer>>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let rate = config.sample_rate;
    let mixer = Arc::clone(mixer);
    let mut stereo: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let frames = data.len() / channels;
                stereo.resize(frames * 2, 0.0);
                match mixer.lock() {
                    Ok(mut mixer) => mixer.render(&mut stereo, rate),
                    Err(_) => stereo.fill(0.0),
                }
                for (frame, out) in data.chunks_mut(channels).enumerate() {
                    let (left, right) = (stereo[frame * 2], stereo[frame * 2 + 1]);
                    for (channel, sample) in out.iter_mut().enumerate() {
                        let value = match (channels, channel) {
                            (1, _) => (left + right) / 2.0,
                            (_, 0) => left,
                            (_, 1) => right,
                            _ => 0.0,
                        };
                        *sample = T::from_sample(value);
                    }
                }
            },
            |error| eprintln!("áudio: {error}"),
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_with(value: f32, ms: usize) -> Arc<AudioBuffer> {
        let buffer = Arc::new(AudioBuffer::default());
        buffer.push_for_test(&vec![value; 48 * ms * 2]);
        buffer
    }

    const PEER: PeerId = PeerId::from_u128(1);

    #[test]
    fn a_source_waits_for_its_prefill_then_plays_with_its_volume() {
        let mut mixer = Mixer::default();
        let buffer = buffer_with(0.5, 20); // not enough yet
        mixer.set_sources(vec![(PEER, Arc::clone(&buffer), 0.5)]);
        let mut out = vec![0f32; 960 * 2];
        mixer.render(&mut out, 48_000);
        assert!(
            out.iter().all(|s| *s == 0.0),
            "it must wait for the prefill"
        );

        buffer.push_for_test(&vec![0.5; 48 * 60 * 2]);
        mixer.render(&mut out, 48_000);
        // 0.5 sample at volume 0.5.
        assert!(
            out.iter().all(|s| (*s - 0.25).abs() < 1e-5),
            "{:?}",
            &out[..4]
        );
    }

    #[test]
    fn two_sources_add_and_never_clip() {
        let mut mixer = Mixer::default();
        mixer.set_sources(vec![
            (PEER, buffer_with(0.8, 100), 1.0),
            (PeerId::from_u128(2), buffer_with(0.8, 100), 1.0),
        ]);
        let mut out = vec![0f32; 480 * 2];
        mixer.render(&mut out, 48_000);
        assert!(out.iter().all(|s| *s == 1.0), "1.6 is clamped to 1");
    }

    #[test]
    fn running_dry_goes_silent_and_waits_to_refill() {
        let mut mixer = Mixer::default();
        let buffer = buffer_with(0.4, 70);
        mixer.set_sources(vec![(PEER, Arc::clone(&buffer), 1.0)]);
        let mut out = vec![0f32; 48 * 200 * 2]; // asks for more than there is
        mixer.render(&mut out, 48_000);
        assert!(out[..100].iter().any(|s| *s > 0.0));
        assert!(
            out[out.len() - 10..].iter().all(|s| *s == 0.0),
            "the tail is silence"
        );
        // Dry now: a small refill is not played until the prefill is there again.
        buffer.push_for_test(&vec![0.4; 48 * 10 * 2]);
        let mut out = vec![0f32; 960 * 2];
        mixer.render(&mut out, 48_000);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_slower_output_rate_consumes_proportionally() {
        let mut mixer = Mixer::default();
        let buffer = buffer_with(0.3, 200);
        mixer.set_sources(vec![(PEER, Arc::clone(&buffer), 1.0)]);
        let before = buffer.buffered_ms();
        // One second at 44.1 kHz needs one second of source.
        let mut out = vec![0f32; 44_100 * 2];
        mixer.render(&mut out, 44_100);
        let used = before - buffer.buffered_ms();
        assert!(
            used > 150.0 && used <= 200.0,
            "used {used} ms of a 200 ms buffer"
        );
        assert!(out[..2000].iter().all(|s| (*s - 0.3).abs() < 1e-3));
    }

    #[test]
    fn a_sender_running_slightly_fast_does_not_make_the_delay_grow() {
        let mut mixer = Mixer::default();
        let buffer = buffer_with(0.3, 80);
        mixer.set_sources(vec![(PEER, Arc::clone(&buffer), 1.0)]);
        let mut out = vec![0f32; 960 * 2];
        // Every 20 ms tick the sender delivers 0.4% more than the card plays:
        // five minutes of that would pile up far more than the 200 ms cap.
        let mut peak = 0f32;
        for _ in 0..15_000 {
            buffer.push_for_test(&vec![0.3; 964 * 2]);
            mixer.render(&mut out, 48_000);
            peak = peak.max(buffer.buffered_ms());
        }
        let waiting = buffer.buffered_ms();
        assert!(waiting < 120.0, "{waiting} ms waiting at the end");
        assert!(peak < 200.0, "the buffer hit its cap ({peak} ms)");
    }

    #[test]
    fn a_sender_running_slightly_slow_does_not_run_dry() {
        let mut mixer = Mixer::default();
        let buffer = buffer_with(0.3, 90);
        mixer.set_sources(vec![(PEER, Arc::clone(&buffer), 1.0)]);
        let mut out = vec![0f32; 960 * 2];
        let mut silent_ticks = 0;
        for _ in 0..15_000 {
            buffer.push_for_test(&vec![0.3; 956 * 2]);
            mixer.render(&mut out, 48_000);
            silent_ticks += usize::from(out.iter().all(|s| *s == 0.0));
        }
        assert_eq!(silent_ticks, 0, "it ran dry and went silent");
    }

    #[test]
    fn muted_sources_are_drained_not_played() {
        let mut mixer = Mixer::default();
        let buffer = buffer_with(0.5, 100);
        mixer.set_sources(vec![(PEER, Arc::clone(&buffer), 0.0)]);
        let mut out = vec![0f32; 960 * 2];
        mixer.render(&mut out, 48_000);
        assert!(out.iter().all(|s| *s == 0.0));
        assert!(buffer.buffered_ms() < 100.0, "muted sound must not pile up");
    }
}
