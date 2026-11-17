//! M2 spike command line: `loopback`, `send` and `recv`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use telinhas_spike::link::{LinkConfig, RecvLink, SendLink};
use telinhas_spike::media::StreamConfig;
use telinhas_spike::signaling::Signaling;

#[derive(Parser)]
#[command(about = "Telinhas M2 spike: synthetic H.264 over WebRTC")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Two peers in this process, over loopback.
    Loopback {
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// 0 to 5: how noisy the picture is. Higher needs more bandwidth.
        #[arg(long, default_value_t = 3)]
        noise: u8,
    },
    /// Sends the synthetic stream to the other participant of a room.
    Send {
        #[command(flatten)]
        room: RoomArgs,
        #[arg(long, default_value_t = 1280)]
        width: usize,
        #[arg(long, default_value_t = 720)]
        height: usize,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        /// Encoder ceiling, in kbps.
        #[arg(long, default_value_t = 4000)]
        max_kbps: u32,
        /// 0 to 5: how noisy the picture is. Higher needs more bandwidth.
        #[arg(long, default_value_t = 3)]
        noise: u8,
    },
    /// Lists monitors, windows and GPUs, and tests which H.264 encoders work.
    Probe,
    /// Captures the screen, encodes it with NVENC through ffmpeg and reports
    /// the bitrate the content needs.
    Measure {
        /// `monitor:N`, `window:HANDLE` or `window:TEXT`.
        #[arg(long)]
        source: String,
        #[arg(long, default_value_t = 30)]
        seconds: u64,
        #[arg(long, default_value_t = 60)]
        fps: u32,
        /// `cq:N` (constant quality, lower is better), `cbr:KBPS` or
        /// `capped:N:KBPS` (constant quality with a bitrate ceiling).
        #[arg(long, default_value = "cq:23")]
        rate: String,
        /// Scale before encoding, as `WIDTHxHEIGHT`, such as `1280x720`.
        #[arg(long)]
        scale: Option<String>,
        #[arg(long)]
        no_cursor: bool,
        /// Also save the encoded video to this file (raw H.264, open it with
        /// VLC or mpv) to judge the quality by eye.
        #[arg(long)]
        save: Option<String>,
    },
    /// Captures a monitor or window and measures it, without encoding.
    Capture {
        /// `monitor:N`, `window:HANDLE` or `window:TEXT` (title or program).
        #[arg(long)]
        source: String,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// Leave the mouse cursor out of the picture.
        #[arg(long)]
        no_cursor: bool,
        /// Ask Windows to hide the capture border.
        #[arg(long)]
        no_border: bool,
        /// Ask Windows for at most this many frames per second.
        #[arg(long)]
        max_fps: Option<u32>,
    },
    /// Receives, decodes and measures.
    Recv {
        #[command(flatten)]
        room: RoomArgs,
    },
}

#[derive(Args)]
struct RoomArgs {
    /// Signaling URL, such as `ws://127.0.0.1:8080/ws` or `wss://host/ws`.
    #[arg(long)]
    server: String,
    /// Room code to join. Omit to create a room.
    #[arg(long)]
    code: Option<String>,
    #[arg(long, default_value = "spike")]
    nick: String,
    /// How long to run once connected.
    #[arg(long, default_value_t = 600)]
    seconds: u64,
    /// Same machine only: bind loopback, no STUN.
    #[arg(long)]
    local: bool,
    /// TURN relay, as `turn:host:3478`.
    #[arg(long, requires_all = ["turn_user", "turn_pass"])]
    turn_url: Option<String>,
    #[arg(long)]
    turn_user: Option<String>,
    #[arg(long)]
    turn_pass: Option<String>,
}

impl RoomArgs {
    fn link(&self) -> LinkConfig {
        let mut link = if self.local {
            LinkConfig::loopback()
        } else {
            LinkConfig::internet()
        };
        if let (Some(url), Some(user), Some(pass)) =
            (&self.turn_url, &self.turn_user, &self.turn_pass)
        {
            link.turn = Some((url.clone(), user.clone(), pass.clone()));
        }
        link
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Loopback { seconds, noise } => loopback(seconds, noise).await,
        Command::Send {
            room,
            width,
            height,
            fps,
            max_kbps,
            noise,
        } => {
            let cfg = StreamConfig {
                width,
                height,
                fps,
                max_bps: max_kbps * 1000,
                noise_level: noise,
                ..StreamConfig::default()
            };
            send(&room, cfg).await
        }
        Command::Recv { room } => recv(&room).await,
        Command::Capture {
            source,
            seconds,
            no_cursor,
            no_border,
            max_fps,
        } => telinhas_spike::capture_cmd::run(&source, seconds, !no_cursor, !no_border, max_fps),
        Command::Measure {
            source,
            seconds,
            fps,
            rate,
            scale,
            no_cursor,
            save,
        } => {
            let scale = scale
                .map(|text| {
                    let (w, h) = text
                        .split_once('x')
                        .ok_or_else(|| anyhow::anyhow!("use WIDTHxHEIGHT"))?;
                    anyhow::Ok((w.parse()?, h.parse()?))
                })
                .transpose()?;
            telinhas_spike::measure::run(&telinhas_spike::measure::Args {
                source,
                seconds,
                fps,
                rate,
                scale,
                cursor: !no_cursor,
                save,
            })
        }
        Command::Probe => {
            telinhas_spike::probe::run();
            Ok(())
        }
    }
}

