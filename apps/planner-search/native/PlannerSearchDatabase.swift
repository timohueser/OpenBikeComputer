import CryptoKit
import Foundation
import SQLite3

/// The owner serializes queries. Each connection attaches at most eight immutable cells.
final class PlannerSearchDatabase {
    private struct Group { let indices: [Int]; let database: PlannerSearchConnection }
    private struct Plan { let order: String?; let limit: Int?; let branches: [String] }
    private let files: [URL]
    private let bounds: [URL: [Double]]
    private let metadata: PlannerSearchConnection
    private let merge: PlannerSearchConnection
    private var groups: [Group] = []
    private var fuzzy: PlannerFuzzyIndex?
    private var plans: [String: Plan] = [:]

    convenience init(_ file: URL) throws { try self.init(files: [file]) }
    init(files: [URL], bounds: [URL: [Double]] = [:]) throws {
        guard let first = files.first, Set(files).count == files.count,
              bounds.values.allSatisfy({ $0.count == 4 && $0.allSatisfy(\.isFinite) && $0[0] < $0[2] && $0[1] < $0[3] }) else {
            throw plannerSearchError("Invalid search cell selection")
        }
        self.files = files; self.bounds = bounds
        metadata = try PlannerSearchConnection(first.path)
        merge = try PlannerSearchConnection(":memory:", memory: true)
        let reference = try Self.provenance(metadata)
        for file in files {
            let current = try Self.provenance(PlannerSearchConnection(file.path))
            guard current["schema"] == reference["schema"], current["osm_sha256"] == reference["osm_sha256"] else {
                throw plannerSearchError("Search components have incompatible provenance")
            }
            let component = file.pathComponents.first { $0 == "pois" || $0 == "addresses" }
            if let component, current["component"] != "\"\(component)\"" {
                throw plannerSearchError("Search component identity differs")
            }
            let expected = bounds[file].flatMap { try? JSONSerialization.data(withJSONObject: $0, options: [.sortedKeys]) }
            let coverage = expected.map { String(decoding: $0, as: UTF8.self) } ?? reference["bounds"]
            guard current["bounds"] == coverage else { throw plannerSearchError("Search components have incompatible coverage") }
        }
        if files.count == 1 { groups = [Group(indices: [0], database: metadata)]; return }
        for start in stride(from: 0, to: files.count, by: 8) {
            let indices = Array(start..<min(start + 8, files.count))
            let database = try PlannerSearchConnection(":memory:", memory: true)
            for index in indices {
                var uri = URLComponents(url: files[index], resolvingAgainstBaseURL: false)
                uri?.queryItems = [URLQueryItem(name: "mode", value: "ro"), URLQueryItem(name: "immutable", value: "1")]
                guard let path = uri?.string else { throw plannerSearchError("Invalid search file") }
                _ = try database.rows("ATTACH DATABASE ? AS c\(index)", [path])
                _ = try database.rows("PRAGMA c\(index).cache_size=-\(max(128, 32768 / files.count))")
                _ = try database.rows("PRAGMA c\(index).mmap_size=0")
            }
            groups.append(Group(indices: indices, database: database))
        }
        guard let connection = groups.first?.database.connection else { throw plannerSearchError("Search cells did not open") }
        let state = try PlannerFuzzyIndex(connection: connection)
        var words = Set<Data>()
        for group in groups {
            guard let connection = group.database.connection else { throw plannerSearchError("Search connection closed") }
            try state.install(connection)
            for i in group.indices {
                let rows = try group.database.rows("SELECT rowid,term FROM c\(i).lexicon ORDER BY rowid")
                state.owners.append([UInt64](repeating: 0, count: rows.count / 64 + 1))
                for (offset, row) in rows.enumerated() {
                    guard let id = row["rowid"] as? Int64, id == offset + 1, let term = row["term"] as? String else {
                        throw plannerSearchError("Invalid search lexicon")
                    }
                    if words.insert(Data(term.utf8)).inserted {
                        state.owners[i][Int(id) / 64] |= 1 << UInt64(id % 64)
                        try state.add(term)
                    }
                }
            }
        }
        fuzzy = state
    }

    private static func provenance(_ connection: PlannerSearchConnection) throws -> [String: String] {
        var result: [String: String] = [:]
        for row in try connection.rows("SELECT * FROM metadata") {
            guard let key = row["key"] as? String, let value = row["value"] as? String else {
                throw plannerSearchError("Invalid search metadata")
            }
            if key == "bounds" {
                let object = try JSONSerialization.jsonObject(with: Data(value.utf8))
                let canonical = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
                result[key] = String(decoding: canonical, as: UTF8.self)
            } else { result[key] = value }
        }
        return result
    }

