import Foundation
import SQLite3

private final class PlannerTokens { var values: [Data] = [] }

/// Global statistics and one owner per term make cell FTS ranks match a single index.
final class PlannerFuzzyIndex {
    var owners: [[UInt64]] = []
    private var frequencies: [Data: Int] = [:]
    private var count = 0
    private var tokenCount = 0
    private var idf: [Double] = []
    private var tokenizer = fts5_tokenizer()
    private var tokenInstance: OpaquePointer?

    init(connection: OpaquePointer) throws {
        let api = try Self.api(connection)
        var context: UnsafeMutableRawPointer?
        guard let find = api.pointee.xFindTokenizer,
            find(api, "trigram", &context, &tokenizer) == SQLITE_OK,
            let create = tokenizer.xCreate,
            create(context, nil, 0, &tokenInstance) == SQLITE_OK
        else {
            throw plannerSearchError("Cannot open the search tokenizer")
        }
    }
    deinit { tokenizer.xDelete?(tokenInstance) }

    private static func api(_ connection: OpaquePointer) throws -> UnsafeMutablePointer<fts5_api> {
        var pointer: UnsafeMutablePointer<fts5_api>?
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(connection, "SELECT fts5(?1)", -1, &statement, nil) == SQLITE_OK else {
            throw plannerSearchError("Search requires FTS5")
        }
        let status = "fts5_api_ptr".withCString { kind in
            withUnsafeMutablePointer(to: &pointer) { slot in
                let bound = sqlite3_bind_pointer(statement, 1, slot, kind, nil)
                let result = bound == SQLITE_OK ? sqlite3_step(statement) : bound
                sqlite3_finalize(statement)
                return result
            }
        }
        guard status == SQLITE_ROW, let pointer else { throw plannerSearchError("Cannot open the search index API") }
        return pointer
    }

    func install(_ connection: OpaquePointer) throws {
        let api = try Self.api(connection)
        let state = Unmanaged.passUnretained(self).toOpaque()
        guard
            sqlite3_create_function_v2(
                connection, "obc_owned", 2, SQLITE_UTF8, state,
                { context, _, values in
                    guard let context, let values, let raw = sqlite3_user_data(context) else { return }
                    let state = Unmanaged<PlannerFuzzyIndex>.fromOpaque(raw).takeUnretainedValue()
                    let cell = Int(sqlite3_value_int(values[0]))
                    let row = sqlite3_value_int64(values[1])
                    guard state.owners.indices.contains(cell), row >= 0, row / 64 < state.owners[cell].count else {
                        sqlite3_result_error(context, "Invalid term owner", -1)
                        return
                    }
                    sqlite3_result_int(context, state.owners[cell][Int(row / 64)] & (1 << UInt64(row % 64)) != 0 ? 1 : 0)
                }, nil, nil, nil) == SQLITE_OK, let create = api.pointee.xCreateFunction,
            create(
                api, "obc_rank", state,
                { api, fts, result, _, _ in
                    guard let api, let fts, let result, let raw = api.pointee.xUserData?(fts) else { return }
                    let state = Unmanaged<PlannerFuzzyIndex>.fromOpaque(raw).takeUnretainedValue()
                    guard let phraseCount = api.pointee.xPhraseCount, let instanceCount = api.pointee.xInstCount,
                        let instance = api.pointee.xInst, let columnSize = api.pointee.xColumnSize
                    else {
                        sqlite3_result_error(result, "Incomplete FTS API", -1)
                        return
                    }
                    let n = Int(phraseCount(fts))
                    guard n == state.idf.count, state.count > 0, state.tokenCount > 0 else {
                        sqlite3_result_error(result, "Invalid FTS statistics", -1)
                        return
                    }
                    var frequency = [Double](repeating: 0, count: n)
                    var instances: Int32 = 0
                    guard instanceCount(fts, &instances) == SQLITE_OK else {
                        sqlite3_result_error(result, "Cannot read FTS instances", -1)
                        return
                    }
                    for i in 0..<instances {
                        var phrase: Int32 = 0
                        var column: Int32 = 0
                        var offset: Int32 = 0
                        guard instance(fts, i, &phrase, &column, &offset) == SQLITE_OK, (0..<n).contains(Int(phrase)) else {
                            sqlite3_result_error(result, "Invalid FTS instance", -1)
                            return
                        }
                        frequency[Int(phrase)] += 1
                    }
                    var size: Int32 = 0
                    guard columnSize(fts, -1, &size) == SQLITE_OK else {
                        sqlite3_result_error(result, "Cannot read FTS length", -1)
                        return
                    }
                    let average = Double(state.tokenCount) / Double(state.count)
                    let denominator = 1.2 * (0.25 + 0.75 * Double(size) / average)
                    let score = (0..<n).reduce(0.0) { $0 + state.idf[$1] * frequency[$1] * 2.2 / (frequency[$1] + denominator) }
                    sqlite3_result_double(result, -score)
                }, nil) == SQLITE_OK
        else { throw plannerSearchError("Cannot register cell search functions") }
    }

    private func tokens(_ word: String, flags: Int32) throws -> [Data] {
        guard let tokenize = tokenizer.xTokenize else { throw plannerSearchError("Search tokenizer is unavailable") }
        let list = PlannerTokens()
        let status = word.withCString { text in
            tokenize(
                tokenInstance, Unmanaged.passUnretained(list).toOpaque(), flags, text, Int32(word.utf8.count),
                { raw, _, text, count, _, _ in
                    guard let raw, let text, count >= 0 else { return SQLITE_ERROR }
                    Unmanaged<PlannerTokens>.fromOpaque(raw).takeUnretainedValue().values.append(
                        Data(bytes: text, count: Int(count)))
                    return SQLITE_OK
                })
        }
        guard status == SQLITE_OK else { throw plannerSearchError("Cannot tokenize search text") }
        return list.values
    }
    func add(_ word: String) throws {
        let values = try tokens(word, flags: FTS5_TOKENIZE_DOCUMENT)
        count += 1
        tokenCount += values.count
        for token in Set(values) { frequencies[token, default: 0] += 1 }
    }
    func prepare(grams: [String]) throws {
        idf = try grams.flatMap { try tokens($0, flags: FTS5_TOKENIZE_QUERY) }.map {
            let frequency = Double(frequencies[$0] ?? 0)
            return max(1e-6, log((Double(count) - frequency + 0.5) / (frequency + 0.5)))
        }
    }
}
