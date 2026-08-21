import Foundation

/// The result of an authenticated BLE imperative command. Protocol-v4 object transfers use
/// `objectControl` records instead; clock, bond, and weather request control remain here.
public struct CommandResult: Equatable, Sendable {
    public enum Status: UInt8, Equatable, Sendable, CaseIterable {
        case ok = 0
        case unknownCommand = 1
        case notFound = 2
        case busy = 3
        case error = 4
    }

    public var command: UInt8
    public var status: Status
    public var detail: UInt8

    public init(command: UInt8, status: Status, detail: UInt8 = 0) {
        self.command = command
        self.status = status
        self.detail = detail
    }
}

/// The live subset of the legacy `status` characteristic: command result (`msg = 3`) only.
/// Object transfer results, catalog edges, download announcements, and weather hints retired with
/// protocol v2; unknown discriminators remain ignorable for forward compatibility.
public enum StatusMessage: Equatable, Sendable {
    case commandResult(CommandResult)
    case unknown(UInt8)

    public func encode() -> Data {
        var data = Data()
        switch self {
        case .commandResult(let result):
            data.append(3)
            data.append(result.command)
            data.append(result.status.rawValue)
            data.append(result.detail)
        case .unknown(let message):
            data.append(message)
        }
        return data
    }

    public init(decoding data: Data) throws {
        guard let message = data.first else { throw StatusMessageError.truncated }
        let base = data.startIndex
        switch message {
        case 3:
            guard data.count >= 4 else { throw StatusMessageError.truncated }
            guard let status = CommandResult.Status(rawValue: data[base + 2]) else {
                throw StatusMessageError.unknownStatus(data[base + 2])
            }
            self = .commandResult(CommandResult(
                command: data[base + 1], status: status, detail: data[base + 3]
            ))
        default:
            self = .unknown(message)
        }
    }
}

public enum StatusMessageError: Error, Equatable, Sendable {
    case truncated
    case unknownStatus(UInt8)
}
