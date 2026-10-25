//! The Settings window: appearance, colors, layout, the shared library and updates.
use super::*;

/// One row of the Settings color section: a checkbox that enables an override, plus a color
/// picker shown while enabled. Returns true when the value changed.
pub(super) fn color_override_row(
    ui: &mut Ui,
    label: &str,
    value: &mut Option<[u8; 3]>,
    default: Color32,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, label).changed() {
            *value = if enabled {
                let [r, g, b, _] = default.to_array();
                Some([r, g, b])
            } else {
                None
            };
            changed = true;
        }
        if let Some(rgb) = value.as_mut() {
            changed |= ui.color_edit_button_srgb(rgb).changed();
        }
    });
    changed
}

impl EloraApp {
    /// Settings window: theme, text size, color overrides, glow strength and layout toggles.
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut changed = false;
        let defaults = self.theme.palette();

        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(380.0)
            .show(ctx, |ui| {
                // The window now carries the notes folder and shortcut reference too, so
                // it must scroll rather than run off the bottom of a short window.
                let max_height = (ctx.screen_rect().height() - 160.0).max(240.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(max_height)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Appearance").strong());
                        ui.horizontal_wrapped(|ui| {
                            for t in [
                                ThemeMode::Obsidian,
                                ThemeMode::Paper,
                                ThemeMode::Nordic,
                                ThemeMode::Matcha,
                                ThemeMode::Sepia,
                                ThemeMode::Midnight,
                                ThemeMode::Matugen,
                            ] {
                                if ui.selectable_label(self.theme == t, t.name()).clicked() {
                                    self.theme = t;
                                    changed = true;
                                }
                            }
                        });
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.font_size, 11.0..=22.0)
                                    .text("Text size"),
                            )
                            .changed();
                        ui.separator();

                        ui.label(RichText::new("Colors").strong());
                        ui.label("Tick a color to override the theme's default.");
                        changed |= color_override_row(
                            ui,
                            "Background",
                            &mut self.colors.bg,
                            defaults.bg_canvas,
                        );
                        changed |= color_override_row(
                            ui,
                            "Sidebar",
                            &mut self.colors.sidebar,
                            defaults.bg_sidebar,
                        );
                        changed |= color_override_row(
                            ui,
                            "Text",
                            &mut self.colors.fg,
                            defaults.fg_primary,
                        );
                        changed |= color_override_row(
                            ui,
                            "Accent",
                            &mut self.colors.accent,
                            defaults.accent,
                        );
                        changed |=
                            color_override_row(ui, "Pin", &mut self.colors.pin, defaults.pin_color);
                        changed |= color_override_row(
                            ui,
                            "Muted text",
                            &mut self.colors.fg_muted,
                            defaults.fg_muted,
                        );
                        changed |= color_override_row(
                            ui,
                            "Borders",
                            &mut self.colors.border,
                            defaults.border,
                        );
                        changed |= color_override_row(
                            ui,
                            "Danger",
                            &mut self.colors.danger,
                            defaults.danger,
                        );
                        if ui.button("Reset colors to theme").clicked() {
                            self.colors = Default::default();
                            changed = true;
                        }
                        ui.separator();

                        ui.label(RichText::new("Glow highlights").strong());
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.glow_strength, 0.0..=1.0)
                                    .text("Glow strength"),
                            )
                            .changed();
                        ui.horizontal(|ui| {
                            ui.label("Default glow color");
                            changed |= ui.color_edit_button_srgb(&mut self.highlight_rgb).changed();
                        });
                        ui.separator();

                        changed |= ui
                            .checkbox(&mut self.show_formatting_bar, "Show formatting bar")
                            .changed();
                        changed |= ui
                            .checkbox(&mut self.roomy_cards, "Tall note cards")
                            .on_hover_text(
                                "Off, each note is one line — a small icon and its title — so about \
                                 twice as many fit in the list; hovering shows the date and tags. \
                                 On brings back the taller card with its date and tag row.",
                            )
                            .changed();
                        changed |= ui
                            .checkbox(
                                &mut self.show_card_previews,
                                "Show a preview line on note cards",
                            )
                            .on_hover_text(
                                "Off leaves just the title, date and tags, so the sidebar's \
                                 cards are shorter and more notes fit at once. On adds the first \
                                 lines of each note. Only the tall cards have room for it.",
                            )
                            .changed();
                        ui.separator();

                        ui.label(RichText::new("Note graph").strong());
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new("Motion")
                                    .size(11.5)
                                    .color(defaults.fg_secondary),
                            );
                            for motion in [GraphMotion::Orbit, GraphMotion::Slow, GraphMotion::Still]
                            {
                                let hint = match motion {
                                    GraphMotion::Orbit => {
                                        "Notes drift around the graph at the usual pace."
                                    }
                                    GraphMotion::Slow => {
                                        "Notes drift at a gentler pace, so the graph is calmer."
                                    }
                                    GraphMotion::Still => {
                                        "No automatic movement. Dragging empty space still turns \
                                         the graph."
                                    }
                                };
                                if ui
                                    .selectable_label(self.graph_motion == motion, motion.name())
                                    .on_hover_text(hint)
                                    .clicked()
                                {
                                    self.graph_motion = motion;
                                    changed = true;
                                }
                            }
                        });
                        ui.label(
                            RichText::new(match self.graph_motion {
                                GraphMotion::Orbit => "The graph drifts while its window is open.",
                                GraphMotion::Slow => {
                                    "A quieter drift, for a graph that should not pull the eye."
                                }
                                GraphMotion::Still => {
                                    "The graph holds its place. Drag it to turn it by hand."
                                }
                            })
                            .size(10.5)
                            .color(defaults.fg_muted),
                        );
                        ui.separator();

                        ui.label(RichText::new("Notes folder").strong());
                        let notes_dir = self.storage.root_dir().display().to_string();
                        ui.add(
                            egui::Label::new(
                                RichText::new(notes_dir.clone())
                                    .monospace()
                                    .size(11.0)
                                    .color(defaults.fg_muted),
                            )
                            .wrap(),
                        );
                        if ui
                            .small_button(format!("{}  Copy path", icon::COPY))
                            .clicked()
                        {
                            ui.ctx().copy_text(notes_dir);
                            self.status_message = Some("Copied notes folder path".to_string());
                        }

                        ui.separator();

                        ui.label(RichText::new("Shortcuts").strong());
                        egui::Grid::new("elora_settings_shortcuts")
                            .num_columns(2)
                            .spacing([14.0, 4.0])
                            .show(ui, |ui| {
                                for (keys, action) in [
                                    ("Right-click selection", "Format / glow highlight"),
                                    ("Ctrl+B", "Bold"),
                                    ("Ctrl+Shift+H", "Glow highlight"),
                                    ("Tab / Shift+Tab", "Indent / unindent"),
                                    ("Enter", "Keep list & indentation"),
                                    ("Shift+Enter", "Stay inside quote / item"),
                                    ("Ctrl+F", "Find in the open note"),
                                    ("Ctrl+Shift+F", "Search all notes"),
                                    ("Ctrl+K", "Command palette & quick switcher"),
                                ] {
                                    ui.label(
                                        RichText::new(keys)
                                            .monospace()
                                            .size(11.0)
                                            .color(defaults.fg_secondary),
                                    );
                                    ui.label(
                                        RichText::new(action).size(11.5).color(defaults.fg_muted),
                                    );
                                    ui.end_row();
                                }
                            });
                        ui.separator();

                        ui.label(RichText::new("Shared library").strong());
                        self.shared_library_settings(ui, &defaults);
                        ui.separator();

                        ui.label(RichText::new("Updates").strong());
                        self.update_settings_row(ui, &defaults);
                        ui.separator();

                        if ui
                            .button(format!("{}  Reset all settings to defaults", icon::RESTORE))
                            .clicked()
                        {
                            let d = AppSettings::default();
                            self.theme = ThemeMode::from_name(&d.theme_name);
                            self.colors = d.colors;
                            self.font_size = d.font_size;
                            self.glow_strength = d.glow_strength;
                            self.highlight_rgb = d.highlight_rgb;
                            self.show_formatting_bar = d.show_formatting_bar;
                            self.show_card_previews = d.show_card_previews;
                            self.roomy_cards = d.roomy_cards;
                            self.graph_motion = d.graph_motion;
                            self.sort = d.sort;
                            self.peer_color = d.peer_color;
                            self.collab.set_color(self.peer_color);
                            changed = true;
                        }
                    });
            });

        if !open {
            self.show_settings = false;
        }
        if changed {
            self.apply_look(ctx);
            self.persist_settings();
        }
    }

    /// The shared library section of Settings: where the server is and whether we are on it.
    pub(super) fn shared_library_settings(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let connected = self.collab.is_connected();
        let state_line = match self.collab.state() {
            ConnectionState::Offline => {
                if self.collab.is_active() {
                    "Reconnecting…".to_string()
                } else {
                    "Not connected".to_string()
                }
            }
            ConnectionState::Connecting => "Connecting…".to_string(),
            ConnectionState::Online { server, you } => format!("Connected to {server} as {you}"),
        };
        ui.label(RichText::new(state_line).size(11.5).color(if connected {
            p.accent
        } else {
            p.fg_secondary
        }));
        // Why it did not connect, kept in view instead of only flashing by in the toast: the
        // usual answers are a port that is not forwarded and a firewall that drops it.
        if !connected {
            if let Some(problem) = self.collab.last_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{} {problem}", icon::ALERT_TRIANGLE))
                            .size(11.0)
                            .color(p.fg_muted),
                    )
                    .wrap(),
                );
            }
        }
        let mut watchers: Vec<&str> = self
            .notes
            .iter()
            .filter(|n| self.is_remote(&n.id))
            .flat_map(|n| self.collab.watchers(&n.id).iter().map(|u| u.name.as_str()))
            .collect();
        watchers.sort_unstable();
        watchers.dedup();
        if !watchers.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} In a note right now: {}",
                    icon::USERS,
                    watchers.join(", ")
                ))
                .size(11.0)
                .color(p.fg_muted),
            );
        }
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Address");
            ui.add(
                TextEdit::singleline(&mut self.server_form.host)
                    .hint_text("192.168.1.20 or notes.example.com")
                    .desired_width(210.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Port");
            let mut port = self.server_form.port.to_string();
            if ui
                .add(TextEdit::singleline(&mut port).desired_width(70.0))
                .changed()
            {
                if let Ok(parsed) = port.trim().parse::<u16>() {
                    self.server_form.port = parsed;
                }
            }
            ui.label("Password");
            ui.add(
                TextEdit::singleline(&mut self.server_form.password)
                    .password(true)
                    .desired_width(140.0),
            );
        });
        // The name is public: the server and everyone on it see it. The identifier is the
        // secret behind it, which only the server reads, so nobody can be somebody else just by
        // typing their name. The same identifier on another computer is the same person.
        let mut name_changed = false;
        let mut name_committed = false;
        let mut identifier_changed = false;
        let mut identifier_committed = false;
        ui.horizontal(|ui| {
            ui.label("Your name");
            let account = crate::net::os_username();
            let typed = ui
                .add(
                    TextEdit::singleline(&mut self.display_name)
                        .hint_text(&account)
                        .desired_width(170.0),
                )
                .on_hover_text(
                    "The name others see next to your caret, and the one they add to a note to \
                     share it with you. It has to be free on this server: the first person to use \
                     a name keeps it.",
                );
            name_changed = typed.changed();
            name_committed = typed.lost_focus();
        });
        // The identifier is a key, not a word: it is made once by the app, copied to another
        // computer to be the same person there, and never edited for fun. There is no "make a
        // new one" button because the server keeps a name bound to the key that claimed it —
        // a fresh key would be a stranger, locked out of the name and the notes behind it.
        ui.horizontal(|ui| {
            ui.label("Identifier");
            let typed = ui
                .add(
                    TextEdit::singleline(&mut self.identifier)
                        .password(true)
                        .desired_width(170.0),
                )
                .on_hover_text(
                    "The secret behind your name. Only the server reads it, to know it is really \
                     you; nobody else ever sees it, so nobody can connect as you. It is how your \
                     notes follow you to another computer: copy this value into the same box \
                     there. Treat it like a password.",
                );
            let long_enough = self.identifier.trim().len() >= MIN_IDENTIFIER;
            identifier_changed = typed.changed() && long_enough;
            // A pasted key only takes effect when the box is left, and only if it is a different
            // one from the connection already open: clicking Copy must not re-key.
            identifier_committed =
                typed.lost_focus() && long_enough && self.identifier != self.sent_identifier;
            if ui
                .small_button("Copy")
                .on_hover_text("Copy this identifier, to paste into EloraNotes on another computer")
                .clicked()
            {
                ui.ctx().copy_text(self.identifier.clone());
                self.status_message = Some(
                    "Identifier copied; paste it into EloraNotes on your other computer"
                        .to_string(),
                );
            }
        });
        if self.identifier.trim().len() < MIN_IDENTIFIER {
            ui.label(
                RichText::new(
                    "This identifier is too short to be a key: it needs at least 16 characters. \
                     Paste the one from your other computer here.",
                )
                .size(10.5)
                .color(p.danger),
            );
        }
        // The public half of the identifier: what a note records as its owner or as somebody it
        // was shared with. It cannot be turned back into the identifier, which is why it is safe
        // to show — and it is what `elora-server adopt` takes, so it needs to be copyable.
        let public_key = crate::people::public_key(self.identifier.trim());
        ui.horizontal(|ui| {
            ui.label("Your key");
            ui.label(
                RichText::new(crate::net::short_fingerprint(&public_key))
                    .size(10.5)
                    .color(p.fg_muted),
            )
            .on_hover_text(
                "The public half of your identifier. Notes record it, not your name, so they \
                 follow you even after you rename yourself. Give it to whoever runs the server \
                 when older notes have to be handed over.",
            );
            if ui
                .small_button("Copy")
                .on_hover_text("Copy your full key")
                .clicked()
            {
                ui.ctx().copy_text(public_key.clone());
                self.status_message = Some("Key copied".to_string());
            }
        });
        // How this person looks in somebody else's note: the colour of their caret and of the
        // name beside it. Nothing on this screen changes, so the hint says so in words.
        let mut color_changed = false;
        ui.horizontal(|ui| {
            ui.label("Your colour");
            let automatic = crate::collab::default_peer_color(&self.server_display_name());
            let mut chosen = self.peer_color.unwrap_or(automatic);
            if ui
                .color_edit_button_srgb(&mut chosen)
                .on_hover_text(
                    "The colour others see your name and caret in while a note is shared",
                )
                .changed()
            {
                self.peer_color = Some(chosen);
                color_changed = true;
            }
            if self.peer_color.is_some()
                && ui
                    .small_button("Automatic")
                    .on_hover_text("Go back to the colour picked from your name")
                    .clicked()
            {
                self.peer_color = None;
                color_changed = true;
            }
        });
        ui.label(
            RichText::new(
                "Your colour in a shared note: the name beside your caret and the caret itself, \
                 as it looks on everybody else's screen. Nothing here changes.",
            )
            .size(10.5)
            .color(p.fg_muted),
        );
        if color_changed {
            // Applies to the people already in a note with you, and is remembered for the next
            // connection, so it never has to be picked again.
            self.collab.set_color(self.peer_color);
            self.persist_settings();
        }
        if name_changed || identifier_changed {
            self.persist_settings();
        }
        if (name_committed || identifier_committed) && self.collab.is_connected() {
            // Which notes the server shows depends on who it thinks this client is, so this is a
            // new conversation with the server rather than a rename while it is open.
            self.disconnect_server();
            self.connect_server();
        }

        ui.horizontal(|ui| {
            if connected {
                if ui
                    .button(format!("{}  Disconnect", icon::WIFI_OFF))
                    .clicked()
                {
                    self.disconnect_server();
                }
            } else if ui.button(format!("{}  Connect", icon::CLOUD)).clicked() {
                self.connect_server();
            }
            if self.server.fingerprint.is_some()
                && ui
                    .small_button(format!("{}  Forget this server's key", icon::RESTORE))
                    .on_hover_text("Ask for the certificate code again on the next connection")
                    .clicked()
            {
                self.server.fingerprint = None;
                self.server_form.fingerprint = None;
                self.persist_settings();
            }
        });

        if self.server.fingerprint.is_some() {
            let short = crate::net::short_fingerprint(
                self.server.fingerprint.as_deref().unwrap_or_default(),
            );
            ui.label(
                RichText::new(format!("Trusted key {short}"))
                    .size(10.5)
                    .color(p.fg_muted),
            );
        }
        ui.label(
            RichText::new(
                "The server's notes appear in the sidebar under its name. A note can be closed to \
                 everybody but the people you name on it; they add your name, and the server \
                 knows it is you by the identifier only it can see.",
            )
            .size(10.5)
            .color(p.fg_muted),
        );
    }
}
