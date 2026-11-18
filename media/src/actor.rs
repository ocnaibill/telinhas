//! The task that owns every connection and answers the application's commands.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use webrtc::data_channel::DataChannelEvent;

use telinhas_protocol::{PeerId, SignalKind, Value};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::link::{LinkConfig, RecvLink, SendLink, decoder_channels, spawn_decoder};
use crate::send::{BroadcastSpec, Encoded, Targets, spawn_broadcast};
use crate::stats::{Path, Stats, smooth};
use crate::{Command, Event, EventSender, Latest};

/// A picture unconfirmed for this long is considered lost.
const LOST_AFTER: Duration = Duration::from_secs(2);
/// Never allow fewer pictures in flight than this, whatever the frame rate.
const MIN_IN_FLIGHT: usize = 8;

pub(crate) struct Shared {
    pub link: LinkConfig,
    pub events: EventSender,
    pub latest: Latest,
    pub stats: Arc<Mutex<Stats>>,
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn tell(&self, event: Event) {
        let _ = self.events.send(event);
        (self.wake)();
    }
}

/// A running broadcast: the encoder thread and the pictures it produces.
struct Broadcast {
    stop: Arc<AtomicBool>,
    need_key: Arc<AtomicBool>,
    targets: Targets,
    frames: broadcast::Sender<Arc<Encoded>>,
    thread: std::thread::JoinHandle<()>,
}

/// Dropping `stop` tells the task to close its connection and end.
struct Outgoing {
    /// Answers from the viewer.
    answers: UnboundedSender<Value>,
    _stop: oneshot::Sender<()>,
}

struct Incoming {
    _stop: oneshot::Sender<()>,
}

#[derive(Default)]
struct State {
    broadcast: Option<Broadcast>,
    outgoing: HashMap<PeerId, Outgoing>,
    incoming: HashMap<PeerId, Incoming>,
}

pub(crate) async fn run(shared: Shared, mut inbox: UnboundedReceiver<Command>) {
    let shared = Arc::new(shared);
    let mut state = State::default();
    while let Some(command) = inbox.recv().await {
        match command {
            Command::StartBroadcast { spec, viewers } => {
                if state.broadcast.is_some() {
                    switch_broadcast(&shared, &mut state, spec, viewers);
                } else {
                    start_broadcast(&shared, &mut state, spec, viewers);
                }
            }
            Command::StopBroadcast => stop_broadcast(&shared, &mut state),
            Command::PeerJoined(peer) => {
                if state.broadcast.is_some() {
                    start_outgoing(&shared, &mut state, peer);
                }
            }
            Command::PeerLeft(peer) => {
                drop_outgoing(&shared, &mut state, peer);
                drop_incoming(&shared, &mut state, peer);
            }
            Command::StreamEnded(peer) => drop_incoming(&shared, &mut state, peer),
            Command::Signal {
                from,
                kind,
                payload,
            } => match kind {
                SignalKind::Offer => start_incoming(&shared, &mut state, from, payload),
                SignalKind::Answer => {
                    if let Some(out) = state.outgoing.get(&from) {
                        let _ = out.answers.send(payload);
                    }
                }
                // Descriptions are complete, there is nothing to add.
                SignalKind::Ice => {}
            },
            Command::Shutdown => break,
        }
    }
    stop_broadcast(&shared, &mut state);
    let peers: Vec<PeerId> = state.incoming.keys().copied().collect();
    for peer in peers {
        drop_incoming(&shared, &mut state, peer);
    }
}

fn start_broadcast(
    shared: &Arc<Shared>,
    state: &mut State,
    spec: BroadcastSpec,
    viewers: Vec<PeerId>,
) {
    let (frames, _) = broadcast::channel(8);
    let stop = Arc::new(AtomicBool::new(false));
    let need_key = Arc::new(AtomicBool::new(false));
    let targets: Targets = Arc::default();
    let thread = spawn_broadcast(
        spec,
        frames.clone(),
        Arc::clone(&stop),
        Arc::clone(&need_key),
        Arc::clone(&targets),
        Arc::clone(&shared.stats),
        {
            let shared = Arc::clone(shared);
            Arc::new(move |event| shared.tell(event))
        },
    );
    state.broadcast = Some(Broadcast {
        stop,
        need_key,
        targets,
        frames,
        thread,
    });
    for peer in viewers {
        start_outgoing(shared, state, peer);
    }
}

