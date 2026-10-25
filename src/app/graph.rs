//! The note graph and the report of links that lead nowhere.
use super::*;

/// One live note in the graph view. `links` holds indices of the notes it links to.
pub(super) struct GraphNode {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) links: Vec<usize>,
}

/// Where a note sits for a single frame of the graph.
pub(super) struct GraphDot {
    pub(super) pos: Pos2,
    /// Which orbit the note travels on: 0 is the inner one.
    pub(super) ring: usize,
    /// Angle along the orbit after this frame's turn, in radians.
    pub(super) angle: f32,
    /// Radii of the note's orbit, so its trail follows the same ellipse.
    pub(super) orbit: Vec2,
    /// Share of the turn this orbit takes; outer orbits move more slowly.
    pub(super) speed: f32,
    pub(super) radius: f32,
    pub(super) degree: usize,
}

/// How many orbits the notes are spread over. A small notebook looks lost on three rings, so it
/// gets one until there are enough notes to fill more.
pub(super) fn graph_ring_count(count: usize) -> usize {
    if count <= 8 {
        1
    } else if count <= 24 {
        2
    } else {
        3
    }
}

/// Pixels from the centre to an orbit, as an ellipse so the graph does not read as a target.
pub(super) fn graph_orbit(rect: Rect, ring: usize, rings: usize) -> Vec2 {
    let fractions: &[f32] = match rings {
        1 => &[0.32],
        2 => &[0.21, 0.36],
        _ => &[0.15, 0.26, 0.37],
    };
    let short = rect.width().min(rect.height());
    let fraction = fractions.get(ring).copied().unwrap_or(0.37);
    Vec2::new(short * fraction * 1.06, short * fraction * 0.92)
}

pub(super) fn graph_node_radius(degree: usize) -> f32 {
    (4.2 + (degree as f32).sqrt() * 1.7).min(9.5)
}

/// Keeps a note's label short enough to sit beside its orbit.
pub(super) fn graph_label(label: &str) -> String {
    const LIMIT: usize = 24;
    let mut chars = label.chars();
    let mut short: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    short
}

/// Places every note for one frame. The best-connected notes take the inner orbits, where the eye
/// lands first; each orbit turns at its own speed; `spin` is the shared turn in radians, so the
/// window can animate it and a drag can push it by hand.
pub(super) fn graph_layout(nodes: &[GraphNode], rect: Rect, spin: f32) -> Vec<GraphDot> {
    let count = nodes.len();
    if count == 0 {
        return Vec::new();
    }
    let center = rect.center();
    let rings = graph_ring_count(count);

    let mut degree = vec![0usize; count];
    for (i, node) in nodes.iter().enumerate() {
        for &j in &node.links {
            degree[i] += 1;
            if j < count && j != i {
                degree[j] += 1;
            }
        }
    }

    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|&a, &b| {
        degree[b]
            .cmp(&degree[a])
            .then_with(|| nodes[a].label.cmp(&nodes[b].label))
            .then_with(|| a.cmp(&b))
    });

    let mut sizes = vec![0usize; rings];
    match rings {
        1 => sizes[0] = count,
        2 => {
            sizes[0] = ((count as f32) * 0.4).ceil() as usize;
            sizes[1] = count - sizes[0];
        }
        _ => {
            sizes[0] = ((count as f32) * 0.24).ceil() as usize;
            sizes[1] = ((count as f32) * 0.34).ceil() as usize;
            sizes[2] = count - sizes[0] - sizes[1];
        }
    }

    let mut ring_of = vec![0usize; count];
    let mut next = 0;
    for (ring, &size) in sizes.iter().enumerate() {
        for _ in 0..size {
            if next < count {
                ring_of[order[next]] = ring;
                next += 1;
            }
        }
    }

    let mut placed = vec![0usize; rings];
    let mut dots: Vec<Option<GraphDot>> = (0..count).map(|_| None).collect();
    for &i in &order {
        let ring = ring_of[i];
        let share = sizes[ring].max(1);
        let slot = placed[ring];
        placed[ring] += 1;
        // Start at the top, and give each orbit its own phase so nodes do not line up like spokes.
        let base = -TAU / 4.0 + TAU * slot as f32 / share as f32 + ring as f32 * 0.55;
        let speed = 1.0 / (1.0 + ring as f32 * 0.6);
        let angle = base + spin * speed;
        let orbit = graph_orbit(rect, ring, rings);
        dots[i] = Some(GraphDot {
            pos: center + Vec2::new(angle.cos() * orbit.x, angle.sin() * orbit.y),
            ring,
            angle,
            orbit,
            speed,
            radius: graph_node_radius(degree[i]),
            degree: degree[i],
        });
    }
    dots.into_iter()
        .map(|dot| dot.expect("every note is placed on an orbit"))
        .collect()
}

