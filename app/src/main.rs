use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Telinhas")
            .with_inner_size([1360.0, 820.0])
            .with_min_inner_size([980.0, 640.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Telinhas",
        options,
        Box::new(|cc| Ok(Box::new(telinhas::TelinhasApp::new(cc)))),
    )
}
