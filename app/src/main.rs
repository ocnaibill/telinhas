use eframe::egui;

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Telinhas")
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
