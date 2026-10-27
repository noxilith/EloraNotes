//! The note graph and the report of links that lead nowhere.
use super::*;
use egui::ecolor::Hsva;
use std::collections::{BTreeMap, HashMap};

/// One live note in the graph view. `links` holds indices of the notes it links to.
pub(super) struct GraphNode {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) links: Vec<usize>,
}

/// How far apart two linked notes like to sit, in graph units.
const LINK_LENGTH: f32 = 34.0;
/// How many rounds the layout takes to settle. It runs a few at a time, so the notes are seen
/// finding their places instead of the window freezing while they do.
const SETTLE_STEPS: usize = 260;
/// Room kept between two notes in the ring of unlinked ones.
const RING_SPACING: f32 = 15.0;
const MIN_ZOOM: f32 = 0.03;
const MAX_ZOOM: f32 = 12.0;

/// The graph: linked notes pulled into clusters, one colour per cluster, and the notes nothing
/// links to set out on an arc beneath them. Positions are in graph units; `zoom` and `focus` say
/// which part of that space the canvas shows.
pub(super) struct GraphView {
    pub(super) nodes: Vec<GraphNode>,
    /// Each pair of linked notes once, however many ways the links run.
    pub(super) edges: Vec<(usize, usize)>,
    pub(super) degree: Vec<usize>,
    adjacent: Vec<Vec<usize>>,
    /// The colour group of a linked note: the cluster it belongs to, largest cluster first.
    pub(super) group: Vec<Option<usize>>,
    pub(super) connected: Vec<usize>,
    pub(super) orphans: Vec<usize>,
    /// Where the linked notes are; a note nothing links to is placed by `world_positions`.
    pos: Vec<Vec2>,
    steps: usize,
    pub(super) zoom: f32,
    /// The graph point shown in the middle of the canvas.
    pub(super) focus: Vec2,
    /// Keep the whole graph in view until the user zooms or drags it.
    pub(super) auto_fit: bool,
    /// Phase of the slow sway of the unlinked notes.
    pub(super) sway: f32,
    /// Whether a note has been given a place yet.
    placed: Vec<bool>,
    /// Links read from shared notes, waiting to be folded in with the next `flush`.
    staged: Vec<(String, Vec<String>)>,
    last_flush: Instant,
    /// Shared notes read so far, and how many there are to read; `None` when nothing is loading.
    pub(super) loading: Option<(usize, usize)>,
    /// Why the shared notes could not be read, when they could not.
    pub(super) load_error: Option<String>,
}

