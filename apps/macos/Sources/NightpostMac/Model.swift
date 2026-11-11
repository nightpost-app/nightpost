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

    var phase = Phase.connecting(t("connecting-daemon"))
    var contacts: [ContactInfo] = []
    var messages: [String: [HistoryItem]] = [:]
    var unread: [String: Int] = [:]
    private(set) var selected: String?
    var network = Network()
    var links: [String: LinkState] = [:]
    var readReceipts = false
    /// We tell people we are writing, and so see when they do.
    var typingIndicators = false
    /// Who is writing to us now, counted so only the last ping takes them away.
    var typing: [String: Int] = [:]
    /// When we last told someone we are writing.
    private var pinged: Date?
    /// The language spoken; choosing another draws everything again.
    var language = Words.language
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
    /// The settings tab shown: "devices" to join an account from the start.
    var settingsTab = "appearance"
    /// Moves on when our device list changed, for the settings to read it again.
    var devicesVersion = 0
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
    func run(passphrase: String? = nil, recovery: String? = nil) async {
        let events: AsyncThrowingStream<Event, any Error>
        do {
            guard let attached = try await attach(passphrase: passphrase, recovery: recovery) else { return }
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
            if case let .typingIndicators(on) = try await Daemon.call(.typingIndicators(on: nil)) {
                typingIndicators = on
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
        phase = .failed(t("daemon-stopped"))
    }

    /// The event stream, starting the daemon first if none answers. nil:
    /// it needs the passphrase (again) and the phase now asks for it.
    private func attach(passphrase: String?, recovery: String?) async throws -> AsyncThrowingStream<Event, any Error>? {
        // A daemon left running from before an update would not know what
        // this app asks: stop it, the one beside this app takes over.
        if Daemon.runningVersion != Daemon.bundledVersion && Daemon.answers {
            phase = .connecting(t("restarting-update"))
            _ = try? await Daemon.call(.shutdown)
            for _ in 0..<20 where Daemon.answers { try await Task.sleep(for: .milliseconds(500)) }
        }
        if let events = try? await Daemon.listen() {
            return events
        }
        // It answers, but not this yet: a daemon is starting Tor.
        let starting = Daemon.answers
        let asked = passphrase != nil || recovery != nil
        if !starting && !asked && Daemon.usesPassphrase {
            phase = .locked("")
            return nil
        }
        phase = .connecting(t("starting"))
        let daemon = starting ? nil : try Daemon.start(passphrase: passphrase, recovery: recovery)
        while true {
            try await Task.sleep(for: .seconds(1))
            if let events = try? await Daemon.listen() {
                return events
            }
            let text = (try? String(contentsOf: Daemon.log, encoding: .utf8)) ?? ""
            let lines = text.split(separator: "\n").map(String.init)
            if let daemon, !daemon.isRunning {
                // A wrong key, or a keychain without it (a new Mac): the
                // recovery words are the way out, offered on that screen.
                if (asked && text.contains("wrong key")) || text.contains(t("err-keychain-lost")) {
                    phase = .locked(text.contains(t("err-keychain-lost")) ? t("err-keychain-lost") : t("wrong-passphrase"))
                    return nil
                }
                throw ClientError.daemon("the daemon stopped (\(daemon.terminationStatus)): \(lines.last ?? "")")
            }
            // Its progress lines, not the log lines around them.
            phase = .connecting(lines.last(where: { $0.hasPrefix("Tor:") }) ?? t("starting"))
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

    /// We are writing to them: at most one ping every few seconds, as on
    /// other systems.
    func typed(to name: String) {
        guard typingIndicators, pinged.map({ Date().timeIntervalSince($0) >= 4 }) ?? true else { return }
        pinged = Date()
        Task { _ = try? await Daemon.call(.typing(name: name)) }
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

    func setTimer(_ name: String, _ seconds: UInt32) {
        Task { await ask(.timer(name: name, seconds: seconds)) }
    }

    /// The recovery words, or nil (the reason is in `status`).
    func recoveryPhrase() async -> String? {
        switch try? await Daemon.call(.recoveryPhrase) {
        case let .recoveryPhrase(words): return words
        case let .error(message): status = message
        default: break
        }
        return nil
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

    /// Stops the daemon (locking the database if it has a passphrase), then
    /// this app.
    func quit() {
        Task {
            _ = try? await Daemon.call(.shutdown)
            NSApp.terminate(nil)
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
        guard !name.isEmpty else { addError = t("need-name"); return }
        Task {
            do {
                switch try await Daemon.call(byCode ? .inviteCode(name: name) : .invite(name: name)) {
                case let .invite(link): dialog = .invite(name: name, link: link)
                case let .code(code):
                    codes.insert(name)
                    dialog = .code(name: name, code: code)
                case let .error(message): addError = message
                case let other: addError = t("err-unexpected", ["reply": other])
                }
            } catch {
                addError = error.localizedDescription
            }
        }
    }

    /// Their link, or the code they read us. The sheet shows the search; if
    /// it was closed meanwhile, the answer goes to the status line.
    func accept(_ given: String, name: String) {
        guard !given.isEmpty else { addError = t("need-given"); return }
        guard !name.isEmpty else { addError = t("need-name-top"); return }
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
            case .done: status = keep ? t("kept", ["name": name]) : t("turned-down", ["name": name])
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
            typing[from] = nil
            arrival = (from, (arrival?.count ?? 0) + 1)
            messages[from, default: []].append(HistoryItem(id: id, outgoing: false, time: time, text: text, delivered: true, read: false))
            let here = selected == from
            if here { markRead() }
            if !here || !NSApp.isActive {
                if !here {
                    unread[from, default: 0] += 1
                    badge()
                }
                if !isMuted(from) { Notifier.shared.message(from: from, text: text) }
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
        case let .typingIndicators(on): typingIndicators = on
        case let .typing(from):
            let n = (typing.values.max() ?? 0) + 1
            typing[from] = n
            Task {
                try? await Task.sleep(for: .seconds(5))
                if typing[from] == n { typing[from] = nil }
            }
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
        case let .timerChanged(name, seconds, _):
            if let i = contacts.firstIndex(where: { $0.name == name }) { contacts[i].timer = seconds }
        case let .expired(name):
            Task {
                if case let .history(items) = try? await Daemon.call(.history(name: name, limit: Self.history)) {
                    messages[name] = items
                }
            }
        case let .network(network): self.network = network
        case let .link(name, state): links[name] = state
        case let .fingerprint(name, number):
            Notifier.shared.post(title: "Nightpost", body: t("notify-used-invite", ["name": name]), name: nil)
            dialog = codes.contains(name) ? .confirm(name: name, number: number) : .waiting(name: name, number: number)
        // Nobody else asks the app to close: Nightpost is stopping.
        case let .close(interface): if interface == "window" { NSApp.terminate(nil) }
        // Another account's now: everything shown so far is gone.
        case .paired:
            status = t("paired")
            settingsTab = "appearance"
            (messages, selected) = ([:], nil)
            Task {
                if case let .readReceipts(on) = try? await Daemon.call(.readReceipts(on: nil)) { readReceipts = on }
                if case let .typingIndicators(on) = try? await Daemon.call(.typingIndicators(on: nil)) { typingIndicators = on }
                try? await reloadContacts()
            }
        case .removed: status = t("err-device-removed")
        case .devicesChanged: devicesVersion += 1
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