impl EloraApp {
    /// Every `[[wiki link]]` in a live note that points at no note, with the titles of the notes
    /// that link to it. The same rule the link itself follows decides whether it is broken, so
    /// the report never lists something the link would have opened.
    pub(super) fn collect_broken_links(&self) -> Vec<(String, Vec<String>)> {
        let live: Vec<&Note> = self.notes.iter().filter(|n| !n.trashed).collect();
        let mut broken: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for note in &live {
            for target in extract_wiki_links(&note.body) {
                let found = live.iter().any(|n| {
                    n.title.eq_ignore_ascii_case(&target)
                        || n.display_title().eq_ignore_ascii_case(&target)
                });
                if found {
                    continue;
                }
                let from = note.display_title();
                let entry = broken.entry(target).or_default();
                if !entry.iter().any(|title| title.eq_ignore_ascii_case(&from)) {
                    entry.push(from);
                }
            }
        }
        broken.into_iter().collect()
    }

    /// Opens the note graph, or closes it when it is already open. Builds the links once on open.
    pub(super) fn toggle_graph(&mut self) {
        if self.graph.is_some() {
            self.graph = None;
            return;
        }
        let live: Vec<&Note> = self.notes.iter().filter(|n| !n.trashed).collect();
        let mut nodes: Vec<GraphNode> = live
            .iter()
            .map(|n| GraphNode {
                id: n.id.clone(),
                label: n.display_title(),
                links: Vec::new(),
            })
            .collect();
        for (i, note) in live.iter().enumerate() {
            for target in extract_wiki_links(&note.body) {
                let found = live.iter().position(|n| {
                    n.title.eq_ignore_ascii_case(&target)
                        || n.display_title().eq_ignore_ascii_case(&target)
                });
                if let Some(j) = found {
                    if j != i && !nodes[i].links.contains(&j) {
                        nodes[i].links.push(j);
                    }
                }
            }
        }
        self.graph_angle = 0.0;
        self.graph = Some(nodes);
    }

    /// The report of links that lead nowhere: one row per missing note, with where the links
    /// were found. Clicking one creates that note and opens it, which is the fix for most of them.
    pub(super) fn broken_links_window(&mut self, ctx: &egui::Context) {
        if !self.broken_links_open {
            return;
        }
        let defaults = self.theme.palette();
        let mut open = true;
        let mut clicked: Option<String> = None;
        egui::Window::new("Links that lead nowhere")
            .id(Id::new("elora_broken_links"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(420.0)
            .show(ctx, |ui| {
                if self.broken_links.is_empty() {
                    ui.label(
                        RichText::new("Every [[link]] in your notes leads to a note that exists.")
                            .size(12.0)
                            .color(defaults.fg_secondary),
                    );
                    return;
                }
                ui.label(
                    RichText::new(format!(
                        "{} note{} linked to but not written yet. Click one to write it.",
                        self.broken_links.len(),
                        if self.broken_links.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ))
                    .size(11.5)
                    .color(defaults.fg_muted),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (target, from) in &self.broken_links {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(format!("{} [[{target}]]", icon::LINK))
                                            .size(12.0)
                                            .color(defaults.accent),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Create this note and open it")
                                .clicked()
                            {
                                clicked = Some(target.clone());
                            }
                            let mut list = String::from("linked from ");
                            list.push_str(&from.join(", "));
                            ui.label(RichText::new(list).size(10.5).color(defaults.fg_muted));
                            ui.add_space(3.0);
                        }
                    });
            });
        if let Some(target) = clicked {
            self.broken_links_open = false;
            self.open_or_create_wiki_link(&target);
        }
        if !open {
            self.broken_links_open = false;
        }
    }

    /// The graph as a small system of orbits: a soft core, a faint path for each ring, and the
    /// notes drifting along them at different speeds. The busiest notes travel inside. Hovering
    /// slows the system so a note can be clicked, dragging empty space turns it, and a click opens
    /// the note. The whole thing animates, so it asks for a frame while it is open.
    pub(super) fn graph_window(&mut self, ctx: &egui::Context) {
        let Some(nodes) = self.graph.as_ref() else {
            return;
        };
        let p = self.theme.palette_with(&self.colors, self.glow_strength);
        let fade = |color: Color32, alpha: u8| {
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
        };
        let mut open = true;
        let mut clicked: Option<String> = None;
        let mut spin = self.graph_angle;
        let motion = self.graph_motion;

        egui::Window::new("Note Graph")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 500.0])
            .show(ctx, |ui| {
                let size = ui.available_size().max(Vec2::splat(260.0));
                let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(6), p.bg_canvas);

                if nodes.is_empty() {
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "No notes yet",
                        FontId::proportional(14.0),
                        p.fg_muted,
                    );
                    return;
                }

