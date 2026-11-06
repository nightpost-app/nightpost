import SwiftUI
import UniformTypeIdentifiers

@main
struct NightpostApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        Window("Nightpost", id: "main") {
            RootView()
                .environment(model)
                .frame(minWidth: 720, minHeight: 480)
                .task { await model.run() }
                .task { await ShellCommand.offer() }
                .task { await Snapshot.run() }
        }
        .defaultSize(width: 1040, height: 700)
        // The page reaches the top edge, as on other systems; macOS keeps
        // its own buttons in the corner.
        .windowStyle(.hiddenTitleBar)
        .windowToolbarStyle(.unified(showsTitle: false))

        Settings {
            SettingsView().environment(model)
        }

        // As the tray icon on other systems: there while Nightpost runs.
        MenuBarExtra {
            MenuBarMenu().environment(model)
        } label: {
            Image(nsImage: menuBarIcon).accessibilityLabel("Nightpost")
        }
    }
}

/// The app icon's letter and crescent (icon.swift), in one ink: a template
/// image, which macOS colours for the menu bar.
let menuBarIcon: NSImage = {
    let image = NSImage(size: NSSize(width: 20, height: 18), flipped: false) { _ in
        guard let c = NSGraphicsContext.current?.cgContext else { return false }
        // icon.swift's 1024 grid, from the letter's corner to the moon's top.
        c.scaleBy(x: 18 / 560, y: 18 / 560)
        c.translateBy(x: -236, y: -278)
        let moon = CGPoint(x: 712, y: 724)
        c.setFillColor(.black)
        c.setStrokeColor(.black)
        c.beginTransparencyLayer(auxiliaryInfo: nil)
        c.setLineWidth(48)
        c.setLineJoin(.round)
        c.setLineCap(.round)
        c.addPath(CGPath(roundedRect: CGRect(x: 262, y: 300, width: 500, height: 340), cornerWidth: 44, cornerHeight: 44, transform: nil))
        c.strokePath()
        c.move(to: CGPoint(x: 290, y: 612))
        c.addLine(to: CGPoint(x: 512, y: 452))
        c.addLine(to: CGPoint(x: 734, y: 612))
        c.strokePath()
        c.setBlendMode(.clear)
        c.fillEllipse(in: CGRect(x: moon.x - 142, y: moon.y - 142, width: 284, height: 284))
        c.endTransparencyLayer()
        c.beginTransparencyLayer(auxiliaryInfo: nil)
        c.fillEllipse(in: CGRect(x: moon.x - 112, y: moon.y - 112, width: 224, height: 224))
        c.setBlendMode(.clear)
        c.fillEllipse(in: CGRect(x: moon.x - 52, y: moon.y - 40, width: 200, height: 200))
        c.endTransparencyLayer()
        return true
    }
    image.isTemplate = true
    return image
}()

/// The menu bar's menu: the window, the terminal interface, stopping.
struct MenuBarMenu: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Text(networkSummary(model.network).1)
        Divider()
        Button(t("tray-open-window")) {
            openWindow(id: "main")
            NSApp.activate()
        }
        Button(t("tray-close-window")) {
            NSApp.windows.first { $0.identifier?.rawValue.hasPrefix("main") == true }?.close()
        }
        Divider()
        Button(t("tray-open-terminal"), action: openTerminal)
        Button(t("tray-close-terminal")) {
            Task { _ = try? await Daemon.call(.close(interface: "terminal")) }
        }
        Divider()
        Button(t("tray-stop")) { model.quit() }
    }

    /// Terminal runs a .command file it is handed, with no permission to ask.
    private func openTerminal() {
        let quote = { (s: String) in "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'" }
        let cli = Bundle.main.bundleURL.appending(path: "Contents/MacOS/nightpost").path
        let dir = ProcessInfo.processInfo.environment["NIGHTPOST_DIR"].map { "NIGHTPOST_DIR=\(quote($0)) " } ?? ""
        let script = FileManager.default.temporaryDirectory.appending(path: "Nightpost.command")
        do {
            try "#!/bin/sh\n\(dir)exec \(quote(cli))\n".write(to: script, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
            Terminal.open(script)
        } catch {
            model.status = error.localizedDescription
        }
    }
}

/// The terminal the menu bar opens: one chosen in Settings, else whatever
/// opens .command files.
enum Terminal {
    static let key = "terminal"

    /// The apps that say they run .command files by name: text editors
    /// open them too, as text.
    static var installed: [URL] {
        guard let type = UTType(filenameExtension: "command") else { return [] }
        return NSWorkspace.shared.urlsForApplications(toOpen: type).filter { url in
            let types = Bundle(url: url)?.object(forInfoDictionaryKey: "CFBundleDocumentTypes") as? [[String: Any]] ?? []
            return types.contains {
                ($0["CFBundleTypeExtensions"] as? [String] ?? []).contains("command")
                    || ($0["LSItemContentTypes"] as? [String] ?? []).contains(type.identifier)
            }
        }
    }

    /// Terminals that run a program named on their command line, by bundle:
    /// how each wants it. Handed a .command file instead, Ghostty asks
    /// every time whether to run it.
    static let commandLines = [
        "com.mitchellh.ghostty": ["-e"],
        "org.alacritty": ["-e"],
        "net.kovidgoyal.kitty": [],
        "com.github.wez.wezterm": ["start", "--"],
    ]

