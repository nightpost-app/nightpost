// The settings window, with the sections of the GUI on other systems:
// appearance, network and bridges, notifications, security, about.

import AppKit
import SwiftUI
import UniformTypeIdentifiers

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
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        TabView(selection: $model.settingsTab) {
            Tab(t("appearance"), systemImage: "circle.lefthalf.filled", value: "appearance") { Pane { AppearancePane() } }
            Tab(t("network"), systemImage: "chart.bar", value: "network") { Pane { NetworkPane() } }
            Tab(t("devices-title"), systemImage: "laptopcomputer.and.iphone", value: "devices") { Pane { DevicesPane() } }
            Tab(t("accounts-title"), systemImage: "person.2", value: "accounts") { Pane { AccountsPane() } }
            Tab(t("notifications"), systemImage: "bell", value: "notifications") { Pane { NotificationsPane() } }
            Tab(t("security"), systemImage: "lock", value: "security") { Pane { SecurityPane() } }
            Tab(t("about"), systemImage: "info.circle", value: "about") { Pane { AboutPane() } }
        }
        .id(model.language)
        .frame(width: 640)
        .foregroundStyle(Palette.text)
        .background(WindowGround())
    }
}

/// The tabs' bar in the page's colour rather than the system's grey.
private struct WindowGround: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { Host() }
    func updateNSView(_ view: NSView, context: Context) {}

    final class Host: NSView {
        override func viewDidMoveToWindow() {
            guard let window else { return }
            window.titlebarAppearsTransparent = true
            window.backgroundColor = Palette.groundNS
        }
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
        // macOS lets a scroll view's content run on under the toolbar; with
        // the tabs' bar transparent, it would show through the tabs.
        .clipped()
        // The same size on every tab: the window does not jump from one to the next.
        .frame(height: 600)
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
                if !help.isEmpty { Help(help) }
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
    @Environment(AppModel.self) private var model
    @AppStorage("theme") private var theme = Theme.system
    @AppStorage(Terminal.key) private var terminal = ""
    /// Kept here: choosing the language already spoken redraws nothing else.
    @State private var chosen = Words.chosen

    var body: some View {
        Block(title: t("theme")) {
            HStack(spacing: 10) {
                swatch(.system, t("theme-system"), 0x0F1522, 0xF4F5F7)
                swatch(.dark, t("theme-dark"), 0x0F1522, 0x111A29)
                swatch(.light, t("theme-light"), 0xF4F5F7, 0xFFFFFF)
            }
            Help(t("mac-theme-help"))
        }
        Block(title: t("language")) {
            // Buttons like the theme cards above, not the system’s menu. (Rounder
            // than 12 points, cacheDisplay draws stray edges at their ends.)
            HStack(spacing: 10) {
                pill(nil, t("theme-system"))
                ForEach(Words.names, id: \.code) { pill($0.code, $0.name) }
            }
            Help(t("language-help"))
        }
        Block(title: t("terminal")) {
            HStack(spacing: 10) {
                terminalPill("", t("theme-system"))
                ForEach(terminals, id: \.path) { terminalPill($0.path, $0.deletingPathExtension().lastPathComponent) }
                Button(t("terminal-other"), action: pickTerminal).buttonStyle(.plain)
                    .font(.ui(14, .medium)).foregroundStyle(Palette.soft)
                    .padding(.horizontal, 16).frame(height: 34)
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.line2))
                    .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .fixedSize()
            }
            Help(t("mac-terminal-help"))
        }
    }

    /// Those that say they run .command files, and one picked by hand.
    private var terminals: [URL] {
        let found = Terminal.installed
        guard !terminal.isEmpty, !found.contains(where: { $0.path == terminal }) else { return found }
        return found + [URL(fileURLWithPath: terminal)]
    }

    /// Any app: macOS hands it the .command file.
    private func pickTerminal() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.application]
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        if panel.runModal() == .OK, let url = panel.url { terminal = url.path }
    }

    private func terminalPill(_ path: String, _ label: String) -> some View {
        let on = terminal == path
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        return Button { terminal = path } label: {
            Text(verbatim: label)
                .font(.ui(14, on ? .semibold : .medium))
                .foregroundStyle(on ? Palette.strong : Palette.soft)
                .padding(.horizontal, 16)
                .frame(height: 34)
                .background(on ? Palette.selected : .clear, in: shape)
                .overlay(shape.strokeBorder(on ? Palette.main : Palette.line2, lineWidth: on ? 2 : 1))
                .contentShape(shape)
        }
        .buttonStyle(.plain)
        .fixedSize()
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }

    private func pill(_ code: String?, _ label: String) -> some View {
        let on = chosen == code
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        return Button { choose(code) } label: {
            Text(verbatim: label)
                .font(.ui(14, on ? .semibold : .medium))
                .foregroundStyle(on ? Palette.strong : Palette.soft)
                .padding(.horizontal, 16)
                .frame(height: 34)
                .background(on ? Palette.selected : .clear, in: shape)
                .overlay(shape.strokeBorder(on ? Palette.main : Palette.line2, lineWidth: on ? 2 : 1))
                .contentShape(shape)
        }
        .buttonStyle(.plain)
        .fixedSize()
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }

    private func choose(_ code: String?) {
        Words.choose(code)
        chosen = Words.chosen
        model.language = Words.language
        // The daemon's words too: errors, Tor's progress.
        Task { _ = try? await Daemon.call(.language) }
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
        default: net.bridged ? t("net-bridged-help") : t("net-connected-help")
        }
        Block(title: t("connection")) {
            Card(title: title, help: detail) {
                Circle().fill(stage.color).frame(width: 12, height: 12).padding(.top, 5)
            }
        }
        Block(title: t("bridges")) { BridgesEditor() }
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
        Help(t("bridges-help"))
        Choice(title: t("bridges-builtin"), on: !own, pick: { pick(false) }) {
            Help(t("bridges-builtin-help"))
        }
        Choice(title: t("bridges-own"), on: own, pick: { pick(true) }) {
            Help(t("bridges-own-help"))
        }
        if own {
            Text(t("bridges-lines")).font(.ui(13.5)).foregroundStyle(Palette.soft)
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
        Button(t("bridges-save"), action: save).buttonStyle(PrimaryButton())
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
                    saved = kept.isEmpty ? t("bridges-reset") : t("bridges-saved")
                case let .error(message):
                    saved = ""
                    error = message
                case let other:
                    error = t("err-unexpected", ["reply": other])
                }
            } catch {
                saved = ""
                self.error = t("daemon-silent", ["error": error.localizedDescription])
            }
        }
    }
}

