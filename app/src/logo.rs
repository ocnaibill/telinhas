//! The Telinhas logo (`assets/logo-telinhas.svg`), recolored for the dark
//! theme: the original is black with a white screen, which would vanish on
//! the dark background.

use std::sync::{Arc, OnceLock};

use eframe::egui;

const SVG: &str = include_str!("../assets/logo-telinhas.svg");

/// Width over height of the logo's drawing.
pub const ASPECT: f32 = 1956.77 / 1443.06;

/// Light body, dark screen.
fn recolored() -> String {
    SVG.replacen("<svg ", "<svg fill=\"#eceef5\" ", 1)
        .replace("fill: #fff;", "fill: #0f1016;")
}

fn bytes() -> Arc<[u8]> {
    static BYTES: OnceLock<Arc<[u8]>> = OnceLock::new();
    Arc::clone(BYTES.get_or_init(|| Arc::from(recolored().into_bytes())))
}

/// The logo as an egui image of the given height.
pub fn image(height: f32) -> egui::Image<'static> {
    egui::Image::from_bytes("bytes://telinhas-logo.svg", bytes())
        .fit_to_exact_size(egui::vec2(height * ASPECT, height))
}

/// Dark square with rounded corners, behind the logo in the app icon: a bare
/// light logo would vanish on a light Explorer or taskbar.
const ICON_BACKGROUND: [u8; 3] = [0x16, 0x17, 0x1f];

fn rounded_square(size: f32, radius: f32) -> Option<resvg::tiny_skia::Path> {
    use resvg::tiny_skia::PathBuilder;
    let (s, r) = (size, radius);
    let mut pb = PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(s - r, 0.0);
    pb.quad_to(s, 0.0, s, r);
    pb.line_to(s, s - r);
    pb.quad_to(s, s, s - r, s);
    pb.line_to(r, s);
    pb.quad_to(0.0, s, 0.0, s - r);
    pb.line_to(0.0, r);
    pb.quad_to(0.0, 0.0, r, 0.0);
    pb.close();
    pb.finish()
}

/// The app icon at `size` pixels: the logo on a dark rounded square.
fn render_icon(size: u32) -> Option<resvg::tiny_skia::Pixmap> {
    use resvg::{tiny_skia, usvg};
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let side = size as f32;

    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(
        ICON_BACKGROUND[0],
        ICON_BACKGROUND[1],
        ICON_BACKGROUND[2],
        255,
    );
    paint.anti_alias = true;
    let square = rounded_square(side, side * 0.22)?;
    pixmap.fill_path(
        &square,
        &paint,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );

    let tree = usvg::Tree::from_data(&bytes(), &usvg::Options::default()).ok()?;
    let drawing = tree.size();
    let scale = (side * 0.70) / drawing.width().max(drawing.height());
    let dx = (side - drawing.width() * scale) / 2.0;
    let dy = (side - drawing.height() * scale) / 2.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(pixmap)
}

/// The icon as PNG data, for the sizes inside the `.ico` file.
pub fn icon_png(size: u32) -> Option<Vec<u8>> {
    render_icon(size)?.encode_png().ok()
}

/// The icon as the window's icon.
pub fn window_icon() -> Option<egui::IconData> {
    const SIZE: u32 = 256;
    let pixmap = render_icon(SIZE)?;
    // The pixmap holds premultiplied colors; icons want straight alpha.
    let mut rgba = pixmap.take();
    for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let alpha = u32::from(px[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut px[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    Some(egui::IconData {
        rgba,
        width: SIZE,
        height: SIZE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_is_recolored_for_the_dark_theme() {
        let svg = recolored();
        assert!(svg.contains("fill=\"#eceef5\""));
        assert!(svg.contains("fill: #0f1016;"));
        assert!(!svg.contains("fill: #fff;"));
    }

    #[test]
    fn the_icon_is_a_dark_rounded_square_with_a_light_logo() {
        let icon = window_icon().expect("the logo renders");
        assert_eq!((icon.width, icon.height), (256, 256));
        let at = |x: usize, y: usize| &icon.rgba[(y * 256 + x) * 4..][..4];
        assert_eq!(at(0, 0)[3], 0, "the corner is transparent");
        assert_eq!(at(128, 6)[3], 255, "the edge of the square is solid");
        assert!(at(128, 6)[0] < 60, "the square is dark");
        assert!(
            icon.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] == 255 && p[0] > 200),
            "the logo is light"
        );
    }

    #[test]
    fn every_icon_size_encodes_as_png() {
        for size in [16, 32, 48, 256] {
            let png = icon_png(size).expect("renders");
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        }
    }
}
