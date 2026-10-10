import SwiftUI

/// One centred column, as on other systems: who, the route to them, the
/// messages, the composer.
struct ConversationView: View {
    @Environment(AppModel.self) private var model
    let name: String
    /// Room on the left for the window's buttons, once the list is folded.
    var inset: CGFloat = 0
    @State private var draft = ""
    @FocusState private var writing: Bool

    static let column: CGFloat = 832

    var body: some View {
        let link = model.links[name]
        VStack(spacing: 0) {
            header(link)
            RouteView(name: name)
                .padding(.top, 14)
                .padding(.horizontal, 32)
                .frame(maxWidth: Self.column + 64)
                .zIndex(1)
            MessageList(name: name, waiting: link == .unreachable ? "Will be sent once \(name) is reachable" : "Sending…")
            composer
            Text(verbatim: model.status)
                .font(.ui(12.5))
                .foregroundStyle(Palette.muted)
                .frame(minHeight: 30)
                .padding(.horizontal, 32)
        }
        .frame(maxWidth: .infinity)
        .background(Palette.ground)
        .onAppear { writing = true }
    }

    private func header(_ link: LinkState?) -> some View {
        let person = model.contacts.first { $0.name == name }?.person ?? Person()
        let (stage, reach) = Route.line(name, model.network, link)
        return HStack(spacing: 14) {
            Avatar(name: name, photo: person.photo, size: 48)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text(name)
                        .font(.ui(28, .semibold))
                        .foregroundStyle(Palette.strong)
                        .lineLimit(1)
                    if person.verified {
                        Icon(parts: Icon.verified, weight: 2)
                            .foregroundStyle(Palette.main)
                            .frame(width: 23, height: 23)
                            .help("Safety number compared: you both read the same digits, this is really \(name).")
                    }
                }
                Text(reach)
                    .font(.ui(14, stage == .wait ? .regular : .medium))
                    .foregroundStyle(stage == .wait ? Palette.muted : stage.color)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.leading, max(28, inset))
        .padding(.trailing, 32)
        .padding(.top, 14)
        .frame(maxWidth: Self.column + 64)
        .frame(maxWidth: .infinity)
        .contentShape(Rectangle())
        .gesture(WindowDragGesture())
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 10) {
            TextField("Message", text: $draft, prompt: Text(verbatim: "Write to \(name)").foregroundStyle(Palette.muted), axis: .vertical)
                .textFieldStyle(.plain)
                .font(.ui(15.5))
                .foregroundStyle(Palette.text)
                .lineLimit(1...10)
                .focused($writing)
                .onSubmit(send)
                .padding(.horizontal, 16)
                .padding(.vertical, 13)
                .frame(minHeight: 48)
                .background(Palette.input, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Palette.line))
                .clickToFocus($writing)
            Button("Send", action: send)
                .buttonStyle(PrimaryButton(height: 48, radius: 14))
                .help("Return sends; Option-Return starts a new line.")
        }
        .padding(.horizontal, 32)
        .padding(.top, 10)
        .frame(maxWidth: Self.column + 64)
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
        ScrollView {
            VStack(spacing: 0) {
                ForEach(Array(messages.enumerated()), id: \.element.id) { i, m in
                    if i == 0 || !When.sameDay(messages[i - 1].time, m.time) {
                        Text(When.day(m.time))
                            .font(.ui(13))
                            .foregroundStyle(Palette.muted)
                            .padding(.top, 18)
                            .padding(.bottom, 4)
                    }
                    // Messages from one side within five minutes share the last one's time.
                    let next = messages.indices.contains(i + 1) ? messages[i + 1] : nil
                    let last = next.map { $0.outgoing != m.outgoing || $0.time - m.time > 5 * 60_000 || !When.sameDay($0.time, m.time) } ?? true
                    Bubble(message: m, meta: last || (m.outgoing && !m.delivered), waiting: waiting)
                }
            }
            .padding(.top, 8)
            .padding(.bottom, 12)
            .frame(maxWidth: ConversationView.column)
            .padding(.horizontal, 32)
            .frame(maxWidth: .infinity)
        }
        .scrollIndicators(.automatic)
        // Stays on the newest message as messages come.
        .defaultScrollAnchor(.bottom)
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
    }
}

private struct Bubble: View {
    let message: HistoryItem
    let meta: Bool
    let waiting: String

