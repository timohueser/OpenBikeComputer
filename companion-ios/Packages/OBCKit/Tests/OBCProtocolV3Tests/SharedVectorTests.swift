import Foundation
import Testing

@testable import OBCProtocolV3

/// The Swift half of the DOS1 cross-language acceptance gate (`Device_Object_Vectors_v2.md` §7):
/// this codec was written from the normative tables alone and is judged only by the checked-in
/// fixtures under `specs/vectors/device-object-v2/`.
@Suite("Device Object v3 — control vectors")
struct ControlVectorTests {
    @Test("every control fixture decodes and re-encodes byte-exactly", arguments: DeviceObjectVectors.controls)
    func control(_ entry: DeviceObjectVectors.Entry) throws {
        try VectorExerciser.exercise(entry)
    }
}

@Suite("Device Object v3 — negative vectors")
struct NegativeVectorTests {
    @Test("every negative fixture is rejected with its exact category and detail", arguments: DeviceObjectVectors.negatives)
    func negative(_ entry: DeviceObjectVectors.Entry) throws {
        try VectorExerciser.exercise(entry)
    }
}

@Suite("Device Object v3 — stream vectors")
struct StreamVectorTests {
    @Test("every stream fixture decodes and re-encodes byte-exactly", arguments: DeviceObjectVectors.streams)
    func stream(_ entry: DeviceObjectVectors.Entry) throws {
        try VectorExerciser.exercise(entry)
    }
}

@Suite("Device Object v3 — transcripts")
struct TranscriptVectorTests {
    @Test("every transcript replays as a decode/encode sequence", arguments: DeviceObjectVectors.transcripts)
    func transcript(_ entry: DeviceObjectVectors.Entry) throws {
        try VectorExerciser.exercise(entry)
    }
}
