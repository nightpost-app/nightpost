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
            if !isNote {
                RouteView(name: name)
                    .padding(.top, 14)
                    .padding(.horizontal, 32)
                    .frame(maxWidth: Self.column + 64)
                    .zIndex(1)
            }
            MessageList(name: name, waiting: [.unreachable, .older, .newer].contains(link) ? t("waiting-reachable", ["name": name]) : t("sending"))
            if link == .older || link == .newer { UpdateNotice(name: name, theirs: link == .older) }
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

    private var isNote: Bool { model.contacts.first { $0.name == name }?.note ?? false }

    private func header(_ link: LinkState?) -> some View {
        let person = model.contacts.first { $0.name == name }?.person ?? Person()
        let (stage, reach) = isNote ? (Route.Stage.wait, t("note-here")) : Route.line(name, model.network, link)
        return HStack(spacing: 14) {
            Button { if !isNote { model.panel.toggle() } } label: { who(person, stage, reach) }
                .buttonStyle(WhoButton(open: model.panel))
                .help(t("about-name", ["name": name]))
            Spacer(minLength: 0)
        }
        .padding(.leading, max(22, inset - 6))
        .padding(.trailing, 32)
        .padding(.top, 8)
        .frame(maxWidth: Self.column + 64)
        .frame(maxWidth: .infinity)
        .contentShape(Rectangle())
        .gesture(WindowDragGesture())
    }

    private func who(_ person: Person, _ stage: Route.Stage, _ reach: String) -> some View {
        HStack(spacing: 14) {
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
                            .help(t("mac-verified-help", ["name": name]))
                    }
                    if let seconds = model.contacts.first(where: { $0.name == name })?.timer, seconds > 0 {
                        Icon(parts: [.circle(12, 13, 8), .path("M12 9v4l2.5 2M9.5 3h5")], weight: 2)
                            .foregroundStyle(Palette.muted)
                            .frame(width: 19, height: 19)
                            .help(t("timer-mark-help", ["timer": timerLabel(seconds)]))
                    }
                    if model.isMuted(name) {
                        Icon(parts: Icon.muted, weight: 2)
                            .foregroundStyle(Palette.muted)
                            .frame(width: 19, height: 19)
                            .help(t("mac-muted-help", ["name": name]))
                    }
                }
                Text(reach)
                    .font(.ui(14, stage == .wait ? .regular : .medium))
                    .foregroundStyle(stage == .wait ? Palette.muted : stage.color)
                    .lineLimit(1)
            }
        }
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 10) {
            TextField(t("mac-message"), text: $draft, prompt: Text(verbatim: t("write-to", ["name": name])).foregroundStyle(Palette.muted), axis: .vertical)
                .textFieldStyle(.plain)
                .font(.ui(15.5))
                .foregroundStyle(Palette.text)
                .lineLimit(1...10)
                .focused($writing)
                .onSubmit(send)
                .onChange(of: draft) { _, now in
                    if !now.allSatisfy(\.isWhitespace) { model.typed(to: name) }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 13)
                .frame(minHeight: 48)
                .background(Palette.input, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Palette.line))
                .clickToFocus($writing)
            Button(t("send"), action: send)
                .buttonStyle(PrimaryButton(height: 48, radius: 14))
                .help(t("mac-return-sends"))
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

/// One side must update: said where the eye goes to write.
private struct UpdateNotice: View {
    let name: String
    /// They must update, not us.
    let theirs: Bool

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
        HStack(alignment: .top, spacing: 12) {
            Icon(parts: [.circle(12, 12, 9), .path("M16 12l-4-4-4 4M12 16V8")], weight: 2)
                .foregroundStyle(Palette.fail)
                .frame(width: 22, height: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(theirs ? t("update-older-title", ["name": name]) : t("update-newer-title"))
                    .font(.ui(15, .semibold))
                    .foregroundStyle(Palette.strong)
                Text(theirs ? t("update-older-help", ["name": name]) : t("update-newer-help", ["name": name]))
                    .font(.ui(14))
                    .foregroundStyle(Palette.soft)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(Palette.fail.opacity(0.1), in: shape)
        .overlay(shape.stroke(Palette.fail.opacity(0.4)))
        .frame(maxWidth: ConversationView.column)
        .padding(.horizontal, 32)
        .padding(.top, 8)
        .accessibilityElement(children: .combine)
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
                if model.typing[name] != nil { Typing(name: name) }
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

/// They are writing: three dots, which blink in turn unless motion is
/// reduced.
private struct Typing: View {
    @Environment(\.accessibilityReduceMotion) private var still
    let name: String

    var body: some View {
        let shape = UnevenRoundedRectangle(topLeadingRadius: 16, bottomLeadingRadius: 4, bottomTrailingRadius: 16, topTrailingRadius: 16, style: .continuous)
        TimelineView(.animation(paused: still)) { context in
            let time = context.date.timeIntervalSinceReferenceDate
            HStack(spacing: 5) {
                ForEach(0..<3, id: \.self) { i in
                    // The GUI's blink: 1.2 s, each dot 0.2 s after the one before.
                    let phase = (time - 0.2 * Double(i)).truncatingRemainder(dividingBy: 1.2) / 1.2
                    let lit = phase < 0.3 ? phase / 0.3 : phase < 0.6 ? (0.6 - phase) / 0.3 : 0
                    Circle()
                        .fill(Palette.muted)
                        .frame(width: 7, height: 7)
                        .opacity(still ? [1, 0.7, 0.45][i] : 0.35 + 0.65 * lit)
                }
            }
        }
        .padding(.horizontal, 15)
        .frame(height: 40)
        .background(Palette.inBg, in: shape)
        .overlay { shape.stroke(Palette.inBorder) }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 6)
        .accessibilityElement()
        .accessibilityLabel(t("typing", ["name": name]))
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
        .accessibilityLabel(read ? t("read") : t("delivered"))
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
        .accessibilityLabel(t("route-label", ["name": name]))
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
        [t("step-you"), net.bridged ? t("step-private-entrance") : t("step-entrance"), t("step-relay"), t("step-relay"), t("step-meeting"), name][i]
    }

    private func tips(_ net: Network, _ route: Route) -> [(String, String, String)] {
        let line = net.line.hasPrefix("Tor: ") ? String(net.line.dropFirst(5)) : net.line
        // What the step in orange or red is doing.
        let now = route.fail != nil || route.live == 0 ? line : route.live != nil ? t("reaching", ["name": name]) : ""
        let tips = [
            (t("step-computer"), t("step-computer-help", ["name": name])),
            net.bridged
                ? (t("step-private-entrance"), t("step-bridge-help"))
                : (t("step-entrance"), t("step-guard-help")),
            (t("step-relay"), t("step-relay-1-help")),
            (t("step-relay"), t("step-relay-2-help")),
            (t("step-meeting"), t("step-meeting-help", ["name": name])),
            (name, t("step-device-help", ["name": name])),
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

/// Their avatar and name open their panel.
private struct WhoButton: ButtonStyle {
    let open: Bool
    @State private var hover = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .padding(6)
            .background(open || hover ? Palette.selected : .clear, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .contentShape(Rectangle())
            .onHover { hover = $0 }
    }
}