                // The orbits keep turning while the window is open; reaching for a note slows them
                // to a crawl so it can be clicked, and dragging empty space turns the whole system.
                let pointer_over = response.hover_pos().is_some();
                let dt = ctx.input(|i| i.stable_dt).min(0.05);
                spin = (spin + dt * motion.speed() * if pointer_over { 0.18 } else { 1.0 })
                    .rem_euclid(TAU);
                if response.dragged() {
                    spin = (spin + response.drag_delta().x * 0.006).rem_euclid(TAU);
                }
                let dots = graph_layout(nodes, rect, spin);
                let center = rect.center();
                let rings = graph_ring_count(nodes.len());

                // A soft core to orbit around, and a faint path for each ring.
                painter.circle_filled(center, 38.0, fade(p.accent, 8));
                painter.circle_filled(center, 19.0, fade(p.accent, 16));
                painter.circle_stroke(center, 11.0, Stroke::new(0.8_f32, fade(p.accent, 40)));
                painter.circle_filled(center, 4.0, fade(p.accent, 150));
                for ring in 0..rings {
                    let orbit = graph_orbit(rect, ring, rings);
                    let path: Vec<Pos2> = (0..96)
                        .map(|step| {
                            let angle = TAU * step as f32 / 96.0;
                            center + Vec2::new(angle.cos() * orbit.x, angle.sin() * orbit.y)
                        })
                        .collect();
                    painter.add(egui::Shape::closed_line(
                        path,
                        Stroke::new(0.7_f32, fade(p.border, 70)),
                    ));
                }

