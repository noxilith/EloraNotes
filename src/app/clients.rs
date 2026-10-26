//! The part of Settings that finds the AI apps on this computer and adds EloraNotes to them.
use super::*;
use crate::mcp_clients::{self, Client, Environment, Launch, Report, State};
use std::sync::mpsc::{Receiver, TryRecvError};

/// What a button press asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Task {
    Add,
    Remove,
}

type Finished = Vec<(Client, Result<String, String>)>;

#[derive(Default)]
pub(super) struct McpClients {
    reports: Vec<Report>,
    /// The command the reports were made for, so a change of options is noticed.
    scanned_for: Option<Launch>,
    scan: Option<Receiver<Vec<Report>>>,
    /// When the command last changed, so typing a folder name does not scan on every letter.
    changed_at: Option<Instant>,
    work: Option<Receiver<Finished>>,
    /// What the last press on each app said, and whether it worked.
    notes: HashMap<Client, (bool, String)>,
    /// A computer other than this one to look at; only the tests set it.
    pub(super) environment: Option<Environment>,
}

impl McpClients {
    fn environment(&self) -> Environment {
        self.environment
            .clone()
            .unwrap_or_else(Environment::current)
    }

    pub(super) fn busy(&self) -> bool {
        self.scan.is_some() || self.work.is_some()
    }

    #[cfg(test)]
    pub(super) fn reports(&self) -> &[Report] {
        &self.reports
    }

