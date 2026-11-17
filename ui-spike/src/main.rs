//! M4a spike: N live video tiles in an egui window, with the overlays of the
//! "sala ao vivo" mockup, to measure what the UI path costs (docs/UI-BIBLIOTECA.md).

mod video;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    Vec2,
};
use eframe::egui_wgpu::{self, wgpu};

use video::{Frame, VideoCallback, VideoGpu};

const INDIGO: Color32 = Color32::from_rgb(88, 101, 242);
const GREEN: Color32 = Color32::from_rgb(34, 197, 94);
const RED: Color32 = Color32::from_rgb(220, 38, 38);
const PANEL: Color32 = Color32::from_rgb(24, 25, 32);
const CARD: Color32 = Color32::from_rgb(32, 33, 42);
const BACKGROUND: Color32 = Color32::from_rgb(17, 18, 24);

#[derive(Parser, Clone)]
#[command(about = "M4a: video tiles in egui")]
struct Args {
    /// How many video tiles.
    #[arg(long, default_value_t = 3)]
    tiles: usize,
    #[arg(long, default_value_t = 1920)]
    width: u32,
    #[arg(long, default_value_t = 1080)]
    height: u32,
    #[arg(long, default_value_t = 60)]
    fps: u32,
    /// How long to run before closing on its own.
    #[arg(long, default_value_t = 20)]
    seconds: u64,
    /// Show one still picture per tile and upload nothing after the first
    /// frame, to isolate the cost of the UI from the cost of the video.
    #[arg(long)]
    static_frames: bool,
    /// Do not wait for the display's refresh: measures the most the path can do.
    #[arg(long)]
    no_vsync: bool,
    /// Leave out the webcam picture-in-picture.
    #[arg(long)]
    no_webcam: bool,
}

// ----- synthetic video -------------------------------------------------------------

/// A wide base picture whose rows are copied at a moving offset, so producing
/// a frame costs a copy and the picture visibly scrolls.
struct Base {
    width: u32,
    height: u32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

const SCROLL: usize = 256;

fn make_base(width: u32, height: u32, tint: u8) -> Base {
    let (w, h) = (width as usize, height as usize);
    let (yw, cw, ch) = (w + SCROLL, w / 2 + SCROLL / 2, h / 2);
    let mut y = vec![0u8; yw * h];
    for row in 0..h {
        for col in 0..yw {
            let bars = if (col / 64) % 2 == 0 { 40 } else { 0 };
            let grad = (row * 120 / h) as u8;
            y[row * yw + col] = 40 + bars + grad + ((col % SCROLL) / 4) as u8;
        }
    }
    let mut u = vec![128u8; cw * ch];
    let mut v = vec![128u8; cw * ch];
    for row in 0..ch {
        for col in 0..cw {
            u[row * cw + col] = 100u8
                .wrapping_add(tint.wrapping_mul(30))
                .wrapping_add((col % 64) as u8);
            v[row * cw + col] = 150u8
                .wrapping_sub(tint.wrapping_mul(20))
                .wrapping_add((row * 90 / ch) as u8);
        }
    }
    Base {
        width,
        height,
        y,
        u,
        v,
    }
}

fn make_frame(base: &Base, seq: u64) -> Frame {
    let (w, h) = (base.width as usize, base.height as usize);
    let shift = (seq as usize * 4) % SCROLL;
    let (yw, cw) = (w + SCROLL, w / 2 + SCROLL / 2);
    let mut y = Vec::with_capacity(w * h);
    for row in 0..h {
        y.extend_from_slice(&base.y[row * yw + shift..row * yw + shift + w]);
    }
    let mut u = Vec::with_capacity(w * h / 4);
    let mut v = Vec::with_capacity(w * h / 4);
    for row in 0..h / 2 {
        let start = row * cw + shift / 2;
        u.extend_from_slice(&base.u[start..start + w / 2]);
        v.extend_from_slice(&base.v[start..start + w / 2]);
    }
    Frame {
        seq,
        width: base.width,
        height: base.height,
        y,
        u,
        v,
    }
}

type Latest = Arc<Mutex<Arc<Frame>>>;

fn spawn_producer(base: Base, fps: u32, latest: Latest, stop: Arc<AtomicBool>, live: bool) {
    std::thread::spawn(move || {
        if !live {
            return;
        }
        let period = Duration::from_secs_f64(1.0 / f64::from(fps));
        let mut next = Instant::now();
        let mut seq = 1u64;
        while !stop.load(Ordering::Relaxed) {
            *latest.lock().unwrap() = Arc::new(make_frame(&base, seq));
            seq += 1;
            next += period;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > period * 5 {
                next = now;
            }
        }
    });
}

// ----- metrics -------------------------------------------------------------------------

fn cpu_seconds() -> f64 {
    if let Some(seconds) = telinhas_capture::process_cpu_seconds() {
        return seconds;
    }
    // Linux: utime and stime, in clock ticks (100 per second), after the
    // command name in parentheses.
    std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|text| {
            let after = text.rsplit_once(") ")?.1.to_owned();
            let fields: Vec<&str> = after.split_whitespace().collect();
            let utime: f64 = fields.get(11)?.parse().ok()?;
            let stime: f64 = fields.get(12)?.parse().ok()?;
            Some((utime + stime) / 100.0)
        })
        .unwrap_or(0.0)
}

fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

#[derive(Default)]
struct Metrics {
    frames: u64,
    frame_ms: Vec<f64>,
    all_frame_ms: Vec<f64>,
    last_report: Option<Instant>,
    last_cpu: f64,
    status: String,
}

// ----- the app -----------------------------------------------------------------------------

struct Tile {
    name: &'static str,
    game: &'static str,
    volume: f32,
    latest: Latest,
}

struct App {
    args: Args,
    tiles: Vec<Tile>,
    webcam: Option<Latest>,
    stop: Arc<AtomicBool>,
    started: Instant,
    metrics: Metrics,
    cpu_start: f64,
    finished: bool,
}

const NAMES: [(&str, &str); 6] = [
    ("Lucas", "Cyberpunk 2077"),
    ("Mariana", "VS Code"),
    ("Pedro", "Figma"),
    ("Ana", "Blender"),
    ("Caio", "Terminal"),
    ("Bia", "Navegador"),
];

impl App {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        let render_state = cc.wgpu_render_state.as_ref().expect("the wgpu renderer");
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(VideoGpu::new(
                &render_state.device,
                render_state.target_format,
            ));

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BACKGROUND;
        visuals.window_fill = PANEL;
        visuals.widgets.noninteractive.bg_fill = PANEL;
        visuals.selection.bg_fill = INDIGO;
        visuals.window_corner_radius = CornerRadius::same(16);
        cc.egui_ctx.set_visuals(visuals);

