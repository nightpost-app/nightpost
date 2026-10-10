// The settings window, with the sections of the GUI on other systems:
// appearance, network and bridges, notifications, security, about.

import AppKit
import SwiftUI

enum Theme: String {
    case system, dark, light

    @MainActor func apply() {
        NSApp.appearance = switch self {
        case .system: nil
        case .dark: NSAppearance(named: .darkAqua)
        case .light: NSAppearance(named: .aqua)
        }
    }
}

struct SettingsView: View {
    var body: some View {
        TabView {
            Tab("Appearance", systemImage: "circle.lefthalf.filled") { Pane { AppearancePane() } }
            Tab("Network", systemImage: "chart.bar") { Pane { NetworkPane() } }
            Tab("Notifications", systemImage: "bell") { Pane { NotificationsPane() } }
            Tab("Security", systemImage: "lock") { Pane { SecurityPane() } }
            Tab("About", systemImage: "info.circle") { Pane { AboutPane() } }
        }
        .frame(width: 560)
        .foregroundStyle(Palette.text)
    }
}

/// One section's page, scrolling if it has to.
private struct Pane<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 28) { content }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(28)
        }
        .frame(minHeight: 380, idealHeight: 520)
        .background(Palette.ground)
    }
}

/// A heading and what goes under it.
private struct Block<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(verbatim: title).font(.ui(18, .semibold)).foregroundStyle(Palette.strong)
            content
        }
    }
}

/// style.css's `.card`.
private struct Card<Leading: View>: View {
    let title: String
    let help: String
    @ViewBuilder let leading: Leading

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            leading
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: title).font(.ui(15, .semibold)).foregroundStyle(Palette.strong)
                Help(help)
            }
            Spacer(minLength: 0)
        }
        .padding(16)
        .background(Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Palette.line))
    }
}

/// style.css's `.toggle-row`.
private struct ToggleRow: View {
    let title: String
    let help: String
    @Binding var on: Bool

    var body: some View {
        Toggle(isOn: $on) {
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: title).font(.ui(15, .semibold)).foregroundStyle(Palette.strong)
                Help(help)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .toggleStyle(.switch)
        .tint(Palette.main)
        .padding(16)
        .background(Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Palette.line))
    }
}

/// style.css's `.choice`: a radio button with its text.
private struct Choice<Detail: View>: View {
    let title: String
    let on: Bool
    let pick: () -> Void
    @ViewBuilder let detail: Detail

    var body: some View {
        Button(action: pick) {
            HStack(alignment: .top, spacing: 12) {
                Circle()
                    .strokeBorder(on ? Palette.main : Palette.muted, lineWidth: on ? 5 : 2)
                    .frame(width: 18, height: 18)
                    .padding(.top, 1)
                VStack(alignment: .leading, spacing: 4) {
                    Text(verbatim: title).font(.ui(15, .semibold)).foregroundStyle(Palette.strong)
                    detail
                }
                Spacer(minLength: 0)
            }
            .padding(14)
            .background(on ? Palette.selected : Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(on ? Palette.main : Palette.line))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }
}

private struct Warning: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(verbatim: text).font(.ui(14, .semibold)).foregroundStyle(Palette.fail).fixedSize(horizontal: false, vertical: true)
    }
}

private struct AppearancePane: View {
    @AppStorage("theme") private var theme = Theme.system

    var body: some View {
        Block(title: "Theme") {
            HStack(spacing: 10) {
                swatch(.system, "As the system", 0x0F1522, 0xF4F5F7)
                swatch(.dark, "Dark", 0x0F1522, 0x111A29)
                swatch(.light, "Light", 0xF4F5F7, 0xFFFFFF)
            }
            Help("“As the system” follows your Mac’s light or dark mode, and changes with it.")
        }
    }

    private func swatch(_ option: Theme, _ label: String, _ a: UInt32, _ b: UInt32) -> some View {
        let on = theme == option
        return Button { theme = option } label: {
            VStack(spacing: 10) {
                HStack(spacing: 0) { Palette.hex(a); Palette.hex(b) }
                    .frame(height: 54)
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Palette.line2))
                Text(verbatim: label).font(.ui(14, on ? .semibold : .medium)).foregroundStyle(on ? Palette.strong : Palette.soft)
            }
            .padding(10)
            .background(on ? Palette.selected : Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(on ? Palette.main : Palette.line, lineWidth: on ? 2 : 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }
}

private struct NetworkPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let net = model.network
        let (stage, title) = networkSummary(net)
        let line = net.line.hasPrefix("Tor: ") ? String(net.line.dropFirst(5)) : net.line
        let detail = switch stage {
        case .fail: line
        case .live: line.isEmpty ? "\(net.percent)%" : line
        default: net.bridged ? "This network does not see that you use Tor." : "Messages pass three relays before they reach anyone."
        }
        Block(title: "Connection") {
            Card(title: title, help: detail) {
                Circle().fill(stage.color).frame(width: 12, height: 12).padding(.top, 5)
            }
        }
        Block(title: "Bridges") { BridgesEditor() }
    }
}

