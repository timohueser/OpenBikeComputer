import SwiftUI

/// The signpost mark, with a lighter post for dark surfaces.
public struct OBCLogo: View {
    public init() {}

    public var body: some View {
        Image("Signpost", bundle: .module)
            .resizable()
            .scaledToFit()
            .accessibilityHidden(true)
    }
}
