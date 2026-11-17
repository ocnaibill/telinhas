//! What the interface shows. For now it is fed with test data; the signaling,
//! media and capture layers will fill the same types later.

use telinhas_capture as capture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// First launch: ask for a nickname (DEC-14).
    FirstRun,
    Home,
    Room,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Grid,
    Focus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Original,
    Economy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryMode {
    /// Microphone and screen off (DEC-35).
    Watching,
    /// Opens the broadcast setup right away.
    Broadcasting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub name: String,
    pub admin: bool,
    pub latency_ms: u32,
    pub state: ConnectionState,
    pub is_me: bool,
}

#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub owner: String,
    /// What is being shared, such as a program or a screen.
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Encoded by hardware; software may stutter (DEC-15).
    pub hardware: bool,
    pub volume: f32,
    pub visible: bool,
    pub quality: Quality,
    pub has_webcam: bool,
}

pub struct Room {
    pub code: String,
    pub require_approval: bool,
    pub participants: Vec<Participant>,
    pub streams: Vec<StreamInfo>,
    pub layout: Layout,
    /// Index into `streams` of the stream shown large in the focus layout.
    pub focus: usize,
    /// Whether I am broadcasting.
    pub broadcasting: bool,
    pub mic_on: bool,
    pub camera_on: bool,
}

impl Room {
    pub fn me(&self) -> Option<&Participant> {
        self.participants.iter().find(|p| p.is_me)
    }

    pub fn i_am_admin(&self) -> bool {
        self.me().is_some_and(|p| p.admin)
    }

    pub fn live_count(&self) -> usize {
        self.streams.len()
    }

    pub fn average_latency(&self) -> u32 {
        let others: Vec<u32> = self
            .participants
            .iter()
            .filter(|p| !p.is_me && p.state == ConnectionState::Connected)
            .map(|p| p.latency_ms)
            .collect();
        if others.is_empty() {
            0
        } else {
            others.iter().sum::<u32>() / others.len() as u32
        }
    }

    /// A room with a few people and streams, for trying the interface.
    pub fn demo(code: &str, my_name: &str) -> Self {
        let person = |name: &str, admin: bool, latency_ms: u32, is_me: bool| Participant {
            name: name.to_owned(),
            admin,
            latency_ms,
            state: ConnectionState::Connected,
            is_me,
        };
        let stream = |owner: &str, title: &str, width, height, fps, hardware, webcam| StreamInfo {
            owner: owner.to_owned(),
            title: title.to_owned(),
            width,
            height,
            fps,
            hardware,
            volume: 0.8,
            visible: true,
            quality: Quality::Original,
            has_webcam: webcam,
        };
        Self {
            code: code.to_owned(),
            require_approval: false,
            participants: vec![
                person("Lucas", true, 14, false),
                person("Mariana", false, 22, false),
                person("Pedro", false, 31, false),
                person(my_name, false, 0, true),
            ],
            streams: vec![
                stream("Lucas", "Cyberpunk 2077", 1920, 1080, 60, true, true),
                stream("Mariana", "VS Code", 2560, 1440, 30, true, false),
                stream("Pedro", "Figma", 1920, 1080, 30, false, false),
            ],
            layout: Layout::Grid,
            focus: 0,
            broadcasting: false,
            mic_on: false,
            camera_on: false,
        }
    }
}

// ----- codes ----------------------------------------------------------------------

/// Crockford base32 without `I L O U` (DEC-07).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A code like `K7QM-2X9D`, for the home screen's preview. The real code is
/// drawn by the server.
pub fn preview_code(seed: u64) -> String {
    let mut state = seed | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ALPHABET[(state % 32) as usize] as char
    };
    let first: String = (0..4).map(|_| next()).collect();
    let second: String = (0..4).map(|_| next()).collect();
    format!("{first}-{second}")
}

/// Whether `text` could be a room code (8 symbols, hyphen optional).
pub fn looks_like_code(text: &str) -> bool {
    let cleaned: Vec<char> = text
        .chars()
        .filter(|c| *c != '-' && !c.is_whitespace())
        .collect();
    cleaned.len() == 8 && cleaned.iter().all(|c| c.is_ascii_alphanumeric())
}

pub fn valid_nickname(text: &str) -> bool {
    let trimmed = text.trim();
    (1..=24).contains(&trimmed.chars().count()) && !trimmed.chars().any(char::is_control)
}

// ----- bandwidth (RF-06) --------------------------------------------------------------

