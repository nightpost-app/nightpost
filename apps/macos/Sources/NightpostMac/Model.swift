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
    /// The sheet over the window, if any.
    var dialog: Dialog?
    /// Their panel is open beside the conversation.
    var panel = false
    /// Whom we are looking for with what they gave us, while the Add sheet
    /// says so.
    var looking: String?
    /// Why the last thing tried in the Add sheet failed.
    var addError = ""
    /// Those we made a code for: their safety number is ours to confirm.
    private var codes: Set<String> = []
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
        // The message itself comes back as Event.sent.
        let sent = await ask(.send(to: name, text: text))
        if sent { status = "" }
        return sent
    }

    /// Changes what we keep about them; the daemon announces the result.
    func update(_ name: String, _ change: (inout Person) -> Void) {
        var person = contacts.first { $0.name == name }?.person ?? Person()
        change(&person)
        Task { await ask(.setPerson(name: name, person: person)) }
    }

    func rename(_ name: String, to: String) {
        Task { await ask(.rename(name: name, to: to)) }
    }

    func remove(_ name: String, wipe: Bool) {
        panel = false
        Task { if await ask(.remove(name: name, wipe: wipe)) { status = "\(name) removed" } }
    }

    /// Their safety number, over everything.
    func compare(_ name: String) {
        Task {
            switch try? await Daemon.call(.verify(name: name)) {
            case let .safetyNumber(_, number): dialog = .safety(name: name, number: number)
            case let .error(message): status = message
            default: break
            }
        }
    }

    /// False if the daemon refused it or could not be asked; why goes to
    /// `status`.
    @discardableResult
    private func ask(_ request: Request) async -> Bool {
        do {
            if case let .error(message) = try await Daemon.call(request) {
                status = message
                return false
            }
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

    /// A link, or a code to read out, for someone not added yet.
    func invite(_ name: String, byCode: Bool) {
        guard !name.isEmpty else { addError = "Give them a name first."; return }
        Task {
            do {
                switch try await Daemon.call(byCode ? .inviteCode(name: name) : .invite(name: name)) {
                case let .invite(link): dialog = .invite(name: name, link: link)
                case let .code(code):
                    codes.insert(name)
                    dialog = .code(name: name, code: code)
                case let .error(message): addError = message
                case let other: addError = "unexpected reply \(other)"
                }
            } catch {
                addError = error.localizedDescription
            }
        }
    }

    /// Their link, or the code they read us. The sheet shows the search; if
    /// it was closed meanwhile, the answer goes to the status line.
    func accept(_ given: String, name: String) {
        guard !given.isEmpty else { addError = "Paste their link, or type the code they read you."; return }
        guard !name.isEmpty else { addError = "Give them a name first, at the top."; return }
        // A code is digits and separators; anything else is taken for a link.
        let byCode = given.allSatisfy { $0.isASCII && ($0.isNumber || " -.".contains($0)) }
        addError = ""
        looking = name
        Task {
            let reply: Reply
            do {
                reply = try await Daemon.call(byCode ? .acceptCode(code: given, name: name) : .accept(link: given, name: name))
            } catch {
                reply = .error(error.localizedDescription)
            }
            let shown = looking == name && dialog == .add
            looking = nil
            switch reply {
            case let .added(name):
                if dialog == .add { dialog = nil }
                status = "\(name) added"
            case let .error(message):
                if shown { addError = message } else { status = message }
            default: break
            }
        }
    }

    /// Someone used our code: keep them, or turn them down.
    func confirm(_ name: String, keep: Bool) {
        dialog = nil
        codes.remove(name)
        Task {
            switch try? await Daemon.call(.confirm(name: name, keep: keep)) {
            case .done: status = keep ? "\(name) kept" : "\(name) turned down"
            case let .error(message): status = message
            default: break
            }
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
                if !isMuted(from) { Notifier.shared.post(title: "Nightpost", body: "Message from \(from)", name: from) }
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
            switch dialog {
            case let .waiting(n, _) where n == name, let .invite(n, _) where n == name: dialog = nil
            default: break
            }
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
        case let .fingerprint(name, number):
            Notifier.shared.post(title: "Nightpost", body: "\(name) used your invitation", name: nil)
            dialog = codes.contains(name) ? .confirm(name: name, number: number) : .waiting(name: name, number: number)
        case .unknown: break
        }
    }
}

enum Dialog: Equatable {
    case add
    case invite(name: String, link: String)
    case code(name: String, code: String)
    /// Someone used our code: keep them only if the numbers match.
    case confirm(name: String, number: String)
    /// We used a code: read our number to the inviter.
    case waiting(name: String, number: String)
    /// To compare with them, full screen.
    case safety(name: String, number: String)
}
