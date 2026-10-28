//! Where the notes of the graph sit: linked notes pulled into clusters, one group of colour per
//! cluster, and the notes nothing links to set out on an arc below. It knows nothing about windows,
//! so the desktop app draws it with egui and the phone app draws the same layout with SwiftUI.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

#[cfg(feature = "gui")]
pub use egui::Vec2;
#[cfg(not(feature = "gui"))]
pub use plain::Vec2;

/// A two-number vector, for builds that have no egui to borrow one from. It does what the layout
/// asks of egui's `Vec2` and nothing more.
#[cfg(not(feature = "gui"))]
mod plain {
    use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Vec2 {
        pub x: f32,
        pub y: f32,
    }

    impl Vec2 {
        pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

        pub const fn new(x: f32, y: f32) -> Self {
            Self { x, y }
        }

        pub const fn splat(value: f32) -> Self {
            Self { x: value, y: value }
        }

        pub fn length(self) -> f32 {
            self.x.hypot(self.y)
        }

        pub fn min(self, other: Self) -> Self {
            Self::new(self.x.min(other.x), self.y.min(other.y))
        }

        pub fn max(self, other: Self) -> Self {
            Self::new(self.x.max(other.x), self.y.max(other.y))
        }
    }

    impl Add for Vec2 {
        type Output = Vec2;
        fn add(self, other: Vec2) -> Vec2 {
            Vec2::new(self.x + other.x, self.y + other.y)
        }
    }

    impl Sub for Vec2 {
        type Output = Vec2;
        fn sub(self, other: Vec2) -> Vec2 {
            Vec2::new(self.x - other.x, self.y - other.y)
        }
    }

    impl Mul<f32> for Vec2 {
        type Output = Vec2;
        fn mul(self, factor: f32) -> Vec2 {
            Vec2::new(self.x * factor, self.y * factor)
        }
    }

    impl Div<f32> for Vec2 {
        type Output = Vec2;
        fn div(self, divisor: f32) -> Vec2 {
            Vec2::new(self.x / divisor, self.y / divisor)
        }
    }

    impl Neg for Vec2 {
        type Output = Vec2;
        fn neg(self) -> Vec2 {
            Vec2::new(-self.x, -self.y)
        }
    }

    impl AddAssign for Vec2 {
        fn add_assign(&mut self, other: Vec2) {
            *self = *self + other;
        }
    }

    impl SubAssign for Vec2 {
        fn sub_assign(&mut self, other: Vec2) {
            *self = *self - other;
        }
    }
}

/// One live note in the graph view. `links` holds indices of the notes it links to.
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub links: Vec<usize>,
}

/// How far apart two linked notes like to sit, in graph units.
const LINK_LENGTH: f32 = 34.0;
/// How many rounds the layout takes to settle. It runs a few at a time, so the notes are seen
/// finding their places instead of the window freezing while they do.
const SETTLE_STEPS: usize = 260;
/// Room kept between two notes in the ring of unlinked ones.
const RING_SPACING: f32 = 15.0;
pub const MIN_ZOOM: f32 = 0.03;
pub const MAX_ZOOM: f32 = 12.0;

/// The graph: linked notes pulled into clusters, one colour per cluster, and the notes nothing
/// links to set out on an arc beneath them. Positions are in graph units; `zoom` and `focus` say
/// which part of that space the canvas shows.
pub struct GraphView {
    pub nodes: Vec<GraphNode>,
    /// Each pair of linked notes once, however many ways the links run.
    pub edges: Vec<(usize, usize)>,
    pub degree: Vec<usize>,
    pub adjacent: Vec<Vec<usize>>,
    /// The colour group of a linked note: the cluster it belongs to, largest cluster first.
    pub group: Vec<Option<usize>>,
    pub connected: Vec<usize>,
    pub orphans: Vec<usize>,
    /// Where the linked notes are; a note nothing links to is placed by `world_positions`.
    pos: Vec<Vec2>,
    steps: usize,
    pub zoom: f32,
    /// The graph point shown in the middle of the canvas.
    pub focus: Vec2,
    /// Keep the whole graph in view until the user zooms or drags it.
    pub auto_fit: bool,
    /// Phase of the slow sway of the unlinked notes.
    pub sway: f32,
    /// Whether a note has been given a place yet.
    placed: Vec<bool>,
    /// Links read from shared notes, waiting to be folded in with the next `flush`.
    staged: Vec<(String, Vec<String>)>,
    last_flush: Instant,
    /// Shared notes read so far, and how many there are to read; `None` when nothing is loading.
    pub loading: Option<(usize, usize)>,
    /// Why the shared notes could not be read, when they could not.
    pub load_error: Option<String>,
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
    pub fn new(nodes: Vec<GraphNode>) -> Self {
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
    pub fn stage_links(&mut self, id: String, targets: Vec<String>) {
        self.staged.push((id, targets));
    }

    /// Folds staged links into the graph, at most a few times a second unless `force` is set,
    /// and warms the layout up a little so the new notes find their places.
    pub fn flush(&mut self, force: bool) -> bool {
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

    pub fn settled(&self) -> bool {
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
    pub fn settle(&mut self, budget: Duration) {
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
    pub fn world_positions(&self) -> Vec<Vec2> {
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
    pub fn fit_target(world: &[Vec2], canvas: Vec2) -> (f32, Vec2) {
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