    var body: some View {
        let out = message.outgoing
        let shape = UnevenRoundedRectangle(
            topLeadingRadius: 16, bottomLeadingRadius: out ? 16 : 4,
            bottomTrailingRadius: out ? 4 : 16, topTrailingRadius: 16, style: .continuous)
        VStack(alignment: out ? .trailing : .leading, spacing: 3) {
            // Verbatim: received text is never read as Markdown.
            Text(verbatim: message.text)
                .font(.ui(15.5))
                .lineSpacing(3)
                .textSelection(.enabled)
                .foregroundStyle(out ? Palette.outText : Palette.strong)
                .padding(.horizontal, 15)
                .padding(.vertical, 10)
                .background(out ? Palette.outBg : Palette.inBg, in: shape)
                .overlay { if !out { shape.stroke(Palette.inBorder) } }
            if meta {
                HStack(spacing: 5) {
                    Text(When.clock(message.time)).monospacedDigit()
                    if out && !message.delivered {
                        Icon(parts: [.circle(12, 12, 8)], weight: 3).frame(width: 14, height: 14)
                        Text(waiting).foregroundStyle(Palette.live)
                    } else if out {
                        Ticks(read: message.read)
                    }
                }
                .font(.ui(12.5))
                .foregroundStyle(Palette.muted)
            }
        }
        .frame(maxWidth: 544, alignment: out ? .trailing : .leading)
        .frame(maxWidth: .infinity, alignment: out ? .trailing : .leading)
        .padding(.top, 6)
    }
}

/// Signal's marks: two rings once delivered, filled once read.
private struct Ticks: View {
    let read: Bool

    var body: some View {
        Canvas { context, size in
            // The GUI's drawing: viewBox 2 0 24 16.
            context.scaleBy(x: size.height / 16, y: size.height / 16)
            context.translateBy(x: -2, y: 0)
            let style = StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round)
            let ring = { (x: CGFloat, r: CGFloat) in Path(ellipseIn: CGRect(x: x - r, y: 8 - r, width: 2 * r, height: 2 * r)) }
            let check = { (dx: CGFloat) in SVGPath.parse("M\(7.3 + dx) 8.2l1.8 1.8 3.6-3.8") }
            for x in [10.0, 18.0] {
                if x == 18 { context.fill(ring(18, 7.6), with: .color(Palette.ground)) }
                if read {
                    context.fill(ring(x, 6), with: .foreground)
                    context.stroke(ring(x, 6), with: .foreground, style: style)
                    context.stroke(check(x - 10), with: .color(Palette.ground), style: style)
                } else {
                    context.stroke(ring(x, 6), with: .foreground, style: style)
                    context.stroke(check(x - 10), with: .foreground, style: style)
                }
            }
        }
        .frame(width: 13 * 24 / 16, height: 13)
        .accessibilityLabel(read ? "Read" : "Delivered")
    }
}