/// Snowflake when needed, or lines of one's own, kept by the daemon in the
/// database; Tor reconnects through them without a restart.
private struct BridgesEditor: View {
    @State private var own = false
    @State private var text = ""
    @State private var error = ""
    @State private var saved = ""

    var body: some View {
        Help("Some networks (university or work Wi-Fi) block Tor. Nightpost then goes in through a bridge, an entrance that is not on the public list.")
        Choice(title: "Built-in, when needed", on: !own, pick: { pick(false) }) {
            Help("Snowflake, nothing to set up. Used only if Tor does not get through.")
        }
        Choice(title: "Bridges of your own", on: own, pick: { pick(true) }) {
            Help("From a friend, bridges.torproject.org (WebTunnel gets through most often), or your own server. Used all the time.")
        }
        if own {
            Text("Bridge lines, one per line").font(.ui(13.5)).foregroundStyle(Palette.soft)
            TextEditor(text: $text)
                .font(.ui(13))
                .scrollContentBackground(.hidden)
                .padding(8)
                .frame(height: 96)
                .background(Palette.input, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Palette.line2))
                .onChange(of: text) { saved = "" }
        }
        if !error.isEmpty { Warning(error) }
        Button("Save bridges", action: save).buttonStyle(PrimaryButton())
        if !saved.isEmpty { Help(saved).accessibilityAddTraits(.updatesFrequently) }
        Color.clear.frame(height: 0).task {
            if case let .bridges(lines) = try? await Daemon.call(.bridges(lines: nil)) {
                own = !lines.isEmpty
                text = lines.joined(separator: "\n")
            }
        }
    }

    private func pick(_ mine: Bool) {
        own = mine
        saved = ""
    }

    private func save() {
        let lines = own ? text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init) : []
        Task {
            do {
                switch try await Daemon.call(.bridges(lines: lines)) {
                case let .bridges(kept):
                    error = ""
                    saved = kept.isEmpty ? "Back to the built-in bridges." : "Bridges saved: Tor reconnects through them."
                case let .error(message):
                    saved = ""
                    error = message
                case let other:
                    error = "unexpected reply \(other)"
                }
            } catch {
                saved = ""
                self.error = "Nightpost's daemon does not answer: \(error.localizedDescription)"
            }
        }
    }
}

private struct NotificationsPane: View {
    @AppStorage("notify") private var notify = true
    @AppStorage("sound") private var sound = false
    @AppStorage("shown") private var shown = Shown.who.rawValue

    var body: some View {
        Block(title: "Notifications") {
            ToggleRow(title: "Show notifications", help: "When a message arrives and Nightpost is not in front.", on: $notify)
            ToggleRow(title: "Play a sound", help: "A short chime with each notification.", on: $sound)
        }
        Block(title: "What a notification shows") {
            choice(.all, "Who and what", "Léa", "See you at eight?")
            choice(.who, "Who wrote", "Nightpost", "Message from Léa")
            choice(.nothing, "Nothing", "Nightpost", "New message")
            Help("A notification can show on a locked screen.")
        }
    }

    private func choice(_ option: Shown, _ label: String, _ title: String, _ body: String) -> some View {
        Choice(title: label, on: shown == option.rawValue, pick: { shown = option.rawValue }) {
            VStack(alignment: .leading, spacing: 1) {
                Text(verbatim: title).font(.ui(13, .semibold)).foregroundStyle(Palette.strong)
                Text(verbatim: body).font(.ui(13)).foregroundStyle(Palette.soft)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(Palette.tip, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Palette.line))
        }
    }
}

private struct SecurityPane: View {
    enum Change { case add, replace, remove }

    @Environment(AppModel.self) private var model
    @AppStorage("lockOnClose") private var lockOnClose = false
    @State private var has = Daemon.usesPassphrase
    @State private var change: Change?
    @State private var current = ""
    @State private var new = ""
    @State private var again = ""
    @State private var error = ""
    @State private var busy = false
    @State private var done = ""

