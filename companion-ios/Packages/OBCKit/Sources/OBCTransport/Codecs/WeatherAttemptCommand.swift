import Foundation

/// Report an active or failed attempt without satisfying the weather request.
public enum WeatherAttemptCommand {
    public static let commandByte: UInt8 = 8

    public static func encode(requestID: UInt32, started: Bool) -> Data {
        precondition(requestID != 0)
        return Data([commandByte, UInt8(truncatingIfNeeded: requestID),
                     UInt8(truncatingIfNeeded: requestID >> 8),
                     UInt8(truncatingIfNeeded: requestID >> 16),
                     UInt8(truncatingIfNeeded: requestID >> 24), started ? 1 : 0])
    }
}
