import SwiftUI

@main
struct NightpostApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        Window("Nightpost", id: "main") {
            RootView()
                .environment(model)
                .frame(minWidth: 720, minHeight: 480)
                .task { await model.run() }
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

    var body: some View {
        Group {
            switch model.phase {
            case let .connecting(line): Splash(line: line)
            case let .locked(why): LockView(why: why)
            case let .failed(why): Splash(line: why, failed: true)
            case .ready: MainView()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Palette.ground)
        .foregroundStyle(Palette.text)
        .ignoresSafeArea()
    }
}

/// While the daemon starts, or when it could not.
struct Splash: View {
    let line: String
    var failed = false

    var body: some View {
        VStack(spacing: 14) {
            Text("Nightpost").font(.ui(46, .bold)).foregroundStyle(Palette.strong)
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
            Text("Nightpost").font(.ui(46, .bold)).foregroundStyle(Palette.strong)
            Text("Your messages are locked with a passphrase.").font(.ui(15)).foregroundStyle(Palette.muted)
            SecureField("Passphrase", text: $passphrase)
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
            Button("Unlock", action: unlock)
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
                .help(sideHidden ? "Show the list" : "Hide the list")
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
            Text("Nobody here yet").font(.ui(30, .semibold)).foregroundStyle(Palette.strong)
            Text("Nightpost only connects people who know each other. Send someone a one-time link, or read them a code on the phone.")
                .font(.ui(16))
                .lineSpacing(4)
                .foregroundStyle(Palette.soft)
            Button("Add someone") { model.dialog = .add }.buttonStyle(PrimaryButton())
        }
        .frame(maxWidth: 480, alignment: .leading)
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Palette.ground)
        .contentShape(Rectangle())
        .gesture(WindowDragGesture())
    }
}

/// The list's toggle: quiet until pointed at.
private struct ToggleButton: ButtonStyle {
    @State private var hover = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(hover ? Palette.strong : Palette.soft)
            .frame(width: 30, height: 26)
            .background(hover ? Palette.selected : .clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .contentShape(Rectangle())
            .onHover { hover = $0 }
    }
}

/// For now: what the daemon keeps. The rest comes with the settings window.
struct SettingsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            Toggle("Send read receipts", isOn: Binding(get: { model.readReceipts }, set: { on in
                Task {
                    if case let .readReceipts(now) = try? await Daemon.call(.readReceipts(on: on)) { model.readReceipts = now }
                }
            }))
            Text("They see when you read their messages, and you see when they read yours. Off on both sides unless both turn it on.")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .formStyle(.grouped)
        .frame(width: 440)
    }
}