                let hover = response.hover_pos().and_then(|pos| {
                    dots.iter()
                        .enumerate()
                        .filter(|(_, dot)| dot.pos.distance(pos) <= dot.radius + 6.0)
                        .min_by(|(_, a), (_, b)| {
                            a.pos
                                .distance(pos)
                                .partial_cmp(&b.pos.distance(pos))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|(i, _)| i)
                });

                // One line per wiki link, drawn once even when two notes link at each other.
                let mut edges: BTreeSet<(usize, usize)> = BTreeSet::new();
                for (i, node) in nodes.iter().enumerate() {
                    for &j in &node.links {
                        if j >= nodes.len() || i == j {
                            continue;
                        }
                        edges.insert(if i < j { (i, j) } else { (j, i) });
                    }
                }

                // A chord bends along its orbit instead of cutting straight across the middle.
                let edge = |a: Pos2, b: Pos2, width: f32, color: Color32| {
                    let mid = a + (b - a) * 0.5;
                    let radial = mid - center;
                    let mut side = Vec2::new(-radial.y, radial.x);
                    if side.length() < 1.0 {
                        side = Vec2::new(-(b - a).y, (b - a).x);
                    }
                    let side = side.normalized();
                    let bow = (a.distance(b) * 0.11).clamp(6.0, 36.0);
                    let c1 = a + (mid - a) * 0.5 + side * bow;
                    let c2 = b + (mid - b) * 0.5 + side * bow;
                    painter.add(egui::Shape::CubicBezier(
                        egui::epaint::CubicBezierShape::from_points_stroke(
                            [a, c1, c2, b],
                            false,
                            Color32::TRANSPARENT,
                            Stroke::new(width, color),
                        ),
                    ));
                };

                for &(a, b) in &edges {
                    if hover.is_some_and(|h| h == a || h == b) {
                        continue;
                    }
                    edge(dots[a].pos, dots[b].pos, 1.0, fade(p.border, 110));
                }
                if let Some(h) = hover {
                    for &(a, b) in &edges {
                        if h == a || h == b {
                            edge(dots[a].pos, dots[b].pos, 4.5, fade(p.accent, 26));
                            edge(dots[a].pos, dots[b].pos, 1.3, fade(p.accent, 200));
                        }
                    }
                }

                let neighbour = |i: usize| {
                    hover.is_some_and(|h| {
                        h != i && (nodes[h].links.contains(&i) || nodes[i].links.contains(&h))
                    })
                };
                for (i, dot) in dots.iter().enumerate() {
                    let is_hover = hover == Some(i);
                    // The busiest notes are the bright ones in the middle; the quiet outer ring
                    // steps back so the middle of the graph is where the eye goes.
                    let color = if is_hover || dot.ring == 0 {
                        p.accent
                    } else if neighbour(i) {
                        p.accent_soft
                    } else if dot.ring == 1 {
                        fade(p.fg_secondary, 220)
                    } else {
                        fade(p.fg_secondary, 170)
                    };
                    // Two faint ghosts behind each note carry the direction of its orbit.
                    for (step, alpha) in [(1.0_f32, 18_u8), (2.0, 9_u8)] {
                        let tail = dot.angle - 0.045 * dot.speed * step;
                        let radius = dot.radius * (1.0 - 0.3 * step);
                        painter.circle_filled(
                            center + Vec2::new(tail.cos() * dot.orbit.x, tail.sin() * dot.orbit.y),
                            radius.max(1.4),
                            fade(color, alpha),
                        );
                    }
                    if is_hover {
                        painter.circle_filled(dot.pos, dot.radius * 2.8, fade(color, 22));
                        painter.circle_filled(dot.pos, dot.radius * 1.8, fade(color, 38));
                    } else {
                        painter.circle_filled(dot.pos, dot.radius + 4.0, fade(color, 10));
                    }
                    painter.circle_filled(dot.pos, dot.radius, color);
                    painter.circle_stroke(
                        dot.pos,
                        dot.radius + 1.8,
                        Stroke::new(0.8_f32, fade(color, if is_hover { 200 } else { 55 })),
                    );
                }

                let labels_all = nodes.len() <= 40;
                let mut taken: Vec<Rect> = Vec::new();
                let mut draw_label = |i: usize, strong: bool, force: bool| {
                    let dot = &dots[i];
                    let label = graph_label(&nodes[i].label);
                    let font = FontId::proportional(if strong { 12.0 } else { 11.0 });
                    let color = if strong {
                        p.fg_primary
                    } else {
                        fade(p.fg_secondary, 225)
                    };
                    let galley = ui.fonts(|fonts| fonts.layout_no_wrap(label, font, color));
                    let outward = (dot.pos - center).normalized();
                    let anchor = dot.pos + outward * (dot.radius + 7.0);
                    let align = if outward.x.abs() > 0.45 {
                        if outward.x > 0.0 {
                            Align2::LEFT_CENTER
                        } else {
                            Align2::RIGHT_CENTER
                        }
                    } else if outward.y > 0.0 {
                        Align2::CENTER_TOP
                    } else {
                        Align2::CENTER_BOTTOM
                    };
                    let mut box_rect = align.anchor_size(anchor, galley.size());
                    let margin = 6.0;
                    let dx = if box_rect.left() < rect.left() + margin {
                        rect.left() + margin - box_rect.left()
                    } else if box_rect.right() > rect.right() - margin {
                        rect.right() - margin - box_rect.right()
                    } else {
                        0.0
                    };
                    let dy = if box_rect.top() < rect.top() + margin {
                        rect.top() + margin - box_rect.top()
                    } else if box_rect.bottom() > rect.bottom() - margin {
                        rect.bottom() - margin - box_rect.bottom()
                    } else {
                        0.0
                    };
                    box_rect = box_rect.translate(Vec2::new(dx, dy));
                    let pill = box_rect.expand2(Vec2::new(3.5, 1.5));
                    // Rings get crowded long before the graph does, so a label that would land on
                    // another one steps aside; the pointer still names every note on hover.
                    if !force && taken.iter().any(|placed| placed.intersects(pill)) {
                        return;
                    }
                    taken.push(pill);
                    let background = if strong { p.bg_elevated } else { p.bg_canvas };
                    painter.rect_filled(
                        pill,
                        CornerRadius::same(3),
                        fade(background, if strong { 240 } else { 200 }),
                    );
                    painter.galley(box_rect.min, galley, color);
                };

                // Inner orbits, and the busiest notes on each, get their names first.
                let mut labelled: Vec<usize> = (0..dots.len())
                    .filter(|&i| hover != Some(i) && (labels_all || neighbour(i)))
                    .collect();
                labelled.sort_by_key(|&i| (dots[i].ring, std::cmp::Reverse(dots[i].degree)));
                for i in labelled {
                    draw_label(i, false, false);
                }
                if let Some(i) = hover {
                    draw_label(i, true, true);
                }

                if let Some(i) = hover {
                    response
                        .clone()
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    response.clone().on_hover_text(format!(
                        "{}\n{} link{} · click to open",
                        nodes[i].label,
                        nodes[i].links.len(),
                        if nodes[i].links.len() == 1 { "" } else { "s" }
                    ));
                }
                if response.clicked() {
                    if let Some(i) = hover {
                        clicked = Some(nodes[i].id.clone());
                    }
                }
            });

        self.graph_angle = spin;
        if let Some(id) = clicked {
            self.selected_id = Some(id);
            self.graph = None;
        } else if !open {
            self.graph = None;
        }
        if self.graph.is_some() && motion != GraphMotion::Still {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}
