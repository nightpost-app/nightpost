// The daemon's local API (crates/core/src/api.rs), as JSON lines: requests
// tagged by "cmd", replies by "reply", events by "event", snake_case.

import Foundation

enum Request: Encodable, Sendable {
    case send(to: String, text: String)
    case listen
    case invite(name: String)
    case accept(link: String, name: String)
    case inviteCode(name: String)
    case acceptCode(code: String, name: String)
    case confirm(name: String, keep: Bool)
    case contacts
    case verify(name: String)
    case history(name: String, limit: UInt32)
    case status
    case rename(name: String, to: String)
    case remove(name: String, wipe: Bool)
    case setPerson(name: String, person: Person)
    case setPassphrase(current: String?, new: String?)
    case markRead(name: String)
    case readReceipts(on: Bool?)
    case bridges(lines: [String]?)
    case shutdown

    private enum Key: String, CodingKey {
        case cmd, to, text, name, link, code, keep, limit, wipe, person, current, new, on, lines
    }

    func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        func cmd(_ name: String) throws { try c.encode(name, forKey: .cmd) }
        switch self {
        case let .send(to, text): try cmd("send"); try c.encode(to, forKey: .to); try c.encode(text, forKey: .text)
        case .listen: try cmd("listen")
        case let .invite(name): try cmd("invite"); try c.encode(name, forKey: .name)
        case let .accept(link, name): try cmd("accept"); try c.encode(link, forKey: .link); try c.encode(name, forKey: .name)
        case let .inviteCode(name): try cmd("invite_code"); try c.encode(name, forKey: .name)
        case let .acceptCode(code, name): try cmd("accept_code"); try c.encode(code, forKey: .code); try c.encode(name, forKey: .name)
        case let .confirm(name, keep): try cmd("confirm"); try c.encode(name, forKey: .name); try c.encode(keep, forKey: .keep)
        case .contacts: try cmd("contacts")
        case let .verify(name): try cmd("verify"); try c.encode(name, forKey: .name)
        case let .history(name, limit): try cmd("history"); try c.encode(name, forKey: .name); try c.encode(limit, forKey: .limit)
        case .status: try cmd("status")
        case let .rename(name, to): try cmd("rename"); try c.encode(name, forKey: .name); try c.encode(to, forKey: .to)
        case let .remove(name, wipe): try cmd("remove"); try c.encode(name, forKey: .name); try c.encode(wipe, forKey: .wipe)
        case let .setPerson(name, person): try cmd("set_person"); try c.encode(name, forKey: .name); try c.encode(person, forKey: .person)
        case let .setPassphrase(current, new): try cmd("set_passphrase"); try c.encode(current, forKey: .current); try c.encode(new, forKey: .new)
        case let .markRead(name): try cmd("mark_read"); try c.encode(name, forKey: .name)
        case let .readReceipts(on): try cmd("read_receipts"); try c.encode(on, forKey: .on)
        case let .bridges(lines): try cmd("bridges"); try c.encode(lines, forKey: .lines)
        case .shutdown: try cmd("shutdown")
        }
    }
}

enum Reply: Decodable, Sendable {
    case sent(id: String)
    case invite(link: String)
    case code(String)
    case added(name: String)
    case done
    case contacts([ContactInfo])
    case history([HistoryItem])
    case safetyNumber(name: String, number: String)
    case listening
    case status(network: Network, links: [LinkEntry])
    case readReceipts(on: Bool)
    case bridges([String])
    case error(String)
    /// From a newer daemon.
    case unknown(String)

    private enum Key: String, CodingKey {
        case reply, id, link, code, name, contacts, messages, number, network, links, on, lines, message
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        let tag = try c.decode(String.self, forKey: .reply)
        switch tag {
        case "sent": self = .sent(id: try c.decode(String.self, forKey: .id))
        case "invite": self = .invite(link: try c.decode(String.self, forKey: .link))
        case "code": self = .code(try c.decode(String.self, forKey: .code))
        case "added": self = .added(name: try c.decode(String.self, forKey: .name))
        case "done": self = .done
        case "contacts": self = .contacts(try c.decode([ContactInfo].self, forKey: .contacts))
        case "history": self = .history(try c.decode([HistoryItem].self, forKey: .messages))
        case "safety_number": self = .safetyNumber(name: try c.decode(String.self, forKey: .name), number: try c.decode(String.self, forKey: .number))
        case "listening": self = .listening
        case "status": self = .status(network: try c.decode(Network.self, forKey: .network), links: try c.decode([LinkEntry].self, forKey: .links))
        case "read_receipts": self = .readReceipts(on: try c.decode(Bool.self, forKey: .on))
        case "bridges": self = .bridges(try c.decode([String].self, forKey: .lines))
        case "error": self = .error(try c.decode(String.self, forKey: .message))
        default: self = .unknown(tag)
        }
    }
}

