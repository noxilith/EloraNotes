//! The update check: the worker threads, the launch-time prompt and the Settings row.
use super::*;

/// Messages from the update worker threads to the UI.
pub(super) enum UpdateEvent {
    Checked(Result<Option<UpdateOffer>, String>),
    Staged(Result<PathBuf, String>),
}

/// What the update prompt is currently showing.
#[derive(Clone)]
pub(super) enum UpdateState {
    Idle,
    Checking,
    /// A check started from Settings found nothing newer.
    UpToDate,
    Available(UpdateOffer),
    Downloading(Release),
    /// A newer release exists but a package manager owns this copy, so there is nothing to
    /// download; the prompt only explains how to get it.
    Managed {
        version: String,
        manager: &'static str,
    },
    Error(String),
}

impl EloraApp {
    /// Kicks off a version check on a worker thread so the frame does not wait for GitHub.
    /// `quiet` is set when Settings asked for it: the outcome is reported there instead of in a
    /// popup, and "already current" is worth saying because the user asked.
    pub(super) fn start_update_check(&mut self, ctx: &egui::Context, quiet: bool) {
        update::cleanup_stale_updates();
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        self.quiet_update = quiet;
        self.update_state = UpdateState::Checking;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = update::check_for_update();
            let _ = tx.send(UpdateEvent::Checked(result));
            ctx.request_repaint();
        });
    }

    /// Downloads the release asset in the background; the binary is swapped in on the UI
    /// thread once the file is ready.
    pub(super) fn start_update_download(&mut self, ctx: &egui::Context, release: Release) {
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        self.update_state = UpdateState::Downloading(release.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = update::stage_download(&release);
            let _ = tx.send(UpdateEvent::Staged(result));
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_update(&mut self) {
        let was_downloading = matches!(self.update_state, UpdateState::Downloading(_));
        let mut events = Vec::new();
        if let Some(rx) = &self.update_rx {
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    // Each worker sends one message and then hangs up, so a disconnect is
                    // only a failure when nothing at all arrived.
                    Err(TryRecvError::Disconnected) => {
                        if events.is_empty() {
                            events.push(if was_downloading {
                                UpdateEvent::Staged(Err(
                                    "The update download stopped unexpectedly.".to_string(),
                                ))
                            } else {
                                UpdateEvent::Checked(Err(
                                    "The update check stopped unexpectedly.".to_string()
                                ))
                            });
                        }
                        break;
                    }
                }
            }
        }

        for event in events {
            match event {
                UpdateEvent::Checked(Ok(Some(offer))) => {
                    self.update_rx = None;
                    self.update_state = UpdateState::Available(offer);
                }
                // Offline, rate-limited, already current, or no binary for this platform:
                // there is nothing worth interrupting the user for unless they asked.
                UpdateEvent::Checked(Ok(None)) => {
                    self.update_rx = None;
                    self.update_state = if self.quiet_update {
                        UpdateState::UpToDate
                    } else {
                        UpdateState::Idle
                    };
                }
                UpdateEvent::Checked(Err(error)) => {
                    self.update_rx = None;
                    self.update_state = if self.quiet_update {
                        UpdateState::Error(error)
                    } else {
                        UpdateState::Idle
                    };
                }
                UpdateEvent::Staged(Ok(staged)) => {
                    self.update_rx = None;
                    match update::install_and_restart(&staged) {
                        Ok(()) => std::process::exit(0),
                        Err(error) => self.update_state = UpdateState::Error(error),
                    }
                }
                UpdateEvent::Staged(Err(error)) => {
                    self.update_rx = None;
                    self.update_state = UpdateState::Error(error);
                }
            }
        }
    }

    /// The launch-time prompt: update now (download, swap and restart) or dismiss it for this
    /// run. Nothing is offered when the check failed or the running build is current.
    pub(super) fn update_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let state = self.update_state.clone();
        match state {
            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Checking => {}
            // A check from Settings keeps its result in that window, so nothing pops up over the
            // note the user is reading.
            UpdateState::Available(_) if self.quiet_update => {}
            UpdateState::Available(offer) => match offer {
                UpdateOffer::SelfUpdate(release) => {
                    let mut choice = None;
                    egui::Window::new("Update available")
                        .id(Id::new("elora_update_window"))
                        .collapsible(false)
                        .resizable(false)
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(ctx, |ui| {
                            ui.set_min_width(320.0);
                            ui.label(
                                RichText::new(format!(
                                    "EloraNotes {} is available",
                                    release.version
                                ))
                                .size(14.0)
                                .strong(),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "You are running {}. Update and restart now?",
                                    update::CURRENT_VERSION
                                ))
                                .size(12.0)
                                .color(p.fg_secondary),
                            );
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new("Update now").size(12.5))
                                            .fill(p.accent),
                                    )
                                    .clicked()
                                {
                                    choice = Some(true);
                                }
                                if ui.button("Later").clicked() {
                                    choice = Some(false);
                                }
                            });
                        });
                    match choice {
                        Some(true) => self.start_update_download(ctx, release),
                        Some(false) => self.update_state = UpdateState::Idle,
                        None => {}
                    }
                }
                // Flatpak owns its own files: offering to replace them would fail after the
                // download, so the prompt just points at the command that can.
                UpdateOffer::Managed { version, manager } => {
                    self.update_state = UpdateState::Managed { version, manager };
                }
            },
            UpdateState::Managed { version, manager } => {
                let mut dismiss = false;
                egui::Window::new("Update available")
                    .id(Id::new("elora_update_window"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(320.0);
                        ui.label(
                            RichText::new(format!("EloraNotes {version} is available"))
                                .size(14.0)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "You are running {}.",
                                update::CURRENT_VERSION
                            ))
                            .size(12.0)
                            .color(p.fg_secondary),
                        );
                        ui.label(
                            RichText::new(format!(
                                "This copy was installed with {manager}, which owns its own files. Install the new bundle over it, in the folder where you saved it:"
                            ))
                            .size(11.5)
                            .color(p.fg_muted),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "flatpak install --reinstall ./EloraNotes-{version}-linux-x86_64.flatpak"
                            ))
                            .size(11.5)
                            .monospace(),
                        );
                        ui.add_space(8.0);
                        if ui.button("Close").clicked() {
                            dismiss = true;
                        }
                    });
                if dismiss {
                    self.update_state = UpdateState::Idle;
                }
            }
            UpdateState::Downloading(release) => {
                egui::Window::new("Updating EloraNotes")
                    .id(Id::new("elora_update_window"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(320.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(16.0));
                            ui.label(format!("Downloading EloraNotes {}…", release.version));
                        });
                        ui.label(
                            RichText::new("EloraNotes restarts itself when the download finishes.")
                                .size(11.5)
                                .color(p.fg_muted),
                        );
                    });
            }
            UpdateState::Error(message) => {
                let mut dismiss = false;
                egui::Window::new("Update failed")
                    .id(Id::new("elora_update_window"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(320.0);
                        ui.label(RichText::new(message).size(12.0).color(p.danger));
                        ui.add_space(8.0);
                        if ui.button("Close").clicked() {
                            dismiss = true;
                        }
                    });
                if dismiss {
                    self.update_state = UpdateState::Idle;
                }
            }
        }
    }

    /// The update section of Settings: what is running, what the last check found, and the one
    /// button that acts on it. Everything the launch prompt can do is reachable from here too.
    pub(super) fn update_settings_row(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        let mut check = false;
        let mut install: Option<Release> = None;

        ui.label(
            RichText::new(format!(
                "You are running EloraNotes {}.",
                update::CURRENT_VERSION
            ))
            .size(11.5)
            .color(p.fg_muted),
        );

        match self.update_state.clone() {
            UpdateState::Checking => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(RichText::new("Checking for updates…").size(11.5));
                });
            }
            UpdateState::Downloading(release) => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(RichText::new(format!("Downloading {}…", release.version)).size(11.5));
                });
                ui.label(
                    RichText::new("EloraNotes restarts itself when the download finishes.")
                        .size(11.0)
                        .color(p.fg_muted),
                );
            }
            UpdateState::Available(UpdateOffer::SelfUpdate(release)) => {
                ui.label(
                    RichText::new(format!("EloraNotes {} is available.", release.version))
                        .size(12.0)
                        .strong(),
                );
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!("{}  Update and restart", icon::DOWNLOAD))
                                .size(12.0),
                        )
                        .fill(p.accent),
                    )
                    .clicked()
                {
                    install = Some(release);
                }
            }
            UpdateState::Available(UpdateOffer::Managed { version, manager })
            | UpdateState::Managed { version, manager } => {
                ui.label(
                    RichText::new(format!("EloraNotes {version} is available."))
                        .size(12.0)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!(
                        "This copy was installed with {manager}, which owns its own files."
                    ))
                    .size(11.0)
                    .color(p.fg_muted),
                );
                let command = format!(
                    "flatpak install --reinstall ./EloraNotes-{version}-linux-x86_64.flatpak"
                );
                ui.label(RichText::new(&command).size(11.0).monospace());
                if ui
                    .small_button(format!("{}  Copy command", icon::COPY))
                    .clicked()
                {
                    ui.ctx().copy_text(command);
                    self.status_message = Some("Copied the update command".to_string());
                }
            }
            UpdateState::UpToDate => {
                ui.label(
                    RichText::new(format!("{}  You're up to date.", icon::CHECK))
                        .size(11.5)
                        .color(p.fg_secondary),
                );
                if ui
                    .small_button(format!("{}  Check again", icon::REFRESH))
                    .clicked()
                {
                    check = true;
                }
            }
            UpdateState::Error(message) => {
                ui.label(RichText::new(message).size(11.5).color(p.danger));
                if ui
                    .small_button(format!("{}  Try again", icon::REFRESH))
                    .clicked()
                {
                    check = true;
                }
            }
            UpdateState::Idle => {
                if ui
                    .button(format!("{}  Check for updates", icon::REFRESH))
                    .clicked()
                {
                    check = true;
                }
            }
        }

        if check {
            self.start_update_check(&ctx, true);
        } else if let Some(release) = install {
            self.start_update_download(&ctx, release);
        }
    }
}
