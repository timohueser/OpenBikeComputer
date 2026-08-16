import Foundation
import Testing

@testable import OBCProtocolV3

/// §13's stream fault-body transport set, swept as a table rather than trusted from the two
/// fixtures that happen to touch its edges.
///
/// The set was unpinned when this codec was first written — §13 said "namespace-zero transport
/// category/details" without enumerating them, which is a set every §12 category but
/// `semanticValidation` belongs to on a literal reading. Two independent implementations guessed
/// differently on `resourceLimit`; the contract now names exactly ten categories, and this sweep is
/// what keeps a future edit from re-opening the question.
@Suite("Device Object v3 — stream fault categories")
struct StreamFaultCategoryTests {
    /// §13, verbatim: `invalidFrame`, `invalidDescriptor`, `invalidOffset`, `invalidSession`,
    /// `checksumFailure`, `mediaUnavailable`, `mediaIo`, `cancelled`, `linkLost`, `internal`.
    static let transportSet: Set<ErrorCategory> = [
        .invalidFrame, .invalidDescriptor, .invalidOffset, .invalidSession, .checksumFailure,
        .mediaUnavailable, .mediaIo, .cancelled, .linkLost, .internal,
    ]

    /// A nonterminal fault frame (fault bit alone, disposition `0`) carrying `category`, built by
    /// patching the category field of a frame that is otherwise identical to the
    /// `fault-resume-with-new-session` vector.
    static func faultRecord(category: UInt16) -> [UInt8] {
        var writer = ByteWriter()
        writer.u32(17)  // SessionId
        writer.u64(0)  // status direction carries offset zero
        writer.u16(UInt16(StreamFault.bodyBytes))
        writer.u8(StreamDirection.status.rawValue)
        writer.u8(StreamFlags.fault.rawValue)
        writer.u16(category)
        writer.u16(1)  // detail
        writer.u64(4)  // expected next offset
        writer.u64(4)  // durable next offset
        writer.u8(StreamFaultDisposition.resumeWithNewSession.rawValue)
        writer.zeros(3)
        return writer.bytes
    }

    @Test("the set is exactly ten categories")
    func setSize() {
        #expect(Self.transportSet.count == 10)
        #expect(ErrorCategory.allCases.filter(\.isStreamFaultCategory).count == 10)
        #expect(Set(ErrorCategory.allCases.filter(\.isStreamFaultCategory)) == Self.transportSet)
    }

    @Test(
        "every §12 category is accepted or rejected in a real fault frame exactly as §13 says",
        arguments: ErrorCategory.allCases)
    func sweep(_ category: ErrorCategory) throws {
        let record = Self.faultRecord(category: category.rawValue)
        if Self.transportSet.contains(category) {
            let frame = try StreamFrame.decode(record)
            guard case .fault(let fault) = frame.body else {
                throw VectorError("\(category.name) did not decode as a fault")
            }
            #expect(fault.category == category)
            #expect(try frame.encoded() == record)
        } else {
            do {
                _ = try StreamFrame.decode(record)
                Issue.record("\(category.name) is not a transport category but decoded")
            } catch let fault as WireFault {
                // §13's rejection is a descriptor-level unknown enum, the same one the checked-in
                // `stream-fault-domain-category` and `stream-fault-semantic-category` vectors pin.
                #expect(fault.category == .invalidDescriptor)
                #expect(fault.detailName == "unknownEnum")
            }
        }
    }

    /// The two categories §13 calls out by name, with the reasons it gives.
    @Test("resourceLimit and semanticValidation are the named exclusions")
    func namedExclusions() {
        #expect(!ErrorCategory.resourceLimit.isStreamFaultCategory)
        #expect(!ErrorCategory.semanticValidation.isStreamFaultCategory)
    }

    /// A category number §12 does not define at all is rejected the same way, so the sweep above
    /// covers the whole `u16` space by construction rather than only the 22 registered values.
    @Test("an unregistered category number is rejected too")
    func unregisteredCategory() {
        #expect(throws: WireFault.self) {
            _ = try StreamFrame.decode(Self.faultRecord(category: 9999))
        }
    }
}
