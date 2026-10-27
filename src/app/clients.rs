//! The part of Settings that finds the AI apps on this computer, adds EloraNotes to them as an MCP
//! server, and gives them the memory skill.
use super::*;
use crate::mcp_clients::{self, Client, Environment, Launch, Report, State};
use crate::skills::{self, Place};
use std::sync::mpsc::{Receiver, TryRecvError};

/// What a button press asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Task {
    Add,
    Remove,
}

/// One thing a button asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Job {
    Mcp(Client, Task),
    Skill(Place, Task),
}

/// What a press was about, so its answer is shown under the right row.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Mcp(Client),
    Skill(Place),
}

impl Key {
    fn label(self) -> String {
        match self {
            Key::Mcp(client) => client.name().to_string(),
            Key::Skill(place) => format!("Memory skill for {}", place.name()),
        }
    }
}

/// What one look at the computer found.
struct Scanned {
    clients: Vec<Report>,
    skills: Vec<skills::Report>,
}

type Finished = Vec<(Key, Result<String, String>)>;

#[derive(Default)]
pub(super) struct McpClients {
    reports: Vec<Report>,
    skill_reports: Vec<skills::Report>,
    /// The command the reports were made for, so a change of options is noticed.
    scanned_for: Option<Launch>,
    scan: Option<Receiver<Scanned>>,
    /// When the command last changed, so typing a folder name does not scan on every letter.
    changed_at: Option<Instant>,
    work: Option<Receiver<Finished>>,
    /// What the last press on each row said, and whether it worked.
    notes: HashMap<Key, (bool, String)>,
    /// What saving the skill as a ZIP said.
    zip_note: Option<(bool, String)>,
    /// A computer other than this one to look at; only the tests set it.
    pub(super) environment: Option<Environment>,
    sandboxed: Option<bool>,
    /// Look at the AI apps once, a few seconds after the window opens, and say so if one still
    /// starts an old copy of EloraNotes. Only the real window turns this on.
    pub(super) watch_at_start: bool,
    started: Option<Instant>,
    start_check: StartCheck,
}

/// Where the look at launch has got to.
#[derive(Default, PartialEq, Eq)]
enum StartCheck {
    #[default]
    Waiting,
    Looking,
    Done,
}

impl McpClients {
    /// The apps that start a different copy of EloraNotes than this one.
    fn moved(&self) -> Vec<Client> {
        self.reports
            .iter()
            .filter(|report| report.moved)
            .map(|report| report.client)
            .collect()
    }

    fn environment(&self) -> Environment {
        self.environment
            .clone()
            .unwrap_or_else(Environment::current)
    }

    fn is_sandboxed(&mut self) -> bool {
        if self.sandboxed.is_none() {
            self.sandboxed = Some(self.environment().sandboxed);
        }
        self.sandboxed.unwrap_or(false)
    }

    pub(super) fn busy(&self) -> bool {
        self.scan.is_some() || self.work.is_some()
    }

    #[cfg(test)]
    pub(super) fn reports(&self) -> &[Report] {
        &self.reports
    }

    #[cfg(test)]
    pub(super) fn skill_reports(&self) -> &[skills::Report] {
        &self.skill_reports
    }

