// The GUI's look on other systems (crates/gui/src/style.css), natively:
// its colours, its typeface, its icons; and the dates and the route, worked
// out the same way.

import AppKit
import SwiftUI

/// style.css's tokens, by night and by day, following the system.
enum Palette {
    static let ground = Color(nsColor: groundNS)
    /// For AppKit: a window's own background.
    static let groundNS = dynamicNS(0x0F1522, 0xF4F5F7)
    static let side = dynamic(0x0A0F19, 0xFFFFFF)
    static let panel = dynamic(0x111A29, 0xF7F8FA)
    static let line = dynamic(0x1E2A3E, 0xE2E4E8)
    static let line2 = dynamic(0x2E3E5C, 0xC9CED6)
    static let text = dynamic(0xDDE4F1, 0x1E2430)
    static let strong = dynamic(0xEEF2F9, 0x10141D)
    static let muted = dynamic(0x8A96AD, 0x646B78)
    static let soft = dynamic(0xA3AEC2, 0x4A5160)
    static let inBg = dynamic(0x182236, 0xFFFFFF)
    static let inBorder = dynamic(0x22304A, 0xE2E4E8)
    static let outBg = dynamic(0x22385A, 0xDCE7F8)
    static let outText = dynamic(0xEEF2F9, 0x10141D)
    static let main = dynamic(0x5CC8C2, 0x1F5FBF)
    static let mainInk = dynamic(0x0B1A1A, 0xFFFFFF)
    /// Something under way.
    static let live = dynamic(0xF28C28, 0xE8740C)
    static let fail = dynamic(0xE5736A, 0xB3261E)
    /// Not reached yet.
    static let wait = dynamic(0x3A4760, 0xC3C9D2)
    static let selected = dynamic(0x182236, 0xE8EEF8)
    static let input = dynamic(0x141C2B, 0xFFFFFF)
    static let tip = dynamic(0x1C2740, 0xFFFFFF)
    /// Under a dialog, over the blurred window.
    static let veil = dynamic(0x05080E, 0x10141D, alpha: (0.5, 0.25))
    static let shadow = dynamic(0x000000, 0x10141D, alpha: (0.55, 0.18))

    static func hex(_ value: UInt32) -> Color {
        Color(nsColor: nsHex(value))
    }

    private static func nsHex(_ value: UInt32, alpha: Double = 1) -> NSColor {
        NSColor(srgbRed: Double(value >> 16 & 0xFF) / 255, green: Double(value >> 8 & 0xFF) / 255, blue: Double(value & 0xFF) / 255, alpha: alpha)
    }

    private static func dynamic(_ night: UInt32, _ day: UInt32, alpha: (Double, Double) = (1, 1)) -> Color {
        Color(nsColor: dynamicNS(night, day, alpha: alpha))
    }

    private static func dynamicNS(_ night: UInt32, _ day: UInt32, alpha: (Double, Double) = (1, 1)) -> NSColor {
        NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? nsHex(night, alpha: alpha.0) : nsHex(day, alpha: alpha.1)
        }
    }
}

extension Font {
    /// Space Grotesk, the GUI's typeface (variable, 300 to 700).
    static func ui(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .custom("Space Grotesk", size: size).weight(weight)
    }
}

extension View {
    /// A plain text field reacts only on its line of text; this makes the
    /// whole box round it take the click.
    func clickToFocus(_ focus: FocusState<Bool>.Binding) -> some View {
        contentShape(Rectangle()).onTapGesture { focus.wrappedValue = true }
    }
}

/// style.css's `.primary`.
struct PrimaryButton: ButtonStyle {
    var height: CGFloat = 42
    var radius: CGFloat = 10
    /// Takes the row's whole width.
    var wide = false
    @State private var hover = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.ui(15, .semibold))
            .foregroundStyle(Palette.mainInk)
            .padding(.horizontal, 22)
            .frame(maxWidth: wide ? .infinity : nil)
            .frame(height: height)
            .contentShape(Rectangle())
            .background(Palette.main, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
            .brightness(configuration.isPressed ? -0.05 : hover ? 0.06 : 0)
            .onHover { hover = $0 }
    }
}