    private func plan(_ sql: String) -> Plan {
        if let cached = plans[sql] { return cached }
        let orderRange = sql.range(of: "ORDER BY", options: [.caseInsensitive, .backwards])
        let limitRange = sql.range(of: "LIMIT", options: [.caseInsensitive, .backwards])
        let order = orderRange.map { String(sql[$0.upperBound..<(limitRange?.lowerBound ?? sql.endIndex)]).trimmingCharacters(in: .whitespacesAndNewlines) }
        let limit = limitRange.flatMap { Int(sql[$0.upperBound...].trimmingCharacters(in: .whitespacesAndNewlines)) }
        let branches = files.indices.map { i in sql.replacingOccurrences(
            of: "\\b(FROM|JOIN)\\s+(place_records|places|names|compact_names|terms|fuzzy|lexicon|spatial|addresses)\\b",
            with: "$1 c\(i).$2", options: [.regularExpression, .caseInsensitive]) }
        let result = Plan(order: order, limit: limit, branches: branches)
        if plans.count >= 128 { plans.removeAll() }
        plans[sql] = result
        return result
    }
    private func selected(_ indices: [Int], _ options: [String: Any]) -> [Int] {
        guard let requested = options["bounds"] as? [Double], requested.count == 4 else { return indices }
        return indices.filter { i in
            guard let b = bounds[files[i]] else { return true }
            return b[0] <= requested[2] && b[2] >= requested[0] && b[1] <= requested[3] && b[3] >= requested[1]
        }
    }
    private func statement(_ sql: String, _ parameters: [Any], _ options: [String: Any], _ group: Group) throws -> (String, [Any])? {
        if files.count == 1 { return (sql, parameters) }
        if sql.contains("FROM fuzzy"), let fuzzy {
            let count = sql.components(separatedBy: "(instr(l.term,?)>0)").count - 1
            guard let grams = Array(parameters.prefix(count)) as? [String] else { throw plannerSearchError("Invalid fuzzy query") }
            try fuzzy.prepare(grams: grams)
            let parts = group.indices.map { i in
                let branch = sql.replacingOccurrences(of: "SELECT l.term,", with: "SELECT l.term,obc_rank(fuzzy) AS _rank,")
                    .replacingOccurrences(of: "FROM fuzzy JOIN lexicon", with: "FROM c\(i).fuzzy JOIN c\(i).lexicon")
                    .replacingOccurrences(of: "ORDER BY hits DESC,rank LIMIT", with: "AND obc_owned(\(i),l.rowid) ORDER BY hits DESC,_rank,l.term LIMIT")
                return "SELECT * FROM (\(branch))"
            }
            return ("SELECT term,hits,_rank FROM (" + parts.joined(separator: " UNION ALL ") + ") ORDER BY hits DESC,_rank,term LIMIT 256",
                    group.indices.flatMap { _ in parameters })
        }
        let normalized = sql.contains("FROM terms ") ? sql.replacingOccurrences(of: "ORDER BY rank, ", with: "ORDER BY ") : sql
        let plan = plan(normalized), indices = selected(group.indices, options)
        guard !indices.isEmpty else { return nil }
        let parts = indices.map { "SELECT * FROM (\(plan.branches[$0]))" }
        var values = indices.flatMap { _ in parameters }
        if parts.count == 1 { return (parts[0], values) }
        var merged = "SELECT DISTINCT p.* FROM (" + parts.joined(separator: " UNION ALL ") + ") p"
        if let order = plan.order {
            merged += " ORDER BY " + order
            values.append(contentsOf: parameters.suffix(order.filter { $0 == "?" }.count))
        }
        if let limit = plan.limit { merged += " LIMIT \(limit)" }
        return (merged, values)
    }

