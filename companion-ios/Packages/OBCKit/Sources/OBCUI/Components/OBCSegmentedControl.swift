import SwiftUI

/// **Segmented Control** (§9, EXT) — the field-guide take on iOS segments:
/// `parchment-3` sunken track (11pt radius, 3pt padding) with the selected
/// segment raised on `panel` (8pt radius, soft shadow). Drives the
/// Planned / Tracked split on the main screen.
public struct OBCSegmentedControl: View {
    @Binding var selection: Int
    let labels: [String]
    @Namespace private var thumb

    public init(selection: Binding<Int>, labels: [String]) {
        self._selection = selection
        self.labels = labels
    }

    public var body: some View {
        HStack(spacing: 3) {
            ForEach(labels.indices, id: \.self) { index in
                Button {
                    withAnimation(.easeOut(duration: 0.15)) { selection = index }
                } label: {
                    Text(labels[index])
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(selection == index ? OBCTheme.ink : OBCTheme.inkSoft)
                        .padding(.vertical, 8)
                        .frame(maxWidth: .infinity)
                        .background {
                            if selection == index {
                                RoundedRectangle(cornerRadius: 8)
                                    .fill(OBCTheme.panel)
                                    .shadow(color: OBCTheme.ink.opacity(0.16), radius: 1.5, y: 1)
                                    .matchedGeometryEffect(id: "thumb", in: thumb)
                            }
                        }
                }
                .buttonStyle(.plain)
            }
        }
        .padding(3)
        .background(OBCTheme.parchment3)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
    }
}

#Preview("Segmented") {
    struct Demo: View {
        @State private var tab = 0
        var body: some View {
            OBCSegmentedControl(selection: $tab, labels: ["Planned", "Tracked"])
                .padding(20)
                .background(OBCTheme.parchment)
        }
    }
    return Demo()
}
