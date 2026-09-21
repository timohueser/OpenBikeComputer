import SwiftUI

// System-styled wrappers: the action sheet, the text-field alert, and the system
// pairing sheet. These are native presentations on purpose: the app tint carries the
// brand, and the pairing alert stays system blue.

public extension View {
    /// A bottom-anchored destructive confirm (delete route, forget device). Every
    /// destructive path routes through this; there is no one-gesture destroy.
    func obcDestructiveConfirm(
        _ title: String,
        isPresented: Binding<Bool>,
        message: String,
        actionTitle: String,
        onConfirm: @escaping () -> Void
    ) -> some View {
        confirmationDialog(title, isPresented: isPresented, titleVisibility: .visible) {
            Button(actionTitle, role: .destructive, action: onConfirm)
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(message)
        }
    }

    /// A centered alert with an inline input, shared by route rename and device
    /// rename.
    func obcRenameAlert(
        _ title: String,
        isPresented: Binding<Bool>,
        name: Binding<String>,
        placeholder: String = "Name",
        message: String? = nil,
        onSave: @escaping () -> Void
    ) -> some View {
        alert(title, isPresented: isPresented) {
            TextField(placeholder, text: name)
            Button("Cancel", role: .cancel) {}
            Button("Save", action: onSave)
        } message: {
            if let message { Text(message) }
        }
    }
}

/// A documentation wrapper, no custom UI. Pairing runs through two native iOS prompts
/// that must not be themed: the Bluetooth permission prompt, whose intent string lives
/// in the app target as `NSBluetoothAlwaysUsageDescription`, and the bonding alert iOS
/// raises when the device asks for an encrypted link. The bonding alert renders in
/// system blue; that is expected. Do not attempt a custom passkey UI.
public enum OBCSystemPairing {
    public static let expectation =
        "Bluetooth permission + pairing alerts are native, system-blue prompts; the app never re-skins them."
}
