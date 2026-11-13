// Talking to the daemon: its Unix socket in the data directory, the token
// from api.token as the first line, then one JSON line per request.

import CryptoKit
import Darwin
import Foundation

enum ClientError: LocalizedError {
    case noDaemon(String)
    case closed
    case daemon(String)
    case unexpected(String)

    var errorDescription: String? {
        switch self {
        case let .noDaemon(why): "no daemon running: \(why)"
        case .closed: "the daemon closed the connection"
        case let .daemon(message): message
        case let .unexpected(reply): "unexpected reply \(reply)"
        }
    }
}

enum Daemon {
    /// `NIGHTPOST_DIR` if set (tests run several), else where the daemon
    /// puts it on macOS.
    static let root: URL = {
        if let dir = ProcessInfo.processInfo.environment["NIGHTPOST_DIR"] {
            return URL(fileURLWithPath: dir)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Application Support/Nightpost")
    }()

    /// `NIGHTPOST_ACCOUNT`: which account this app instance is for (one
    /// each, as on other systems); none is the main one. Only a name the
    /// daemon would accept, since it becomes a folder.
    static let account: String? = {
        guard let name = ProcessInfo.processInfo.environment["NIGHTPOST_ACCOUNT"], !name.isEmpty, name != "default",
              name.count <= 32, name.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-" || $0 == "_") })
        else { return nil }
        return name
    }()

    /// The data directory of that account: the root for the main one.
    static let dir: URL = account.map { root.appending(path: "accounts").appending(path: $0) } ?? root

    /// The window's title: with the account's name when it is not the main one.
    static var title: String { account.map { "Nightpost — \($0)" } ?? "Nightpost" }

    /// The accounts of this Nightpost, the main one first (accounts.json).
    static func accounts() -> [String] {
        struct Entry: Decodable { let name: String }
        let others = (try? Data(contentsOf: root.appending(path: "accounts.json")))
            .flatMap { try? JSONDecoder().decode([Entry].self, from: $0) }?.map(\.name) ?? []
        return ["default"] + others.filter { $0 != "default" }
    }

    /// Run from a key (`NIGHTPOST_PORTABLE=1`, set by its launcher): the
    /// Mac is borrowed. The passphrase alone opens the database, and
    /// closing stops everything.
    static let portable = ProcessInfo.processInfo.environment["NIGHTPOST_PORTABLE"] == "1"

    /// Beside a database that also needs the passphrase; always from a key.
    static var usesPassphrase: Bool {
        portable || FileManager.default.fileExists(atPath: dir.appending(path: "uses-passphrase").path)
    }

    /// In the data directory, or from a key (exFAT holds no socket) where
    /// the daemon puts it then: in TMPDIR, named after the real path of the
    /// data directory (runtime_path in crates/core/src/lib.rs).
    static var socket: URL {
        guard portable else { return dir.appending(path: "api.sock") }
        let real = realpath(dir.path, nil).map { p in defer { free(p) }; return String(cString: p) } ?? dir.path
        let hash = SHA256.hash(data: Data(real.utf8)).prefix(8).map { String(format: "%02x", $0) }.joined()
        let tmp = ProcessInfo.processInfo.environment["TMPDIR"] ?? NSTemporaryDirectory()
        return URL(fileURLWithPath: tmp).appending(path: "nightpost-\(hash).sock")
    }

    /// Stops the daemon before returning, as the app quits.
    static func stopNow() {
        _ = try? Connection(.shutdown).nextLine()
    }

    static var log: URL { dir.appending(path: "daemon.log") }

    /// What the running daemon wrote at its start; bundle.sh gives the app
    /// the version of the daemon beside it.
    static var runningVersion: String? { try? String(contentsOf: dir.appending(path: "api.version"), encoding: .utf8) }
    static var bundledVersion: String? { Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String }

    /// One request on its own connection.
    static func call(_ request: Request) async throws -> Reply {
        try await withCheckedThrowingContinuation { answer in
            // Blocking reads, off the main thread. Not FileHandle.bytes: its
            // reads all go through one serial queue, so the event stream's
            // wait would hold every request up behind it.
            DispatchQueue.global().async {
                answer.resume(with: Result {
                    let connection = try Connection(request)
                    guard let line = try connection.nextLine() else { throw ClientError.closed }
                    return try JSONDecoder().decode(Reply.self, from: Data(line.utf8))
                })
            }
        }
    }

    /// The event stream, once the daemon has said it listens.
    static func listen() async throws -> AsyncThrowingStream<Event, any Error> {
        let (stream, events) = AsyncThrowingStream<Event, any Error>.makeStream()
        try await withCheckedThrowingContinuation { (ready: CheckedContinuation<Void, any Error>) in
            // A thread of its own: it waits for the next event for as long
            // as the app runs.
            Thread {
                let connection: Connection
                do {
                    connection = try Connection(.listen)
                    guard let first = try connection.nextLine() else { throw ClientError.closed }
                    switch try JSONDecoder().decode(Reply.self, from: Data(first.utf8)) {
                    case .listening: ready.resume()
                    case let .error(message): throw ClientError.daemon(message)
                    case let other: throw ClientError.unexpected("\(other)")
                    }
                } catch {
                    ready.resume(throwing: error)
                    return
                }
                do {
                    while let line = try connection.nextLine() {
                        // A kind of event this version does not know is skipped.
                        if let event = try? JSONDecoder().decode(Event.self, from: Data(line.utf8)) {
                            events.yield(event)
                        }
                    }
                    events.finish()
                } catch {
                    events.finish(throwing: error)
                }
            }.start()
        }
        return stream
    }

    /// Whether a daemon answers on the socket at all (it may be starting).
    static var answers: Bool { (try? Connection(nil)) != nil }

    /// `nightpost daemon`, from inside the app (or `NIGHTPOST_BIN`), its
    /// output in daemon.log. The passphrase, then the recovery words, go on
    /// its stdin a line each, never in its arguments or environment.
    static func start(passphrase: String?, recovery: String? = nil) throws -> Process {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        FileManager.default.createFile(atPath: log.path, contents: nil, attributes: [.posixPermissions: 0o600])
        let output = try FileHandle(forWritingTo: log)
        let binary = ProcessInfo.processInfo.environment["NIGHTPOST_BIN"].map { URL(fileURLWithPath: $0) }
            ?? Bundle.main.url(forAuxiliaryExecutable: "nightpost")
        guard let binary else { throw ClientError.noDaemon("nightpost is missing from the app") }
        let process = Process()
        process.executableURL = binary
        process.arguments = (account.map { ["--account", $0] } ?? []) + (recovery == nil ? ["daemon"] : ["daemon", "--recovery"])
        process.standardOutput = output
        process.standardError = output
        let input = Pipe()
        let lines = [passphrase, recovery].compactMap { $0 }
        process.standardInput = lines.isEmpty ? FileHandle.nullDevice : input
        // ponytail: same process group as the app; started from a terminal, a
        // Ctrl-C there stops the daemon too. posix_spawn with SETPGROUP if it matters.
        try process.run()
        if !lines.isEmpty {
            try input.fileHandleForWriting.write(contentsOf: Data(lines.map { $0 + "\n" }.joined().utf8))
            try input.fileHandleForWriting.close()
        }
        return process
    }
}

/// A connection to the daemon, past the token; blocking, for one thread.
private final class Connection {
    private let fd: Int32
    private var buffer = Data()

