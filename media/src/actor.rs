//! The task that owns every connection and answers the application's commands.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use telinhas_protocol::{PeerId, SignalKind, Value};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::link::{LinkConfig, RecvLink, SendLink, spawn_decoder};
use crate::send::{BroadcastSpec, Encoded, Targets, spawn_broadcast};
use crate::{Command, Event, EventSender, Latest};

pub(crate) struct Shared {
    pub link: LinkConfig,
    pub events: EventSender,
    pub latest: Latest,
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
                stop_broadcast(&mut state);
                start_broadcast(&shared, &mut state, spec, viewers);
            }
            Command::StopBroadcast => stop_broadcast(&mut state),
            Command::PeerJoined(peer) => {
                if state.broadcast.is_some() {
                    start_outgoing(&shared, &mut state, peer);
                }
            }
            Command::PeerLeft(peer) => {
                drop_outgoing(&mut state, peer);
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
    stop_broadcast(&mut state);
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
    spawn_broadcast(
        spec,
        frames.clone(),
        Arc::clone(&stop),
        Arc::clone(&need_key),
        Arc::clone(&targets),
        shared.events.clone(),
        Arc::clone(&shared.wake),
    );
    state.broadcast = Some(Broadcast {
        stop,
        need_key,
        targets,
        frames,
    });
    for peer in viewers {
        start_outgoing(shared, state, peer);
    }
}

fn stop_broadcast(state: &mut State) {
    if let Some(broadcast) = state.broadcast.take() {
        broadcast.stop.store(true, Ordering::Relaxed);
    }
    state.outgoing.clear();
}

fn drop_outgoing(state: &mut State, peer: PeerId) {
    if state.outgoing.remove(&peer).is_some()
        && let Some(broadcast) = &state.broadcast
    {
        broadcast.targets.lock().unwrap().remove(&peer);
    }
}

fn drop_incoming(shared: &Shared, state: &mut State, peer: PeerId) {
    state.incoming.remove(&peer);
    shared.latest.lock().unwrap().remove(&peer);
}

/// Sends the broadcast to one viewer: offer, wait for the answer, then write
/// every encoded picture to the viewer's track.
fn start_outgoing(shared: &Arc<Shared>, state: &mut State, peer: PeerId) {
    if state.broadcast.is_none() {
        return;
    }
    drop_outgoing(state, peer);
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
    loop {
        match frames.recv().await {
            Ok(encoded) => link.write(encoded.data.clone(), encoded.duration).await?,
            Err(RecvError::Lagged(_)) => need_key.store(true, Ordering::Relaxed),
            Err(RecvError::Closed) => return Ok(()),
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
        let (units_tx, units_rx) = std::sync::mpsc::channel();
        let publisher = Arc::clone(&shared);
        let announced = AtomicBool::new(false);
        spawn_decoder(units_rx, move |picture| {
            publisher.latest.lock().unwrap().insert(peer, picture);
            if announced.swap(true, Ordering::Relaxed) {
                (publisher.wake)();
            } else {
                publisher.tell(Event::Receiving(peer));
            }
        });
        let Ok(mut link) = RecvLink::new(&shared.link, units_tx).await else {
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