        let stop = Arc::new(AtomicBool::new(false));
        let live = !args.static_frames;
        let mut tiles = Vec::new();
        for index in 0..args.tiles {
            let base = make_base(args.width, args.height, index as u8);
            let first = Arc::new(make_frame(&base, 0));
            let latest: Latest = Arc::new(Mutex::new(first));
            spawn_producer(base, args.fps, Arc::clone(&latest), Arc::clone(&stop), live);
            let (name, game) = NAMES[index % NAMES.len()];
            tiles.push(Tile {
                name,
                game,
                volume: 0.7,
                latest,
            });
        }
        let webcam = (!args.no_webcam).then(|| {
            let base = make_base(640, 360, 3);
            let latest: Latest = Arc::new(Mutex::new(Arc::new(make_frame(&base, 0))));
            spawn_producer(base, 30, Arc::clone(&latest), Arc::clone(&stop), live);
            latest
        });
        Self {
            args,
            tiles,
            webcam,
            stop,
            started: Instant::now(),
            metrics: Metrics::default(),
            cpu_start: cpu_seconds(),
            finished: false,
        }
    }

    fn measure(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let dt = ctx.input(|i| i.unstable_dt) as f64 * 1000.0;
        self.metrics.frames += 1;
        if self.metrics.frames > 5 {
            self.metrics.frame_ms.push(dt);
            self.metrics.all_frame_ms.push(dt);
        }
        let now = Instant::now();
        let last = *self.metrics.last_report.get_or_insert(now);
        if now - last >= Duration::from_secs(2) {
            let wall = (now - last).as_secs_f64();
            let cpu = cpu_seconds();
            let uploaded = frame
                .wgpu_render_state()
                .and_then(|rs| {
                    rs.renderer
                        .write()
                        .callback_resources
                        .get_mut::<VideoGpu>()
                        .map(VideoGpu::take_uploaded)
                })
                .unwrap_or(0);
            let fps = self.metrics.frame_ms.len() as f64 / wall;
            let status = format!(
                "ui {fps:.1} fps | frame p50={:.1} p95={:.1} max={:.1} ms | cpu {:.0}% of a core | upload {:.0} MB/s",
                percentile(&self.metrics.frame_ms, 0.5),
                percentile(&self.metrics.frame_ms, 0.95),
                percentile(&self.metrics.frame_ms, 1.0),
                (cpu - self.metrics.last_cpu) / wall * 100.0,
                uploaded as f64 / wall / 1e6,
            );
            println!("{status}");
            self.metrics.status = status;
            self.metrics.frame_ms.clear();
            self.metrics.last_cpu = cpu;
            self.metrics.last_report = Some(now);
        }
    }

    fn finish(&self) {
        let wall = self.started.elapsed().as_secs_f64();
        let cpu = cpu_seconds() - self.cpu_start;
        let frames = &self.metrics.all_frame_ms;
        println!(
            "FINAL tiles={} {}x{} @{} fps source, {}{}: ui {:.1} fps over {wall:.0}s | frame p50={:.1} p95={:.1} p99={:.1} max={:.1} ms | cpu {:.0}% of a core",
            self.args.tiles,
            self.args.width,
            self.args.height,
            self.args.fps,
            if self.args.static_frames {
                "static"
            } else {
                "live"
            },
            if self.args.no_vsync { ", no vsync" } else { "" },
            frames.len() as f64 / wall,
            percentile(frames, 0.5),
            percentile(frames, 0.95),
            percentile(frames, 0.99),
            percentile(frames, 1.0),
            cpu / wall * 100.0,
        );
    }
}

fn pill(painter: &egui::Painter, anchor: Pos2, text: &str, fill: Color32, color: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color);
    let rect = Rect::from_min_size(anchor, galley.size() + Vec2::new(16.0, 8.0));
    painter.rect_filled(rect, CornerRadius::same(255), fill);
    painter.galley(rect.min + Vec2::new(8.0, 4.0), galley, color);
    rect
}

