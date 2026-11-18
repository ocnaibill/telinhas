// A release build on Windows opens no console window next to the app. Debug
// builds, and release builds made with `--features console`, keep it for logs.
#![cfg_attr(
    all(windows, not(debug_assertions), not(feature = "console")),
    windows_subsystem = "windows"
)]

use eframe::egui;

fn main() -> eframe::Result {
    // `telinhas --audio-test`: a tone through the app's own sound output, to
    // tell a missing sound setup from a bug.
    if std::env::args().any(|a| a == "--audio-test") {
        match telinhas::audio_test() {
            Ok(place) => println!("tocou um tom de 2 s em: {place}"),
            Err(reason) => println!("sem som: {reason}"),
        }
        return Ok(());
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Telinhas")
        // Wayland and the desktop match the window to its launcher and icon by this.
        .with_app_id("telinhas")
        .with_inner_size([1360.0, 820.0])
        .with_min_inner_size([980.0, 640.0]);
    if let Some(icon) = telinhas::window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Telinhas",
        options,
        Box::new(|cc| Ok(Box::new(telinhas::TelinhasApp::new(cc)))),
    )
}
