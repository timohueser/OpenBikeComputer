import Foundation

public enum PlaceContact {
    public static func website(_ value: String?) -> URL? {
        guard let first = value?.split(separator: ";").map({ $0.trimmingCharacters(in: .whitespacesAndNewlines) })
            .first(where: { !$0.isEmpty }) else { return nil }
        let text = first.hasPrefix("//") ? "https:" + first : first.contains(":") ? first : "https://" + first
        guard let url = URL(string: text), ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
              let host = url.host, !host.isEmpty else { return nil }
        return url
    }

    public static func numbers(_ value: String?) -> [String] {
        value?.split(separator: ";").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty } ?? []
    }

    public static func phone(_ value: String) -> URL? {
        let number = value.filter { !" ()./-\t\r\n".contains($0) }
        guard number.range(of: #"^\+?[0-9]+$"#, options: .regularExpression) != nil else { return nil }
        return URL(string: "tel:" + number)
    }
}
