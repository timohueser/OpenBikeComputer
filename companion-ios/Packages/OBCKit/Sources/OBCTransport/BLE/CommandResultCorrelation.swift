/// Correlates the command-byte-only results that remain on the BLE `status` characteristic.
/// Once an exchange becomes ambiguous, results stay rejected until a fresh connection arrives.
struct CommandResultCorrelation {
    private(set) var isAvailable = true
    private var pending: [UInt8: CommandResult] = [:]

    mutating func reconnect() {
        isAvailable = true
        pending.removeAll()
    }

    mutating func invalidate() {
        isAvailable = false
        pending.removeAll()
    }

    mutating func clearPending() {
        pending.removeAll()
    }

    mutating func clearPending(command: UInt8) {
        pending.removeValue(forKey: command)
    }

    mutating func receive(_ result: CommandResult) {
        guard isAvailable else { return }
        pending[result.command] = result
    }

    mutating func take(command: UInt8) -> CommandResult? {
        guard isAvailable else { return nil }
        return pending.removeValue(forKey: command)
    }
}
