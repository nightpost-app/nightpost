// What the views show, kept up to date from the daemon's events.

import AppKit
import Foundation
import Observation

@MainActor @Observable
final class AppModel {
    enum Phase: Equatable {
        /// Starting the daemon, with its latest progress line.
        case connecting(String)
        /// The database needs the passphrase; why the last one failed.
        case locked(String)
        case failed(String)
        case ready
    }

    /// As many messages as the GUI on other systems loads per conversation.
    static let history: UInt32 = 200

    var phase = Phase.connecting("Starting…")
    var contacts: [ContactInfo] = []
    var messages: [String: [HistoryItem]] = [:]
    var unread: [String: Int] = [:]
    private(set) var selected: String?
    var network = Network()
    var links: [String: LinkState] = [:]
    var readReceipts = false
    /// The last message in, for the route to show it coming: from whom, and
    /// a count so each one plays again.
    var arrival: (name: String, count: Int)?
    /// The Add someone sheet is open.
    var adding = false
    /// What went wrong last, for the window to show.
    var status = ""

    /// Most recent conversation first, then those not started yet.
    var sortedContacts: [ContactInfo] {
        contacts.sorted {
            let (a, b) = (messages[$0.name]?.last?.time, messages[$1.name]?.last?.time)
            return a == b ? $0.name < $1.name : (a ?? 0) > (b ?? 0)
        }
    }

    /// Attaches to the daemon (starting it if none answers), then follows
    /// its events until it stops.
    func run(passphrase: String? = nil) async {
        let events: AsyncThrowingStream<Event, any Error>
        do {
            guard let attached = try await attach(passphrase: passphrase) else { return }
            events = attached
        } catch {
            phase = .failed(error.localizedDescription)
            return
        }
        do {
            if case let .status(network, links) = try await Daemon.call(.status) {
                self.network = network
                self.links = Dictionary(links.map { ($0.name, $0.state) }, uniquingKeysWith: { $1 })
            }
            if case let .readReceipts(on) = try await Daemon.call(.readReceipts(on: nil)) {
                readReceipts = on
            }
            try await reloadContacts()
        } catch {
            status = error.localizedDescription
        }
        Notifier.shared.setUp { [weak self] name in self?.select(name) }
        phase = .ready
        do {
            for try await event in events {
                apply(event)
            }
        } catch {}
        phase = .failed("The daemon stopped. Reopen Nightpost to start it again.")
    }

    /// The event stream, starting the daemon first if none answers. nil:
    /// it needs the passphrase (again) and the phase now asks for it.
    private func attach(passphrase: String?) async throws -> AsyncThrowingStream<Event, any Error>? {
        if let events = try? await Daemon.listen() {
            return events
        }
        // It answers, but not this yet: a daemon is starting Tor.
        let starting = Daemon.answers
        let asked = passphrase != nil
        if !starting && !asked && Daemon.usesPassphrase {
            phase = .locked("")
            return nil
        }
        phase = .connecting("Starting…")
        let daemon = starting ? nil : try Daemon.start(passphrase: passphrase)
        while true {
            try await Task.sleep(for: .seconds(1))
            if let events = try? await Daemon.listen() {
                return events
            }
            let text = (try? String(contentsOf: Daemon.log, encoding: .utf8)) ?? ""
            let lines = text.split(separator: "\n").map(String.init)
            if let daemon, !daemon.isRunning {
                if asked && text.contains("wrong key") {
                    phase = .locked("That passphrase does not open them. Try again.")
                    return nil
                }
                throw ClientError.daemon("the daemon stopped (\(daemon.terminationStatus)): \(lines.last ?? "")")
            }
            // Its progress lines, not the log lines around them.
            phase = .connecting(lines.last(where: { $0.hasPrefix("Tor:") }) ?? "Starting…")
        }
    }

    func select(_ name: String?) {
        selected = name
        guard let name else { return }
        unread[name] = nil
        badge()
        if messages[name] == nil {
            Task {
                if case let .history(items) = try? await Daemon.call(.history(name: name, limit: Self.history)) {
                    messages[name] = items
                }
            }
        }
        markRead()
    }

    /// The open conversation, on screen, is read; the daemon tells them only
    /// with read receipts on.
    func markRead() {
        guard let name = selected, NSApp.isActive else { return }
        Task {
            if case let .error(message) = try? await Daemon.call(.markRead(name: name)) { status = message }
        }
    }

    /// False if the daemon refused it (the reason is in `status`).
    func send(_ text: String, to name: String) async -> Bool {
        do {
            // The message itself comes back as Event.sent.
            if case let .error(message) = try await Daemon.call(.send(to: name, text: text)) {
                status = message
                return false
            }
            status = ""
            return true
        } catch {
            status = error.localizedDescription
            return false
        }
    }

    /// The contacts, and the history of those not loaded yet: the list
    /// shows each one's last message.
    func reloadContacts() async throws {
        guard case let .contacts(list) = try await Daemon.call(.contacts) else { return }
        for contact in list where messages[contact.name] == nil {
            if case let .history(items) = try await Daemon.call(.history(name: contact.name, limit: Self.history)) {
                messages[contact.name] = items
            }
        }
        contacts = list
        if selected == nil, let latest = sortedContacts.first {
            select(latest.name)
        }
    }

    func isMuted(_ name: String) -> Bool {
        guard let until = contacts.first(where: { $0.name == name })?.person.mutedUntil else { return false }
        return until > UInt64(Date().timeIntervalSince1970 * 1000)
    }

    /// The Dock shows how many messages wait to be read.
    private func badge() {
        let count = unread.values.reduce(0, +)
        NSApp.dockTile.badgeLabel = count > 0 ? "\(count)" : nil
    }

    private func apply(_ event: Event) {
        switch event {
        case let .message(from, id, time, text):
            arrival = (from, (arrival?.count ?? 0) + 1)
            messages[from, default: []].append(HistoryItem(id: id, outgoing: false, time: time, text: text, delivered: true, read: false))
            let here = selected == from
            if here { markRead() }
            if !here || !NSApp.isActive {
                if !here {
                    unread[from, default: 0] += 1
                    badge()
                }
                if !isMuted(from) { Notifier.shared.post(from: from) }
            }
        case let .sent(to, id, time, text):
            messages[to, default: []].append(HistoryItem(id: id, outgoing: true, time: time, text: text, delivered: false, read: false))
        case let .delivered(to, id):
            if let i = messages[to]?.firstIndex(where: { $0.id == id }) {
                messages[to]?[i].delivered = true
            }
        case let .read(to, ids):
            let ids = Set(ids)
            guard var list = messages[to] else { break }
            for i in list.indices where ids.contains(list[i].id) { list[i].read = true }
            messages[to] = list
        case let .readReceipts(on): readReceipts = on
        case let .contactAdded(name):
            Task {
                try? await reloadContacts()
                select(name)
            }
        case let .contactRenamed(name, to):
            messages[to] = messages.removeValue(forKey: name)
            unread[to] = unread.removeValue(forKey: name)
            links[to] = links.removeValue(forKey: name)
            if let i = contacts.firstIndex(where: { $0.name == name }) { contacts[i].name = to }
            if selected == name { selected = to }
        case let .contactRemoved(name):
            contacts.removeAll { $0.name == name }
            messages[name] = nil
            unread[name] = nil
            links[name] = nil
            badge()
            if selected == name { selected = nil }
        case let .personChanged(name, person):
            if let i = contacts.firstIndex(where: { $0.name == name }) { contacts[i].person = person }
        case let .network(network): self.network = network
        case let .link(name, state): links[name] = state
        case .fingerprint, .unknown: break
        }
    }
}