/// A square button with a line round it, like the gear under the list.
struct OutlineButton: ButtonStyle {
    var on = false
    @State private var hover = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(on ? Palette.main : hover ? Palette.strong : Palette.soft)
            .frame(width: 42, height: 42)
            .background(on || hover ? Palette.selected : .clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(on ? Palette.main : Palette.line2))
            .contentShape(Rectangle())
            .onHover { hover = $0 }
    }
}

/// The photo chosen for them, or three rings, the same for a name on every
/// machine: an onion of one's own.
struct Avatar: View {
    let name: String
    let photo: String?
    var size: CGFloat = 44

    private static let rings: [[UInt32]] = [
        [0xF2B880, 0xC8553D, 0x2D3047],
        [0xA7C4E2, 0x3A6EA5, 0xF4E285],
        [0xB8D8BA, 0x5B8E7D, 0xEF8354],
        [0xD7C3E8, 0x7A5C99, 0xF2EEF6],
        [0xF6D6AD, 0xD08C60, 0x3F4B3B],
        [0x9FD8D3, 0x2F7F86, 0xFCE7C8],
        [0xF4B6C2, 0xB04A6C, 0x2E294E],
        [0xC9D6A3, 0x6B8F3A, 0xF3EBD3],
    ]

    var body: some View {
        if let image = picture {
            Image(nsImage: image).resizable().scaledToFill().frame(width: size, height: size).clipShape(Circle())
        } else {
            // FNV-1a, like the GUI on other systems.
            let hash = name.utf8.reduce(UInt32(0x811C_9DC5)) { ($0 ^ UInt32($1)) &* 0x0100_0193 }
            let colors = Self.rings[Int(hash % UInt32(Self.rings.count))].map(Palette.hex)
            ZStack {
                Circle().fill(colors[0])
                Circle().fill(colors[1]).padding(size * 7 / 46)
                Circle().fill(colors[2]).padding(size * 14 / 46)
            }
            .frame(width: size, height: size)
        }
    }

    /// Our own re-encoded PNG, as a data URI the daemon checked.
    private var picture: NSImage? {
        guard let photo, let comma = photo.firstIndex(of: ","),
              let data = Data(base64Encoded: String(photo[photo.index(after: comma)...])) else { return nil }
        return NSImage(data: data)
    }
}

/// A line icon drawn from SVG path data on a 24 × 24 grid, like the GUI's
/// (Lucide style; the handshake is Lucide's, ISC licence).
struct Icon: View {
    enum Part {
        case path(String, dash: [CGFloat] = [])
        case circle(CGFloat, CGFloat, CGFloat)
        case rect(CGFloat, CGFloat, CGFloat, CGFloat, radius: CGFloat)
        /// Erases a wide line through what is drawn before it.
        case cut(String)
    }

    let parts: [Part]
    var weight: CGFloat = 1.7

    var body: some View {
        Canvas { canvas, size in
          canvas.drawLayer { context in
            context.scaleBy(x: size.width / 24, y: size.height / 24)
            for part in parts {
                let style = StrokeStyle(lineWidth: weight, lineCap: .round, lineJoin: .round)
                switch part {
                case let .path(data, dash):
                    var dashed = style
                    dashed.dash = dash
                    context.stroke(SVGPath.parse(data), with: .foreground, style: dashed)
                case let .circle(x, y, r):
                    context.stroke(Path(ellipseIn: CGRect(x: x - r, y: y - r, width: 2 * r, height: 2 * r)), with: .foreground, style: style)
                case let .rect(x, y, w, h, radius):
                    context.stroke(Path(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerRadius: radius), with: .foreground, style: style)
                case let .cut(data):
                    var eraser = context
                    eraser.blendMode = .destinationOut
                    eraser.stroke(SVGPath.parse(data), with: .color(.black), style: StrokeStyle(lineWidth: 5, lineCap: .round))
                }
            }
          }
        }
    }

