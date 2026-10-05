import Foundation
import SQLite3

final class PlannerSearchConnection {
    private var connection: OpaquePointer?
    private var statements: [String: OpaquePointer] = [:]

    init(_ path: String, memory: Bool = false) throws {
        guard
            sqlite3_open_v2(
                path, &connection,
                memory ? SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_URI : SQLITE_OPEN_READONLY | SQLITE_OPEN_URI,
                nil) == SQLITE_OK
        else {
            if let connection { sqlite3_close(connection) }
            connection = nil
            throw NSError(domain: "PlannerSearch", code: 1, userInfo: [NSLocalizedDescriptionKey: "Cannot open search database"])
        }
        sqlite3_exec(connection, "PRAGMA cache_size=-4096; PRAGMA mmap_size=0", nil, nil, nil)
        sqlite3_create_function_v2(
            connection, "sqrt", 1, SQLITE_UTF8 | SQLITE_DETERMINISTIC, nil,
            { context, _, values in
                guard let value = values?[0] else {
                    sqlite3_result_null(context)
                    return
                }
                let number = sqlite3_value_double(value)
                if number < 0 { sqlite3_result_null(context) } else { sqlite3_result_double(context, number.squareRoot()) }
            }, nil, nil, nil)
    }

    deinit {
        statements.values.forEach { sqlite3_finalize($0) }
        sqlite3_close(connection)
    }

    func rows(_ sql: String, _ values: [Any] = []) throws -> [[String: Any]] {
        let statement: OpaquePointer
        if let cached = statements[sql] {
            statement = cached
        } else {
            if statements.count >= 100 {
                statements.values.forEach { sqlite3_finalize($0) }
                statements.removeAll()
            }
            var prepared: OpaquePointer?
            guard sqlite3_prepare_v2(connection, sql, -1, &prepared, nil) == SQLITE_OK, let prepared else {
                throw databaseError()
            }
            statement = prepared
            statements[sql] = statement
        }
        defer {
            sqlite3_reset(statement)
            sqlite3_clear_bindings(statement)
        }
        for (offset, value) in values.enumerated() {
            let index = Int32(offset + 1)
            if value is NSNull {
                sqlite3_bind_null(statement, index)
            } else if let number = value as? NSNumber {
                sqlite3_bind_double(statement, index, number.doubleValue)
            } else if let text = value as? String {
                sqlite3_bind_text(
                    statement, index, text, Int32(text.utf8.count), unsafeBitCast(-1, to: sqlite3_destructor_type.self))
            } else {
                throw NSError(domain: "PlannerSearch", code: 2)
            }
        }
        var rows: [[String: Any]] = []
        while true {
            let status = sqlite3_step(statement)
            if status == SQLITE_DONE { break }
            guard status == SQLITE_ROW else { throw databaseError() }
            var row: [String: Any] = [:]
            for column in 0..<sqlite3_column_count(statement) {
                let name = String(cString: sqlite3_column_name(statement, column))
                switch sqlite3_column_type(statement, column) {
                case SQLITE_INTEGER: row[name] = sqlite3_column_int64(statement, column)
                case SQLITE_FLOAT: row[name] = sqlite3_column_double(statement, column)
                case SQLITE_TEXT:
                    row[name] = String(
                        decoding: UnsafeBufferPointer(
                            start: sqlite3_column_text(statement, column),
                            count: Int(sqlite3_column_bytes(statement, column))), as: UTF8.self)
                default: row[name] = NSNull()
                }
            }
            rows.append(row)
        }
        return rows
    }

    private var jsonQueries: [String: String] = [:]
    func json(_ sql: String, _ values: [Any]) throws -> String {
        let query: String
        if let existing = jsonQueries[sql] {
            query = existing
        } else {
            let fields = try columns(sql).map {
                "'" + $0.replacingOccurrences(of: "'", with: "''") + "',r.\"" + $0.replacingOccurrences(of: "\"", with: "\"\"")
                    + "\""
            }.joined(separator: ",")
            query = "SELECT json_group_array(json_object(" + fields + ")) AS payload FROM (" + sql + ") r"
            if jsonQueries.count >= 128 { jsonQueries.removeAll() }
            jsonQueries[sql] = query
        }
        let rows = try self.rows(query, values)
        guard let payload = rows.first?["payload"] as? String else { throw databaseError() }
        return "{\"rows\":" + payload + "}"
    }
    private func columns(_ sql: String) throws -> [String] {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(connection, sql, -1, &statement, nil) == SQLITE_OK else { throw databaseError() }
        defer { sqlite3_finalize(statement) }
        return (0..<sqlite3_column_count(statement)).map { String(cString: sqlite3_column_name(statement, $0)) }
    }
    private func databaseError() -> NSError {
        NSError(
            domain: "PlannerSearch", code: Int(sqlite3_errcode(connection)),
            userInfo: [NSLocalizedDescriptionKey: String(cString: sqlite3_errmsg(connection))])
    }
}
