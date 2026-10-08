import SwiftUI
import UIKit

/// The OBCDevice shell: one window over one host. No BLE, no OBCKit — the device's own firmware
/// runs behind `HostController`, and this file only says when it starts, pauses and stops.
@main
struct OBCDeviceApp: App {
    @State private var controller = HostController()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            DeviceView(controller: controller)
                .preferredColorScheme(.dark)
                .task { controller.start() }
                .onOpenURL { controller.receive($0) }
                // The host stays open across scene phases; only the display link stops.
                .onChange(of: scenePhase, initial: true) { _, phase in
                    controller.setActive(phase == .active)
                }
                // The one place the host is closed: a handle is closed exactly once, and iOS gives
                // no later moment than this.
                .onReceive(NotificationCenter.default.publisher(for: UIApplication.willTerminateNotification)) { _ in
                    controller.close()
                }
        }
    }
}