    static let shield = "M12 3l7 2.8v5.6c0 4.4-3 7.7-7 9.4-4-1.7-7-5-7-9.4V5.8z"
    static let verified: [Part] = [.path(shield), .path("M8.5 12l2.5 2.5 4.5-5")]
    /// A whole bell, struck through with room round the bar, as on other systems.
    static let muted: [Part] = [
        .path("M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3C3 17 6 15 6 8"), .path("M10.3 21a1.94 1.94 0 0 0 3.4 0"),
        .cut("M3 3l18 18"), .path("M3 3l18 18"),
    ]
    /// The route's steps: this computer, the entrance, two relays, the
    /// meeting point, their device.
    static let steps: [[Part]] = [
        [.path("M5 6.5h14v8.5H5z"), .path("M3 18h18")],
        [.path(shield, dash: [2.4, 2.2]), .circle(12, 11, 2.2), .path("M12 13.2V16")],
        [.path(shield), .circle(12, 11.5, 2.6), .path("M5 11.5h4.4M14.6 11.5H19")],
        [.path(shield), .circle(12, 11.5, 2.6), .path("M5 11.5h4.4M14.6 11.5H19")],
        [
            .path("m11 17 2 2a1 1 0 1 0 3-3"),
            .path("m14 14 2.5 2.5a1 1 0 1 0 3-3l-3.88-3.88a3 3 0 0 0-4.24 0l-.88.88a1 1 0 1 1-3-3l2.81-2.81a5.79 5.79 0 0 1 7.06-.87l.47.28a2 2 0 0 0 1.42.25L21 4"),
            .path("m21 3 1 11h-2"),
            .path("M3 3 2 14l6.5 6.5a1 1 0 1 0 3-3"),
            .path("M3 4h8"),
        ],
        [.rect(7, 3, 10, 18, radius: 2.2), .path("M11 18h2")],
    ]
}

/// Enough of SVG's path syntax for the icons above: moves, lines, cubic
/// curves and circular arcs, absolute or relative.
enum SVGPath {
    static func parse(_ data: String) -> Path {
        let tokens = data.matches(of: /[A-Za-z]|-?(?:\d+\.?\d*|\.\d+)/).map { String($0.output) }
        var path = Path()
        var i = 0
        var command: Character = "M"
        var at = CGPoint.zero
        var start = CGPoint.zero
        func number() -> CGFloat {
            defer { i += 1 }
            return CGFloat(Double(tokens[i]) ?? 0)
        }
        func point(_ relative: Bool) -> CGPoint {
            let x = number(), y = number()
            return relative ? CGPoint(x: at.x + x, y: at.y + y) : CGPoint(x: x, y: y)
        }
        while i < tokens.count {
            if let letter = tokens[i].first, letter.isLetter {
                command = letter
                i += 1
                if letter == "z" || letter == "Z" {
                    path.closeSubpath()
                    at = start
                    continue
                }
            }
            let relative = command.isLowercase
            switch command.uppercased() {
            case "M":
                at = point(relative)
                start = at
                path.move(to: at)
                // Further pairs are lines.
                command = relative ? "l" : "L"
            case "L":
                at = point(relative)
                path.addLine(to: at)
            case "H":
                let x = number()
                at.x = relative ? at.x + x : x
                path.addLine(to: at)
            case "V":
                let y = number()
                at.y = relative ? at.y + y : y
                path.addLine(to: at)
            case "C":
                let c1 = point(relative), c2 = point(relative), end = point(relative)
                path.addCurve(to: end, control1: c1, control2: c2)
                at = end
            case "A":
                let r = number()
                _ = number() // ry: the icons' arcs are circles
                _ = number() // rotation
                let large = number() != 0, sweep = number() != 0
                let end = point(relative)
                arc(&path, from: at, to: end, radius: r, large: large, sweep: sweep)
                at = end
            default:
                i += 1
            }
        }
        return path
    }

    /// SVG's endpoint arc as a centre and two angles (SVG 1.1, F.6.5).
    private static func arc(_ path: inout Path, from p0: CGPoint, to p1: CGPoint, radius: CGFloat, large: Bool, sweep: Bool) {
        let hx = (p0.x - p1.x) / 2, hy = (p0.y - p1.y) / 2
        let d2 = hx * hx + hy * hy
        guard d2 > 0 else { return }
        let r = max(radius, d2.squareRoot())
        let k = (large == sweep ? -1 : 1) * max(0, (r * r - d2) / d2).squareRoot()
        let center = CGPoint(x: (p0.x + p1.x) / 2 + k * hy, y: (p0.y + p1.y) / 2 - k * hx)
        let a0 = atan2(p0.y - center.y, p0.x - center.x), a1 = atan2(p1.y - center.y, p1.x - center.x)
        // y points down: SVG's positive sweep is SwiftUI's "not clockwise".
        path.addArc(center: center, radius: r, startAngle: .radians(a0), endAngle: .radians(a1), clockwise: !sweep)
    }
}