async fn loopback(seconds: u64, noise: u8) -> Result<()> {
    let link = LinkConfig::loopback();
    let mut sender = SendLink::new(&link).await?;
    let mut receiver = RecvLink::new(&link).await?;
    let offer = sender.create_offer().await?;
    let answer = receiver.accept_offer(&offer).await?;
    sender.accept_answer(&answer).await?;
    sender.wait_connected().await?;
    receiver.wait_connected().await?;
    println!("connected");
    let cfg = StreamConfig {
        noise_level: noise,
        ..StreamConfig::default()
    };
    run_sender(&sender, cfg, seconds, Some(&receiver)).await?;
    println!("receiver: {}", receiver.stats.report());
    Ok(())
}

async fn send(room: &RoomArgs, cfg: StreamConfig) -> Result<()> {
    let mut signaling = Signaling::connect(&room.server).await?;
    let joined = signaling.enter(room.code.as_deref(), &room.nick).await?;
    println!("room {}", joined["code"].as_str().unwrap_or("?"));
    println!("waiting for the receiver to join...");
    let peer = signaling.wait_for_peer(&joined).await?;

    let mut sender = SendLink::new(&room.link()).await?;
    let offer = sender.create_offer().await?;
    signaling
        .send(json!({ "type": "signal", "to": peer, "kind": "offer", "payload": { "sdp": offer } }))
        .await?;
    let (_, answer) = signaling.wait_signal("answer").await?;
    sender.accept_answer(&answer).await?;
    sender.wait_connected().await?;
    println!("connected");

    signaling
        .send(json!({
            "type": "stream_state", "stream_id": "synthetic", "kind": "screen", "live": true,
            "meta": { "width": cfg.width, "height": cfg.height, "fps": cfg.fps }
        }))
        .await?;
    run_sender(&sender, cfg, room.seconds, None).await?;
    signaling
        .send(json!({
            "type": "stream_state", "stream_id": "synthetic", "kind": "screen", "live": false
        }))
        .await?;
    sender.close().await?;
    Ok(())
}

async fn recv(room: &RoomArgs) -> Result<()> {
    let mut signaling = Signaling::connect(&room.server).await?;
    let joined = signaling.enter(room.code.as_deref(), &room.nick).await?;
    println!("room {}", joined["code"].as_str().unwrap_or("?"));
    println!("waiting for an offer...");
    let (from, offer) = signaling.wait_signal("offer").await?;

    let mut receiver = RecvLink::new(&room.link()).await?;
    let answer = receiver.accept_offer(&offer).await?;
    signaling
        .send(
            json!({ "type": "signal", "to": from, "kind": "answer", "payload": { "sdp": answer } }),
        )
        .await?;
    receiver.wait_connected().await?;
    println!("connected");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(room.seconds);
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = tick.tick() => println!(
                "recv: {} | {}",
                receiver.stats.report(),
                receiver.stats.take_window()
            ),
            _ = tokio::time::sleep_until(deadline) => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    println!(
        "FINAL recv (wall clock; only meaningful if both clocks are synced): {}",
        receiver.stats.report()
    );
    receiver.close().await?;
    Ok(())
}

async fn run_sender(
    sender: &SendLink,
    cfg: StreamConfig,
    seconds: u64,
    also: Option<&RecvLink>,
) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let streaming = {
        let stop = Arc::clone(&stop);
        async move { sender.stream(cfg, stop).await }
    };
    let timer = {
        let stop = Arc::clone(&stop);
        async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        println!(
                            "send: estimate {:.0} kbps, {}\n      round trip {}",
                            sender.estimate_kbps(),
                            sender.counters,
                            sender.round_trip.take_window()
                        );
                        if let Some(receiver) = also {
                            println!("recv: {}", receiver.stats.report());
                        }
                    }
                    _ = tokio::time::sleep_until(deadline) => break,
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
            stop.store(true, Ordering::Relaxed);
        }
    };
    let (streamed, ()) = tokio::join!(streaming, timer);
    streamed?;
    println!(
        "FINAL send: estimate {:.0} kbps, {}\n      round trip (sender clock only): {}",
        sender.estimate_kbps(),
        sender.counters,
        sender.round_trip.report()
    );
    if sender.round_trip.decoded() == 0 {
        bail!("no frame was acknowledged by the receiver");
    }
    Ok(())
}