    /// Looks at every app, on a thread of its own: asking an app's command what it holds can take
    /// a moment, and the window must keep drawing meanwhile.
    pub(super) fn start_scan(&mut self, launch: &Launch, wake: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (environment, launch_copy, wake) = (self.environment(), launch.clone(), wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Scanned {
                clients: mcp_clients::scan(&environment, &launch_copy),
                skills: skills::scan(&environment),
            });
            wake.request_repaint();
        });
        self.scan = Some(rx);
        self.scanned_for = Some(launch.clone());
        self.changed_at = None;
    }

    pub(super) fn start_work(&mut self, jobs: Vec<Job>, launch: &Launch, wake: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (environment, launch_copy, wake) = (self.environment(), launch.clone(), wake.clone());
        std::thread::spawn(move || {
            let done: Finished = jobs
                .into_iter()
                .map(|job| match job {
                    Job::Mcp(client, Task::Add) => (
                        Key::Mcp(client),
                        mcp_clients::add(&environment, client, &launch_copy),
                    ),
                    Job::Mcp(client, Task::Remove) => {
                        (Key::Mcp(client), mcp_clients::remove(&environment, client))
                    }
                    Job::Skill(place, Task::Add) => {
                        (Key::Skill(place), skills::add(&environment, place))
                    }
                    Job::Skill(place, Task::Remove) => {
                        (Key::Skill(place), skills::remove(&environment, place))
                    }
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
                Ok(found) => {
                    self.reports = found.clients;
                    self.skill_reports = found.skills;
                    self.scan = None;
                }
                Err(TryRecvError::Disconnected) => self.scan = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.work {
            match rx.try_recv() {
                Ok(done) => {
                    for (key, result) in done {
                        let (ok, text) = match result {
                            Ok(text) => (true, text),
                            Err(text) => (false, text),
                        };
                        said.push(if ok {
                            text.clone()
                        } else {
                            format!("{}: {text}", key.label())
                        });
                        self.notes.insert(key, (ok, text));
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
    /// A few seconds after launch, looks at the AI apps without changing anything. If one still
    /// starts an old copy of EloraNotes (the app was reinstalled somewhere else, or moved), says
    /// where to fix it, so a connection that quietly stopped working is not a mystery.
    pub(super) fn check_ai_apps_at_start(&mut self, ctx: &egui::Context) {
        if !self.mcp_clients.watch_at_start || self.mcp_clients.start_check == StartCheck::Done {
            return;
        }
        let began = *self.mcp_clients.started.get_or_insert_with(Instant::now);
        if self.mcp_clients.start_check == StartCheck::Waiting {
            if began.elapsed() < Duration::from_secs(4) {
                ctx.request_repaint_after(Duration::from_secs(1));
                return;
            }
            let (program, args) =
                settings::mcp_launch_command(self.storage.root_dir(), &self.mcp_options);
            let launch = Launch { program, args };
            self.mcp_clients.start_scan(&launch, ctx);
            self.mcp_clients.start_check = StartCheck::Looking;
        }
        if let Some(message) = self.mcp_clients.poll().into_iter().last() {
            self.status_message = Some(message);
        }
        if self.mcp_clients.scan.is_some() {
            ctx.request_repaint_after(Duration::from_millis(300));
            return;
        }
        self.mcp_clients.start_check = StartCheck::Done;
        let moved = self.mcp_clients.moved();
        if !moved.is_empty() {
            let names: Vec<&str> = moved.iter().map(|client| client.name()).collect();
            self.status_message = Some(format!(
                "{} still starts an older or different copy of EloraNotes. Settings → AI assistants → Update fixes it.",
                names.join(" and ")
            ));
        }
    }

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
        if self.mcp_clients.is_sandboxed() {
            let app_id = std::env::var("FLATPAK_ID")
                .unwrap_or_else(|_| "io.github.noxilith.EloraNotes".to_string());
            let grant = mcp_clients::flatpak_grant_command(&app_id);
            egui::Frame::new()
                .fill(p.bg_elevated)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "EloraNotes is running in Flatpak's sandbox, which hides your other \
                             apps' settings from it, so it cannot find or change them yet. Run \
                             this once in a terminal, then restart EloraNotes:",
                        )
                        .size(11.0)
                        .color(p.fg_secondary),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(&grant)
                                .monospace()
                                .size(10.0)
                                .color(p.fg_muted),
                        )
                        .wrap(),
                    );
                    if ui
                        .small_button(format!("{}  Copy command", icon::COPY))
                        .clicked()
                    {
                        ui.ctx().copy_text(grant.clone());
                        self.status_message = Some("Copied the command".to_string());
                    }
                    ui.label(
                        RichText::new(
                            "Claude Code and Codex are command-line tools the sandbox cannot run; \
                             add those under Set it up by hand.",
                        )
                        .size(10.5)
                        .color(p.fg_muted),
                    );
                });
        }
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

        let mut jobs: Vec<Job> = Vec::new();
        // Apps that start another copy of EloraNotes come first, with one button for all of them.
        let moved: Vec<Client> = found
            .iter()
            .filter(|report| report.moved)
            .map(|report| report.client)
            .collect();
        if !moved.is_empty() {
            egui::Frame::new()
                .fill(p.bg_elevated)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    let names: Vec<&str> = moved.iter().map(|client| client.name()).collect();
                    ui.label(
                        RichText::new(format!(
                            "{} still start{} an older or different copy of EloraNotes.",
                            names.join(", "),
                            if names.len() == 1 { "s" } else { "" }
                        ))
                        .size(11.5)
                        .color(p.fg_primary),
                    );
                    ui.label(
                        RichText::new(
                            "Updating points them at this copy and uses the options above. \
                             Restart each app afterwards so it loads the new one.",
                        )
                        .size(10.5)
                        .color(p.fg_muted),
                    );
                    let label = if moved.len() == 1 {
                        "Update it".to_string()
                    } else {
                        format!("Update all {}", moved.len())
                    };
                    if ui
                        .add_enabled(!busy, egui::Button::new(label).small())
                        .clicked()
                    {
                        jobs = moved
                            .iter()
                            .map(|client| Job::Mcp(*client, Task::Add))
                            .collect();
                    }
                });
            ui.add_space(4.0);
        }
        for report in &found {
            ui.horizontal_wrapped(|ui| {
                let name = ui.label(RichText::new(report.client.name()).strong().size(12.5));
                if !report.place.is_empty() {
                    name.on_hover_text(format!("Settings: {}", report.place));
                }
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
                    jobs.push(Job::Mcp(report.client, Task::Add));
                }
                if can_update && ui.add_enabled(!busy, egui::Button::new(RichText::new("Update").size(11.5)).small()).clicked() {
                    jobs.push(Job::Mcp(report.client, Task::Add));
                }
                if can_remove && ui.add_enabled(!busy, egui::Button::new(RichText::new("Remove").size(11.5)).small()).clicked() {
                    jobs.push(Job::Mcp(report.client, Task::Remove));
                }
            });
            if let Some((ok, text)) = self.mcp_clients.notes.get(&Key::Mcp(report.client)) {
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

        let to_add: Vec<Job> = found
            .iter()
            .filter(|report| matches!(report.state, State::NotAdded | State::Outdated))
            .map(|report| Job::Mcp(report.client, Task::Add))
            .collect();
        let any_missing = found.iter().any(|report| report.state == State::NotAdded);
        let any_stale = found.iter().any(|report| report.state == State::Outdated);
        let all_label = match (any_missing, any_stale) {
            (true, true) => format!("Add or update all {}", to_add.len()),
            (false, true) => format!("Update all {}", to_add.len()),
            _ => format!("Add to all {}", to_add.len()),
        };
        if to_add.len() > 1
            && jobs.is_empty()
            && ui
                .add_enabled(!busy, egui::Button::new(all_label).small())
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
            for job in &jobs {
                if let Job::Mcp(client, _) = job {
                    self.mcp_clients.notes.remove(&Key::Mcp(*client));
                }
            }
            self.mcp_clients.start_work(jobs, launch, &ctx);
        }
    }
}