    var body: some View {
        Block(title: "How your messages are locked") {
            Card(
                title: has ? "With your keychain and a passphrase" : "With your system keychain",
                help: has
                    ? "Your messages and keys are stored encrypted. Nightpost asks for the passphrase each time it starts."
                    : "Your messages and keys are stored encrypted. The key opens when you log in to this Mac."
            ) {
                Icon(parts: [.rect(5, 11, 14, 10, radius: 2), .path("M8 11V8a4 4 0 0 1 8 0v3")], weight: 1.8)
                    .foregroundStyle(Palette.main)
                    .frame(width: 22, height: 22)
            }
            if change == nil {
                HStack(spacing: 8) {
                    if has {
                        Button("Change the passphrase") { open(.replace) }.buttonStyle(SecondaryButton())
                        Button("Remove the passphrase") { open(.remove) }.buttonStyle(SecondaryButton())
                    } else {
                        Button("Add a passphrase") { open(.add) }.buttonStyle(SecondaryButton())
                    }
                }
                if !done.isEmpty { Help(done) }
            }
        }
        if let change { form(change) }
        Block(title: "Read receipts") {
            ToggleRow(
                title: "Send read receipts",
                help: "On: people see when you have read their messages, and you see when they read yours, if they have it on too. Off: neither.",
                on: Binding(get: { model.readReceipts }, set: { on in
                    Task {
                        switch try? await Daemon.call(.readReceipts(on: on)) {
                        case let .readReceipts(now): model.readReceipts = now
                        case let .error(message): model.status = message
                        default: break
                        }
                    }
                })
            )
        }
        Block(title: "Leaving") {
            ToggleRow(
                title: has ? "Closing the window locks Nightpost" : "Closing the window quits Nightpost",
                help: has
                    ? "Messages stop arriving, and your passphrase is asked for when you open Nightpost again."
                    : "Messages stop arriving until you open Nightpost again. Add a passphrase above for this to lock it.",
                on: $lockOnClose
            )
            Button(has ? "Quit and lock" : "Quit Nightpost") { model.quit() }.buttonStyle(SecondaryButton())
        }
    }

    private func form(_ change: Change) -> some View {
        let (title, action) = switch change {
        case .add: ("Add a passphrase", "Add the passphrase")
        case .replace: ("Change the passphrase", "Change it")
        case .remove: ("Remove the passphrase", "Remove it")
        }
        return Block(title: title) {
            if change != .add { field("Current passphrase", $current) }
            if change != .remove {
                field("New passphrase", $new)
                field("The same again", $again)
                Warning("Forget it and your messages are gone: nobody can recover it, not even with this Mac.")
            } else {
                Help("The keychain alone will open your messages again.")
            }
            if !error.isEmpty { Warning(error) }
            HStack(spacing: 8) {
                Button(action) { save(change) }
                .buttonStyle(PrimaryButton(height: 40, radius: 8))
                .disabled(busy)
                Button("Cancel") { open(nil) }.buttonStyle(SecondaryButton())
            }
        }
    }

    private func field(_ label: String, _ text: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(verbatim: label).font(.ui(13.5)).foregroundStyle(Palette.soft)
            SecureField("", text: text)
                .textFieldStyle(.plain)
                .font(.ui(15))
                .padding(.horizontal, 12)
                .frame(width: 320, height: 38)
                .background(Palette.input, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Palette.line2))
        }
    }

    private func open(_ next: Change?) {
        change = next
        (current, new, again, error, done) = ("", "", "", "", "")
    }

    private func save(_ change: Change) {
        let wants = change != .remove
        if wants && new.isEmpty { error = "Choose a passphrase."; return }
        if wants && new != again { error = "The two passphrases differ."; return }
        busy = true
        let request = Request.setPassphrase(current: change == .add ? nil : current, new: wants ? new : nil)
        Task {
            switch try? await Daemon.call(request) {
            case .done:
                has = wants
                open(nil)
                done = wants ? "Passphrase saved." : "Passphrase removed."
            case let .error(message): error = message
            default: error = "Nightpost's daemon does not answer."
            }
            busy = false
        }
    }
}

private struct AboutPane: View {
    var body: some View {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        Block(title: "Nightpost") {
            Text(verbatim: version).font(.ui(14)).monospacedDigit().foregroundStyle(Palette.muted)
            Help("Messages between people who know each other, over Tor, with no server in between.")
        }
        Block(title: "Your data") {
            row("Folder", Daemon.dir.path)
            Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([Daemon.dir]) }
                .buttonStyle(SecondaryButton())
        }
        Block(title: "Licences") {
            row("Nightpost", "AGPL-3.0")
            row("Space Grotesk", "SIL Open Font License 1.1")
            row("lyrebird (bridges)", "BSD-3-Clause, Tor Project")
            row("Icons", "Lucide, ISC")
        }
    }

    private func row(_ key: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 16) {
            Text(verbatim: key).font(.ui(14)).foregroundStyle(Palette.muted).frame(width: 150, alignment: .leading)
            Text(verbatim: value).font(.ui(14)).foregroundStyle(Palette.text).textSelection(.enabled)
        }
    }
}
