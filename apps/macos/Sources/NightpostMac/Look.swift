// Colours, avatars, dates and how far a message gets: shared by the views,
// and the same as the GUI on other systems.

import AppKit
import SwiftUI

enum Palette {
    static let accent = dynamic(light: 0x1F5FBF, dark: 0x5CC8C2)
    /// Text on the accent.
    static let accentInk = dynamic(light: 0xFFFFFF, dark: 0x0B1A1A)
    /// Something under way.
    static let live = dynamic(light: 0xE8740C, dark: 0xF28C28)
    static let fail = dynamic(light: 0xB3261E, dark: 0xE5736A)
    /// Not reached yet.
    static let wait = dynamic(light: 0xC3C9D2, dark: 0x55585F)

    static func hex(_ value: UInt32) -> Color {
        Color(red: Double(value >> 16 & 0xFF) / 255, green: Double(value >> 8 & 0xFF) / 255, blue: Double(value & 0xFF) / 255)
    }

    private static func dynamic(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            NSColor(hex(appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light))
        })
    }
}

/// The photo chosen for them, or three rings, the same for a name on every
/// machine: an onion of one's own.
struct Avatar: View {
    let name: String
    let photo: String?
    var size: CGFloat = 36

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

enum When {
    static func date(_ millis: UInt64) -> Date { Date(timeIntervalSince1970: Double(millis) / 1000) }

    static func clock(_ millis: UInt64) -> String {
        date(millis).formatted(date: .omitted, time: .shortened)
    }

    /// "Today", "Yesterday", "Monday 5 October", "5 October 2025".
    static func day(_ millis: UInt64) -> String {
        let then = date(millis), calendar = Calendar.current
        if calendar.isDateInToday(then) { return "Today" }
        if calendar.isDateInYesterday(then) { return "Yesterday" }
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
        case 1: return "Yesterday"
        case 2...6: return then.formatted(.dateTime.weekday(.wide))
        default: return then.formatted(.dateTime.day().month(.abbreviated))
        }
    }

    static func sameDay(_ a: UInt64, _ b: UInt64) -> Bool {
        Calendar.current.isDate(date(a), inSameDayAs: date(b))
    }
}

/// Where a message to someone gets now: the steps reached, the one being
/// tried, the one that failed. Steps: this computer, the entrance (a guard,
/// or a bridge), two relays, the meeting point, their device.
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
        if route.fail != nil { return (.fail, "Tor is not getting through") }
        if route.live == 0 { return (.live, "Connecting to Tor, \(net.percent)%") }
        switch link {
        case .open: return (.done, "Connected")
        case .idle: return (.done, "Reachable")
        case .trying: return (.live, "Reaching \(name)…")
        case .unreachable: return (.wait, "Not reachable right now")
        case nil: return (.wait, "Reached when you write")
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
        case .done: Palette.accent
        case .live: Palette.live
        case .fail: Palette.fail
        case .wait: Palette.wait
        }
    }
}

/// What the bottom of the list says about Tor.
func networkSummary(_ net: Network) -> (Route.Stage, String) {
    if net.entry == false || net.circuit == false { return (.fail, "Tor is not getting through") }
    if net.percent < 100 { return (.live, "Connecting to Tor, \(net.percent)%") }
    return (.done, net.bridged ? "Connected through a bridge" : "Connected to Tor")
}
