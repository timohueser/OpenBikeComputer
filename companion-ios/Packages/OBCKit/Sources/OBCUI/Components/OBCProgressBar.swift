import SwiftUI

/// **Progress Bar** (§9, NEW) — 8pt pill, forest fill on a `parchment-3`
/// track. Mirrors the device's own upload/sync bar for a shared mental model.
public struct OBCProgressBar: View {
    /// 0…1 fraction complete.
    let value: Double

    public init(value: Double) { self.value = value }

    public var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(OBCTheme.parchment3)
                Capsule()
                    .fill(OBCTheme.tint)
                    .frame(width: geo.size.width * max(0, min(value, 1)))
            }
        }
        .frame(height: 8)
        .animation(.easeOut(duration: 0.15), value: value)
        .accessibilityElement(children: .ignore)
        .accessibilityValue("\(Int((max(0, min(value, 1)) * 100).rounded())) percent")
    }
}

#Preview("Progress") {
    VStack(spacing: 16) {
        OBCProgressBar(value: 0.62)
        OBCProgressBar(value: 0.1)
        OBCProgressBar(value: 1)
    }
    .padding(20)
    .background(OBCTheme.parchment)
}
