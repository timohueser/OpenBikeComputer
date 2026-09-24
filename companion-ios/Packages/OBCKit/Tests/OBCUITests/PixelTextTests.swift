import Foundation
import Testing
@testable import OBCUI

struct PixelTextTests {
    /// The package's glyph strips are copies of the firmware's; the firmware file is the source.
    @Test(arguments: PixelFont.allCases)
    func stripMatchesTheFirmwareFile(_ font: PixelFont) throws {
        let firmware = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent("../../../../../firmware/obc-render/fonts/terminus/\(font.resourceName).raw")
            .standardized
        #expect(try Data(contentsOf: firmware) == font.strip)
    }

    @Test
    func bitmapIsTrimmedToTheInkAndUnmappedScalarsDrawAsQuestionMarks() {
        let word = PixelBitmap("Ä?", font: .caption)
        #expect(word.width == 20)
        #expect(word.height < PixelFont.caption.cellHeight)
        #expect(word.runs.map(\.y).min() == 0)

        let fallback = PixelBitmap("→", font: .caption)
        let question = PixelBitmap("?", font: .caption)
        #expect(fallback.runs == question.runs)
    }
}
