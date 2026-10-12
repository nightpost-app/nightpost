import Foundation

/// The words shown, from the same Fluent files as the other interfaces
/// (crates/core/locales, copied into the bundle by bundle.sh): NIGHTPOST_LANG,
/// else the language chosen in the settings (the `language` file the daemon
/// reads too), else the system's, among English, French, Spanish and German.
enum Words {
    static let codes = ["en", "fr", "es", "de"]
    nonisolated(unsafe) private(set) static var language = wanted()
    nonisolated(unsafe) fileprivate static var chosenTable = table(language)
    fileprivate static let english = table("en")
    private static var file: URL { Daemon.dir.appending(path: "language") }

    private static func wanted() -> String {
        let set = ProcessInfo.processInfo.environment["NIGHTPOST_LANG"].flatMap { $0.isEmpty ? nil : $0 }
        let wanted = set ?? chosen ?? Locale.preferredLanguages.first ?? "en"
        let code = String(wanted.prefix { $0.isLetter }).lowercased()
        return codes.contains(code) ? code : "en"
    }

    /// The language chosen in the settings; nil: the system's.
    static var chosen: String? {
        let text = (try? String(contentsOf: file, encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines)
        return text?.isEmpty == false ? text : nil
    }

    /// Each language's code and its name, in itself.
    static var names: [(code: String, name: String)] {
        codes.map { ($0, table($0)["language-name"] ?? $0) }
    }

    /// Chooses the language (nil: the system's) and speaks it from now on.
    static func choose(_ code: String?) {
        if let code { try? code.write(to: file, atomically: true, encoding: .utf8) } else { try? FileManager.default.removeItem(at: file) }
        language = wanted()
        chosenTable = table(language)
    }

    /// One message per line, `key = text`, as i18n.rs reads them.
    static func table(_ code: String) -> [String: String] {
        guard let url = Bundle.main.url(forResource: code, withExtension: "ftl"),
              let text = try? String(contentsOf: url, encoding: .utf8)
        else { return [:] }
        var table: [String: String] = [:]
        for line in text.split(separator: "\n") where !line.hasPrefix("#") {
            guard let equals = line.range(of: " = ") else { continue }
            table[line[..<equals.lowerBound].trimmingCharacters(in: .whitespaces)] = line[equals.upperBound...].trimmingCharacters(in: .whitespaces)
        }
        return table
    }
}

/// The message `key`, its `{ $name }` placeables filled from `args`.
/// Missing in the language: the English one; missing in English: the key.
func t(_ key: String, _ args: [String: Any] = [:]) -> String {
    var text = Words.chosenTable[key] ?? Words.english[key] ?? key
    for (name, value) in args {
        text = text.replacingOccurrences(of: "{ $\(name) }", with: "\(value)")
    }
    return text
}
