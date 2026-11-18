//! Telinhas desktop client.

mod app;
mod logo;
mod mockvideo;
mod model;
mod net_events;
mod playback;

/// Plays a three-second tone through the app's sound output and says where it
/// played, or why it could not. For `telinhas --audio-test`.
pub fn audio_test() -> Result<String, String> {
    playback::Playback::self_test()
}
mod screens;
mod theme;
mod updater;
mod video;

pub use app::TelinhasApp;

/// The window icon, drawn from the logo.
pub fn window_icon() -> Option<eframe::egui::IconData> {
    logo::window_icon()
}

/// The app icon as PNG at `size` pixels, to build the `.ico` file.
pub fn icon_png(size: u32) -> Option<Vec<u8>> {
    logo::icon_png(size)
}

/// Human-readable name and version, used by the CLI and the window title.
pub fn banner() -> String {
    format!("Telinhas {}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_contains_name_and_version() {
        let banner = banner();
        assert!(banner.starts_with("Telinhas "));
        assert!(banner.ends_with(env!("CARGO_PKG_VERSION")));
    }
}
