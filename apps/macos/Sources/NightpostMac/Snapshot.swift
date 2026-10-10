// For development: with NIGHTPOST_SNAPSHOT=<file.png>, the app draws its own
// window into that file every two seconds (macOS lets an app draw itself,
// not the screen). NIGHTPOST_APPEARANCE=light|dark picks the appearance.

import AppKit

@MainActor
enum Snapshot {
    static func run() async {
        let environment = ProcessInfo.processInfo.environment
        if let appearance = environment["NIGHTPOST_APPEARANCE"].flatMap(Theme.init(rawValue:)) {
            UserDefaults.standard.set(appearance.rawValue, forKey: "theme")
        }
        guard let file = environment["NIGHTPOST_SNAPSHOT"] else { return }
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(2))
            guard let window = NSApp.keyWindow ?? NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
                  let view = window.contentView?.superview,
                  let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { continue }
            view.cacheDisplay(in: view.bounds, to: bitmap)
            try? bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: file))
        }
    }
}