/// The way to them through Tor, lit as far as things got; a message going
/// out or coming in runs along it. Steps say what they are on hover.
struct RouteView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var still
    let name: String
    @State private var hovered: Int?
    @State private var width: CGFloat = 0

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
            let width = geo.size.width
            let (side, pad, points) = Self.layout(width)
            let labels = width >= 624
            let height = width * 140 / 900
            ZStack(alignment: .topLeading) {
                ForEach(0..<5, id: \.self) { i in
                    let stage = route.hop(i)
                    if stage != .wait {
                        Self.hop(points, i).stroke(stage.color.opacity(stage == .live ? 0.22 : 0.16), style: StrokeStyle(lineWidth: 11, lineCap: .round))
                    }
                    Self.hop(points, i).stroke(stage.color, style: wire(stage))
                }
                if !still {
                    if sending { Pulse(path: Self.whole(points), inward: false, loops: true) }
                    if let arriving { Pulse(path: Self.whole(points), inward: true, loops: false).id(arriving) }
                }
                ForEach(0..<6, id: \.self) { i in
                    let stage = route.step(i)
                    VStack(spacing: 3) {
                        tile(i, stage: stage, side: side)
                            .onHover { hovered = $0 ? i : (hovered == i ? nil : hovered) }
                        Text(verbatim: label(i, net))
                            .font(.ui(12.5))
                            .foregroundStyle(stage == .live || stage == .fail ? stage.color : stage == .wait ? Palette.muted : Palette.soft)
                            .fixedSize()
                            .opacity(labels ? 1 : 0)
                    }
                    .fixedSize()
                    .alignmentGuide(.leading) { $0.width / 2 - points[i].x }
                    .alignmentGuide(.top) { _ in side / 2 - points[i].y }
                }
            }
            .frame(width: width, height: height + 2 * pad, alignment: .topLeading)
        }
        .aspectRatio(900 / 140, contentMode: .fit)
        .padding(.vertical, 10)
        .background(Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Palette.line))
        // Over the panel's edge and the messages, not under them.
        .overlay(alignment: .topLeading) {
            if let i = hovered {
                let (side, _, points) = Self.layout(width)
                Tip(text: tips[i], live: route.step(i) == .fail ? Palette.fail : Palette.live)
                    .frame(width: 240)
                    .offset(
                        x: i == 0 ? points[i].x - side / 2 - 10 : i == 5 ? points[i].x + side / 2 + 10 - 240 : points[i].x - 120,
                        y: 10 + points[i].y + side / 2 + 22)
                    .allowsHitTesting(false)
            }
        }
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Route to \(name)")
    }

    /// As in style.css: steps from 32 to 44 points between 464 and 704
    /// wide, the room above and below from 10 points to none, and no labels
    /// under 624 (the drawing then moves down 5 points to sit centred).
    private static func layout(_ width: CGFloat) -> (side: CGFloat, pad: CGFloat, points: [CGPoint]) {
        let side = min(44, max(32, width * 0.05 + 8.8))
        let pad = min(10, max(0, 29.3 - width * 0.0417))
        let drop: CGFloat = width >= 624 ? 0 : 5
        let height = width * 140 / 900
        return (side, pad, steps.map { CGPoint(x: $0.x / 900 * width, y: pad + drop + $0.y / 140 * height) })
    }

    private func wire(_ stage: Route.Stage) -> StrokeStyle {
        switch stage {
        case .done: StrokeStyle(lineWidth: 1.6)
        case .live: StrokeStyle(lineWidth: 2, dash: [6, 6])
        case .fail: StrokeStyle(lineWidth: 1.6, dash: [2, 5])
        case .wait: StrokeStyle(lineWidth: 1.6, lineCap: .round, dash: [2, 5])
        }
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

    /// A rounded square for a relay, a circle for a device: tinted, with a
    /// line round it, in the step's colour.
    private func tile(_ i: Int, stage: Route.Stage, side: CGFloat) -> some View {
        // Circular style, not continuous: a continuous corner as wide as the
        // side is not a circle.
        let shape = RoundedRectangle(cornerRadius: i == 0 || i == 5 ? side / 2 : side * 0.27, style: i == 0 || i == 5 ? .circular : .continuous)
        return Icon(parts: Icon.steps[i])
            .frame(width: side * 0.55, height: side * 0.55)
            .frame(width: side, height: side)
            .background(Palette.ground, in: shape)
            .background(stage.color.opacity(0.1), in: shape)
            .overlay(shape.stroke(stage.color.opacity(0.7), lineWidth: 1.25))
            .foregroundStyle(stage.color)
            .contentShape(shape)
    }

    private func label(_ i: Int, _ net: Network) -> String {
        ["You", net.bridged ? "Private entrance" : "Entrance", "Relay", "Relay", "Meeting point", name][i]
    }

    private func tips(_ net: Network, _ route: Route) -> [(String, String, String)] {
        let line = net.line.hasPrefix("Tor: ") ? String(net.line.dropFirst(5)) : net.line
        // What the step in orange or red is doing.
        let now = route.fail != nil || route.live == 0 ? line : route.live != nil ? "Reaching \(name)…" : ""
        let tips = [
            ("Your computer", "Messages are locked here before they leave. Only \(name)’s device can open them."),
            net.bridged
                ? ("Private entrance", "A bridge: a way into Tor that is not on the public list, so this network cannot tell it apart.")
                : ("Entrance", "The first Tor relay. It sees your address, never where messages go."),
            ("Relay", "Kept for weeks: it guards your side of the route. It cannot read the message."),
            ("Relay", "Changes every few hours. It knows neither who writes nor to whom."),
            ("Meeting point", "Where your route and \(name)’s meet. Neither of you learns where the other is."),
            (name, "\(name)’s device, the only one that can open your messages."),
        ]
        return tips.enumerated().map { i, tip in
            let stage = route.step(i)
            return (tip.0, tip.1, stage == .live || stage == .fail ? now : "")
        }
    }
}

/// style.css's `.tip`.
private struct Tip: View {
    let text: (String, String, String)
    let live: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(verbatim: text.0).font(.ui(13.5, .semibold)).foregroundStyle(Palette.strong)
            Text(verbatim: text.1).font(.ui(13)).foregroundStyle(Palette.soft)
            if !text.2.isEmpty {
                Text(verbatim: text.2).font(.ui(13)).foregroundStyle(live).padding(.top, 4)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Palette.tip, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Palette.line2))
        .shadow(color: Palette.shadow, radius: 12, y: 10)
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
            path.trimmedPath(from: max(0, head - 0.03), to: min(1, head + 0.03))
                .stroke(Palette.live, style: StrokeStyle(lineWidth: 5, lineCap: .round))
                .opacity(!loops && t >= 1 ? 0 : 1)
        }
        .allowsHitTesting(false)
    }
}