impl EloraApp {
    /// The "Memory bank" part of Settings → AI assistants: the skill that teaches an assistant to
    /// use EloraNotes as its long-term memory, and the buttons that put it where assistants read it.
    pub(super) fn memory_skill_ui(&mut self, ui: &mut egui::Ui, p: &Palette, launch: &Launch) {
        let ctx = ui.ctx().clone();
        let busy = self.mcp_clients.busy();
        ui.add_space(8.0);
        ui.label(RichText::new("Memory bank").strong());
        ui.label(
            RichText::new(
                "Let an assistant remember across conversations. The memory skill teaches it to \
                 look in EloraNotes before it starts and to save what is worth keeping, as \
                 ordinary notes in an “AI Memory” folder that you can read, edit and delete. It \
                 works through the connection above, so the assistant has to be added there too.",
            )
            .size(11.5)
            .color(p.fg_muted),
        );
        if self.mcp_options.read_only {
            ui.label(
                RichText::new(
                    "Read-only is on above, so an assistant could recall memories but not save any.",
                )
                .size(11.0)
                .color(p.danger),
            );
        }

        let found: Vec<skills::Report> = self
            .mcp_clients
            .skill_reports
            .iter()
            .filter(|report| report.state != skills::State::NotFound)
            .cloned()
            .collect();
        let missing: Vec<&str> = self
            .mcp_clients
            .skill_reports
            .iter()
            .filter(|report| report.state == skills::State::NotFound)
            .map(|report| report.place.name())
            .collect();

        let mut jobs: Vec<Job> = Vec::new();
        for report in &found {
            ui.horizontal_wrapped(|ui| {
                let name = ui.label(RichText::new(report.place.name()).strong().size(12.5));
                name.on_hover_text(format!("Skill file: {}", report.file.display()));
                match &report.state {
                    skills::State::Added => {
                        ui.label(
                            RichText::new(format!("{} added", icon::CHECK))
                                .size(11.5)
                                .color(p.accent),
                        );
                    }
                    skills::State::NotAdded => {
                        ui.label(RichText::new("not added yet").size(11.5).color(p.fg_muted));
                    }
                    skills::State::Outdated => {
                        ui.label(
                            RichText::new("added, out of date")
                                .size(11.5)
                                .color(p.fg_secondary),
                        )
                        .on_hover_text(
                            "The skill there differs from this version of EloraNotes: it is older, or was edited. Update replaces it and keeps a copy of the old file.",
                        );
                    }
                    skills::State::Unreadable(why) => {
                        ui.label(RichText::new(why).size(11.0).color(p.fg_muted));
                    }
                    skills::State::NotFound => {}
                }
                let (can_add, can_update, can_remove) = match report.state {
                    skills::State::NotAdded => (true, false, false),
                    skills::State::Outdated => (false, true, true),
                    skills::State::Added => (false, false, true),
                    _ => (false, false, false),
                };
                let small = |text: &str| egui::Button::new(RichText::new(text).size(11.5)).small();
                if can_add && ui.add_enabled(!busy, small("Add")).clicked() {
                    jobs.push(Job::Skill(report.place, Task::Add));
                }
                if can_update && ui.add_enabled(!busy, small("Update")).clicked() {
                    jobs.push(Job::Skill(report.place, Task::Add));
                }
                if can_remove && ui.add_enabled(!busy, small("Remove")).clicked() {
                    jobs.push(Job::Skill(report.place, Task::Remove));
                }
            });
            if let Some((ok, text)) = self.mcp_clients.notes.get(&Key::Skill(report.place)) {
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
        let to_add: Vec<Job> = found
            .iter()
            .filter(|report| {
                matches!(
                    report.state,
                    skills::State::NotAdded | skills::State::Outdated
                )
            })
            .map(|report| Job::Skill(report.place, Task::Add))
            .collect();
        if to_add.len() > 1
            && ui
                .add_enabled(
                    !busy,
                    egui::Button::new(
                        RichText::new(format!("Add to all {}", to_add.len())).size(11.5),
                    )
                    .small(),
                )
                .clicked()
        {
            jobs = to_add;
        }
        if !missing.is_empty() && !self.mcp_clients.skill_reports.is_empty() {
            ui.label(
                RichText::new(format!("Not found: {}.", missing.join(", ")))
                    .size(10.5)
                    .color(p.fg_muted),
            );
        }
        if !jobs.is_empty() {
            for job in &jobs {
                if let Job::Skill(place, _) = job {
                    self.mcp_clients.notes.remove(&Key::Skill(*place));
                }
            }
            self.mcp_clients.start_work(jobs, launch, &ctx);
        }

        // Claude Desktop and claude.ai keep skills in the app, not in a folder, so they take a
        // ZIP that is uploaded by hand.
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Claude Desktop and claude.ai")
                    .strong()
                    .size(12.5),
            );
            if ui
                .add(egui::Button::new(RichText::new("Save skill as .zip").size(11.5)).small())
                .on_hover_text("They take a skill as an uploaded file: Customize → Skills → Upload")
                .clicked()
            {
                let folder = dirs::download_dir()
                    .or_else(dirs::home_dir)
                    .unwrap_or_default();
                let file = folder.join("elora-memory-skill.zip");
                self.mcp_clients.zip_note = Some(match skills::save_zip(&file) {
                    Ok(text) => (true, text),
                    Err(text) => (false, text),
                });
            }
        });
        if let Some((ok, text)) = &self.mcp_clients.zip_note {
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
}
