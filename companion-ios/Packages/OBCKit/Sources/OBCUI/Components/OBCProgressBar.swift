import SwiftUI

/// A pill progress bar. It mirrors the device's own upload and sync bar, for a shared
/// mental model.
public struct OBCProgressBar: View {
    /// 0…1 fraction complete.
    let value: Double

    public init(value: Double) { self.value = value }

    public var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(OBCTheme.fill)
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