/// Changes what is broadcast (another screen or window, a new size or rate)
/// without touching the connections: only the encoder starts over, and its
/// first picture is a keyframe (DEC-11, DEC-44).
fn switch_broadcast(
    shared: &Arc<Shared>,
    state: &mut State,
    spec: BroadcastSpec,
    viewers: Vec<PeerId>,
) {
    let Some(old) = state.broadcast.take() else {
        return;
    };
    old.stop.store(true, Ordering::Relaxed);
    let _ = old.thread.join();
    let stop = Arc::new(AtomicBool::new(false));
    old.need_key.store(true, Ordering::Relaxed);
    let thread = spawn_broadcast(
        spec,
        old.frames.clone(),
        Arc::clone(&stop),
        Arc::clone(&old.need_key),
        Arc::clone(&old.targets),
        Arc::clone(&shared.stats),
        {
            let shared = Arc::clone(shared);
            Arc::new(move |event| shared.tell(event))
        },
    );
    state.broadcast = Some(Broadcast {
        stop,
        thread,
        ..old
    });
    for peer in viewers {
        if !state.outgoing.contains_key(&peer) {
            start_outgoing(shared, state, peer);
        }
    }
}

fn stop_broadcast(shared: &Shared, state: &mut State) {
    if let Some(broadcast) = state.broadcast.take() {
        broadcast.stop.store(true, Ordering::Relaxed);
    }
    state.outgoing.clear();
    shared.stats.lock().unwrap().viewers.clear();
}

fn drop_outgoing(shared: &Shared, state: &mut State, peer: PeerId) {
    if state.outgoing.remove(&peer).is_some()
        && let Some(broadcast) = &state.broadcast
    {
        broadcast.targets.lock().unwrap().remove(&peer);
    }
    shared.stats.lock().unwrap().viewers.remove(&peer);
}

fn drop_incoming(shared: &Shared, state: &mut State, peer: PeerId) {
    state.incoming.remove(&peer);
    shared.latest.lock().unwrap().remove(&peer);
    shared.stats.lock().unwrap().receiving.remove(&peer);
}

/// Sends the broadcast to one viewer: offer, wait for the answer, then write
/// every encoded picture to the viewer's track.
fn start_outgoing(shared: &Arc<Shared>, state: &mut State, peer: PeerId) {
    if state.broadcast.is_none() {
        return;
    }
    drop_outgoing(shared, state, peer);
    let Some(broadcast) = &state.broadcast else {
        return;
    };
    let (answers_tx, answers) = unbounded_channel::<Value>();
    let (stop_tx, mut stop) = oneshot::channel::<()>();
    let frames = broadcast.frames.subscribe();
    let need_key = Arc::clone(&broadcast.need_key);
    let targets = Arc::clone(&broadcast.targets);
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        let Ok(mut link) = SendLink::new(&shared.link).await else {
            return;
        };
        tokio::select! {
            _ = &mut stop => {}
            // One viewer failing must not stop the others, so the error ends
            // only this task.
            _ = drive_outgoing(&shared, peer, &mut link, answers, frames, &need_key, &targets) => {}
        }
        targets.lock().unwrap().remove(&peer);
        link.close().await;
    });
    state.outgoing.insert(
        peer,
        Outgoing {
            answers: answers_tx,
            _stop: stop_tx,
        },
    );
}

