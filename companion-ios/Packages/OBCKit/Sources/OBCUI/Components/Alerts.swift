import SwiftUI

// System-styled wrappers: the action sheet, the text-field alert, and the
// documented system pairing sheet. These are native presentations on purpose —
// the app tint (forest) carries the brand; the pairing alert stays system blue.

public extension View {
    /// Action sheet — bottom-anchored destructive confirm (delete route,
    /// forget device): warning-red action + separated Cancel on a scrim.
    /// Every destructive path routes through this — no one-gesture destroy.
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

    /// Text-field alert — centered alert with an inline input, shared by route
    /// rename and device rename. The forest app tint colors the caret/buttons.
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

/// System pairing sheet — documentation wrapper, no custom UI.
///
/// Pairing an OBC runs through two native iOS prompts that cannot and should
/// not be themed:
/// - The Bluetooth permission prompt (first CoreBluetooth use). The intent
///   string lives in the app target's Info configuration
///   (`NSBluetoothAlwaysUsageDescription`).
/// - The pairing/bonding alert iOS raises when the device requests an
///   encrypted link. It renders in system blue — that is expected, not a
///   brand slip. Do not attempt a custom passkey UI.
///
/// What the app *does* own is everything around them: the empty/error states
/// for the failure paths, the spinner while scanning, and the copy that sets
/// the prompts up.
public enum OBCSystemPairing {
    /// The rule, kept referencable from code review: system prompts stay
    /// system-styled.
    public static let expectation =
        "Bluetooth permission + pairing alerts are native, system-blue prompts; the app never re-skins them."
}
