import SwiftUI
import OBCDomain

/// Picks the date of Day 1, or removes it. Each day's date follows from it.
public struct TripStartDateSheet: View {
    @State private var date: Date
    private let hasDate: Bool
    private let onSet: (CivilDay?) -> Void
    @Environment(\.dismiss) private var dismiss

    public init(startDay: CivilDay?, onSet: @escaping (CivilDay?) -> Void) {
        _date = State(initialValue: startDay?.date() ?? Date())
        hasDate = startDay != nil
        self.onSet = onSet
    }

    public var body: some View {
        NavigationStack {
            VStack(spacing: 16) {
                DatePicker("Start date", selection: $date, displayedComponents: .date)
                    .datePickerStyle(.graphical)
                    .tint(OBCTheme.tint)
                    .padding(.horizontal, 12)
                    .background(OBCTheme.surface)
                    .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
                Text("Each day gets its date from Day 1.")
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 4)
                if hasDate {
                    Button("Remove the start date") {
                        onSet(nil)
                        dismiss()
                    }
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("trip.startDate.clear")
                }
                Spacer()
            }
            .padding(20)
            .background(OBCTheme.page.ignoresSafeArea())
            .navigationTitle("Start date")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") {
                        onSet(CivilDay(date))
                        dismiss()
                    }
                    .fontWeight(.semibold)
                    .accessibilityIdentifier("trip.startDate.save")
                }
            }
        }
        .tint(OBCTheme.tint)
    }
}