async fn drive_outgoing(
    shared: &Shared,
    peer: PeerId,
    link: &mut SendLink,
    mut answers: UnboundedReceiver<Value>,
    mut frames: broadcast::Receiver<Arc<Encoded>>,
    need_key: &AtomicBool,
    targets: &Targets,
) -> anyhow::Result<()> {
    let offer = link.create_offer().await?;
    shared.tell(Event::Signal {
        to: peer,
        kind: SignalKind::Offer,
        payload: offer,
    });
    let answer = tokio::time::timeout(Duration::from_secs(30), answers.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no answer"))?;
    link.accept_answer(answer).await?;
    link.wait_connected().await?;
    targets
        .lock()
        .unwrap()
        .insert(peer, Arc::clone(&link.estimate));
    // The new viewer needs a picture it can start from.
    need_key.store(true, Ordering::Relaxed);
    // When each picture was taken and when it was handed to the network, until
    // the viewer confirms it.
    let mut sent: HashMap<u32, (Instant, Instant)> = HashMap::new();
    let mut path = Path::default();
    let mut open = false;
    let mut last_feedback = Instant::now();
    loop {
        tokio::select! {
            frame = frames.recv() => match frame {
                Ok(encoded) => {
                    // A picture whose confirmation did not come back in this
                    // long is given up on.
                    sent.retain(|_, (captured, _)| captured.elapsed() < LOST_AFTER);
                    // Too many pictures still unconfirmed means the network
                    // cannot keep up. Sending more only adds delay, so skip
                    // this one and ask for a keyframe to start over from.
                    let limit = (0.5 / encoded.duration.as_secs_f32().max(0.001)) as usize;
                    if sent.len() > limit.max(MIN_IN_FLIGHT) {
                        need_key.store(true, Ordering::Relaxed);
                        continue;
                    }
                    sent.insert(encoded.counter, (encoded.captured_at, Instant::now()));
                    link.write(encoded.data.clone(), encoded.duration).await?;
                }
                Err(RecvError::Lagged(_)) => need_key.store(true, Ordering::Relaxed),
                Err(RecvError::Closed) => return Ok(()),
            },
            event = link.channel.poll() => match event {
                Some(DataChannelEvent::OnOpen) => open = true,
                Some(DataChannelEvent::OnMessage(message)) => {
                    let text = String::from_utf8_lossy(&message.data).into_owned();
                    if text == "K" {
                        need_key.store(true, Ordering::Relaxed);
                        continue;
                    }
                    let mut parts = text.split(',');
                    let counter = parts.next().and_then(|c| c.parse::<u32>().ok());
                    let held = parts.next().and_then(|h| h.parse::<f32>().ok());
                    let (Some(counter), Some(held)) = (counter, held) else { continue };
                    let Some((captured, written)) = sent.remove(&counter) else { continue };
                    let total = captured.elapsed().as_secs_f32() * 1000.0;
                    let encode = written.duration_since(captured).as_secs_f32() * 1000.0;
                    path.total_ms = smooth(path.total_ms, total);
                    path.encode_ms = smooth(path.encode_ms, encode);
                    path.viewer_ms = smooth(path.viewer_ms, held);
                    path.network_ms = smooth(path.network_ms, (total - encode - held).max(0.0));
                    path.estimate_kbps =
                        (f64::from_bits(link.estimate.load(Ordering::Relaxed)) / 1000.0) as f32;
                    shared.stats.lock().unwrap().viewers.insert(peer, path.clone());
                    // Tell the viewer too, so its screen can show where the time goes.
                    if open && last_feedback.elapsed() >= Duration::from_millis(500) {
                        last_feedback = Instant::now();
                        let _ = link
                            .channel
                            .send_text(&format!(
                                "L,{:.1},{:.1},{:.1},{:.1},{:.0}",
                                path.total_ms,
                                path.encode_ms,
                                path.network_ms,
                                path.viewer_ms,
                                path.estimate_kbps
                            ))
                            .await;
                    }
                }
                Some(DataChannelEvent::OnClose) | None => return Ok(()),
                _ => {}
            },
        }
        if link.failed() {
            return Ok(());
        }
    }
}

/// Receives from one broadcaster: take its offer, answer, then decode.
fn start_incoming(shared: &Arc<Shared>, state: &mut State, peer: PeerId, offer: Value) {
    drop_incoming(shared, state, peer);
    let (stop_tx, mut stop) = oneshot::channel::<()>();
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        let (feed, input) = decoder_channels();
        let publisher = Arc::clone(&shared);
        let announced = AtomicBool::new(false);
        spawn_decoder(input, peer, Arc::clone(&shared.stats), move |picture| {
            publisher.latest.lock().unwrap().insert(peer, picture);
            if announced.swap(true, Ordering::Relaxed) {
                (publisher.wake)();
            } else {
                publisher.tell(Event::Receiving(peer));
            }
        });
        let Ok(mut link) = RecvLink::new(&shared.link, peer, Arc::clone(&shared.stats), feed).await
        else {
            return;
        };
        tokio::select! {
            _ = &mut stop => {}
            // Once answered, the connection delivers on its own until stopped.
            () = async {
                if let Ok(answer) = link.accept_offer(offer).await {
                    shared.tell(Event::Signal {
                        to: peer,
                        kind: SignalKind::Answer,
                        payload: answer,
                    });
                    std::future::pending::<()>().await;
                }
            } => {}
        }
        link.close().await;
    });
    state.incoming.insert(peer, Incoming { _stop: stop_tx });
}