enum Event: Decodable, Sendable {
    case message(from: String, id: String, time: UInt64, text: String)
    case sent(to: String, id: String, time: UInt64, text: String)
    case delivered(to: String, id: String)
    case read(to: String, ids: [String])
    case readReceipts(on: Bool)
    case contactAdded(name: String)
    case fingerprint(name: String, safetyNumber: String)
    case network(Network)
    case link(name: String, state: LinkState)
    case contactRenamed(name: String, to: String)
    case contactRemoved(name: String)
    case personChanged(name: String, person: Person)
    /// From a newer daemon.
    case unknown(String)

    private enum Key: String, CodingKey {
        case event, from, to, id, ids, time, text, on, name, network, state, person
        case safetyNumber = "safety_number"
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        let tag = try c.decode(String.self, forKey: .event)
        func s(_ key: Key) throws -> String { try c.decode(String.self, forKey: key) }
        switch tag {
        case "message": self = .message(from: try s(.from), id: try s(.id), time: try c.decode(UInt64.self, forKey: .time), text: try s(.text))
        case "sent": self = .sent(to: try s(.to), id: try s(.id), time: try c.decode(UInt64.self, forKey: .time), text: try s(.text))
        case "delivered": self = .delivered(to: try s(.to), id: try s(.id))
        case "read": self = .read(to: try s(.to), ids: try c.decode([String].self, forKey: .ids))
        case "read_receipts": self = .readReceipts(on: try c.decode(Bool.self, forKey: .on))
        case "contact_added": self = .contactAdded(name: try s(.name))
        case "fingerprint": self = .fingerprint(name: try s(.name), safetyNumber: try s(.safetyNumber))
        case "network": self = .network(try c.decode(Network.self, forKey: .network))
        case "link": self = .link(name: try s(.name), state: try c.decode(LinkState.self, forKey: .state))
        case "contact_renamed": self = .contactRenamed(name: try s(.name), to: try s(.to))
        case "contact_removed": self = .contactRemoved(name: try s(.name))
        case "person_changed": self = .personChanged(name: try s(.name), person: try c.decode(Person.self, forKey: .person))
        default: self = .unknown(tag)
        }
    }
}

/// How far Tor got.
struct Network: Codable, Sendable, Equatable {
    /// Tor's bootstrap, 0 to 100.
    var percent: UInt8 = 0
    /// Through a bridge rather than straight to a guard.
    var bridged = false
    /// The last probe: the first hop answered, and an onion-service circuit
    /// got built; nil until a probe ran.
    var entry: Bool?
    var circuit: Bool?
    /// The last line for people.
    var line = ""
}

enum LinkState: String, Codable, Sendable {
    case trying, open, idle, unreachable
}

/// One `(name, state)` pair of a status reply.
struct LinkEntry: Decodable, Sendable {
    let name: String
    let state: LinkState

    init(from decoder: any Decoder) throws {
        var c = try decoder.unkeyedContainer()
        name = try c.decode(String.self)
        state = try c.decode(LinkState.self)
    }
}

struct HistoryItem: Decodable, Sendable, Identifiable, Equatable {
    let id: String
    let outgoing: Bool
    let time: UInt64
    let text: String
    var delivered: Bool
    /// Outgoing: they read it (read receipts on, on both sides).
    var read: Bool

    private enum Key: String, CodingKey { case id, outgoing, time, text, delivered, read }

    init(id: String, outgoing: Bool, time: UInt64, text: String, delivered: Bool, read: Bool) {
        (self.id, self.outgoing, self.time, self.text, self.delivered, self.read) = (id, outgoing, time, text, delivered, read)
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        id = try c.decode(String.self, forKey: .id)
        outgoing = try c.decode(Bool.self, forKey: .outgoing)
        time = try c.decode(UInt64.self, forKey: .time)
        text = try c.decode(String.self, forKey: .text)
        delivered = try c.decode(Bool.self, forKey: .delivered)
        read = try c.decodeIfPresent(Bool.self, forKey: .read) ?? false
    }
}

struct ContactInfo: Decodable, Sendable, Identifiable, Equatable {
    var name: String
    let safetyNumber: String
    var person: Person
    var id: String { name }

    private enum Key: String, CodingKey {
        case name, person
        case safetyNumber = "safety_number"
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        name = try c.decode(String.self, forKey: .name)
        safetyNumber = try c.decode(String.self, forKey: .safetyNumber)
        person = try c.decodeIfPresent(Person.self, forKey: .person) ?? Person()
    }
}

/// What we keep about a contact besides the name.
struct Person: Codable, Sendable, Equatable {
    /// We compared safety numbers with them.
    var verified = false
    /// No notifications until then (Unix milliseconds); UInt64.max for good.
    var mutedUntil: UInt64?
    /// A `data:image/png;base64,` URI.
    var photo: String?

    private enum CodingKeys: String, CodingKey {
        case verified, photo
        case mutedUntil = "muted_until"
    }
}
