import SwiftUI

struct ConversationView: View {
    @Environment(AppModel.self) private var model
    let name: String
    @State private var draft = ""
    @FocusState private var writing: Bool

    var body: some View {
        let link = model.links[name]
        let (stage, reach) = Route.line(name, model.network, link)
        VStack(spacing: 0) {
            RouteView(name: name)
                .padding(.horizontal, 24)
                .padding(.vertical, 10)
            Divider()
            MessageList(name: name, waiting: link == .unreachable ? "Will be sent once \(name) is reachable" : "Sending…")
            if !model.status.isEmpty {
                Text(verbatim: model.status)
                    .font(.callout)
                    .foregroundStyle(Palette.fail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20)
            }
            composer
        }
        .navigationTitle(name)
        .navigationSubtitle(reach)
        .toolbar {
            ToolbarItem(placement: .status) {
                // The subtitle's colour: under way, or failing.
                if stage == .live || stage == .fail {
                    Circle().fill(stage.color).frame(width: 8, height: 8)
                }
            }
        }
        .onAppear { writing = true }
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Message", text: $draft, prompt: Text(verbatim: "Write to \(name)"), axis: .vertical)
                .textFieldStyle(.plain)
                .lineLimit(1...8)
                .focused($writing)
                .onSubmit(send)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(.quaternary.opacity(0.7), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            Button(action: send) {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.system(size: 28))
                    .foregroundStyle(Palette.accent)
            }
            .buttonStyle(.plain)
            .disabled(draft.allSatisfy(\.isWhitespace))
            .help("Send (Return). Option-Return starts a new line.")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    private func send() {
        var text = draft
        while text.last?.isWhitespace == true { text.removeLast() }
        guard !text.allSatisfy(\.isWhitespace) else { return }
        draft = ""
        Task {
            if await !model.send(text, to: name) { draft = text }
        }
    }
}

/// The messages, newest at the bottom, with a line for each day.
private struct MessageList: View {
    @Environment(AppModel.self) private var model
    let name: String
    let waiting: String

    var body: some View {
        let messages = model.messages[name] ?? []
        ScrollViewReader { scroll in
            ScrollView {
                VStack(spacing: 3) {
                    ForEach(Array(messages.enumerated()), id: \.element.id) { i, m in
                        if i == 0 || !When.sameDay(messages[i - 1].time, m.time) {
                            Text(When.day(m.time))
                                .font(.caption.weight(.medium))
                                .foregroundStyle(.secondary)
                                .padding(.top, 14)
                                .padding(.bottom, 6)
                        }
                        // Messages from one side within five minutes share the last one's time.
                        let next = messages.indices.contains(i + 1) ? messages[i + 1] : nil
                        let last = next.map { $0.outgoing != m.outgoing || $0.time - m.time > 5 * 60_000 || !When.sameDay($0.time, m.time) } ?? true
                        Bubble(message: m, meta: last || (m.outgoing && !m.delivered), waiting: waiting)
                    }
                }
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 20)
                .padding(.bottom, 8)
            }
            .defaultScrollAnchor(.bottom)
            .onChange(of: messages.last?.id) { _, id in
                withAnimation(.easeOut(duration: 0.2)) { scroll.scrollTo(id, anchor: .bottom) }
            }
        }
    }
}

private struct Bubble: View {
    let message: HistoryItem
    let meta: Bool
    let waiting: String

    var body: some View {
        let out = message.outgoing
        VStack(alignment: out ? .trailing : .leading, spacing: 3) {
            HStack {
                if out { Spacer(minLength: 72) }
                // Verbatim: received text is never read as Markdown.
                Text(verbatim: message.text)
                    .textSelection(.enabled)
                    .foregroundStyle(out ? Palette.accentInk : .primary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .background(out ? AnyShapeStyle(Palette.accent) : AnyShapeStyle(.quaternary), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                if !out { Spacer(minLength: 72) }
            }
            if meta {
                HStack(spacing: 4) {
                    Text(When.clock(message.time))
                    if out && !message.delivered {
                        Image(systemName: "circle")
                        Text(waiting).foregroundStyle(Palette.live)
                    } else if out {
                        // Signal's marks: two rings once delivered, filled once read.
                        HStack(spacing: 1) {
                            Image(systemName: message.read ? "checkmark.circle.fill" : "checkmark.circle")
                            Image(systemName: message.read ? "checkmark.circle.fill" : "checkmark.circle")
                        }
                        .accessibilityElement()
                        .accessibilityLabel(message.read ? "Read" : "Delivered")
                    }
                }
                .font(.caption2)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 4)
                .padding(.bottom, 6)
            }
        }
        .frame(maxWidth: .infinity, alignment: out ? .trailing : .leading)
        .id(message.id)
    }
}

/// The way to them through Tor, lit as far as things got; a message going
/// out or coming in runs along it.
struct RouteView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var still
    let name: String

    /// The steps on a 900 × 140 drawing, rising and falling like a route.
    private static let steps: [CGPoint] = [(80, 62), (230, 44), (390, 78), (530, 44), (670, 78), (820, 62)].map { CGPoint(x: $0.0, y: $0.1) }

