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
        }
        .defaultSize(width: 1040, height: 700)
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        switch model.phase {
        case let .connecting(line): Splash(line: line)
        case let .locked(why): LockView(why: why)
        case let .failed(why): Splash(line: why, failed: true)
        case .ready: MainView()
        }
    }
}

/// While the daemon starts, or when it could not.
struct Splash: View {
    let line: String
    var failed = false

    var body: some View {
        VStack(spacing: 14) {
            Text("Nightpost").font(.system(size: 34, weight: .bold, design: .rounded))
            if failed {
                Text(line).foregroundStyle(.red).multilineTextAlignment(.center)
            } else {
                ProgressView().controlSize(.small)
                Text(line).foregroundStyle(.secondary).monospacedDigit()
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// The database needs the passphrase before the daemon can start.
struct LockView: View {
    @Environment(AppModel.self) private var model
    let why: String
    @State private var passphrase = ""

    var body: some View {
        VStack(spacing: 14) {
            Text("Nightpost").font(.system(size: 34, weight: .bold, design: .rounded))
            Text("Your messages are locked with a passphrase.").foregroundStyle(.secondary)
            SecureField("Passphrase", text: $passphrase)
                .textFieldStyle(.roundedBorder)
                .frame(width: 280)
                .onSubmit(unlock)
            if !why.isEmpty {
                Text(why).foregroundStyle(.red)
            }
            Button("Unlock", action: unlock)
                .keyboardShortcut(.defaultAction)
                .disabled(passphrase.isEmpty)
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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

    var body: some View {
        NavigationSplitView {
            Sidebar()
        } detail: {
            if let name = model.selected, model.contacts.contains(where: { $0.name == name }) {
                ConversationView(name: name).id(name)
            } else {
                ContentUnavailableView(
                    model.contacts.isEmpty ? "Nobody here yet" : "Pick someone",
                    systemImage: "bubble.left.and.bubble.right",
                    description: Text("Nightpost only connects people who know each other.")
                )
            }
        }
        .tint(Palette.accent)
        // Back in front: the open conversation has now been read.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            model.markRead()
        }
    }
}