impl App {
    fn tile_ui(&self, ui: &egui::Ui, cell: Rect, index: usize) {
        let painter = ui.painter();
        let tile = &self.tiles[index];
        let live = index == 0;
        painter.rect_filled(cell, CornerRadius::same(16), CARD);
        let inner = cell.shrink(3.0);
        let frame = tile.latest.lock().unwrap().clone();
        painter.add(video::shape(VideoCallback {
            id: index,
            frame,
            rect: inner,
            radius: 13.0,
        }));
        if live {
            painter.rect_stroke(
                cell,
                CornerRadius::same(16),
                Stroke::new(2.0, GREEN),
                StrokeKind::Inside,
            );
        }
        pill(
            painter,
            inner.min + Vec2::new(12.0, 12.0),
            &format!("● AO VIVO  {}p{}", self.args.height, self.args.fps),
            Color32::from_black_alpha(150),
            Color32::WHITE,
        );
        // Name plate, bottom left, with an avatar.
        let plate = Rect::from_min_size(
            Pos2::new(inner.min.x + 12.0, inner.max.y - 52.0),
            Vec2::new(210.0, 40.0),
        );
        painter.rect_filled(
            plate,
            CornerRadius::same(20),
            Color32::from_black_alpha(170),
        );
        painter.circle_filled(plate.left_center() + Vec2::new(20.0, 0.0), 14.0, INDIGO);
        painter.text(
            plate.left_center() + Vec2::new(20.0, 0.0),
            Align2::CENTER_CENTER,
            &tile.name[..1],
            FontId::proportional(14.0),
            Color32::WHITE,
        );
        painter.text(
            plate.left_center() + Vec2::new(42.0, -8.0),
            Align2::LEFT_CENTER,
            tile.name,
            FontId::proportional(14.0),
            Color32::WHITE,
        );
        painter.text(
            plate.left_center() + Vec2::new(42.0, 9.0),
            Align2::LEFT_CENTER,
            tile.game,
            FontId::proportional(11.0),
            Color32::from_gray(190),
        );
        // Webcam picture-in-picture on the first tile.
        if let (true, Some(webcam)) = (live, &self.webcam) {
            let size = Vec2::new(inner.width() * 0.26, inner.width() * 0.26 * 9.0 / 16.0);
            let rect = Rect::from_min_size(inner.max - size - Vec2::new(12.0, 12.0), size);
            painter.add(video::shape(VideoCallback {
                id: self.tiles.len(),
                frame: webcam.lock().unwrap().clone(),
                rect,
                radius: 10.0,
            }));
            painter.rect_stroke(
                rect,
                CornerRadius::same(10),
                Stroke::new(1.5, Color32::from_white_alpha(90)),
                StrokeKind::Outside,
            );
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.measure(&ctx, frame);
        if self.started.elapsed() >= Duration::from_secs(self.args.seconds) && !self.finished {
            self.finished = true;
            self.stop.store(true, Ordering::Relaxed);
            self.finish();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("● SALA EFÊMERA  NEON-7842")
                        .color(GREEN)
                        .strong(),
                );
                ui.label(RichText::new("14 ms").color(Color32::from_gray(200)));
                ui.separator();
                ui.label(RichText::new(&self.metrics.status).monospace().size(11.0));
            });
            ui.add_space(6.0);
        });

        egui::Panel::right("side")
            .default_size(280.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("TRANSMISSÕES").strong());
                for tile in &mut self.tiles {
                    egui::Frame::new()
                        .fill(CARD)
                        .corner_radius(CornerRadius::same(14))
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(tile.name).strong());
                            ui.label(
                                RichText::new(tile.game)
                                    .small()
                                    .color(Color32::from_gray(170)),
                            );
                            ui.add(
                                egui::Slider::new(&mut tile.volume, 0.0..=1.0).show_value(false),
                            );
                        });
                    ui.add_space(4.0);
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            let area = ui.available_rect_before_wrap();
            let n = self.tiles.len().max(1);
            let cols = (n as f32).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            let gap = 12.0;
            let cell_w = (area.width() - gap * (cols as f32 - 1.0)) / cols as f32;
            let cell_h = (area.height() - gap * (rows as f32 - 1.0)) / rows as f32;
            for index in 0..self.tiles.len() {
                let (col, row) = (index % cols, index / cols);
                let cell = Rect::from_min_size(
                    area.min + Vec2::new(col as f32 * (cell_w + gap), row as f32 * (cell_h + gap)),
                    Vec2::new(cell_w, cell_h),
                );
                ui.allocate_rect(cell, Sense::hover());
                self.tile_ui(ui, cell, index);
            }
        });

        #[allow(deprecated)]
        let bar_id = egui::Id::new("bar");
        egui::Area::new(bar_id)
            .anchor(Align2::CENTER_BOTTOM, [0.0, -18.0])
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(20, 21, 28, 235))
                    .corner_radius(CornerRadius::same(28))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Button::new(RichText::new("🖥 Transmitir").strong())
                                    .fill(INDIGO)
                                    .corner_radius(CornerRadius::same(20)),
                            );
                            ui.add(egui::Button::new("📷").corner_radius(CornerRadius::same(20)));
                            ui.add(egui::Button::new("🎤").corner_radius(CornerRadius::same(20)));
                            ui.add(
                                egui::Button::new("1080p60 ⏶")
                                    .corner_radius(CornerRadius::same(20)),
                            );
                            ui.add(egui::Button::new("⚙").corner_radius(CornerRadius::same(20)));
                            ui.add(
                                egui::Button::new(RichText::new("Sair").strong())
                                    .fill(RED)
                                    .corner_radius(CornerRadius::same(20)),
                            );
                        });
                    });
            });
    }
}

fn main() -> eframe::Result {
    let args = Args::parse();
    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    if args.no_vsync {
        wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 820.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };
    eframe::run_native(
        "Telinhas M4a",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, args)))),
    )
}
