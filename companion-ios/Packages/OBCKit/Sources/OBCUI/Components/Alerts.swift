import SwiftUI

// The choice sheet, the destructive confirm, the rename sheet, and the system pairing sheet. The
// first three are the app's own bottom sheets; the pairing alert stays system blue on purpose.

/// One choice on an `obcChoiceSheet`.
public struct OBCSheetAction {
    /// A `primary` choice is the amber action beside the other choices.
    public enum Role { case normal, primary, destructive }
    let title: String
    let role: Role
    let action: () -> Void

    public init(_ title: String, role: Role = .normal, action: @escaping () -> Void) {
        self.title = title
        self.role = role
        self.action = action
    }
}

public extension View {
    /// The app's bottom sheet for a question with a few answers: a title, one line, the choices,
    /// and Cancel. A `primary` choice is the amber action, and the plain choices beside it are
    /// grouped rows; one plain choice alone is the amber action; a destructive choice is red text.
    /// A sheet, not a confirmation dialog: the dialog pops up as a bubble beside its control.
    /// `onDismiss` runs once the sheet has gone, so a choice can open the next sheet there.
    func obcChoiceSheet(
        _ title: String,
        isPresented: Binding<Bool>,
        message: String? = nil,
        actions: [OBCSheetAction],
        onDismiss: (() -> Void)? = nil
    ) -> some View {
        sheet(isPresented: isPresented, onDismiss: onDismiss) {
            OBCChoiceSheet(title: title, message: message, actions: actions)
        }
    }

    /// A choice sheet with one destructive answer (delete route, forget device). Every destructive
    /// path routes through this; there is no one-gesture destroy.
    func obcDestructiveConfirm(
        _ title: String,
        isPresented: Binding<Bool>,
        message: String?,
        actionTitle: String,
        onConfirm: @escaping () -> Void
    ) -> some View {
        obcChoiceSheet(title, isPresented: isPresented, message: message,
                       actions: [OBCSheetAction(actionTitle, role: .destructive, action: onConfirm)])
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

private struct OBCChoiceSheet: View {
    // Held from presentation: the caller often clears the state these came from on dismissal,
    // and the sheet must not change while it animates out.
    @State private var title: String
    @State private var message: String?
    @State private var actions: [OBCSheetAction]

    @Environment(\.dismiss) private var dismiss

    init(title: String, message: String?, actions: [OBCSheetAction]) {
        _title = State(initialValue: title)
        _message = State(initialValue: message)
        _actions = State(initialValue: actions)
    }

    private var plain: [(offset: Int, element: OBCSheetAction)] {
        Array(actions.enumerated()).filter { $0.element.role == .normal }
    }
    /// Plain choices are rows unless one alone is the amber action.
    private var rows: Bool { plain.count > 1 || actions.contains { $0.role == .primary } }

    var body: some View {
        OBCSheetContainer {
            VStack(alignment: .leading, spacing: 14) {
                Text(title)
                    .font(.system(.title2, weight: .bold))
                    .foregroundStyle(OBCTheme.ink)
                if let message {
                    Text(message)
                        .font(.system(.callout))
                        .foregroundStyle(OBCTheme.secondary)
                        .padding(.bottom, 6)
                }
                ForEach(Array(actions.enumerated()), id: \.offset) { index, action in
                    if action.role == .primary {
                        Button(action.title) { choose(action) }
                            .buttonStyle(.obcPrimary)
                            .accessibilityIdentifier("confirm.action.\(index)")
                    }
                }
                if rows {
                    OBCGroupedSection {
                        ForEach(plain, id: \.offset) { index, action in
                            OBCListRow(label: action.title, showsChevron: true,
                                       showsDivider: index != plain.last?.offset) { choose(action) }
                                .accessibilityIdentifier("confirm.action.\(index)")
                        }
                    }
                }
                ForEach(Array(actions.enumerated()), id: \.offset) { index, action in
                    if action.role == .destructive {
                        Button(action.title) { choose(action) }
                            .buttonStyle(.obcDestructive)
                            .accessibilityIdentifier("confirm.action.\(index)")
                    } else if action.role == .normal && !rows {
                        Button(action.title) { choose(action) }
                            .buttonStyle(.obcPrimary)
                            .accessibilityIdentifier("confirm.action.\(index)")
                    }
                }
                Button("Cancel") { dismiss() }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("confirm.cancel")
            }
        }
    }

    // The choice runs before the sheet closes, as a dialog button does, so a model that clears
    // its question on dismissal still sees the answer.
    private func choose(_ action: OBCSheetAction) {
        action.action()
        dismiss()
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
