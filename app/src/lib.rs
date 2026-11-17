//! Telinhas desktop client.

mod app;
mod logo;
mod mockvideo;
mod model;
mod screens;
mod theme;
mod video;

pub use app::TelinhasApp;

/// The window icon, drawn from the logo.
pub fn window_icon() -> Option<eframe::egui::IconData> {
    logo::window_icon()
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