private struct NotificationsPane: View {
    @AppStorage("notify") private var notify = true
    @AppStorage("sound") private var sound = false
    @AppStorage("shown") private var shown = Shown.who.rawValue

    var body: some View {
        Block(title: t("notifications")) {
            ToggleRow(title: t("notify-show"), help: t("notify-show-help"), on: $notify)
            ToggleRow(title: t("notify-sound"), help: t("notify-sound-help"), on: $sound)
        }
        Block(title: t("notify-shows")) {
            choice(.all, t("notify-who-what"), t("notify-example-name"), t("notify-example-text"))
            choice(.who, t("notify-who"), "Nightpost", t("notify-from", ["name": t("notify-example-name")]))
            choice(.nothing, t("notify-nothing"), "Nightpost", t("notify-new"))
            Help(t("notify-locked"))
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

/// This account's devices: list, add, remove; and, on a new device, the
/// link for one of them to use.
/// The accounts of this Nightpost, each with its own app instance (and
/// daemon): opening one starts it, or brings it forward.
private struct AccountsPane: View {
    @Environment(AppModel.self) private var model
    @State private var accounts = Daemon.accounts()
    @State private var reopen = Daemon.reopensLast
    @State private var name = ""
    @State private var error = ""

    private func open(_ account: String) { Daemon.open(account) }

    private func add() {
        let label = name.trimmingCharacters(in: .whitespaces)
        guard !label.isEmpty, let binary = Bundle.main.url(forAuxiliaryExecutable: "nightpost") else { return }
        let process = Process()
        process.executableURL = binary
        process.arguments = ["accounts", "add", label]
        process.environment = ProcessInfo.processInfo.environment.filter { $0.key != "NIGHTPOST_ACCOUNT" }
        let output = Pipe()
        process.standardError = output
        process.standardOutput = FileHandle.nullDevice
        do { try process.run() } catch { self.error = error.localizedDescription; return }
        process.waitUntilExit()
        if process.terminationStatus == 0 {
            name = ""
            error = ""
            accounts = Daemon.accounts()
            model.status = t("account-added", ["name": label])
            open(label)
        } else {
            error = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        }
    }

    var body: some View {
        Block(title: t("accounts-title")) {
            Help(t("accounts-help"))
            ForEach(accounts, id: \.self) { account in
                let this = account == (Daemon.account ?? "default")
                HStack(spacing: 14) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(verbatim: account == "default" ? t("account-default") : account).font(.ui(15, .semibold)).foregroundStyle(Palette.strong)
                        if this { Help(t("account-this")) }
                    }
                    Spacer(minLength: 0)
                    if !this { Button(t("account-open")) { open(account) }.buttonStyle(SecondaryButton()) }
                }
                .padding(16)
                .background(Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Palette.line))
            }
        }
        ToggleRow(title: t("choose-remember"), help: "", on: $reopen)
            .onChange(of: reopen) { Daemon.reopensLast = reopen }
        Block(title: t("account-add")) {
            Help(t("account-add-help"))
            TextField("", text: $name, prompt: Text(verbatim: t("account-name")).foregroundStyle(Palette.muted))
                .textFieldStyle(.plain)
                .font(.ui(15))
                .padding(.horizontal, 12)
                .frame(height: 38)
                .background(Palette.input, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Palette.line2))
                .onSubmit(add)
            if !error.isEmpty { Text(verbatim: error).font(.ui(14, .semibold)).foregroundStyle(Palette.fail) }
            Button(t("account-add"), action: add).buttonStyle(PrimaryButton(height: 40, radius: 8))
        }
    }
}

