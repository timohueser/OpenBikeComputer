import Foundation
import XCTest
@testable import OBCTransport

final class StatusMessageTests: XCTestCase {
    func testCommandResultRoundTrips() throws {
        for status in CommandResult.Status.allCases {
            let message = StatusMessage.commandResult(
                CommandResult(command: 7, status: status, detail: 9)
            )
            XCTAssertEqual(message.encode().count, 4)
            XCTAssertEqual(try StatusMessage(decoding: message.encode()), message)
        }
    }

    func testRetiredAndFutureDiscriminatorsAreIgnorable() throws {
        for discriminator: UInt8 in [1, 2, 4, 5, 0x7F] {
            XCTAssertEqual(
                try StatusMessage(decoding: Data([discriminator, 1, 2, 3])),
                .unknown(discriminator)
            )
        }
    }

    func testRejectsTruncatedAndUnknownCommandStatus() {
        XCTAssertThrowsError(try StatusMessage(decoding: Data())) {
            XCTAssertEqual($0 as? StatusMessageError, .truncated)
        }
        XCTAssertThrowsError(try StatusMessage(decoding: Data([3, 7, 0]))) {
            XCTAssertEqual($0 as? StatusMessageError, .truncated)
        }

        var command = StatusMessage.commandResult(
            CommandResult(command: 7, status: .ok)
        ).encode()
        command[command.startIndex + 2] = 0x7F
        XCTAssertThrowsError(try StatusMessage(decoding: command)) {
            XCTAssertEqual($0 as? StatusMessageError, .unknownStatus(0x7F))
        }
    }
}