    /// Looks at every app, on a thread of its own: asking an app's command what it holds can take
    /// a moment, and the window must keep drawing meanwhile.
    pub(super) fn start_scan(&mut self, launch: &Launch, wake: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (environment, launch_copy, wake) = (self.environment(), launch.clone(), wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send(mcp_clients::scan(&environment, &launch_copy));
            wake.request_repaint();
        });
        self.scan = Some(rx);
        self.scanned_for = Some(launch.clone());
        self.changed_at = None;
    }

    pub(super) fn start_work(
        &mut self,
        jobs: Vec<(Client, Task)>,
        launch: &Launch,
        wake: &egui::Context,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (environment, launch_copy, wake) = (self.environment(), launch.clone(), wake.clone());
        std::thread::spawn(move || {
            let done: Finished = jobs
                .into_iter()
                .map(|(client, task)| {
                    let result = match task {
                        Task::Add => mcp_clients::add(&environment, client, &launch_copy),
                        Task::Remove => mcp_clients::remove(&environment, client),
                    };
                    (client, result)
                })
                .collect();
            let _ = tx.send(done);
            wake.request_repaint();
        });
        self.work = Some(rx);
    }

    /// Collects what the threads have finished. Returns the messages of finished work, for the
    /// notice at the bottom of the window.
    pub(super) fn poll(&mut self) -> Vec<String> {
        let mut said = Vec::new();
        if let Some(rx) = &self.scan {
            match rx.try_recv() {
                Ok(reports) => {
                    self.reports = reports;
                    self.scan = None;
                }
                Err(TryRecvError::Disconnected) => self.scan = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.work {
            match rx.try_recv() {
                Ok(done) => {
                    for (client, result) in done {
                        let (ok, text) = match result {
                            Ok(text) => (true, text),
                            Err(text) => (false, text),
                        };
                        said.push(if ok {
                            text.clone()
                        } else {
                            format!("{}: {text}", client.name())
                        });
                        self.notes.insert(client, (ok, text));
                    }
                    self.work = None;
                    // What the apps hold has changed, so they are looked at again.
                    self.scanned_for = None;
                }
                Err(TryRecvError::Disconnected) => self.work = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        said
    }
}

impl EloraApp {
    /// Starts a scan when there is none for the command in use. The first one starts at once; a
    /// later change of options waits until it has stopped changing.
    pub(super) fn keep_mcp_clients_current(&mut self, ctx: &egui::Context, launch: &Launch) {
        if let Some(message) = self.mcp_clients.poll().into_iter().last() {
            self.status_message = Some(message);
        }
        if self.mcp_clients.busy() {
            ctx.request_repaint_after(Duration::from_millis(150));
            return;
        }
        if self.mcp_clients.scanned_for.as_ref() == Some(launch) {
            self.mcp_clients.changed_at = None;
            return;
        }
        let first = self.mcp_clients.scanned_for.is_none() && self.mcp_clients.changed_at.is_none();
        let settled = self
            .mcp_clients
            .changed_at
            .is_some_and(|since| since.elapsed() > Duration::from_millis(600));
        if first || settled || self.mcp_clients.reports.is_empty() {
            self.mcp_clients.start_scan(launch, ctx);
        } else {
            if self.mcp_clients.changed_at.is_none() {
                self.mcp_clients.changed_at = Some(Instant::now());
            }
            ctx.request_repaint_after(Duration::from_millis(650));
        }
    }

    /// The "set up automatically" part of Settings → AI assistants.
    pub(super) fn mcp_clients_ui(&mut self, ui: &mut egui::Ui, p: &Palette, launch: &Launch) {
        let ctx = ui.ctx().clone();
        self.keep_mcp_clients_current(&ctx, launch);

        ui.add_space(4.0);
        ui.label(RichText::new("Add it to your AI app").strong());
        let busy = self.mcp_clients.busy();
        let found: Vec<Report> = self
            .mcp_clients
            .reports
            .iter()
            .filter(|report| report.state != State::NotFound)
            .cloned()
            .collect();
        let missing: Vec<&str> = self
            .mcp_clients
            .reports
            .iter()
            .filter(|report| report.state == State::NotFound)
            .map(|report| report.client.name())
            .collect();

        ui.horizontal(|ui| {
            if self.mcp_clients.reports.is_empty() {
                ui.label(
                    RichText::new("Looking for AI apps on this computer…")
                        .size(11.5)
                        .color(p.fg_muted),
                );
            } else if found.is_empty() {
                ui.label(
                    RichText::new("No supported AI app was found. Set it up by hand below.")
                        .size(11.5)
                        .color(p.fg_muted),
                );
            } else {
                ui.label(
                    RichText::new(format!(
                        "Found {} on this computer. Nothing is changed until you press a button.",
                        found.len()
                    ))
                    .size(11.5)
                    .color(p.fg_muted),
                );
            }
            if ui
                .add_enabled(!busy, egui::Button::new(icon::REFRESH).small())
                .on_hover_text("Look again")
                .clicked()
            {
                self.mcp_clients.scanned_for = None;
                self.mcp_clients.changed_at = None;
            }
        });

        let mut jobs: Vec<(Client, Task)> = Vec::new();
        for report in &found {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(report.client.name()).strong().size(12.5));
                match &report.state {
                    State::Added => {
                        ui.label(RichText::new(format!("{} added", icon::CHECK)).size(11.5).color(p.accent));
                    }
                    State::NotAdded => {
                        ui.label(RichText::new("not added yet").size(11.5).color(p.fg_muted));
                    }
                    State::Outdated => {
                        ui.label(
                            RichText::new("added, out of date")
                                .size(11.5)
                                .color(p.fg_secondary),
                        )
                        .on_hover_text(
                            "It points at another copy of EloraNotes or was added with different limits. Update it to use this one.",
                        );
                    }
                    State::Unreadable(why) | State::Unavailable(why) => {
                        ui.label(RichText::new(why).size(11.0).color(p.fg_muted));
                    }
                    State::NotFound => {}
                }
                // Which buttons make sense: add what is missing, bring a stale entry up to date or
                // take it out, and take out what is up to date.
                let (can_add, can_update, can_remove) = match report.state {
                    State::NotAdded => (true, false, false),
                    State::Outdated => (false, true, true),
                    State::Added => (false, false, true),
                    _ => (false, false, false),
                };
                if can_add && ui.add_enabled(!busy, egui::Button::new(RichText::new("Add").size(11.5)).small()).clicked() {
                    jobs.push((report.client, Task::Add));
                }
                if can_update && ui.add_enabled(!busy, egui::Button::new(RichText::new("Update").size(11.5)).small()).clicked() {
                    jobs.push((report.client, Task::Add));
                }
                if can_remove && ui.add_enabled(!busy, egui::Button::new(RichText::new("Remove").size(11.5)).small()).clicked() {
                    jobs.push((report.client, Task::Remove));
                }
            });
            if let Some((ok, text)) = self.mcp_clients.notes.get(&report.client) {
                ui.add(
                    egui::Label::new(RichText::new(text).size(10.5).color(if *ok {
                        p.fg_secondary
                    } else {
                        p.danger
                    }))
                    .wrap(),
                );
            }
        }

        let to_add: Vec<(Client, Task)> = found
            .iter()
            .filter(|report| matches!(report.state, State::NotAdded | State::Outdated))
            .map(|report| (report.client, Task::Add))
            .collect();
        if to_add.len() > 1
            && ui
                .add_enabled(
                    !busy,
                    egui::Button::new(format!("Add to all {}", to_add.len())).small(),
                )
                .clicked()
        {
            jobs = to_add;
        }
        if !missing.is_empty() && !self.mcp_clients.reports.is_empty() {
            ui.label(
                RichText::new(format!("Not found: {}.", missing.join(", ")))
                    .size(10.5)
                    .color(p.fg_muted),
            );
        }
        if !jobs.is_empty() {
            for (client, _) in &jobs {
                self.mcp_clients.notes.remove(client);
            }
            self.mcp_clients.start_work(jobs, launch, &ctx);
        }
    }
}
