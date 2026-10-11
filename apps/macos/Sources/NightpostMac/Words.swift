import Foundation

/// The words shown, from the same Fluent files as the other interfaces
/// (crates/core/locales, copied into the bundle by bundle.sh): the system's
/// language among English, French, Spanish and German, or NIGHTPOST_LANG.
private enum Words {
    static let language: String = {
        let wanted = ProcessInfo.processInfo.environment["NIGHTPOST_LANG"] ?? Locale.preferredLanguages.first ?? "en"
        let code = String(wanted.prefix { $0.isLetter }).lowercased()
        return ["en", "fr", "es", "de"].contains(code) ? code : "en"
    }()
    static let chosen = table(language)
    static let english = table("en")

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
    var text = Words.chosen[key] ?? Words.english[key] ?? key
    for (name, value) in args {
        text = text.replacingOccurrences(of: "{ $\(name) }", with: "\(value)")
    }
    return text
}