/// A pseudo-random number in -1..1 that is the same every time for the same inputs, so a graph
/// opens looking the same twice.
fn unit(index: usize, salt: u32) -> f32 {
    let mut x = (index as u32).wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

impl GraphView {
    pub(super) fn new(nodes: Vec<GraphNode>) -> Self {
        let count = nodes.len();
        let mut view = Self {
            nodes,
            edges: Vec::new(),
            degree: vec![0; count],
            adjacent: vec![Vec::new(); count],
            group: vec![None; count],
            connected: Vec::new(),
            orphans: Vec::new(),
            pos: vec![Vec2::ZERO; count],
            steps: 0,
            zoom: 1.0,
            focus: Vec2::ZERO,
            auto_fit: true,
            sway: 0.0,
            placed: vec![false; count],
            staged: Vec::new(),
            last_flush: Instant::now(),
            loading: None,
            load_error: None,
        };
        view.rebuild();
        view
    }

    /// Works out the links, the clusters and the places of notes not yet placed from the notes'
    /// `links`. Notes already placed stay where they are, so links arriving later grow the graph
    /// instead of reshuffling it.
    fn rebuild(&mut self) {
        let count = self.nodes.len();
        let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
        for (i, node) in self.nodes.iter().enumerate() {
            for &j in &node.links {
                if j < count && j != i {
                    pairs.insert(if i < j { (i, j) } else { (j, i) });
                }
            }
        }
        self.edges = pairs.into_iter().collect();
        let mut adjacent = vec![Vec::new(); count];
        for &(a, b) in &self.edges {
            adjacent[a].push(b);
            adjacent[b].push(a);
        }
        self.degree = adjacent.iter().map(Vec::len).collect();
        self.connected = (0..count).filter(|&i| self.degree[i] > 0).collect();
        let nodes = &self.nodes;
        let mut orphans: Vec<usize> = (0..count).filter(|&i| self.degree[i] == 0).collect();
        orphans.sort_by(|&a, &b| nodes[a].label.cmp(&nodes[b].label).then(a.cmp(&b)));
        self.orphans = orphans;

        self.group = Self::clusters(&adjacent, &self.connected, count);
        let groups = self.group.iter().flatten().max().map_or(0, |g| g + 1);
        let mut members = vec![0usize; groups];
        for g in self.group.iter().flatten() {
            members[*g] += 1;
        }
        // A note with no place yet starts beside a linked note that has one; failing that, each
        // cluster starts around its own point on a circle, so clusters begin apart and the layout
        // only has to untangle each one.
        let spread = LINK_LENGTH * (self.connected.len() as f32).sqrt() * 0.55;
        for &i in &self.connected.clone() {
            if self.placed[i] {
                continue;
            }
            let near: Vec<Vec2> = adjacent[i]
                .iter()
                .filter(|&&j| self.placed[j])
                .map(|&j| self.pos[j])
                .collect();
            self.pos[i] = if near.is_empty() {
                let g = self.group[i].unwrap_or(0);
                let around = if groups > 1 {
                    let angle = TAU * g as f32 / groups as f32;
                    Vec2::new(angle.cos(), angle.sin()) * spread
                } else {
                    Vec2::ZERO
                };
                let reach = LINK_LENGTH * (members[g] as f32).sqrt() * 0.6 + 6.0;
                around + Vec2::new(unit(i, 1), unit(i, 2)) * reach
            } else {
                let mean = near.iter().fold(Vec2::ZERO, |sum, p| sum + *p) / near.len() as f32;
                mean + Vec2::new(unit(i, 3), unit(i, 4)) * LINK_LENGTH
            };
            self.placed[i] = true;
        }
        self.adjacent = adjacent;
    }

    /// Notes the shared library holds link to others in their text, which is only read from the
    /// server a note at a time. What was read waits here until `flush` folds it in.
    pub(super) fn stage_links(&mut self, id: String, targets: Vec<String>) {
        self.staged.push((id, targets));
    }

    /// Folds staged links into the graph, at most a few times a second unless `force` is set,
    /// and warms the layout up a little so the new notes find their places.
    pub(super) fn flush(&mut self, force: bool) -> bool {
        if self.staged.is_empty()
            || (!force && self.last_flush.elapsed() < Duration::from_millis(300))
        {
            return false;
        }
        self.last_flush = Instant::now();
        let mut changed = false;
        for (id, targets) in std::mem::take(&mut self.staged) {
            let Some(from) = self.nodes.iter().position(|n| n.id == id) else {
                continue;
            };
            for target in targets {
                let found = self
                    .nodes
                    .iter()
                    .position(|n| n.label.eq_ignore_ascii_case(&target));
                if let Some(to) = found {
                    if to != from && !self.nodes[from].links.contains(&to) {
                        self.nodes[from].links.push(to);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.rebuild();
            self.steps = self.steps.min(SETTLE_STEPS / 3);
        }
        changed
    }

    /// Groups the linked notes by label propagation: every note takes the label most of its
    /// neighbours have, until nothing changes. Densely linked notes end up sharing one.
    fn clusters(adjacent: &[Vec<usize>], connected: &[usize], count: usize) -> Vec<Option<usize>> {
        let mut label: Vec<usize> = (0..count).collect();
        for round in 0..16_u32 {
            let mut changed = false;
            // Every round visits the notes in a different (but repeatable) order and breaks ties
            // the same way: always taking the smallest label would let one label flood a whole
            // graph through a single link between two topics.
            let mut order = connected.to_vec();
            order.sort_by(|&a, &b| {
                unit(a, round)
                    .partial_cmp(&unit(b, round))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for i in order {
                let mut votes: BTreeMap<usize, usize> = BTreeMap::new();
                for &j in &adjacent[i] {
                    *votes.entry(label[j]).or_default() += 1;
                }
                let top = votes.values().copied().max().unwrap_or(0);
                // A note that is already in one of the leading groups stays there.
                if votes.get(&label[i]).copied() == Some(top) {
                    continue;
                }
                let best = votes
                    .iter()
                    .filter(|(_, &n)| n == top)
                    .map(|(&candidate, _)| candidate)
                    .min_by(|&a, &b| {
                        unit(a, round ^ (i as u32).wrapping_mul(31))
                            .partial_cmp(&unit(b, round ^ (i as u32).wrapping_mul(31)))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                if let Some(best) = best {
                    label[i] = best;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Self::merge_fragments(adjacent, connected, &mut label);
        let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
        for &i in connected {
            *sizes.entry(label[i]).or_default() += 1;
        }
        let mut ranked: Vec<(usize, usize)> = sizes.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let rank: HashMap<usize, usize> = ranked
            .iter()
            .enumerate()
            .map(|(rank, (label, _))| (*label, rank))
            .collect();
        let mut group = vec![None; count];
        for &i in connected {
            group[i] = rank.get(&label[i]).copied();
        }
        group
    }

    /// Folds the smallest groups into the group they are linked to most, until every group is a
    /// real cluster and there are not more colours than the eye can tell apart. A group with no
    /// link to any other (a small island of its own) keeps its label.
    fn merge_fragments(adjacent: &[Vec<usize>], connected: &[usize], label: &mut [usize]) {
        const MIN_SIZE: usize = 4;
        const MAX_GROUPS: usize = 14;
        let mut stuck: BTreeSet<usize> = BTreeSet::new();
        loop {
            let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
            for &i in connected {
                *sizes.entry(label[i]).or_default() += 1;
            }
            let smallest = sizes
                .iter()
                .filter(|(group, _)| !stuck.contains(group))
                .min_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)))
                .map(|(&group, &size)| (group, size));
            let Some((group, size)) = smallest else {
                return;
            };
            if size >= MIN_SIZE && sizes.len() <= MAX_GROUPS {
                return;
            }
            let mut toward: BTreeMap<usize, usize> = BTreeMap::new();
            for &i in connected.iter().filter(|&&i| label[i] == group) {
                for &j in &adjacent[i] {
                    if label[j] != group {
                        *toward.entry(label[j]).or_default() += 1;
                    }
                }
            }
            let target = toward
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(&other, _)| other);
            match target {
                Some(other) => {
                    for &i in connected {
                        if label[i] == group {
                            label[i] = other;
                        }
                    }
                }
                None => {
                    stuck.insert(group);
                }
            }
        }
    }

    pub(super) fn settled(&self) -> bool {
        self.steps >= SETTLE_STEPS
    }

    /// One round of the layout: linked notes pull together, every note pushes the ones near it
    /// away, and a faint pull to the middle keeps separate clusters from drifting off.
    fn step(&mut self) {
        let temperature =
            LINK_LENGTH * (1.0 - self.steps as f32 / SETTLE_STEPS as f32).powf(1.5) + 0.25;
        let mut push = vec![Vec2::ZERO; self.pos.len()];

        let cell = LINK_LENGTH * 3.0;
        let key = |p: Vec2| ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32);
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for &i in &self.connected {
            grid.entry(key(self.pos[i])).or_default().push(i);
        }
        for &i in &self.connected {
            let (cx, cy) = key(self.pos[i]);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(near) = grid.get(&(cx + dx, cy + dy)) else {
                        continue;
                    };
                    for &j in near {
                        if j <= i {
                            continue;
                        }
                        let apart = self.pos[i] - self.pos[j];
                        let distance = apart.length().max(0.01);
                        if distance < cell {
                            let force = apart / distance * (LINK_LENGTH * LINK_LENGTH / distance);
                            push[i] += force;
                            push[j] -= force;
                        }
                    }
                }
            }
        }
        for &(a, b) in &self.edges {
            let along = self.pos[b] - self.pos[a];
            let distance = along.length().max(0.01);
            let force = along / distance * (distance * distance / LINK_LENGTH);
            push[a] += force;
            push[b] -= force;
        }
        for &i in &self.connected {
            push[i] -= self.pos[i] * 0.05;
            let length = push[i].length().max(0.01);
            self.pos[i] += push[i] / length * length.min(temperature);
        }
        self.steps += 1;
    }

    /// Runs rounds of the layout until it has settled or the time is up.
    pub(super) fn settle(&mut self, budget: Duration) {
        let started = Instant::now();
        while !self.settled() && started.elapsed() < budget {
            self.step();
        }
    }

    /// Centre and radius of the linked notes.
    fn core(&self) -> (Vec2, f32) {
        if self.connected.is_empty() {
            return (Vec2::ZERO, 0.0);
        }
        let (mut low, mut high) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for &i in &self.connected {
            low = low.min(self.pos[i]);
            high = high.max(self.pos[i]);
        }
        let center = (low + high) * 0.5;
        let radius = self
            .connected
            .iter()
            .map(|&i| (self.pos[i] - center).length())
            .fold(0.0_f32, f32::max);
        (center, radius)
    }

    /// Where every note is now. The notes nothing links to sit on an arc below the linked ones,
    /// wide enough to open round them; when there are too many for one arc they fill rows outside
    /// it. With nothing linked at all they form a ring.
    pub(super) fn world_positions(&self) -> Vec<Vec2> {
        let mut world = self.pos.clone();
        let (center, radius) = self.core();
        let span = if self.connected.is_empty() {
            TAU
        } else {
            TAU * 0.68
        };
        let mut placed = 0;
        let mut row = 0;
        while placed < self.orphans.len() {
            let ring = (radius * 1.12 + 30.0 + row as f32 * RING_SPACING * 1.4).max(46.0);
            let room = ((ring * span / RING_SPACING).floor() as usize).max(1);
            let take = room.min(self.orphans.len() - placed);
            for k in 0..take {
                let sway = 0.035 * (self.sway + row as f32).sin();
                let angle = TAU / 4.0 - span / 2.0 + span * (k as f32 + 0.5) / take as f32 + sway;
                world[self.orphans[placed + k]] =
                    center + Vec2::new(angle.cos(), angle.sin()) * ring;
            }
            placed += take;
            row += 1;
        }
        world
    }

    /// The zoom and focus that show every note in a canvas of this size.
    pub(super) fn fit_target(world: &[Vec2], canvas: Vec2) -> (f32, Vec2) {
        if world.is_empty() {
            return (1.0, Vec2::ZERO);
        }
        let (mut low, mut high) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for &p in world {
            low = low.min(p);
            high = high.max(p);
        }
        let size = (high - low) + Vec2::splat(70.0);
        let zoom = (canvas.x / size.x)
            .min(canvas.y / size.y)
            .clamp(MIN_ZOOM, 6.0);
        (zoom, (low + high) * 0.5)
    }
}

/// Zooms by `factor` while the graph point under `pointer` stays under it, so scrolling zooms
/// into what the pointer is on instead of into the middle. Returns the new zoom and focus.
pub(super) fn zoom_about(
    zoom: f32,
    focus: Vec2,
    canvas_center: Pos2,
    pointer: Pos2,
    factor: f32,
) -> (f32, Vec2) {
    let new_zoom = (zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let under = focus + (pointer - canvas_center) / zoom;
    (new_zoom, under - (pointer - canvas_center) / new_zoom)
}

/// The colour of one cluster. Golden-ratio steps round the hue wheel keep neighbouring clusters
/// apart however many there are.
pub(super) fn group_color(group: usize) -> Color32 {
    let hue = (group as f32 * 0.618_034).fract();
    Hsva::new(hue, 0.5, 1.0, 1.0).into()
}

/// Keeps a note's label short enough to sit beside its dot.
pub(super) fn graph_label(label: &str) -> String {
    const LIMIT: usize = 24;
    let mut chars = label.chars();
    let mut short: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    short
}

/// What the thread that reads shared notes for the graph reports back.
pub(super) enum GraphLoad {
    /// The links found in one shared note's text, and the version of the note they were read from.
    Links {
        id: String,
        stamp: chrono::DateTime<Utc>,
        targets: Vec<String>,
    },
    /// One note could not be read; the rest go on.
    Skipped,
    /// The library cannot be read at all.
    Failed(String),
    Done,
}

/// Reads the text of shared notes one at a time, on a connection of its own, and reports the
/// `[[links]]` in each. A shared note's text is only on the server until the note is opened, so
/// without this the graph would know nothing about how shared notes link to each other. Stops
/// early when the window is closed, and gives up when the library cannot be reached.
pub(super) fn load_shared_links(
    settings: AppSettings,
    todo: Vec<(String, chrono::DateTime<Utc>)>,
    tx: std::sync::mpsc::Sender<GraphLoad>,
) {
    let shared = crate::mcp_shared::Shared::new();
    let mut failures = 0;
    for (id, stamp) in todo {
        let sent = match shared.read_body(&settings, &id) {
            Ok(body) => {
                failures = 0;
                tx.send(GraphLoad::Links {
                    id,
                    stamp,
                    targets: extract_wiki_links(&body),
                })
            }
            Err(why) => {
                failures += 1;
                if failures >= 3 {
                    let _ = tx.send(GraphLoad::Failed(why));
                    return;
                }
                tx.send(GraphLoad::Skipped)
            }
        };
        if sent.is_err() {
            return;
        }
    }
    let _ = tx.send(GraphLoad::Done);
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

    /// Opens the note graph, or closes it when it is already open. Builds the links once on open;
    /// the links inside shared notes follow as they are read from the server.
    pub(super) fn toggle_graph(&mut self) {
        if self.graph.is_some() {
            self.graph = None;
            self.graph_loader = None;
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
        let resolve = |target: &str| {
            live.iter().position(|n| {
                n.title.eq_ignore_ascii_case(target)
                    || n.display_title().eq_ignore_ascii_case(target)
            })
        };
        // A shared note's text is on the server until the note is opened. What was read before
        // is used again while the note is unchanged; the rest is read in the background.
        let mut todo: Vec<(String, chrono::DateTime<Utc>)> = Vec::new();
        for (i, note) in live.iter().enumerate() {
            let targets = if self.remote_ids.contains(&note.id) && note.body.trim().is_empty() {
                match self.graph_links.get(&note.id) {
                    Some((stamp, targets)) if *stamp == note.updated_at => targets.clone(),
                    _ => {
                        todo.push((note.id.clone(), note.updated_at));
                        Vec::new()
                    }
                }
            } else {
                extract_wiki_links(&note.body)
            };
            for target in targets {
                if let Some(j) = resolve(&target) {
                    if j != i && !nodes[i].links.contains(&j) {
                        nodes[i].links.push(j);
                    }
                }
            }
        }
        let mut view = GraphView::new(nodes);
        self.graph_loader = None;
        if !todo.is_empty() && self.server.is_configured() {
            view.loading = Some((0, todo.len()));
            let settings = AppSettings {
                server: self.server.clone(),
                display_name: self.display_name.clone(),
                identifier: self.identifier.clone(),
                ..AppSettings::default()
            };
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || load_shared_links(settings, todo, tx));
            self.graph_loader = Some(rx);
        }
        self.graph = Some(view);
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

    /// The graph: notes that link to each other pulled into coloured clusters, and the notes
    /// nothing links to set out on an arc below them. Scrolling or pinching zooms into the
    /// pointer, dragging pans, double-clicking (or Fit) shows everything again, hovering lights a
    /// note and the notes it links to, and a click opens it.
    pub(super) fn graph_window(&mut self, ctx: &egui::Context) {
        let motion = self.graph_motion;
        let p = self.theme.palette_with(&self.colors, self.glow_strength);
        // What the thread reading shared notes has found since the last frame.
        let mut arrived: Vec<GraphLoad> = Vec::new();
        if let Some(rx) = &self.graph_loader {
            while let Ok(message) = rx.try_recv() {
                arrived.push(message);
            }
        }
        let mut loader_over = false;
        let mut finishing = false;
        let Some(view) = self.graph.as_mut() else {
            return;
        };
        for message in arrived {
            match message {
                GraphLoad::Links { id, stamp, targets } => {
                    self.graph_links
                        .insert(id.clone(), (stamp, targets.clone()));
                    view.stage_links(id, targets);
                    if let Some((done, _)) = view.loading.as_mut() {
                        *done += 1;
                    }
                }
                GraphLoad::Skipped => {
                    if let Some((done, _)) = view.loading.as_mut() {
                        *done += 1;
                    }
                }
                GraphLoad::Failed(why) => {
                    view.load_error = Some(why);
                    view.loading = None;
                    loader_over = true;
                    finishing = true;
                }
                GraphLoad::Done => {
                    view.loading = None;
                    loader_over = true;
                    finishing = true;
                }
            }
        }
        view.flush(finishing);
        if loader_over {
            self.graph_loader = None;
        }
        let fade = |color: Color32, alpha: u8| {
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
        };
        let mut open = true;
        let mut clicked: Option<String> = None;
        let mut animating = false;

        egui::Window::new("Note Graph")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([820.0, 640.0])
            .show(ctx, |ui| {
                let mut zoom_step: Option<f32> = None;
                let mut fit = false;
                ui.horizontal(|ui| {
                    if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                        zoom_step = Some(0.8);
                    }
                    if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                        zoom_step = Some(1.25);
                    }
                    if ui.small_button("Fit").on_hover_text("Show every note").clicked() {
                        fit = true;
                    }
                    let linked = view.connected.len();
                    ui.label(
                        RichText::new(format!(
                            "{} notes · {} linked · {} on their own  —  scroll to zoom, drag to move",
                            view.nodes.len(),
                            linked,
                            view.orphans.len()
                        ))
                        .size(11.0)
                        .color(p.fg_muted),
                    );
                    if let Some((done, total)) = view.loading {
                        ui.label(
                            RichText::new(format!("Reading shared notes {done}/{total}…"))
                                .size(11.0)
                                .color(p.accent),
                        );
                    } else if let Some(why) = &view.load_error {
                        ui.label(
                            RichText::new(format!("Could not read the shared notes: {why}"))
                                .size(11.0)
                                .color(p.danger),
                        );
                    }
                });
                let size = ui.available_size().max(Vec2::splat(260.0));
                let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(6), p.bg_canvas);

                if view.nodes.is_empty() {
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "No notes yet",
                        FontId::proportional(14.0),
                        p.fg_muted,
                    );
                    return;
                }

                // The layout settles a few rounds a frame, so the clusters are seen forming. A
                // still graph skips the show and is laid out at once.
                let dt = ctx.input(|i| i.stable_dt).min(0.05);
                view.settle(Duration::from_millis(if motion == GraphMotion::Still {
                    300
                } else {
                    7
                }));
                view.sway += dt * motion.speed() * 4.0;
                let world = view.world_positions();
                let center = rect.center();

                // Zoom, pan and fit.
                if fit || response.double_clicked() {
                    view.auto_fit = true;
                }
                if response.dragged() {
                    view.auto_fit = false;
                    view.focus -= response.drag_delta() / view.zoom;
                }
                let pointer = response.hover_pos();
                if response.hovered() {
                    let (scroll, pinch) =
                        ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                    let factor = (scroll * 0.0016).exp() * pinch;
                    if (factor - 1.0).abs() > 1e-4 {
                        view.auto_fit = false;
                        let at = pointer.unwrap_or(center);
                        (view.zoom, view.focus) =
                            zoom_about(view.zoom, view.focus, center, at, factor);
                    }
                }
                if let Some(step) = zoom_step {
                    view.auto_fit = false;
                    (view.zoom, view.focus) =
                        zoom_about(view.zoom, view.focus, center, center, step);
                }
                if view.auto_fit {
                    let (zoom, focus) = GraphView::fit_target(&world, rect.size());
                    let k = if view.settled() { 0.3 } else { 1.0 };
                    view.zoom += (zoom - view.zoom) * k;
                    view.focus += (focus - view.focus) * k;
                    if (zoom - view.zoom).abs() > 0.002 * zoom {
                        animating = true;
                    }
                }
                let zoom = view.zoom;
                let to_screen = |w: Vec2| center + (w - view.focus) * zoom;
                let screen: Vec<Pos2> = world.iter().map(|&w| to_screen(w)).collect();
                let nodes = &view.nodes;
                let size_scale = zoom.sqrt().clamp(0.7, 2.0);
                let radius_of = |i: usize| {
                    (2.2 + (view.degree[i] as f32).sqrt() * 1.15).min(10.0) * size_scale
                };

                let hover = pointer.and_then(|pos| {
                    (0..nodes.len())
                        .filter(|&i| screen[i].distance(pos) <= radius_of(i) + 5.0)
                        .min_by(|&a, &b| {
                            screen[a]
                                .distance(pos)
                                .partial_cmp(&screen[b].distance(pos))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                });
                let linked_to_hover = |i: usize| {
                    hover.is_some_and(|h| h != i && view.edges.binary_search(&(h.min(i), h.max(i))).is_ok())
                };
                let color_of = |i: usize| match view.group[i] {
                    Some(g) => group_color(g),
                    None => p.fg_secondary,
                };

                // Lines first, so the notes sit on top. Those of the hovered note light up and
                // everything else steps back.
                let visible = rect.expand(40.0);
                for &(a, b) in &view.edges {
                    if !visible.intersects(Rect::from_two_pos(screen[a], screen[b])) {
                        continue;
                    }
                    let touches = hover.is_some_and(|h| h == a || h == b);
                    if touches {
                        continue;
                    }
                    let same = view.group[a] == view.group[b];
                    let base = if same { color_of(a) } else { p.fg_muted };
                    let alpha = if hover.is_some() { 22 } else if same { 80 } else { 45 };
                    painter.line_segment(
                        [screen[a], screen[b]],
                        Stroke::new(0.8_f32, fade(base, alpha)),
                    );
                }
                if let Some(h) = hover {
                    for &other in &view.adjacent[h] {
                        painter.line_segment(
                            [screen[h], screen[other]],
                            Stroke::new(1.6_f32, fade(p.accent, 220)),
                        );
                    }
                }

                // The busiest notes glow, the way the middle of a cluster should draw the eye.
                for (i, &at) in screen.iter().enumerate() {
                    if !visible.contains(at) {
                        continue;
                    }
                    let is_hover = hover == Some(i);
                    let near = linked_to_hover(i);
                    let dim = hover.is_some() && !is_hover && !near;
                    let base = if is_hover { p.accent } else { color_of(i) };
                    let alpha = if dim { 70 } else { 255 };
                    let radius = radius_of(i);
                    if view.degree[i] >= 6 || is_hover {
                        painter.circle_filled(at, radius * 2.6, fade(base, if dim { 5 } else { 20 }));
                        painter.circle_filled(at, radius * 1.7, fade(base, if dim { 8 } else { 36 }));
                    }
                    painter.circle_filled(at, radius, fade(base, alpha));
                }

                // Names: the busiest notes first, thinning out as the picture gets smaller, and
                // never two on top of each other. Hovering names a note and the ones it links to.
                let few = nodes.len() <= 40;
                let min_degree = if few { 0 } else { (8.0 / zoom).ceil() as usize };
                let mut taken: Vec<Rect> = Vec::new();
                let mut names = 0;
                let mut draw_label = |i: usize, strong: bool, force: bool| {
                    let label = graph_label(&nodes[i].label);
                    let font = FontId::proportional(if strong { 12.0 } else { 10.5 });
                    let color = if strong {
                        p.fg_primary
                    } else {
                        fade(p.fg_secondary, 225)
                    };
                    let galley = ui.fonts(|fonts| fonts.layout_no_wrap(label, font, color));
                    let anchor = screen[i] + Vec2::new(radius_of(i) + 4.0, 0.0);
                    let mut box_rect = Align2::LEFT_CENTER.anchor_size(anchor, galley.size());
                    if box_rect.right() > rect.right() - 4.0 {
                        box_rect = Align2::RIGHT_CENTER
                            .anchor_size(screen[i] - Vec2::new(radius_of(i) + 4.0, 0.0), galley.size());
                    }
                    let pill = box_rect.expand2(Vec2::new(3.0, 1.0));
                    if !rect.contains_rect(pill) && !force {
                        return;
                    }
                    if !force && taken.iter().any(|placed| placed.intersects(pill)) {
                        return;
                    }
                    taken.push(pill);
                    painter.rect_filled(
                        pill,
                        CornerRadius::same(3),
                        fade(if strong { p.bg_elevated } else { p.bg_canvas }, if strong { 240 } else { 190 }),
                    );
                    painter.galley(box_rect.min, galley, color);
                };
                if let Some(h) = hover {
                    draw_label(h, true, true);
                    for &other in &view.adjacent[h] {
                        draw_label(other, false, false);
                    }
                } else {
                    let mut order: Vec<usize> = (0..nodes.len())
                        .filter(|&i| {
                            visible.contains(screen[i])
                                && if view.group[i].is_some() {
                                    view.degree[i] >= min_degree
                                } else {
                                    few || zoom >= 2.5
                                }
                        })
                        .collect();
                    order.sort_by_key(|&i| std::cmp::Reverse(view.degree[i]));
                    for i in order {
                        if names >= 400 {
                            break;
                        }
                        names += 1;
                        draw_label(i, false, false);
                    }
                }

                if let Some(i) = hover {
                    response
                        .clone()
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    response.clone().on_hover_text(format!(
                        "{}\n{} link{} · click to open",
                        nodes[i].label,
                        view.degree[i],
                        if view.degree[i] == 1 { "" } else { "s" }
                    ));
                }
                if response.clicked() {
                    if let Some(i) = hover {
                        clicked = Some(nodes[i].id.clone());
                    }
                }
                if !view.settled() || view.loading.is_some() {
                    animating = true;
                }
            });

        if let Some(id) = clicked {
            self.selected_id = Some(id);
            self.graph = None;
        } else if !open {
            self.graph = None;
        }
        // Frames are asked for only while something is moving: the layout settling, the view
        // easing into place, or the slow sway. A still graph costs nothing once it has settled.
        if self.graph.is_some() {
            if animating {
                ctx.request_repaint();
            } else if motion != GraphMotion::Still {
                ctx.request_repaint_after(Duration::from_millis(33));
            }
        }
    }
}
