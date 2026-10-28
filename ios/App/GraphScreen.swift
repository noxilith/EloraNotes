import SwiftUI

/// The note graph, laid out by the same Rust code the desktop app uses: linked notes gather into
/// coloured clusters and the notes nothing links to sit on an arc below. Pinch to zoom, drag to
/// move, tap a note to light it and the notes it links to, then open it.
struct GraphScreen: View {
    @Binding var path: [String]
    @EnvironmentObject private var library: Library
    @State private var data: GraphData?
    @State private var loading = true
    @State private var zoom: CGFloat = 1
    @State private var focus = CGPoint.zero
    @State private var size = CGSize.zero
    @State private var selected: Int?
    @GestureState private var drag = CGSize.zero
    @GestureState private var pinch: CGFloat = 1

    private func color(_ group: UInt32?) -> Color {
        guard let group else { return Color.gray }
        let hue = (Double(group) * 0.618034).truncatingRemainder(dividingBy: 1)
        return Color(hue: hue, saturation: 0.5, brightness: 1.0)
    }

    private func radius(_ point: GraphPoint, _ z: CGFloat) -> CGFloat {
        min(10, 2.2 + CGFloat(point.degree).squareRoot() * 1.15) * min(2, max(0.7, z.squareRoot()))
    }

    private func screen(_ point: GraphPoint, _ z: CGFloat, _ drag: CGSize) -> CGPoint {
        CGPoint(
            x: size.width / 2 + (CGFloat(point.x) - focus.x) * z + drag.width,
            y: size.height / 2 + (CGFloat(point.y) - focus.y) * z + drag.height)
    }

    /// Zoom and focus that show every note.
    private func fit() {
        guard let points = data?.points, !points.isEmpty, size.width > 0 else { return }
        let xs = points.map { CGFloat($0.x) }
        let ys = points.map { CGFloat($0.y) }
        let (lowX, highX, lowY, highY) = (xs.min()!, xs.max()!, ys.min()!, ys.max()!)
        let wide = highX - lowX + 70
        let tall = highY - lowY + 70
        zoom = min(6, max(0.03, min(size.width / wide, size.height / tall)))
        focus = CGPoint(x: (lowX + highX) / 2, y: (lowY + highY) / 2)
    }

    private func zoomBy(_ factor: CGFloat) {
        zoom = min(12, max(0.03, zoom * factor))
    }

    private func neighbours(of index: Int) -> Set<Int> {
        guard let edges = data?.edges else { return [] }
        var found: Set<Int> = []
        for edge in edges {
            if Int(edge.from) == index { found.insert(Int(edge.to)) }
            if Int(edge.to) == index { found.insert(Int(edge.from)) }
        }
        return found
    }

    private func hit(_ location: CGPoint) {
        guard let points = data?.points else { return }
        var best: (Int, CGFloat)?
        for (index, point) in points.enumerated() {
            let at = screen(point, zoom, .zero)
            let distance = hypot(at.x - location.x, at.y - location.y)
            if distance <= max(radius(point, zoom) + 12, 20), distance < (best?.1 ?? .infinity) {
                best = (index, distance)
            }
        }
        selected = best?.0
    }

