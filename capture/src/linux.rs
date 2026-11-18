//! Linux capture: the desktop portal picks the screen or window and PipeWire
//! delivers its frames (DEC-29, DEC-45).
//!
//! The portal is the only way a Wayland client may see other windows. It shows
//! its own dialog, so the app never lists screens or windows here; it asks, and
//! gets one stream back. Frames arrive in shared memory (the formats offered
//! carry no DMA-BUF modifiers, so compositors fall back to that) and are copied
//! out as BGRA, like on Windows.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;

use crate::{
    CaptureSupport, Error, FrameRef, Gpu, Monitor, Options, Pacer, Result, Source, Window,
};

/// How long the person has to answer the portal's dialog.
const DIALOG_TIMEOUT: Duration = Duration::from_secs(180);
/// How long to wait for the first frame once the stream is chosen.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

pub fn init_dpi_awareness() {}

pub fn list_monitors() -> Result<Vec<Monitor>> {
    Err(Error::Unsupported)
}

pub fn list_windows() -> Result<Vec<Window>> {
    Err(Error::Unsupported)
}

pub fn list_gpus() -> Result<Vec<Gpu>> {
    Err(Error::Unsupported)
}

/// Whether the desktop offers the screen cast portal at all.
pub fn capture_support() -> Result<CaptureSupport> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(os)?;
    runtime.block_on(async {
        let portal = tokio::time::timeout(Duration::from_secs(5), Screencast::new())
            .await
            .map_err(|_| Error::Os("o portal de captura não respondeu".to_owned()))?
            .map_err(os)?;
        let cursors = portal.available_cursor_modes().await.unwrap_or_default();
        Ok(CaptureSupport {
            graphics_capture: true,
            border_toggle: false,
            cursor_toggle: cursors.contains(CursorMode::EMBEDDED),
        })
    })
}

fn os(error: impl std::fmt::Display) -> Error {
    Error::Os(error.to_string())
}

pub struct Session {
    size: (u32, u32),
    cursor: bool,
    paced_out: Arc<AtomicU64>,
    stop: Option<pw::channel::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn border_status(&self) -> String {
        "the portal decides how a capture is shown".to_owned()
    }

    pub fn frames_paced_out(&self) -> u64 {
        self.paced_out.load(Ordering::Relaxed)
    }

    pub fn max_fps_status(&self) -> String {
        "frames are picked at an even rhythm by the pacer".to_owned()
    }

