import CryptoKit
import Foundation
import SQLite3

final class PlannerSearchDatabase {
    private var connection: OpaquePointer?
    private var statements: [String: OpaquePointer] = [:]

    init(_ file: URL) throws {
        guard sqlite3_open_v2(file.path, &connection, SQLITE_OPEN_READONLY, nil) == SQLITE_OK else {
            throw NSError(domain: "PlannerSearch", code: 1, userInfo: [NSLocalizedDescriptionKey: "Cannot open search database"])
        }
        sqlite3_exec(connection, "PRAGMA query_only=ON; PRAGMA cache_size=-32768; PRAGMA mmap_size=0", nil, nil, nil)
        sqlite3_create_function_v2(connection, "sqrt", 1, SQLITE_UTF8 | SQLITE_DETERMINISTIC, nil, { context, _, values in
            guard let value = values?[0] else { sqlite3_result_null(context); return }
            let number = sqlite3_value_double(value)
            if number < 0 { sqlite3_result_null(context) } else { sqlite3_result_double(context, number.squareRoot()) }
        }, nil, nil, nil)
    }

    deinit {
        statements.values.forEach { sqlite3_finalize($0) }
        sqlite3_close(connection)
    }

    func all(_ sql: String, _ parameters: String) -> String {
        do {
            let values = try JSONSerialization.jsonObject(with: Data(parameters.utf8)) as? [Any] ?? []
            let statement: OpaquePointer
            if let cached = statements[sql] { statement = cached } else {
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
            defer { sqlite3_reset(statement); sqlite3_clear_bindings(statement) }
            for (offset, value) in values.enumerated() {
                let index = Int32(offset + 1)
                if value is NSNull { sqlite3_bind_null(statement, index) }
                else if let number = value as? NSNumber { sqlite3_bind_double(statement, index, number.doubleValue) }
                else if let text = value as? String {
                    sqlite3_bind_text(statement, index, text, Int32(text.utf8.count), unsafeBitCast(-1, to: sqlite3_destructor_type.self))
                } else { throw NSError(domain: "PlannerSearch", code: 2) }
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
                        row[name] = String(decoding: UnsafeBufferPointer(start: sqlite3_column_text(statement, column),
                            count: Int(sqlite3_column_bytes(statement, column))), as: UTF8.self)
                    default: row[name] = NSNull()
                    }
                }
                rows.append(row)
            }
            return String(decoding: try JSONSerialization.data(withJSONObject: ["rows": rows]), as: UTF8.self)
        } catch {
            let payload = ["error": error.localizedDescription]
            return String(decoding: (try? JSONSerialization.data(withJSONObject: payload)) ?? Data(), as: UTF8.self)
        }
    }

    private func databaseError() -> NSError {
        NSError(domain: "PlannerSearch", code: Int(sqlite3_errcode(connection)),
                userInfo: [NSLocalizedDescriptionKey: String(cString: sqlite3_errmsg(connection))])
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