    /// Connects, gives the token, then sends `request` if there is one.
    init(_ request: Request?) throws {
        let path = Array(Daemon.socket.path.utf8)
        var addr = sockaddr_un()
        guard path.count < MemoryLayout.size(ofValue: addr.sun_path) else {
            throw ClientError.noDaemon("socket path too long")
        }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw ClientError.noDaemon(String(cString: strerror(errno))) }
        // A daemon gone mid-write is an error to report, not a signal that
        // ends this app.
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        addr.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &addr.sun_path) { $0.copyBytes(from: path) }
        let connected = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else {
            let why = String(cString: strerror(errno))
            Darwin.close(fd)
            throw ClientError.noDaemon(why)
        }
        self.fd = fd
        let token = try String(contentsOf: Daemon.dir.appending(path: "api.token"), encoding: .utf8)
        try write(Data((token.trimmingCharacters(in: .whitespacesAndNewlines) + "\n").utf8))
        if let request {
            var line = try JSONEncoder().encode(request)
            line.append(0x0A)
            try write(line)
        }
    }

    deinit { Darwin.close(fd) }

    private func write(_ data: Data) throws {
        try data.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) in
            var sent = 0
            while sent < bytes.count {
                let n = Darwin.write(fd, bytes.baseAddress! + sent, bytes.count - sent)
                if n < 0 && errno == EINTR { continue }
                guard n > 0 else { throw ClientError.closed }
                sent += n
            }
        }
    }

    /// The next line, without its newline; nil once the daemon closes.
    func nextLine() throws -> String? {
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            if let end = buffer.firstIndex(of: 0x0A) {
                let line = String(decoding: buffer[buffer.startIndex..<end], as: UTF8.self)
                buffer.removeSubrange(buffer.startIndex...end)
                return line
            }
            let n = read(fd, &chunk, chunk.count)
            if n < 0 && errno == EINTR { continue }
            if n < 0 { throw ClientError.daemon(String(cString: strerror(errno))) }
            if n == 0 { return nil }
            buffer.append(contentsOf: chunk[..<n])
        }
    }
}