    var body: some View {
        VStack(spacing: 0) {
            GeometryReader { geo in
                Canvas { context, canvasSize in
                    guard let data else { return }
                    let bounds = CGRect(origin: .zero, size: canvasSize)
                    let z = zoom * pinch
                    let points = data.points
                    let at = points.map { screen($0, z, drag) }
                    let lit = selected.map { neighbours(of: $0) } ?? []

                    for edge in data.edges {
                        let (a, b) = (Int(edge.from), Int(edge.to))
                        var line = Path()
                        line.move(to: at[a])
                        line.addLine(to: at[b])
                        let touches = selected == a || selected == b
                        let same = points[a].group == points[b].group
                        let shade = same ? color(points[a].group) : Color.gray
                        let opacity = selected == nil ? (same ? 0.35 : 0.2) : (touches ? 0.9 : 0.08)
                        context.stroke(
                            line, with: .color(touches ? Color.accentColor.opacity(0.9) : shade.opacity(opacity)),
                            lineWidth: touches ? 1.6 : 0.8)
                    }

                    for (index, point) in points.enumerated() {
                        let place = at[index]
                        let r = radius(point, z)
                        let dim = selected != nil && selected != index && !lit.contains(index)
                        let base = selected == index ? Color.accentColor : color(point.group)
                        if point.degree >= 6 || selected == index {
                            for (scale, alpha) in [(2.6, 0.08), (1.7, 0.16)] {
                                let glow = CGRect(
                                    x: place.x - r * scale, y: place.y - r * scale,
                                    width: r * scale * 2, height: r * scale * 2)
                                context.fill(Path(ellipseIn: glow), with: .color(base.opacity(dim ? 0.02 : alpha)))
                            }
                        }
                        let dot = CGRect(x: place.x - r, y: place.y - r, width: r * 2, height: r * 2)
                        context.fill(Path(ellipseIn: dot), with: .color(base.opacity(dim ? 0.25 : 1)))
                    }

                    // Names: the busiest notes first, fewer as the picture gets smaller, and never
                    // two on top of each other.
                    let few = points.count <= 40
                    let minimum = few ? 0 : Int((8 / z).rounded(.up))
                    var taken: [CGRect] = []
                    let order = points.indices.sorted { points[$0].degree > points[$1].degree }
                    var drawn = 0
                    for index in order {
                        let point = points[index]
                        let chosen = selected == index || lit.contains(index)
                        let wanted = selected != nil
                            ? chosen
                            : (point.group != nil ? Int(point.degree) >= minimum : (few || z >= 2.5))
                        guard wanted, drawn < 160 else { continue }
                        let place = at[index]
                        guard bounds.insetBy(dx: -20, dy: -20).contains(place) else { continue }
                        let resolved = context.resolve(
                            Text(point.title.count > 24 ? String(point.title.prefix(24)) + "…" : point.title)
                                .font(.system(size: selected == index ? 13 : 10.5))
                                .foregroundColor(selected == index ? .primary : .secondary))
                        let measured = resolved.measure(in: CGSize(width: 220, height: 30))
                        let r = radius(point, z)
                        var box = CGRect(
                            x: place.x + r + 4, y: place.y - measured.height / 2,
                            width: measured.width, height: measured.height)
                        if box.maxX > canvasSize.width - 4 { box.origin.x = place.x - r - 4 - measured.width }
                        let padded = box.insetBy(dx: -3, dy: -1)
                        if !chosen && taken.contains(where: { $0.intersects(padded) }) { continue }
                        taken.append(padded)
                        drawn += 1
                        context.draw(resolved, at: CGPoint(x: box.minX, y: box.midY), anchor: .leading)
                    }
                }
                .gesture(
                    DragGesture(minimumDistance: 6)
                        .updating($drag) { value, state, _ in state = value.translation }
                        .onEnded { value in
                            focus = CGPoint(
                                x: focus.x - value.translation.width / zoom,
                                y: focus.y - value.translation.height / zoom)
                        }
                )
                .simultaneousGesture(
                    MagnifyGesture()
                        .updating($pinch) { value, state, _ in state = value.magnification }
                        .onEnded { value in zoomBy(value.magnification) }
                )
                .simultaneousGesture(SpatialTapGesture().onEnded { value in hit(value.location) })
                .onAppear {
                    size = geo.size
                    fit()
                }
                .onChange(of: geo.size) { _, new in
                    size = new
                    if data != nil { fit() }
                }
            }
            .background(Color(.systemBackground))
            .overlay {
                if loading {
                    VStack(spacing: 8) {
                        ProgressView()
                        Text("Laying out your notes…").font(.footnote).foregroundStyle(.secondary)
                    }
                } else if let data, data.points.isEmpty {
                    Text("No notes yet").foregroundStyle(.secondary)
                }
            }

            if let data, let index = selected, index < data.points.count {
                let point = data.points[index]
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(point.title).font(.headline).lineLimit(1)
                        Text("\(point.degree) link\(point.degree == 1 ? "" : "s")")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Open") { path.append(point.id) }
                        .buttonStyle(.borderedProminent)
                }
                .padding()
                .background(.regularMaterial)
            } else if let why = data?.sharedError {
                Label(why, systemImage: "exclamationmark.triangle")
                    .font(.footnote).foregroundStyle(.secondary)
                    .padding(8)
            }
        }
        .navigationTitle("Graph")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button { zoomBy(1 / 1.4) } label: { Image(systemName: "minus.magnifyingglass") }
                Button { zoomBy(1.4) } label: { Image(systemName: "plus.magnifyingglass") }
                Button { fit() } label: { Image(systemName: "arrow.up.left.and.down.right.magnifyingglass") }
            }
        }
        .task {
            loading = true
            data = try? await library.run { $0.graph() }
            loading = false
            fit()
        }
    }
}