    pub fn cursor_applied(&self) -> bool {
        self.cursor
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

type FrameSink = Box<dyn FnMut(FrameRef<'_>) + Send>;

/// What the PipeWire callbacks share.
struct State {
    format: spa::param::video::VideoInfoRaw,
    on_frame: FrameSink,
    pacer: Option<Pacer>,
    paced_out: Arc<AtomicU64>,
    started: Instant,
    /// Set once the first format is known, to tell `start` the size.
    ready: Option<Sender<Result<(u32, u32)>>>,
    /// RGBx and RGBA frames are turned into BGRA here.
    scratch: Vec<u8>,
}

pub fn start(
    source: Source,
    options: Options,
    on_frame: Box<dyn FnMut(FrameRef<'_>) + Send>,
) -> Result<Session> {
    if source != Source::Portal {
        return Err(Error::Unsupported);
    }
    let (ready_tx, ready_rx): (_, Receiver<Result<(u32, u32)>>) = channel();
    let (stop_tx, stop_rx) = pw::channel::channel::<()>();
    let paced_out = Arc::new(AtomicU64::new(0));
    let state_paced = Arc::clone(&paced_out);
    let stop_slot = Mutex::new(Some(stop_rx));

    let thread = std::thread::Builder::new()
        .name("telinhas-capture-portal".to_owned())
        .spawn(move || {
            let Some(stop_rx) = stop_slot.lock().unwrap().take() else {
                return;
            };
            if let Err(error) = run(options, on_frame, state_paced, ready_tx.clone(), stop_rx) {
                let _ = ready_tx.send(Err(error));
            }
        })
        .map_err(os)?;

    // The person may take a while to choose, then a frame must arrive.
    let size = match ready_rx.recv_timeout(DIALOG_TIMEOUT + FIRST_FRAME_TIMEOUT) {
        Ok(Ok(size)) => size,
        Ok(Err(error)) => {
            let _ = thread.join();
            return Err(error);
        }
        Err(_) => {
            let _ = stop_tx.send(());
            let _ = thread.join();
            return Err(Error::Os("a captura não começou a tempo".to_owned()));
        }
    };
    Ok(Session {
        size,
        cursor: options.cursor,
        paced_out,
        stop: Some(stop_tx),
        thread: Some(thread),
    })
}

/// Opens the portal session, shows its dialog and returns the PipeWire stream
/// that was chosen. The session must stay alive as long as the capture does,
/// so it is returned with the runtime that drives its connection.
async fn open_portal(cursor: bool) -> Result<(u32, OwnedFd, ashpd::desktop::Session<Screencast>)> {
    let portal = Screencast::new().await.map_err(os)?;
    let session = portal
        .create_session(Default::default())
        .await
        .map_err(os)?;
    portal
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(if cursor {
                    CursorMode::EMBEDDED
                } else {
                    CursorMode::HIDDEN
                })
                .set_sources(SourceType::MONITOR | SourceType::WINDOW)
                .set_multiple(false)
                .set_restore_token(None)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .map_err(os)?;
    let response = tokio::time::timeout(DIALOG_TIMEOUT, async {
        portal
            .start(&session, None, Default::default())
            .await?
            .response()
    })
    .await
    .map_err(|_| Error::Os("a escolha da tela demorou demais".to_owned()))?
    .map_err(|e| match e {
        ashpd::Error::Response(_) => Error::Os("a escolha da tela foi cancelada".to_owned()),
        other => os(other),
    })?;
    let stream = response
        .streams()
        .first()
        .ok_or_else(|| Error::Os("nenhuma tela ou janela foi escolhida".to_owned()))?;
    let node = stream.pipe_wire_node_id();
    let fd = portal
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(os)?;
    Ok((node, fd, session))
}

/// The thread of a capture: portal, then PipeWire's loop until told to stop.
fn run(
    options: Options,
    on_frame: FrameSink,
    paced_out: Arc<AtomicU64>,
    ready: Sender<Result<(u32, u32)>>,
    stop: pw::channel::Receiver<()>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(os)?;
    // `_session` is kept: dropping it would close the portal session and end
    // the stream.
    let (node, fd, _session) = runtime.block_on(open_portal(options.cursor))?;

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(os)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(os)?;
    let core = context.connect_fd_rc(fd, None).map_err(os)?;

    let stop_loop = mainloop.clone();
    let _stop = stop.attach(mainloop.loop_(), move |()| stop_loop.quit());

    let stream = pw::stream::StreamBox::new(
        &core,
        "telinhas-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(os)?;

    let state = State {
        format: Default::default(),
        on_frame,
        pacer: options.max_fps.map(Pacer::new),
        paced_out,
        started: Instant::now(),
        ready: Some(ready),
        scratch: Vec::new(),
    };
    let error_loop = mainloop.clone();
    let _listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(move |_, state, _, new| {
            if let pw::stream::StreamState::Error(reason) = new {
                if let Some(ready) = state.ready.take() {
                    let _ = ready.send(Err(Error::Os(format!("PipeWire: {reason}"))));
                }
                error_loop.quit();
            }
        })
        .param_changed(|_, state, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != spa::param::format::MediaType::Video
                || media_subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            if state.format.parse(param).is_err() {
                return;
            }
            let size = state.format.size();
            // Sizes must be even for the encoder.
            if let Some(ready) = state.ready.take() {
                let _ = ready.send(Ok((size.width & !1, size.height & !1)));
            }
        })
        .process(|stream, state| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let chunk_size = data.chunk().size() as usize;
            let chunk_stride = data.chunk().stride();
            let chunk_offset = data.chunk().offset() as usize;
            let Some(bytes) = data.data() else { return };
            if chunk_size == 0 {
                return;
            }

            let size = state.format.size();
            let (width, height) = (size.width as usize, size.height as usize);
            if width == 0 || height == 0 {
                return;
            }
            let micros = state.started.elapsed().as_micros() as i64;
            if let Some(pacer) = state.pacer.as_mut()
                && !pacer.accept(micros)
            {
                state.paced_out.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let stride = if chunk_stride > 0 {
                chunk_stride as usize
            } else {
                width * 4
            };
            let end = (chunk_offset + stride * height).min(bytes.len());
            if chunk_offset >= end || stride < width * 4 {
                return;
            }
            let pixels = &bytes[chunk_offset..end];
            let format = state.format.format();
            let (width, height) = (width & !1, height & !1);

            let started = Instant::now();
            if matches!(
                format,
                spa::param::video::VideoFormat::BGRx | spa::param::video::VideoFormat::BGRA
            ) {
                let copy_micros = started.elapsed().as_micros() as u32;
                (state.on_frame)(FrameRef {
                    width: width as u32,
                    height: height as u32,
                    stride,
                    bgra: pixels,
                    system_micros: micros,
                    copy_micros,
                });
            } else if matches!(
                format,
                spa::param::video::VideoFormat::RGBx | spa::param::video::VideoFormat::RGBA
            ) {
                swap_red_blue(pixels, stride, width, height, &mut state.scratch);
                let copy_micros = started.elapsed().as_micros() as u32;
                (state.on_frame)(FrameRef {
                    width: width as u32,
                    height: height as u32,
                    stride: width * 4,
                    bgra: &state.scratch,
                    system_micros: micros,
                    copy_micros,
                });
            }
        })
        .register()
        .map_err(os)?;

    let fps = options.max_fps.unwrap_or(60).max(1) as i32;
    let video = |formats: &[spa::param::video::VideoFormat]| -> Result<Vec<u8>> {
        let mut choices = vec![formats[0]];
        choices.extend_from_slice(formats);
        let object = spa::pod::object!(
            spa::utils::SpaTypes::ObjectParamFormat,
            spa::param::ParamType::EnumFormat,
            spa::pod::property!(
                spa::param::format::FormatProperties::MediaType,
                Id,
                spa::param::format::MediaType::Video
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::MediaSubtype,
                Id,
                spa::param::format::MediaSubtype::Raw
            ),
            spa::pod::Property::new(
                spa::param::format::FormatProperties::VideoFormat.as_raw(),
                spa::pod::Value::Choice(spa::pod::ChoiceValue::Id(spa::utils::Choice(
                    spa::utils::ChoiceFlags::empty(),
                    spa::utils::ChoiceEnum::Enum {
                        default: spa::utils::Id(choices[0].as_raw()),
                        alternatives: choices.iter().map(|f| spa::utils::Id(f.as_raw())).collect(),
                    },
                ))),
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoSize,
                Choice,
                Range,
                Rectangle,
                spa::utils::Rectangle {
                    width: 1920,
                    height: 1080
                },
                spa::utils::Rectangle {
                    width: 1,
                    height: 1
                },
                spa::utils::Rectangle {
                    width: 8192,
                    height: 8192
                }
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoFramerate,
                Choice,
                Range,
                Fraction,
                spa::utils::Fraction {
                    num: fps as u32,
                    denom: 1
                },
                spa::utils::Fraction { num: 0, denom: 1 },
                spa::utils::Fraction {
                    num: 1000,
                    denom: 1
                }
            ),
        );
        spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(object),
        )
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(os)
    };
    // The formats that are plain 32-bit pixels, the ones the copy handles.
    let bytes = video(&[
        spa::param::video::VideoFormat::BGRx,
        spa::param::video::VideoFormat::BGRA,
        spa::param::video::VideoFormat::RGBx,
        spa::param::video::VideoFormat::RGBA,
    ])?;
    let mut params = [spa::pod::Pod::from_bytes(&bytes)
        .ok_or_else(|| Error::Os("could not build the format request".to_owned()))?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(os)?;

    mainloop.run();
    Ok(())
}

/// Turns RGBx or RGBA rows into tightly packed BGRA: red and blue trade places.
fn swap_red_blue(pixels: &[u8], stride: usize, width: usize, height: usize, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(width * 4 * height);
    for row in pixels.chunks(stride).take(height) {
        for px in row[..width * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
}

/// CPU time used so far by this process, in seconds (user and kernel), from
/// `/proc`. The clock ticks per second are 100 on every Linux in use.
pub fn process_cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command name is in parentheses and may hold spaces: count from its end.
    let after = &stat[stat.rfind(')')? + 2..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    let user: f64 = fields.get(11)?.parse().ok()?;
    let kernel: f64 = fields.get(12)?.parse().ok()?;
    Some((user + kernel) / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_and_blue_trade_places_and_padding_is_dropped() {
        // Two pixels per row, rows padded to 12 bytes.
        let rgba = [
            1, 2, 3, 255, 4, 5, 6, 255, 9, 9, 9, 9, //
            7, 8, 9, 255, 10, 11, 12, 255, 9, 9, 9, 9,
        ];
        let mut out = Vec::new();
        swap_red_blue(&rgba, 12, 2, 2, &mut out);
        assert_eq!(
            out,
            [3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
    }

    #[test]
    fn this_process_has_used_some_cpu() {
        let seconds = process_cpu_seconds().expect("/proc/self/stat is readable");
        assert!(seconds >= 0.0);
    }

    #[test]
    fn only_the_portal_is_a_source_here() {
        let result = start(Source::Monitor(0), Options::default(), Box::new(|_| {}));
        assert!(matches!(result, Err(Error::Unsupported)));
    }
}