    var body: some View {
        let net = model.network
        let link = model.links[name]
        let route = Route(net, link)
        let pending = model.messages[name]?.contains { $0.outgoing && !$0.delivered } ?? false
        // Going out while their device is being reached; never towards someone
        // known to be away: those messages just wait.
        let sending = pending && (link == .trying || link == .open)
        let arriving = model.arrival?.name == name ? model.arrival?.count : nil
        let tips = tips(net, route)
        GeometryReader { geo in
            let points = Self.steps.map { place($0, in: geo.size) }
            ZStack {
                ForEach(0..<5, id: \.self) { i in
                    let stage = route.hop(i)
                    Self.hop(points, i)
                        .stroke(stage.color, style: stage == .wait
                            ? StrokeStyle(lineWidth: 2, lineCap: .round, dash: [0.1, 6])
                            : StrokeStyle(lineWidth: 2.5, lineCap: .round))
                }
                if !still {
                    if sending { Pulse(path: Self.whole(points), inward: false, loops: true) }
                    if let arriving { Pulse(path: Self.whole(points), inward: true, loops: false).id(arriving) }
                }
                ForEach(0..<6, id: \.self) { i in
                    let stage = route.step(i)
                    step(i, stage: stage)
                        .help(Text(verbatim: tips[i]))
                        .position(points[i])
                    Text(verbatim: label(i, net))
                        .font(.caption)
                        .foregroundStyle(stage == .wait ? .secondary : stage == .done ? .primary : stage.color)
                        .lineLimit(1)
                        .fixedSize()
                        .position(x: points[i].x, y: points[i].y + 26)
                }
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .frame(height: 86)
        .frame(maxWidth: 760)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Route to \(name)")
    }

    private func place(_ p: CGPoint, in size: CGSize) -> CGPoint {
        let margin: CGFloat = 40
        return CGPoint(x: margin + (p.x - 80) / 740 * (size.width - 2 * margin), y: 22 + (p.y - 44) / 34 * 22)
    }

    private static func hop(_ points: [CGPoint], _ i: Int) -> Path {
        Path { path in
            path.move(to: points[i])
            curve(&path, points[i], points[i + 1])
        }
    }

    private static func whole(_ points: [CGPoint]) -> Path {
        Path { path in
            path.move(to: points[0])
            for i in 0..<5 { curve(&path, points[i], points[i + 1]) }
        }
    }

    private static func curve(_ path: inout Path, _ a: CGPoint, _ b: CGPoint) {
        let mid = (a.x + b.x) / 2
        path.addCurve(to: b, control1: CGPoint(x: mid, y: a.y), control2: CGPoint(x: mid, y: b.y))
    }

    @ViewBuilder
    private func step(_ i: Int, stage: Route.Stage) -> some View {
        let symbol = ["laptopcomputer", "door.left.hand.open", "shield", "shield", "arrow.triangle.merge", "iphone"][i]
        Image(systemName: symbol)
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(stage.color)
            .frame(width: 30, height: 30)
            .background(Circle().fill(.background))
            .overlay(Circle().stroke(stage.color, lineWidth: stage == .wait ? 1 : 1.5))
    }

    private func label(_ i: Int, _ net: Network) -> String {
        ["You", net.bridged ? "Private entrance" : "Entrance", "Relay", "Relay", "Meeting point", name][i]
    }

    private func tips(_ net: Network, _ route: Route) -> [String] {
        let line = net.line.hasPrefix("Tor: ") ? String(net.line.dropFirst(5)) : net.line
        // What the step in orange or red is doing.
        let now = route.fail != nil || route.live == 0 ? line : route.live != nil ? "Reaching \(name)…" : ""
        let tips = [
            "Your computer\nMessages are locked here before they leave. Only \(name)’s device can open them.",
            net.bridged
                ? "Private entrance\nA bridge: a way into Tor that is not on the public list, so this network cannot tell it apart."
                : "Entrance\nThe first Tor relay. It sees your address, never where messages go.",
            "Relay\nKept for weeks: it guards your side of the route. It cannot read the message.",
            "Relay\nChanges every few hours. It knows neither who writes nor to whom.",
            "Meeting point\nWhere your route and \(name)’s meet. Neither of you learns where the other is.",
            "\(name)\n\(name)’s device, the only one that can open your messages.",
        ]
        return tips.enumerated().map { i, tip in
            let stage = route.step(i)
            return (stage == .live || stage == .fail) && !now.isEmpty ? "\(tip)\n\n\(now)" : tip
        }
    }
}

/// A short dash running along the route: out to them in a loop while a
/// message waits, or once in from them.
private struct Pulse: View {
    let path: Path
    let inward: Bool
    let loops: Bool
    @State private var start = Date.now

    var body: some View {
        TimelineView(.animation) { context in
            let t = context.date.timeIntervalSince(start) / 1.4
            let p = loops ? t.truncatingRemainder(dividingBy: 1) : min(t, 1)
            let head = inward ? 1 - p : p
            path.trimmedPath(from: max(0, head - 0.04), to: min(1, head + 0.04))
                .stroke(Palette.live, style: StrokeStyle(lineWidth: 5, lineCap: .round))
                .opacity(!loops && t >= 1 ? 0 : 1)
        }
        .allowsHitTesting(false)
    }
}
