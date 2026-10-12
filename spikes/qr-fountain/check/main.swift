// Feeds frames.txt to FountainDecoder and checks the result against payload.bin.
// Usage: check DIR
import Foundation

let dir = CommandLine.arguments[1]
let expected = try Data(contentsOf: URL(fileURLWithPath: "\(dir)/payload.bin"))
let frames = try String(contentsOfFile: "\(dir)/frames.txt", encoding: .utf8).split(separator: "\n")
var decoder = FountainDecoder()
for frame in frames {
    switch decoder.receive(String(frame)) {
    case .complete(let payload) where payload == expected:
        print("ok: \(decoder.blockCount) blocks from \(decoder.framesSeen) frames")
        exit(0)
    case .complete, .corrupt, .invalid:
        print("FAIL after \(decoder.framesSeen) frames")
        exit(1)
    default:
        continue
    }
}
print("FAIL: rank \(decoder.rank) of \(decoder.blockCount) after \(decoder.framesSeen) frames")
exit(1)
