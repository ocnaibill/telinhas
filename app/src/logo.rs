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

/// The logo rendered as a square window icon.
pub fn window_icon() -> Option<egui::IconData> {
    use resvg::{tiny_skia, usvg};
    const SIZE: u32 = 256;
    let tree = usvg::Tree::from_data(&bytes(), &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(SIZE, SIZE)?;
    let size = tree.size();
    let scale = (SIZE as f32 * 0.92) / size.width().max(size.height());
    let dx = (SIZE as f32 - size.width() * scale) / 2.0;
    let dy = (SIZE as f32 - size.height() * scale) / 2.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
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
    fn the_window_icon_has_light_and_transparent_pixels() {
        let icon = window_icon().expect("the logo renders");
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 0),
            "has a transparent margin"
        );
        assert!(
            icon.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] == 255 && p[0] > 200),
            "has a light body"
        );
    }
}
