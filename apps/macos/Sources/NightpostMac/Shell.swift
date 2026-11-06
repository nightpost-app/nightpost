import Foundation

/// The `nightpost` command, in the Terminal: a link in /usr/local/bin to the
/// program inside the app (the .dmg only carries the app). Made on launch when
/// missing or left dangling by a moved app; one system password prompt if the
/// folder is not ours, and never asked again after a refusal.
enum ShellCommand {
    private static let link = "/usr/local/bin/nightpost"
    private static let declined = "commandDeclined"

    static func ensure() async {
        // A development run (NIGHTPOST_BIN) must not touch the system.
        guard ProcessInfo.processInfo.environment["NIGHTPOST_BIN"] == nil,
              let target = Bundle.main.url(forAuxiliaryExecutable: "nightpost")?.path else { return }
        let fm = FileManager.default
        if let now = try? fm.destinationOfSymbolicLink(atPath: link) {
            if now == target { return }
            // Someone else's link stays; ours (an app that moved) is replaced.
            if fm.fileExists(atPath: now), !now.hasSuffix(".app/Contents/MacOS/nightpost") { return }
        } else if fm.fileExists(atPath: link) {
            return  // a real file: not ours to replace
        }
        if (try? make(target)) != nil { return }
        guard !UserDefaults.standard.bool(forKey: declined) else { return }
        if !(await asAdministrator(target)) { UserDefaults.standard.set(true, forKey: declined) }
    }

    private static func make(_ target: String) throws {
        let fm = FileManager.default
        try? fm.removeItem(atPath: link)
        try fm.createDirectory(atPath: "/usr/local/bin", withIntermediateDirectories: true)
        try fm.createSymbolicLink(atPath: link, withDestinationPath: target)
    }

    /// The system's own password dialog, through osascript.
    private static func asAdministrator(_ target: String) async -> Bool {
        let q = { (s: String) in s.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") }
        let shell = "mkdir -p /usr/local/bin && ln -sfn '\(target.replacingOccurrences(of: "'", with: "'\\''"))' \(link)"
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
