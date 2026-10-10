// System notifications: who wrote, never what (settings come later).

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

    func post(from name: String) {
        let content = UNMutableNotificationContent()
        content.title = "Nightpost"
        content.body = "Message from \(name)"
        content.userInfo = ["name": name]
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