enum When {
    static func date(_ millis: UInt64) -> Date { Date(timeIntervalSince1970: Double(millis) / 1000) }

    static func clock(_ millis: UInt64) -> String {
        date(millis).formatted(.dateTime.hour(.twoDigits(amPM: .omitted)).minute(.twoDigits))
    }

    /// "Today", "Yesterday", "Monday 5 October", "5 October 2025".
    static func day(_ millis: UInt64) -> String {
        let then = date(millis), calendar = Calendar.current
        if calendar.isDateInToday(then) { return t("today") }
        if calendar.isDateInYesterday(then) { return t("yesterday") }
        if calendar.isDate(then, equalTo: .now, toGranularity: .year) {
            return then.formatted(.dateTime.weekday(.wide).day().month(.wide))
        }
        return then.formatted(.dateTime.day().month(.wide).year())
    }

    /// For the list: the time today, then "Yesterday", the weekday, the date.
    static func short(_ millis: UInt64) -> String {
        let then = date(millis), calendar = Calendar.current
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: then), to: calendar.startOfDay(for: .now)).day ?? 0
        switch days {
        case ...0: return clock(millis)
        case 1: return t("yesterday")
        case 2...6: return then.formatted(.dateTime.weekday(.wide))
        default: return then.formatted(.dateTime.day().month(.abbreviated))
        }
    }

    static func sameDay(_ a: UInt64, _ b: UInt64) -> Bool {
        Calendar.current.isDate(date(a), inSameDayAs: date(b))
    }
}

/// Where a message to someone gets now: the steps reached, the one being
/// tried, the one that failed.
struct Route {
    enum Stage { case done, live, fail, wait }

    let lit: Int
    let live: Int?
    let fail: Int?

    init(_ net: Network, _ link: LinkState?) {
        switch (net.entry, net.percent, net.circuit, link) {
        case (.some(false), _, _, _): (lit, live, fail) = (1, nil, 0)
        case (_, ..<100, _, _): (lit, live, fail) = (1, 0, nil)
        case (_, _, .some(false), _): (lit, live, fail) = (2, nil, 1)
        case (_, _, _, .open), (_, _, _, .idle): (lit, live, fail) = (6, nil, nil)
        case (_, _, _, .trying): (lit, live, fail) = (4, 3, nil)
        default: (lit, live, fail) = (4, nil, nil)
        }
    }

    /// The hop that leaves step `i`.
    func hop(_ i: Int) -> Stage {
        if live == i { return .live }
        if fail == i { return .fail }
        return i + 1 < lit ? .done : .wait
    }

    func step(_ i: Int) -> Stage {
        if i < lit { return .done }
        if i > 0 && live == i - 1 { return .live }
        if i > 0 && fail == i - 1 { return .fail }
        return .wait
    }

    /// Under the name: how things stand with them, and in which colour.
    static func line(_ name: String, _ net: Network, _ link: LinkState?) -> (Stage, String) {
        let route = Route(net, link)
        if route.fail != nil { return (.fail, t("net-blocked")) }
        if route.live == 0 { return (.live, t("connecting-percent", ["percent": net.percent])) }
        switch link {
        case .open: return (.done, t("link-open"))
        case .idle: return (.done, t("link-idle"))
        case .trying: return (.live, t("reaching", ["name": name]))
        case .unreachable: return (.wait, t("link-away"))
        case nil: return (.wait, t("link-unknown"))
        }
    }

    /// Reached since the daemon started, and Tor is up.
    static func reached(_ net: Network, _ link: LinkState?) -> Bool {
        line("", net, link).0 == .done
    }
}

extension Route.Stage {
    var color: Color {
        switch self {
        case .done: Palette.main
        case .live: Palette.live
        case .fail: Palette.fail
        case .wait: Palette.wait
        }
    }
}

/// What the bottom of the list says about Tor.
func networkSummary(_ net: Network) -> (Route.Stage, String) {
    if net.entry == false || net.circuit == false { return (.fail, t("net-blocked")) }
    if net.percent < 100 { return (.live, t("net-connecting")) }
    return (.done, net.bridged ? t("net-bridged") : t("net-connected"))
}
