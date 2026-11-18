//! Looks for a newer version every time the app opens, and installs it.
//!
//! Only builds that know where to look (`TELINHAS_UPDATE_URL` at compile time)
//! check. A development build has no address, so it never replaces itself.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use telinhas_update::Update;

/// Where `latest.json` lives, built into the binary by the release script.
pub(crate) const UPDATE_URL: Option<&str> = option_env!("TELINHAS_UPDATE_URL");

/// How long to wait for the server before opening the app without updating.
const CHECK_TIMEOUT: Duration = Duration::from_secs(4);
/// The installer is big: allow a slow connection time to bring it.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

enum Message {
    /// A newer version exists; downloading starts.
    Found(Box<Update>),
    Progress(u64, Option<u64>),
    /// The installer is here and checked.
    Ready(PathBuf),
    Failed(String),
    UpToDate,
}

/// What the screen shows about the update.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Status {
    /// Nothing to tell: checking, or nothing newer, or no way to update.
    Quiet,
    Downloading {
        version: String,
        /// 0 to 1, or None when the size is not known.
        fraction: Option<f32>,
    },
    /// The installer was started; the app must close now.
    Installing(String),
}

pub(crate) struct Updater {
    rx: Receiver<Message>,
    status: Status,
    version: Option<String>,
    /// Why the last attempt failed, to tell once.
    failure: Option<String>,
}

impl Updater {
    /// Starts looking, unless this build has nowhere to look or was told not to.
    pub(crate) fn start() -> Option<Self> {
        let url = UPDATE_URL.filter(|u| !u.trim().is_empty())?;
        if std::env::args().any(|a| a == "--no-update") {
            return None;
        }
        let (tx, rx) = channel();
        let url = url.to_owned();
        std::thread::Builder::new()
            .name("telinhas-update".to_owned())
            .spawn(move || {
                let current = env!("CARGO_PKG_VERSION");
                let update = match telinhas_update::check(&url, current, CHECK_TIMEOUT) {
                    Ok(Some(update)) => update,
                    Ok(None) => {
                        let _ = tx.send(Message::UpToDate);
                        return;
                    }
                    // No connection to the server: open the app as it is.
                    Err(error) => {
                        let _ = tx.send(Message::Failed(error.to_string()));
                        return;
                    }
                };
                let _ = tx.send(Message::Found(Box::new(update.clone())));
                let dir = std::env::temp_dir().join("telinhas-update");
                let result = telinhas_update::download(
                    &update,
                    &dir,
                    DOWNLOAD_TIMEOUT,
                    &mut |done, total| {
                        let _ = tx.send(Message::Progress(done, total));
                    },
                );
                let _ = tx.send(match result {
                    Ok(path) => Message::Ready(path),
                    Err(error) => Message::Failed(error.to_string()),
                });
            })
            .ok()?;
        Some(Self {
            rx,
            status: Status::Quiet,
            version: None,
            failure: None,
        })
    }

    pub(crate) fn status(&self) -> &Status {
        &self.status
    }

    /// Takes what the update thread said. Returns a problem worth telling the
    /// person, once.
    pub(crate) fn poll(&mut self) -> Option<String> {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Found(update) => {
                    let version = update.version.to_string();
                    self.version = Some(version.clone());
                    self.status = Status::Downloading {
                        version,
                        fraction: None,
                    };
                }
                Message::Progress(done, total) => {
                    if let Status::Downloading { fraction, .. } = &mut self.status {
                        *fraction = total.map(|t| (done as f32 / t.max(1) as f32).clamp(0.0, 1.0));
                    }
                }
                Message::Ready(path) => {
                    let version = self.version.clone().unwrap_or_default();
                    match telinhas_update::run_installer(&path) {
                        Ok(()) => self.status = Status::Installing(version),
                        Err(error) => {
                            self.status = Status::Quiet;
                            self.failure = Some(error.to_string());
                        }
                    }
                }
                Message::Failed(reason) => {
                    // Not having a connection to the update server is not a
                    // reason to bother anyone, unless a download was under way.
                    if matches!(self.status, Status::Downloading { .. }) {
                        self.failure = Some(reason);
                    }
                    self.status = Status::Quiet;
                }
                Message::UpToDate => self.status = Status::Quiet,
            }
        }
        self.failure.take()
    }
}

use eframe::egui::{self, RichText, Vec2};

use crate::app::TelinhasApp;
use crate::theme;

impl TelinhasApp {
    /// Listens to the update thread. When the installer has started, the app
    /// closes so the installer can replace its files.
    pub(crate) fn poll_update(&mut self, ctx: &egui::Context) {
        let Some(updater) = self.updater.as_mut() else {
            return;
        };
        let failure = updater.poll();
        let installing = matches!(updater.status(), Status::Installing(_));
        let downloading = matches!(updater.status(), Status::Downloading { .. });
        if let Some(reason) = failure {
            self.show_toast(format!("Não consegui atualizar: {reason}"));
        }
        if installing {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if downloading {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// A screen over everything while the new version comes in: the app is not
    /// usable then, and the old version is about to be replaced anyway.
    pub(crate) fn update_overlay(&mut self, ctx: &egui::Context) {
        let Some(updater) = &self.updater else {
            return;
        };
        let (version, fraction, installing) = match updater.status() {
            Status::Quiet => return,
            Status::Downloading { version, fraction } => (version.clone(), *fraction, false),
            Status::Installing(version) => (version.clone(), Some(1.0), true),
        };
        egui::Modal::new(theme::id("update_modal")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.vertical_centered(|ui| {
                ui.add(crate::logo::image(48.0));
                ui.add_space(8.0);
                ui.label(RichText::new("Atualizando o Telinhas").size(20.0).strong());
                ui.label(RichText::new(format!("Nova versão {version}")).color(theme::TEXT_DIM));
                ui.add_space(14.0);
                let bar = match fraction {
                    Some(f) => egui::ProgressBar::new(f).show_percentage(),
                    None => egui::ProgressBar::new(0.0).animate(true),
                };
                ui.add_sized(Vec2::new(340.0, 18.0), bar);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(if installing {
                        "Instalando. O app abre sozinho em seguida."
                    } else {
                        "Baixando. Não precisa fazer nada."
                    })
                    .small()
                    .color(theme::TEXT_FAINT),
                );
            });
        });
    }
}

#[cfg(test)]
impl Updater {
    /// An updater showing `status`, with nothing behind it.
    pub(crate) fn showing(status: Status) -> Self {
        let (_tx, rx) = channel();
        Self {
            rx,
            status,
            version: None,
            failure: None,
        }
    }
}
