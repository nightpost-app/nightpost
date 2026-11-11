import AppKit
import Foundation

/// The `nightpost` command, in the Terminal: a link in /usr/local/bin to the
/// program inside the app (the .dmg only carries the app). Never made behind
/// the person's back: offered once at the first launch, and a button in
/// Settings, About, installs or removes it. macOS asks for the password
/// when the folder is not the person's own.
enum ShellCommand {
    private static let link = "/usr/local/bin/nightpost"
    private static let asked = "commandAsked"

    private static var target: String? { Bundle.main.url(forAuxiliaryExecutable: "nightpost")?.path }

    /// The link is there and leads to this app.
    static var installed: Bool {
        guard let target, let now = try? FileManager.default.destinationOfSymbolicLink(atPath: link) else { return false }
        return now == target
    }

    /// Asks once, at the first launch. A development run (NIGHTPOST_BIN) stays out of it.
    @MainActor static func offer() async {
        guard ProcessInfo.processInfo.environment["NIGHTPOST_BIN"] == nil, !Daemon.portable, target != nil,
              !UserDefaults.standard.bool(forKey: asked), !installed else { return }
        UserDefaults.standard.set(true, forKey: asked)
        try? await Task.sleep(for: .seconds(1))
        let alert = NSAlert()
        alert.messageText = t("mac-command-title")
        alert.informativeText = t("mac-command-text")
        alert.addButton(withTitle: t("mac-command-install"))
        alert.addButton(withTitle: t("mac-command-later"))
        if alert.runModal() == .alertFirstButtonReturn { _ = await install() }
    }

    static func install() async -> Bool {
        guard let target else { return false }
        if (try? make(target)) != nil { return true }
        let quoted = target.replacingOccurrences(of: "'", with: "'\\''")
        return await asAdministrator("mkdir -p /usr/local/bin && ln -sfn '\(quoted)' \(link)")
    }

    /// Only a link of ours (to an app's nightpost) goes; a file of the person's stays.
    static func uninstall() async -> Bool {
        guard let now = try? FileManager.default.destinationOfSymbolicLink(atPath: link),
              now.hasSuffix(".app/Contents/MacOS/nightpost") else { return false }
        if (try? FileManager.default.removeItem(atPath: link)) != nil { return true }
        return await asAdministrator("rm \(link)")
    }

    private static func make(_ target: String) throws {
        let fm = FileManager.default
        try? fm.removeItem(atPath: link)
        try fm.createDirectory(atPath: "/usr/local/bin", withIntermediateDirectories: true)
        try fm.createSymbolicLink(atPath: link, withDestinationPath: target)
    }

    /// The system's own password dialog, through osascript.
    private static func asAdministrator(_ shell: String) async -> Bool {
        let q = { (s: String) in s.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") }
        let script = "do shell script \"\(q(shell))\" with administrator privileges with prompt \"\(q(t("mac-command-prompt")))\""
        return await withCheckedContinuation { done in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
            p.arguments = ["-e", script]
            p.standardOutput = FileHandle.nullDevice
            p.standardError = FileHandle.nullDevice
            p.terminationHandler = { done.resume(returning: $0.terminationStatus == 0) }
            do { try p.run() } catch { done.resume(returning: false) }
        }
    }
}