    private func combine(_ replies: [String], columns: [String], order: String? = nil, limit: Int? = nil, parameters: [Any] = []) throws -> String {
        guard !replies.isEmpty else { return "{\"rows\":[]}" }
        let fields = columns.map { name in
            let path = "$.\"" + name.replacingOccurrences(of: "\"", with: "\\\"") + "\""
            return "json_extract(value,'" + path.replacingOccurrences(of: "'", with: "''") + "') AS \"" + name.replacingOccurrences(of: "\"", with: "\"\"") + "\""
        }.joined(separator: ",")
        let parts = replies.map { _ in "SELECT \(fields) FROM json_each(?,'$.rows')" }
        var sql = "SELECT DISTINCT p.* FROM (" + parts.joined(separator: " UNION ALL ") + ") p"
        var values: [Any] = replies
        if let order { sql += " ORDER BY " + order; values.append(contentsOf: parameters.suffix(order.filter { $0 == "?" }.count)) }
        if let limit { sql += " LIMIT \(limit)" }
        return try merge.json(sql, values)
    }
    private func query(_ sql: String, _ parameters: [Any], _ options: [String: Any]) throws -> String {
        if sql.contains("FROM metadata") { return try metadata.json(sql, parameters) }
        if groups.count > 1, sql.contains("SELECT p.* FROM spatial") {
            let narrow = sql.replacingOccurrences(of: "p.*", with: "p.id,p.kind,p.importance,p.lon,p.lat,p.source")
                .replacingOccurrences(of: "JOIN places p", with: "JOIN place_records p")
            let ranked = try query(narrow, parameters, options)
            let ids = "SELECT json_group_array(json_extract(value,'$.id')) AS ids FROM json_each(?,'$.rows')"
            guard let selected = try merge.rows(ids, [ranked]).first?["ids"] as? String else {
                throw plannerSearchError("Missing search candidate identities")
            }
            let records = try places(selected)
            let ordered = """
                WITH records AS MATERIALIZED (SELECT json_extract(value,'$.id') AS id,value FROM json_each(?,'$.rows'))
                SELECT json_object('rows',json_group_array(json(record))) AS result FROM (
                  SELECT r.value AS record FROM json_each(?) i JOIN records r
                    ON r.id=i.value ORDER BY CAST(i.key AS INTEGER))
                """
            guard let result = try merge.rows(ordered, [records, selected]).first?["result"] as? String else {
                throw plannerSearchError("Missing ordered search records")
            }
            return result
        }
        let selected = try groups.compactMap { group -> (Group, String, [Any])? in
            guard let (query, values) = try statement(sql, parameters, options, group) else { return nil }
            return (group, query, values)
        }
        guard let first = selected.first else { return "{\"rows\":[]}" }
        if selected.count == 1 { return try first.0.database.json(first.1, first.2) }
        let replies = try selected.map { try $0.0.database.json($0.1, $0.2) }
        let fuzzyQuery = sql.contains("FROM fuzzy")
        let normalized = sql.contains("FROM terms ") ? sql.replacingOccurrences(of: "ORDER BY rank, ", with: "ORDER BY ") : sql
        let plan = plan(normalized)
        if plan.order == nil, plan.limit == nil, sql.contains("p.*") { return try distinctPlaces(replies) }
        return try combine(replies, columns: first.0.database.columns(first.1),
            order: fuzzyQuery ? "hits DESC,_rank,term" : plan.order, limit: fuzzyQuery ? 256 : plan.limit, parameters: parameters)
    }
    private func distinctPlaces(_ replies: [String]) throws -> String {
        let selected = replies.map { _ in "SELECT value FROM json_each(?,'$.rows')" }.joined(separator: " UNION ALL ")
        let sql = "SELECT json_object('rows',json_group_array(json(value))) AS result FROM (SELECT value FROM ("
            + selected + ") GROUP BY json_extract(value,'$.id'))"
        guard let result = try merge.rows(sql, replies).first?["result"] as? String else {
            throw plannerSearchError("Missing search candidates")
        }
        return result
    }
    private func places(_ ids: String) throws -> String {
        let rows = try groups.map { group in
            let sql = group.indices.map { "SELECT p.* FROM c\($0).places p WHERE p.id IN (SELECT value FROM json_each(?))" }.joined(separator: " UNION ")
            return try group.database.json(sql, group.indices.map { _ in ids })
        }
        return try distinctPlaces(rows)
    }
    func all(_ sql: String, _ parameters: String, _ options: String = "{}") -> String {
        reply {
            guard let values = try JSONSerialization.jsonObject(with: Data(parameters.utf8)) as? [Any],
                  let options = try JSONSerialization.jsonObject(with: Data(options.utf8)) as? [String: Any] else {
                throw plannerSearchError("Invalid search query arguments")
            }
            return try query(sql, values, options)
        }
    }
    private func candidateBatch(_ queries: [[String: Any]]) throws -> String? {
        var order: String?, selections: [(String, [Any], [String: Any], Int)] = []
        for query in queries {
            guard let original = query["sql"] as? String, let parameters = query["params"] as? [Any] else {
                throw plannerSearchError("Invalid candidate query")
            }
            let sql = original.replacingOccurrences(of: "p.*", with: "p.id,p.kind,p.importance,p.lon,p.lat,p.source")
                .replacingOccurrences(of: "JOIN places p", with: "JOIN place_records p")
                .replacingOccurrences(of: "FROM places p", with: "FROM place_records p")
                .replacingOccurrences(of: "ORDER BY rank, ", with: "ORDER BY ")
            let plan = plan(sql)
            guard let current = plan.order, !current.contains("?"), let limit = plan.limit,
                  order == nil || order == current else { return nil }
            order = current
            selections.append((sql, parameters, query["options"] as? [String: Any] ?? [:], limit))
        }
        guard let order else { return nil }
        let replies = try groups.map { group in
            var parts: [String] = [], values: [Any] = []
            for (index, selection) in selections.enumerated() {
                if let (sql, bindings) = try statement(selection.0, selection.1, selection.2, group) {
                    parts.append("SELECT \(index) AS branch,\(selection.3) AS cap,p.* FROM (\(sql)) p")
                    values.append(contentsOf: bindings)
                }
            }
            return parts.isEmpty ? "{\"rows\":[]}" : try group.database.json(parts.joined(separator: " UNION ALL "), values)
        }
        let fields = ["branch", "cap", "id", "kind", "importance", "lon", "lat", "source"].map {
            "json_extract(value,'$.\($0)') AS \($0)"
        }.joined(separator: ",")
        let parts = replies.map { _ in "SELECT \(fields) FROM json_each(?,'$.rows')" }.joined(separator: " UNION ALL ")
        let sql = """
            WITH candidates AS MATERIALIZED (SELECT DISTINCT * FROM (\(parts)))
            SELECT json_group_array(DISTINCT id) AS ids FROM (
              SELECT id,cap,ROW_NUMBER() OVER (PARTITION BY branch ORDER BY \(order)) AS position
              FROM candidates p) WHERE position<=cap
            """
        guard let ids = try merge.rows(sql, replies).first?["ids"] as? String else {
            throw plannerSearchError("Missing search candidate identities")
        }
        return try places(ids)
    }
    func batch(_ encoded: String) -> String {
        reply {
            guard let queries = try JSONSerialization.jsonObject(with: Data(encoded.utf8)) as? [[String: Any]], !queries.isEmpty else {
                throw plannerSearchError("Invalid candidate queries")
            }
            if groups.count > 1, let result = try candidateBatch(queries) { return result }
            var parts: [String] = [], values: [Any] = [], replies: [String] = []
            for query in queries {
                guard let original = query["sql"] as? String, let parameters = query["params"] as? [Any] else {
                    throw plannerSearchError("Invalid candidate query")
                }
                let narrow = original.replacingOccurrences(of: "p.*", with: "p.id,p.kind,p.importance,p.lon,p.lat,p.source")
                    .replacingOccurrences(of: "JOIN places p", with: "JOIN place_records p")
                    .replacingOccurrences(of: "FROM places p", with: "FROM place_records p")
                let options = query["options"] as? [String: Any] ?? [:]
                if groups.count == 1 {
                    if let (sql, bindings) = try statement(narrow, parameters, options, groups[0]) {
                        parts.append("SELECT id FROM (\(sql))"); values.append(contentsOf: bindings)
                    }
                } else { replies.append(try self.query(narrow, parameters, options)) }
            }
            if groups.count == 1 {
                guard !parts.isEmpty else { return "{\"rows\":[]}" }
                let wanted = "WITH wanted AS MATERIALIZED (" + parts.joined(separator: " UNION ") + ") "
                let selected = groups[0].indices.map { i in
                    "SELECT p.* FROM \(files.count == 1 ? "" : "c\(i).")places p WHERE p.id IN (SELECT id FROM wanted)"
                }.joined(separator: " UNION ")
                return try groups[0].database.json(wanted + selected, values)
            }
            let idQuery = "SELECT json_group_array(DISTINCT id) AS ids FROM (" + replies.map { _ in
                "SELECT json_extract(value,'$.id') AS id FROM json_each(?,'$.rows')"
            }.joined(separator: " UNION ALL ") + ")"
            guard let ids = try merge.rows(idQuery, replies).first?["ids"] as? String else { throw plannerSearchError("Missing search candidate identities") }
            return try places(ids)
        }
    }
    private func reply(_ body: () throws -> String) -> String {
        do { return try body() }
        catch { return String(decoding: (try? JSONSerialization.data(withJSONObject: ["error": error.localizedDescription])) ?? Data("{\"error\":\"Search failed\"}".utf8), as: UTF8.self) }
    }
}

func plannerFileHash(_ url: URL) throws -> String {
    let file = try FileHandle(forReadingFrom: url)
    defer { try? file.close() }
    var hash = SHA256()
    while try autoreleasepool(invoking: { () throws -> Bool in
        guard let data = try file.read(upToCount: 1 << 20), !data.isEmpty else { return false }
        hash.update(data: data)
        return true
    }) {}
    return hash.finalize().map { String(format: "%02x", $0) }.joined()
}