private struct DevicesPane: View {
    @Environment(AppModel.self) private var model
    @State private var devices: [DeviceInfo] = []
    @State private var link = ""
    @State private var pairing: String?
    @State private var removing: DeviceInfo?
    @State private var busy = false
    @State private var error = ""

    var body: some View {
        Block(title: t("devices-title")) {
            ForEach(devices) { d in
                HStack(spacing: 14) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(verbatim: d.name).font(.ui(15, .semibold)).foregroundStyle(Palette.strong)
                        let what = [d.this ? t("device-this") : nil, d.mailbox ? t("device-mailbox") : nil].compactMap { $0 }
                        if !what.isEmpty { Help(what.joined(separator: ", ")) }
                    }
                    Spacer(minLength: 0)
                    if !d.this {
                        Button(t("remove-it")) { removing = d }.buttonStyle(SecondaryButton())
                    }
                }
                .padding(16)
                .background(Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Palette.line))
            }
            if let d = removing {
                Text(verbatim: t("device-remove-help", ["name": d.name])).font(.ui(14)).foregroundStyle(Palette.fail)
                HStack(spacing: 8) {
                    Button(t("device-remove", ["name": d.name])) {
                        removing = nil
                        Task {
                            switch try? await Daemon.call(.removeDevice(id: d.id)) {
                            case .done: model.status = t("device-removed"); await load()
                            case let .error(message): error = message
                            default: break
                            }
                        }
                    }
                    .buttonStyle(DangerButton())
                    Button(t("cancel")) { removing = nil }.buttonStyle(SecondaryButton())
                }
            }
        }
        Block(title: t("device-add")) {
            Help(t("device-add-help"))
            TextField("", text: $link, prompt: Text(verbatim: "nightpost:device:…").foregroundStyle(Palette.muted))
                .textFieldStyle(.plain)
                .font(.ui(15))
                .padding(.horizontal, 12)
                .frame(height: 38)
                .background(Palette.input, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Palette.line2))
                .onSubmit(add)
            if !error.isEmpty { Text(verbatim: error).font(.ui(14, .semibold)).foregroundStyle(Palette.fail) }
            Button(busy ? t("device-adding") : t("device-add"), action: add)
                .buttonStyle(PrimaryButton(height: 40, radius: 8))
                .disabled(busy)
        }
        // Nothing here yet (the note alone): this one may join another account.
        if model.contacts.count <= 1 {
            Block(title: t("pair-title")) {
                Help(t("pair-gui-help"))
                if let pairing {
                    if let qr = QR.image(pairing) {
                        Image(nsImage: qr)
                            .interpolation(.none)
                            .resizable()
                            .frame(width: 232, height: 232)
                            .clipShape(RoundedRectangle(cornerRadius: 6))
                            .accessibilityLabel(t("invite-qr"))
                    }
                    Text(verbatim: pairing).font(.ui(12.5)).textSelection(.enabled)
                    CopyButton(text: pairing, label: t("copy-link"))
                } else {
                    Button(t("pair-show")) {
                        Task {
                            switch try? await Daemon.call(.pairingLink) {
                            case let .invite(link): pairing = link
                            case let .error(message): error = message
                            default: break
                            }
                        }
                    }
                    .buttonStyle(SecondaryButton())
                }
            }
        }
        EmptyView()
            .task(id: model.devicesVersion) { await load() }
    }

    private func load() async {
        if case let .devices(list) = try? await Daemon.call(.devices) { devices = list }
    }

    private func add() {
        let text = link.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !busy else { return }
        (busy, error) = (true, "")
        Task {
            switch try? await Daemon.call(.addDevice(link: text)) {
            case let .added(name):
                link = ""
                model.status = t("device-added", ["name": name])
                await load()
            case let .error(message): error = message
            default: error = t("err-unexpected", ["reply": "?"])
            }
            busy = false
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
    /// The recovery words while shown.
    @State private var words: [String]?

    var body: some View {
        Block(title: t("how-locked")) {
            Card(
                title: has ? t("locked-both") : t("locked-keychain"),
                help: has
                    ? t("locked-both-help")
                    : t("mac-locked-keychain-help")
            ) {
                Icon(parts: [.rect(5, 11, 14, 10, radius: 2), .path("M8 11V8a4 4 0 0 1 8 0v3")], weight: 1.8)
                    .foregroundStyle(Palette.main)
                    .frame(width: 22, height: 22)
            }
            if change == nil {
                HStack(spacing: 8) {
                    if has {
                        Button(t("passphrase-change")) { open(.replace) }.buttonStyle(SecondaryButton())
                        Button(t("passphrase-remove")) { open(.remove) }.buttonStyle(SecondaryButton())
                    } else {
                        Button(t("passphrase-add")) { open(.add) }.buttonStyle(SecondaryButton())
                    }
                }
                if !done.isEmpty { Help(done) }
            }
        }
        if let change { form(change) }
        Block(title: t("recovery-title")) {
            Help(t("recovery-help"))
            if let words {
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), alignment: .leading), count: 3), alignment: .leading, spacing: 6) {
                    ForEach(Array(words.enumerated()), id: \.offset) { i, word in
                        Text(verbatim: "\(i + 1). \(word)").font(.ui(14)).foregroundStyle(Palette.strong)
                    }
                }
                .textSelection(.enabled)
            }
            Button(words == nil ? t("recovery-show") : t("recovery-hide")) {
                if words != nil { words = nil; return }
                Task { words = await model.recoveryPhrase()?.split(whereSeparator: \.isWhitespace).map(String.init) }
            }
            .buttonStyle(SecondaryButton())
        }
        Block(title: t("read-receipts")) {
            ToggleRow(
                title: t("read-receipts-send"),
                help: t("read-receipts-help"),
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
        Block(title: t("typing-indicators")) {
            ToggleRow(
                title: t("typing-indicators-send"),
                help: t("typing-indicators-help"),
                on: Binding(get: { model.typingIndicators }, set: { on in
                    Task {
                        switch try? await Daemon.call(.typingIndicators(on: on)) {
                        case let .typingIndicators(now): model.typingIndicators = now
                        case let .error(message): model.status = message
                        default: break
                        }
                    }
                })
            )
        }
        Block(title: t("leaving")) {
            ToggleRow(
                title: has ? t("close-locks") : t("close-quits"),
                help: has
                    ? t("leave-lock-help")
                    : t("leave-quit-help"),
                on: $lockOnClose
            )
            Button(has ? t("quit-lock") : t("quit")) { model.quit() }.buttonStyle(SecondaryButton())
        }
    }

    private func form(_ change: Change) -> some View {
        let (title, action) = switch change {
        case .add: (t("passphrase-add"), t("passphrase-add-it"))
        case .replace: (t("passphrase-change"), t("change-it"))
        case .remove: (t("passphrase-remove"), t("remove-it"))
        }
        return Block(title: title) {
            if change != .add { field(t("passphrase-current"), $current) }
            if change != .remove {
                field(t("passphrase-new"), $new)
                field(t("passphrase-again"), $again)
                Warning(t("mac-passphrase-warning"))
            } else {
                Help(t("passphrase-remove-help"))
            }
            if !error.isEmpty { Warning(error) }
            HStack(spacing: 8) {
                Button(action) { save(change) }
                .buttonStyle(PrimaryButton(height: 40, radius: 8))
                .disabled(busy)
                Button(t("cancel")) { open(nil) }.buttonStyle(SecondaryButton())
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
        if wants && new.isEmpty { error = t("need-passphrase"); return }
        if wants && new != again { error = t("passphrases-differ"); return }
        busy = true
        let request = Request.setPassphrase(current: change == .add ? nil : current, new: wants ? new : nil)
        Task {
            switch try? await Daemon.call(request) {
            case .done:
                has = wants
                open(nil)
                done = wants ? t("passphrase-saved") : t("passphrase-removed")
            case let .error(message): error = message
            default: error = t("mac-daemon-silent")
            }
            busy = false
        }
    }
}

private struct AboutPane: View {
    @State private var command = ShellCommand.installed
    @State private var failed = false

    var body: some View {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        // The logo and the name, then the version on their line, as in the GUI.
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .lastTextBaseline, spacing: 12) {
                Wordmark(size: 32)
                // "0.1.0-alpha.1": the number, then a badge saying it is not stable yet.
                let parts = version.split(separator: "-", maxSplits: 1).map(String.init)
                HStack(alignment: .center, spacing: 6) {
                    Text(verbatim: parts[0]).font(.ui(14)).monospacedDigit().foregroundStyle(Palette.muted)
                    if parts.count > 1, let pre = parts[1].split(separator: ".").first {
                        Text(verbatim: String(pre)).font(.ui(10).weight(.semibold)).foregroundStyle(Palette.main)
                            .padding(.horizontal, 5).padding(.vertical, 1)
                            .overlay(RoundedRectangle(cornerRadius: 5).stroke(Palette.main, lineWidth: 1))
                    }
                }
            }
            Help(t("tagline"))
        }
        Block(title: t("your-data")) {
            row(t("folder"), Daemon.dir.path)
            Button(t("mac-show-in-finder")) { NSWorkspace.shared.activateFileViewerSelecting([Daemon.dir]) }
                .buttonStyle(SecondaryButton())
        }
        Block(title: t("mac-command-section")) {
            Help(command ? t("mac-command-installed") : t("mac-command-missing"))
            Button(command ? t("mac-command-uninstall") : t("mac-command-install")) {
                Task {
                    let was = command
                    let done = was ? await ShellCommand.uninstall() : await ShellCommand.install()
                    command = ShellCommand.installed
                    failed = !done && command == was
                }
            }
            .buttonStyle(SecondaryButton())
            if failed { Warning(t("mac-command-failed")) }
        }
        Block(title: t("licences")) {
            row("Nightpost", "AGPL-3.0")
            row("Space Grotesk", "SIL Open Font License 1.1")
            row(t("licence-bridges"), "BSD-3-Clause, Tor Project")
            row(t("licence-icons"), "Lucide, ISC")
        }
    }

    private func row(_ key: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 16) {
            Text(verbatim: key).font(.ui(14)).foregroundStyle(Palette.muted).frame(width: 150, alignment: .leading)
            Text(verbatim: value).font(.ui(14)).foregroundStyle(Palette.text).textSelection(.enabled)
        }
    }
}


/// Which account to open, when there are several and none was asked for:
/// the pick starts that account's instance of the app, and this one goes.
struct ChooserView: View {
    @State private var reopen = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Wordmark(size: 32).frame(maxWidth: .infinity, alignment: .center)
            Text(verbatim: t("choose-account")).font(.ui(20, .semibold)).foregroundStyle(Palette.strong)
            ForEach(Daemon.accounts(), id: \.self) { account in
                Button {
                    Daemon.reopensLast = reopen
                    Daemon.open(account)
                    NSApp.terminate(nil)
                } label: {
                    Text(verbatim: account == "default" ? t("account-default") : account).frame(maxWidth: .infinity)
                }
                .buttonStyle(SecondaryButton())
            }
            ToggleRow(title: t("choose-remember"), help: "", on: $reopen)
            Spacer(minLength: 0)
        }
        .padding(28)
        .frame(maxWidth: 420, maxHeight: .infinity, alignment: .topLeading)
        .frame(maxWidth: .infinity)
        .background(Palette.ground)
    }
}