    static func open(_ script: URL) {
        let chosen = UserDefaults.standard.string(forKey: key) ?? ""
        let app = !chosen.isEmpty && FileManager.default.fileExists(atPath: chosen)
            ? URL(fileURLWithPath: chosen)
            : NSWorkspace.shared.urlForApplication(toOpen: script)
        guard let app else {
            NSWorkspace.shared.open(script)
            return
        }
        let configuration = NSWorkspace.OpenConfiguration()
        if let id = Bundle(url: app)?.bundleIdentifier, let arguments = commandLines[id] {
            // As `open -na App --args …`: a running one would ignore them.
            configuration.arguments = arguments + [script.path]
            configuration.createsNewApplicationInstance = true
            NSWorkspace.shared.openApplication(at: app, configuration: configuration)
        } else {
            NSWorkspace.shared.open([script], withApplicationAt: app, configuration: configuration)
        }
    }
}

/// Where the window's own buttons sit, in the toolbar's row.
enum Chrome {
    /// The row's height.
    static let top: CGFloat = 52
    /// The right end of the three buttons and the list's toggle.
    static let end: CGFloat = 120
}

struct RootView: View {
    @Environment(AppModel.self) private var model
    @AppStorage("theme") private var theme = Theme.system
    @AppStorage("lockOnClose") private var lockOnClose = false

    var body: some View {
        Group {
            switch model.phase {
            case let .connecting(line): Splash(line: line)
            case let .locked(why): LockView(why: why)
            case let .failed(why): Splash(line: why, failed: true)
            case .ready: MainView()
            }
        }
        // A new language draws every word again.
        .id(model.language)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Palette.ground)
        .foregroundStyle(Palette.text)
        .ignoresSafeArea()
        .onChange(of: theme, initial: true) { theme.apply() }
        // Closing the window: the daemon stops too, if asked.
        .onDisappear { if lockOnClose { model.quit() } }
    }
}

/// While the daemon starts, or when it could not.
struct Splash: View {
    let line: String
    var failed = false

    var body: some View {
        VStack(spacing: 14) {
            Wordmark(size: 46)
            Text(verbatim: line)
                .font(.ui(15))
                .monospacedDigit()
                .foregroundStyle(failed ? Palette.fail : Palette.muted)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 480)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Rectangle())
        .gesture(WindowDragGesture())
    }
}

/// The database needs the passphrase before the daemon can start.
struct LockView: View {
    @Environment(AppModel.self) private var model
    let why: String
    @State private var passphrase = ""
    @FocusState private var typing: Bool

    var body: some View {
        VStack(spacing: 14) {
            Wordmark(size: 46)
            Text(t("mac-locked")).font(.ui(15)).foregroundStyle(Palette.muted)
            SecureField(t("mac-passphrase"), text: $passphrase)
                .textFieldStyle(.plain)
                .font(.ui(15.5))
                .padding(.horizontal, 16)
                .frame(width: 320, height: 46)
                .background(Palette.input, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Palette.line))
                .focused($typing)
                .clickToFocus($typing)
                .onSubmit(unlock)
            if !why.isEmpty {
                Text(why).font(.ui(14, .medium)).foregroundStyle(Palette.fail)
            }
            Button(t("unlock"), action: unlock)
                .buttonStyle(PrimaryButton())
                .keyboardShortcut(.defaultAction)
                .disabled(passphrase.isEmpty)
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { typing = true }
    }

    private func unlock() {
        guard !passphrase.isEmpty else { return }
        let given = passphrase
        passphrase = ""
        Task { await model.run(passphrase: given) }
    }
}

struct MainView: View {
    @Environment(AppModel.self) private var model
    /// The list is folded away (kept between launches).
    @AppStorage("sideHidden") private var sideHidden = false

    var body: some View {
        HStack(spacing: 0) {
            if !sideHidden {
                Sidebar()
                    .frame(width: 310)
                    .transition(.move(edge: .leading))
            }
            Group {
                if let name = model.selected, model.contacts.contains(where: { $0.name == name }) {
                    ConversationView(name: name, inset: sideHidden ? Chrome.end + 10 : 0).id(name)
                } else {
                    empty
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .overlay(alignment: .trailing) {
                if model.panel, let name = model.selected, model.contacts.contains(where: { $0.name == name }) {
                    PersonPanel(name: name)
                        .id(name)
                        .transition(.move(edge: .trailing))
                }
            }
            .animation(.easeOut(duration: 0.18), value: model.panel)
        }
        // Behind a dialog, the window goes out of focus, and out of reach of
        // the keyboard: typing goes to the dialog, not the composer under it.
        .disabled(model.dialog != nil)
        .blur(radius: model.dialog == nil ? 0 : 7)
        .animation(.easeOut(duration: 0.2), value: model.dialog == nil)
        .overlay { DialogLayer() }
        // A toolbar, even this small, puts the window's buttons where other
        // apps have them; its background stays ours.
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Button {
                    withAnimation(.easeInOut(duration: 0.28)) { sideHidden.toggle() }
                } label: {
                    Image(systemName: "sidebar.left")
                }
                .help(sideHidden ? t("show-list") : t("hide-list"))
            }
        }
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        // Back in front: the open conversation has now been read.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            model.markRead()
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(t("empty-title")).font(.ui(30, .semibold)).foregroundStyle(Palette.strong)
            Text(t("empty-help"))
                .font(.ui(16))
                .lineSpacing(4)
                .foregroundStyle(Palette.soft)
            Button(t("add-someone")) { model.dialog = .add }.buttonStyle(PrimaryButton())
        }
        .frame(maxWidth: 480, alignment: .leading)
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Palette.ground)
        .contentShape(Rectangle())
        .gesture(WindowDragGesture())
    }
}