/// Bitrate ceiling for a picture size and rate, in kbps. Fitted to the NVENC
/// measurements in `docs/PLANO-M3.md` (about 8 Mbps for 1080p60, 3 Mbps for
/// 720p30). Provisional until the presets are chosen.
pub fn target_kbps(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = f64::from(width) * f64::from(height) / (1920.0 * 1080.0);
    let rate = f64::from(fps) / 60.0;
    (8000.0 * pixels.powf(0.75) * rate.powf(0.6)).round() as u32
}

/// What the sender's uplink needs: the ceiling plus 25% for bursts (the
/// measured overshoot), for each viewer.
pub fn upload_kbps(per_viewer_kbps: u32, viewers: u32) -> u32 {
    (f64::from(per_viewer_kbps) * 1.25 * f64::from(viewers)).round() as u32
}

// ----- broadcast setup (DEC-36) -----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTab {
    Screens,
    Windows,
}

#[derive(Debug, Clone)]
pub struct Sources {
    pub monitors: Vec<capture::Monitor>,
    pub windows: Vec<capture::Window>,
    /// Programs that make sound, to leave out of the audio (DEC-16).
    pub audio_apps: Vec<String>,
    /// Set when the lists are test data because the system could not be asked.
    pub is_demo: bool,
}

impl Sources {
    pub fn load() -> Self {
        capture::init_dpi_awareness();
        let monitors = capture::list_monitors();
        let windows = capture::list_windows();
        match (monitors, windows) {
            (Ok(monitors), Ok(windows)) if !monitors.is_empty() => {
                let mut audio_apps: Vec<String> = windows
                    .iter()
                    .filter(|w| !w.process.is_empty())
                    .map(|w| w.process.clone())
                    .collect();
                audio_apps.sort();
                audio_apps.dedup();
                Self {
                    monitors,
                    windows,
                    audio_apps,
                    is_demo: false,
                }
            }
            _ => Self::demo(),
        }
    }

