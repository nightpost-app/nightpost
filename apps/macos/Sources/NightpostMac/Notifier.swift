// System notifications, as the settings say: who and what, who, or nothing.

import Foundation
import UserNotifications

@MainActor
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = Notifier()
    /// Opens their conversation when a notification is clicked.
    private var open: ((String) -> Void)?

    func setUp(open: @escaping (String) -> Void) {
        self.open = open
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    /// What the settings let out of a message.
    func message(from name: String, text: String) {
        switch Shown(rawValue: UserDefaults.standard.string(forKey: "shown") ?? "") ?? .who {
        case .all:
            let line = text.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? ""
            post(title: name, body: line.count > 200 ? String(line.prefix(200)) + "…" : line, name: name)
        case .who: post(title: "Nightpost", body: t("notify-from", ["name": name]), name: name)
        case .nothing: post(title: "Nightpost", body: t("notify-new"), name: name)
        }
    }

    /// `name`: whose conversation a click opens.
    func post(title: String, body: String, name: String?) {
        let defaults = UserDefaults.standard
        guard defaults.object(forKey: "notify") as? Bool ?? true else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        if defaults.bool(forKey: "sound") { content.sound = .default }
        if let name { content.userInfo = ["name": name] }
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }

    /// Shown even while Nightpost is in front: it is about another conversation.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let name = response.notification.request.content.userInfo["name"] as? String else { return }
        await MainActor.run { self.open?(name) }
    }
}

/// What a notification shows.
enum Shown: String {
    case all, who, nothing
}
