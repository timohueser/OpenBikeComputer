import SwiftUI

// System-styled wrappers: the action sheet, the rename sheet, and the system
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

    /// The name sheet, shared by every rename and name prompt: a field that starts at `name`,
    /// under Cancel and Save. The draft lives in the sheet, so a refresh of the screen below cannot
    /// reset or close it. Save, enabled while `canSave` accepts the draft, hands it to `onSave`.
    func obcRenameSheet(
        _ title: String,
        isPresented: Binding<Bool>,
        name: String,
        placeholder: String = "Name",
        message: String? = nil,
        saveTitle: String = "Save",
        canSave: @escaping (String) -> Bool = { _ in true },
        onSave: @escaping (String) -> Void
    ) -> some View {
        sheet(isPresented: isPresented) {
            OBCRenameSheet(
                title: title, name: name, placeholder: placeholder, message: message, saveTitle: saveTitle,
                canSave: canSave, onSave: onSave)
        }
    }
}

private struct OBCRenameSheet: View {
    let title: String
    let placeholder: String
    let message: String?
    let saveTitle: String
    let canSave: (String) -> Bool
    let onSave: (String) -> Void

    @State private var draft: String
    @FocusState private var focused: Bool
    @Environment(\.dismiss) private var dismiss

    init(
        title: String, name: String, placeholder: String, message: String?, saveTitle: String,
        canSave: @escaping (String) -> Bool, onSave: @escaping (String) -> Void
    ) {
        self.title = title
        self.placeholder = placeholder
        self.message = message
        self.saveTitle = saveTitle
        self.canSave = canSave
        self.onSave = onSave
        _draft = State(initialValue: name)
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 10) {
                OBCGroupedSection {
                    TextField(placeholder, text: $draft)
                        .font(.system(.callout))
                        .focused($focused)
                        .submitLabel(.done)
                        .onSubmit(save)
                        .padding(16)
                        .accessibilityIdentifier("rename.field")
                }
                if let message {
                    Text(message)
                        .font(.system(.footnote))
                        .foregroundStyle(OBCTheme.secondary)
                        .padding(.horizontal, 4)
                }
            }
            .padding(20)
            .frame(maxHeight: .infinity, alignment: .top)
            .background(OBCTheme.page.ignoresSafeArea())
            .navigationTitle(title)
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .accessibilityIdentifier("rename.cancel")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saveTitle, action: save)
                        .fontWeight(.semibold)
                        .disabled(!canSave(draft))
                        .accessibilityIdentifier("rename.save")
                }
            }
            .onAppear { focused = true }
        }
        .tint(OBCTheme.tint)
        .presentationDetents([.height(message == nil ? 190 : 220)])
    }

    private func save() {
        guard canSave(draft) else { return }
        onSave(draft)
        dismiss()
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
