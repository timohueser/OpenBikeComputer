import Foundation
import SwiftUI

/// Text in the device's own Terminus bitmap font, drawn from the firmware's glyph strips. Use it
/// only where the device speaks: its name, its state, its screen. The empty rows above and below
/// the ink are trimmed, so the frame hugs the letters. A pixel is `scale` points.
public struct PixelText: View {
    let text: String
    let size: PixelFont
    let scale: CGFloat
    let color: Color

    public init(_ text: String, size: PixelFont = .caption, scale: CGFloat = 1, color: Color = OBCTheme.ink) {
        self.text = text
        self.size = size
        self.scale = scale
        self.color = color
    }

    public var body: some View {
        let bitmap = PixelBitmap(text, font: size)
        Canvas { context, _ in
            var path = Path()
            for run in bitmap.runs {
                path.addRect(CGRect(x: CGFloat(run.x) * scale, y: CGFloat(run.y) * scale,
                                    width: CGFloat(run.length) * scale, height: scale))
            }
            context.fill(path, with: .color(color))
        }
        .frame(width: CGFloat(bitmap.width) * scale, height: CGFloat(bitmap.height) * scale)
        .accessibilityElement()
        .accessibilityLabel(text)
    }
}

/// The five Terminus bold cuts the firmware ships, by cell height in pixels.
public enum PixelFont: Int, CaseIterable, Sendable {
    /// 10 x 20.
    case caption = 20
    /// 12 x 24.
    case label = 24
    /// 14 x 28.
    case body = 28
    /// 16 x 32.
    case display = 32
    /// 32 x 64. The strip stops after ASCII, so a later glyph draws blank.
    case huge = 64

    var cellWidth: Int { rawValue / 2 }
    var cellHeight: Int { rawValue }

    /// The strip's file name in the firmware and in the package resources.
    var resourceName: String { "ter_u\(rawValue)b" }

    /// The glyph strip: 16 glyphs a row, MSB first, each strip row byte-aligned.
    var strip: Data { Self.strips[self] ?? Data() }

    private static let strips: [PixelFont: Data] = Dictionary(uniqueKeysWithValues: allCases.map { font in
        let url = Bundle.module.url(forResource: font.resourceName, withExtension: "raw", subdirectory: "Terminus")
        return (font, url.flatMap { try? Data(contentsOf: $0) } ?? Data())
    })

    /// The strip slot of a scalar, in the firmware's `latin` mapping order. An unmapped scalar
    /// draws as `?`, as on the device.
    static func glyphIndex(_ scalar: Unicode.Scalar) -> Int {
        switch scalar.value {
        case 0x20...0x7F: Int(scalar.value - 0x20)
        case 0xA0...0xFF: 96 + Int(scalar.value - 0xA0)
        case 0x100...0x17F: 192 + Int(scalar.value - 0x100)
        default: Int(("?" as Unicode.Scalar).value - 0x20)
        }
    }
}

/// A line of Terminus text as horizontal pixel runs, trimmed to the rows that carry ink.
struct PixelBitmap {
    struct Run: Equatable {
        let x: Int
        let y: Int
        let length: Int
    }

    let width: Int
    let height: Int
    /// The first inked row of the font cell, so a caller can place the trimmed bitmap by its cell.
    let inkTop: Int
    let runs: [Run]

    init(_ text: String, font: PixelFont) {
        let glyphs = text.precomposedStringWithCanonicalMapping.unicodeScalars.map(PixelFont.glyphIndex)
        let strip = [UInt8](font.strip)
        let cellWidth = font.cellWidth
        let stride = 16 * cellWidth / 8

        func ink(_ glyph: Int, _ x: Int, _ y: Int) -> Bool {
            let row = (glyph / 16) * font.cellHeight + y
            let column = (glyph % 16) * cellWidth + x
            let index = row * stride + (column >> 3)
            return index < strip.count && strip[index] & (0x80 >> (column & 7)) != 0
        }

        var runs: [Run] = []
        for y in 0..<font.cellHeight {
            var start: Int?
            for x in 0...(glyphs.count * cellWidth) {
                let on = x < glyphs.count * cellWidth && ink(glyphs[x / cellWidth], x % cellWidth, y)
                if on, start == nil { start = x }
                if !on, let first = start {
                    runs.append(Run(x: first, y: y, length: x - first))
                    start = nil
                }
            }
        }
        let top = runs.map(\.y).min() ?? 0
        let bottom = runs.map(\.y).max() ?? font.cellHeight - 1
        self.width = glyphs.count * cellWidth
        self.height = bottom - top + 1
        self.inkTop = top
        self.runs = runs.map { Run(x: $0.x, y: $0.y - top, length: $0.length) }
    }
}

#Preview("Pixel text") {
    VStack(alignment: .leading, spacing: 12) {
        PixelText("TRAILHEAD", size: .label, color: OBCTheme.onRust)
            .padding(8)
            .background(OBCTheme.rust)
        PixelText("ON DEVICE", color: OBCTheme.ink)
        PixelText("Grimsel Pass", size: .display, scale: 2)
    }
    .padding(20)
    .background(OBCTheme.page)
}
