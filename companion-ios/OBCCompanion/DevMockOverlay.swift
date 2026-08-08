#if DEBUG
import SwiftUI
import Combine
import UIKit
import OBCMock
import OBCUI

// B1P's app-side wiring, Debug-only in its entirety: the shake gesture that opens
// the dev control panel, the panel sheet itself, and the status HUD the XCUITests
// assert. The panel/HUD views live in OBCMock; this file only hosts them.
// B8's hidden second entry point (five taps on Settings ▸ App version) posts the
// same shake notification — see RootView's `devPanelOpener`.
// B11 adds the component-gallery sheet (`-OBCShowUIGallery`) for screenshot review.

extension Notification.Name {
    /// Posted by the `UIWindow` override below on a device shake.
    static let obcDeviceDidShake = Notification.Name("obcDeviceDidShake")
}

extension UIWindow {
    // UIKit delivers shakes to the first responder chain; the window is the last
    // stop, so overriding here catches them app-wide (sim: Device ▸ Shake, ⌃⌘Z).
    open override func motionEnded(_ motion: UIEvent.EventSubtype, with event: UIEvent?) {
        if motion == .motionShake {
            NotificationCenter.default.post(name: .obcDeviceDidShake, object: nil)
        }
        super.motionEnded(motion, with: event)
    }
}

/// Hosts the mock dev tooling around the real UI: status HUD at the bottom edge,
/// panel as a sheet on shake / at launch (`-OBCShowDevPanel`). A no-op when the
/// launch args forced the real `BLETransport` (no control to drive).
struct DevMockOverlay: ViewModifier {
    let control: MockControl?
    let showPanelAtLaunch: Bool
    let showGalleryAtLaunch: Bool
    let hideHUD: Bool
    @State private var panelShown = false
    @State private var galleryShown = false

    func body(content: Content) -> some View {
        mockTooling(around: content)
            // The B11 gallery presents even when the real transport is forced —
            // it needs no MockControl.
            .sheet(isPresented: $galleryShown) {
                OBCComponentGallery()
            }
            .onAppear {
                if showGalleryAtLaunch { galleryShown = true }
            }
    }

    @ViewBuilder
    private func mockTooling(around content: Content) -> some View {
        if let control {
            content
                .overlay(alignment: .bottomTrailing) {
                    if !hideHUD {
                        MockStatusHUD(control: control)
                            .padding(6)
                            .allowsHitTesting(false)
                    }
                }
                .sheet(isPresented: $panelShown) {
                    MockControlPanel(control: control)
                }
                .onReceive(NotificationCenter.default.publisher(for: .obcDeviceDidShake)) { _ in
                    panelShown = true
                }
                .onAppear {
                    if showPanelAtLaunch { panelShown = true }
                }
        } else {
            content
        }
    }
}

extension View {
    func devMockOverlay(
        control: MockControl?,
        showPanelAtLaunch: Bool,
        showGalleryAtLaunch: Bool = false,
        hideHUD: Bool = false
    ) -> some View {
        modifier(DevMockOverlay(
            control: control,
            showPanelAtLaunch: showPanelAtLaunch,
            showGalleryAtLaunch: showGalleryAtLaunch,
            hideHUD: hideHUD
        ))
    }
}
#endif
