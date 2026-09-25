import SwiftUI

/// A card of the device's firmware-update flow, as the firmware draws it.
enum DeviceFirmwareCard: Equatable {
    /// The install confirm: the installed and the sent version over Install and Cancel.
    case confirm(installed: String, update: String)
    /// The frame the device holds while it installs.
    case installing
    /// The first start after an update.
    case updated(version: String)

    var accessibilityText: String {
        switch self {
        case .confirm(_, let update): "The bike computer asks to install \(update), with Install and Cancel"
        case .installing: "The bike computer shows Installing update and Keep power on"
        case .updated(let version): "The bike computer shows Updated to \(version)"
        }
    }
}

/// The device's firmware-update cards in their exact on-glass colours, in the 240 x 320 panel's
/// own pixels, taken from the firmware's layout.
struct DeviceFirmwareScreen: View {
    let card: DeviceFirmwareCard

    var body: some View {
        Canvas { context, size in
            switch card {
            case .confirm(let installed, let update):
                context.deviceFrame(size: size, title: "INSTALL UPDATE")
                versionRow("Installed", installed, capTop: 50, in: &context)
                versionRow("Update", update, capTop: 74, in: &context)
                context.fill(
                    Path(roundedRect: CGRect(x: 12, y: 218, width: 216, height: 42), cornerRadius: 5),
                    with: .color(OBCTheme.deviceTrack)
                )
                context.pixelText("Install", .body, x: 28, capTop: 231, color: OBCTheme.deviceInk)
                context.pixelText("Cancel", .body, x: 28, capTop: 279, color: OBCTheme.deviceInk)
            case .installing:
                context.deviceFrame(size: size, title: "UPDATE")
                var capTop = wrapped("Installing update", .body, capTop: 78, in: &context)
                capTop = wrapped(
                    "The LED blinks while the update installs. The device restarts when it is done.",
                    .label, capTop: capTop + 16, in: &context
                )
                _ = wrapped("Keep power on.", .label, capTop: capTop + 12, color: OBCTheme.deviceWarning, in: &context)
            case .updated(let version):
                context.deviceFrame(size: size, title: "UPDATED")
                var check = Path()
                check.addLines([CGPoint(x: 96, y: 90), CGPoint(x: 112, y: 106), CGPoint(x: 144, y: 74)])
                context.stroke(
                    check, with: .color(OBCTheme.deviceTrack),
                    style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round)
                )
                context.pixelText("Updated to", .body, x: 120, capTop: 142, color: OBCTheme.deviceInk, centered: true)
                context.pixelText(version, .body, x: 120, capTop: 172, color: OBCTheme.deviceTrack, centered: true)
            }
        }
    }

    /// Caption left, version right, one baseline.
    private func versionRow(_ caption: String, _ version: String, capTop: CGFloat, in context: inout GraphicsContext) {
        context.pixelText(caption, .label, x: 12, capTop: capTop, color: OBCTheme.deviceCaption)
        context.pixelText(version, .label, x: 228, capTop: capTop, color: OBCTheme.deviceInk, alignRight: true)
    }

    /// Centred copy, greedily word-wrapped to the 216 px card width as the firmware wraps it.
    /// Returns the cap top of the line after the last.
    private func wrapped(
        _ text: String, _ font: PixelFont, capTop: CGFloat, color: Color = OBCTheme.deviceInk,
        in context: inout GraphicsContext
    ) -> CGFloat {
        let budget = 216 / font.cellWidth
        var lines: [String] = []
        for word in text.split(separator: " ") {
            if let last = lines.last, last.count + 1 + word.count <= budget {
                lines[lines.count - 1] = last + " " + word
            } else {
                lines.append(String(word))
            }
        }
        // The firmware's wrapped line pitch: the cell height less five.
        let pitch = CGFloat(font.cellHeight - 5)
        for (index, line) in lines.enumerated() {
            context.pixelText(line, font, x: 120, capTop: capTop + CGFloat(index) * pitch, color: color, centered: true)
        }
        return capTop + CGFloat(lines.count) * pitch
    }
}
