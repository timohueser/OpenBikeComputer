import SwiftUI

/// A one-time offer on a detail screen. It dismisses with its ✕ or a swipe to the left.
public struct OBCQuietRow: View {
    let systemImage: String
    let title: String
    let onOpen: () -> Void
    let onDismiss: () -> Void

    @State private var offset: CGFloat = 0

    public init(systemImage: String, title: String, onOpen: @escaping () -> Void, onDismiss: @escaping () -> Void) {
        self.systemImage = systemImage
        self.title = title
        self.onOpen = onOpen
        self.onDismiss = onDismiss
    }

    public var body: some View {
        HStack(spacing: 0) {
            Button(action: onOpen) {
                HStack(spacing: 10) {
                    Image(systemName: systemImage)
                        .font(.system(size: 15, weight: .medium))
                        .foregroundStyle(OBCTheme.water)
                    Text(title)
                        .font(.system(size: 15))
                        .foregroundStyle(OBCTheme.ink)
                    Image(systemName: "chevron.right")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(OBCTheme.inkFaint)
                    Spacer(minLength: 0)
                }
                .padding(.leading, 14)
                .frame(maxHeight: .infinity)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("quietRow.open")
            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .frame(width: 44)
                    .frame(maxHeight: .infinity)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Dismiss")
            .accessibilityIdentifier("quietRow.dismiss")
        }
        .frame(height: 44)
        .background(OBCTheme.panel.opacity(0.6))
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium).strokeBorder(OBCTheme.line))
        .offset(x: offset)
        .opacity(1 - Double(min(-offset, 160) / 200))
        .simultaneousGesture(
            DragGesture(minimumDistance: 16)
                .onChanged { value in
                    guard abs(value.translation.width) > abs(value.translation.height) else { return }
                    offset = min(0, value.translation.width)
                }
                .onEnded { value in
                    if value.translation.width < -90 {
                        onDismiss()
                    } else {
                        withAnimation(.spring) { offset = 0 }
                    }
                }
        )
    }
}
