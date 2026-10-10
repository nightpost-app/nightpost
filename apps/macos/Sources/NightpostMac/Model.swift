// What the views show, kept up to date from the daemon's events.

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

    var phase = Phase.connecting("Starting…")
    var contacts: [ContactInfo] = []
    var network = Network()
    var links: [String: LinkState] = [:]
    var readReceipts = false
    /// What went wrong last, for the window to show.
    var status = ""

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

    func reloadContacts() async throws {
        if case let .contacts(contacts) = try await Daemon.call(.contacts) {
            self.contacts = contacts
        }
    }

    private func apply(_ event: Event) {
        switch event {
        case let .network(network): self.network = network
        case let .link(name, state): links[name] = state
        case let .readReceipts(on): readReceipts = on
        case .contactAdded, .contactRenamed, .contactRemoved, .personChanged:
            Task { try? await reloadContacts() }
        default: break
        }
    }
}