    pub fn demo() -> Self {
        let monitor = |index, device: &str, width, height, refresh_hz, primary| capture::Monitor {
            index,
            device: device.to_owned(),
            x: 0,
            y: 0,
            width,
            height,
            refresh_hz,
            primary,
        };
        let window = |handle, title: &str, process: &str, width, height| capture::Window {
            handle,
            title: title.to_owned(),
            process: process.to_owned(),
            width,
            height,
            minimized: false,
        };
        Self {
            monitors: vec![
                monitor(0, "DISPLAY1", 1920, 1080, 144, true),
                monitor(1, "DISPLAY2", 2560, 1080, 60, false),
            ],
            windows: vec![
                window(1, "Cyberpunk 2077", "Cyberpunk2077.exe", 1920, 1080),
                window(2, "VS Code", "Code.exe", 1600, 900),
                window(3, "Discord", "Discord.exe", 1280, 720),
                window(4, "Navegador", "firefox.exe", 1920, 1032),
            ],
            audio_apps: vec![
                "Discord.exe".into(),
                "Spotify.exe".into(),
                "firefox.exe".into(),
            ],
            is_demo: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BroadcastDraft {
    pub tab: SourceTab,
    pub monitor: usize,
    pub window: usize,
    /// Output height in pixels; the width follows the source's proportions.
    pub height: u32,
    pub fps: u32,
    pub mouse: bool,
    pub system_audio: bool,
    pub mic: bool,
    pub excluded_apps: Vec<String>,
    pub webcam: bool,
}

/// Browsers that capture the mouse by default (DEC-27). The real list will be
/// an editable file.
pub const BROWSERS: [&str; 8] = [
    "chrome", "firefox", "msedge", "safari", "brave", "arc", "opera", "vivaldi",
];

pub fn is_browser(process: &str) -> bool {
    let lower = process.to_lowercase();
    BROWSERS.iter().any(|b| lower.contains(b)) || lower.contains("zen")
}

impl BroadcastDraft {
    pub fn new() -> Self {
        Self {
            tab: SourceTab::Screens,
            monitor: 0,
            window: 0,
            height: 1080,
            fps: 60,
            mouse: true,
            system_audio: true,
            mic: false,
            excluded_apps: Vec::new(),
            webcam: false,
        }
    }

    /// The picture size of the chosen source, before scaling.
    pub fn source_size(&self, sources: &Sources) -> (u32, u32) {
        match self.tab {
            SourceTab::Screens => sources
                .monitors
                .get(self.monitor)
                .map_or((1920, 1080), |m| (m.width, m.height)),
            SourceTab::Windows => sources
                .windows
                .get(self.window)
                .map_or((1920, 1080), |w| (w.width, w.height)),
        }
    }

    /// The source's refresh rate, for a screen; windows follow their screen.
    pub fn source_refresh(&self, sources: &Sources) -> u32 {
        sources
            .monitors
            .iter()
            .find(|m| m.primary)
            .or_else(|| sources.monitors.first())
            .map_or(60, |m| m.refresh_hz.max(1))
            .min(match self.tab {
                SourceTab::Screens => sources
                    .monitors
                    .get(self.monitor)
                    .map_or(60, |m| m.refresh_hz.max(1)),
                SourceTab::Windows => u32::MAX,
            })
    }

    /// Heights offered: the source's own and the usual steps below it (RF-22).
    pub fn height_options(&self, sources: &Sources) -> Vec<u32> {
        let (_, source_height) = self.source_size(sources);
        let mut heights: Vec<u32> = [source_height, 1440, 1080, 720, 480]
            .into_iter()
            .filter(|h| *h <= source_height)
            .collect();
        heights.sort_unstable_by(|a, b| b.cmp(a));
        heights.dedup();
        heights.truncate(4);
        heights
    }

    /// Rates offered: 60 at most (DEC-59), and never above the screen's own.
    pub fn fps_options(&self, sources: &Sources) -> Vec<u32> {
        let limit = self.source_refresh(sources).min(60);
        let mut rates: Vec<u32> = [30, 60].into_iter().filter(|f| *f <= limit).collect();
        if rates.is_empty() {
            rates.push(limit.max(15));
        }
        rates
    }

    /// The output size after scaling to `height`.
    pub fn output_size(&self, sources: &Sources) -> (u32, u32) {
        let (w, h) = self.source_size(sources);
        let width = (u64::from(self.height) * u64::from(w) / u64::from(h.max(1))) as u32;
        (width & !1, self.height & !1)
    }

    /// Keeps the choices valid after the source changes.
    pub fn clamp(&mut self, sources: &Sources) {
        let heights = self.height_options(sources);
        if !heights.contains(&self.height) {
            self.height = heights.first().copied().unwrap_or(720);
            if heights.contains(&1080) {
                self.height = 1080;
            }
        }
        let rates = self.fps_options(sources);
        if !rates.contains(&self.fps) {
            self.fps = rates.last().copied().unwrap_or(30);
        }
        self.mouse = match self.tab {
            SourceTab::Screens => true,
            SourceTab::Windows => sources
                .windows
                .get(self.window)
                .is_some_and(|w| is_browser(&w.process)),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_codes_use_the_alphabet() {
        for seed in 1..50 {
            let code = preview_code(seed);
            assert_eq!(code.len(), 9);
            assert!(looks_like_code(&code));
            assert!(
                code.chars()
                    .filter(|c| *c != '-')
                    .all(|c| ALPHABET.contains(&(c as u8)))
            );
        }
    }

    #[test]
    fn codes_and_nicknames_are_checked() {
        assert!(looks_like_code("ABCD-2345"));
        assert!(looks_like_code("abcd2345"));
        assert!(!looks_like_code("ABCD-234"));
        assert!(valid_nickname("Bianco"));
        assert!(!valid_nickname("   "));
        assert!(!valid_nickname(&"x".repeat(25)));
    }

    #[test]
    fn bitrate_ceiling_follows_the_measurements() {
        assert_eq!(target_kbps(1920, 1080, 60), 8000);
        let eco = target_kbps(1280, 720, 30);
        assert!((2500..3500).contains(&eco), "{eco}");
        assert!(target_kbps(1920, 1080, 30) < target_kbps(1920, 1080, 60));
        assert_eq!(upload_kbps(8000, 11), 110_000);
    }

    #[test]
    fn options_come_from_the_source_and_stop_at_60() {
        let sources = Sources::demo();
        let mut draft = BroadcastDraft::new();
        // The 144 Hz screen is offered 30 and 60, never more.
        assert_eq!(draft.fps_options(&sources), vec![30, 60]);
        assert_eq!(draft.height_options(&sources), vec![1080, 720, 480]);
        // The ultrawide at 1080 high.
        draft.monitor = 1;
        assert_eq!(draft.output_size(&sources), (2560, 1080));
        draft.height = 720;
        assert_eq!(draft.output_size(&sources), (1706, 720));
    }

    #[test]
    fn mouse_default_follows_the_source() {
        let sources = Sources::demo();
        let mut draft = BroadcastDraft::new();
        draft.clamp(&sources);
        assert!(draft.mouse, "a whole screen captures the mouse");
        draft.tab = SourceTab::Windows;
        draft.window = 1; // a code editor
        draft.clamp(&sources);
        assert!(!draft.mouse);
        draft.window = 3; // a browser
        draft.clamp(&sources);
        assert!(draft.mouse);
    }
}
