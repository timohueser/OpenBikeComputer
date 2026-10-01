import Foundation

/// Answers "open now?" for the common shape of an OpenStreetMap `opening_hours` value: rules
/// separated by `;`, each a day set (`Mo-Fr`, `Sa,Su`, none for every day, or `24/7`) and time
/// ranges (`08:00-12:00,16:00-20:00`, or `off`). A later rule wins for the days it names. A range
/// past midnight, like `Fr 22:00-02:00`, runs into the next day: its 00:00-02:00 part is Saturday's.
/// Anything outside that shape answers nil, so the badge stays silent instead of guessing.
enum PlannerPreviewOpeningHours {
    private static let days = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]

    static func isOpen(_ spec: String, at date: Date = Date(), calendar: Calendar = .current) -> Bool? {
        let trimmed = spec.trimmingCharacters(in: .whitespaces)
        if trimmed == "24/7" { return true }
        // Calendar weekdays run Sunday = 1; OSM runs Monday first.
        let weekday = (calendar.component(.weekday, from: date) + 5) % 7
        let minute = calendar.component(.hour, from: date) * 60 + calendar.component(.minute, from: date)
        // Today's own ranges, and the after-midnight parts of yesterday's.
        var today: Bool?
        var spill: Bool?
        var parsedAnything = false
        for rule in trimmed.split(separator: ";").map({ $0.trimmingCharacters(in: .whitespaces) }) where !rule.isEmpty {
            guard let (ruleDays, ranges) = parse(rule) else { return nil }
            parsedAnything = true
            if ruleDays.contains(weekday) {
                today = ranges.contains { $0.from <= minute && minute < ($0.wraps ? 24 * 60 : $0.to) }
            }
            if ruleDays.contains((weekday + 6) % 7) {
                spill = ranges.contains { $0.wraps && minute < $0.to }
            }
        }
        return parsedAnything ? (today == true || spill == true) : nil
    }

    private struct Range { let from: Int; let to: Int
        var wraps: Bool { to < from }
    }

    private static func parse(_ rule: String) -> (Set<Int>, [Range])? {
        var tokens = rule.split(separator: " ").map(String.init)
        var ruleDays: Set<Int> = Set(0..<7)
        if let first = tokens.first, first.first?.isLetter == true {
            guard let parsed = parseDays(first) else { return nil }
            ruleDays = parsed; tokens.removeFirst()
        }
        guard let times = tokens.first, tokens.count == 1 else { return nil }
        if times == "off" { return (ruleDays, []) }
        var ranges: [Range] = []
        for part in times.split(separator: ",") {
            let ends = part.split(separator: "-")
            guard ends.count == 2, let from = minutes(ends[0]), let to = minutes(ends[1]) else { return nil }
            ranges.append(Range(from: from, to: to))
        }
        return (ruleDays, ranges)
    }

    private static func parseDays(_ text: String) -> Set<Int>? {
        var result: Set<Int> = []
        for group in text.split(separator: ",") {
            let ends = group.split(separator: "-").map(String.init)
            guard let first = days.firstIndex(of: ends[0]) else { return nil }
            if ends.count == 1 { result.insert(first); continue }
            guard ends.count == 2, let last = days.firstIndex(of: ends[1]) else { return nil }
            var day = first
            repeat { result.insert(day); day = (day + 1) % 7 } while day != (last + 1) % 7
        }
        return result
    }

    private static func minutes(_ text: Substring) -> Int? {
        let parts = text.split(separator: ":")
        guard parts.count == 2, let h = Int(parts[0]), let m = Int(parts[1]), (0...24).contains(h), (0..<60).contains(m) else { return nil }
        return h * 60 + m
    }
}
