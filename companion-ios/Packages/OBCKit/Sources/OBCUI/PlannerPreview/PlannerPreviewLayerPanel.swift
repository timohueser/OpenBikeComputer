#if os(iOS)
import SwiftUI

enum PlannerPreviewNetwork: String, CaseIterable {
    case none, cycling, hiking

    var title: String {
        switch self { case .none: "Off"; case .cycling: "Cycling"; case .hiking: "Hiking" }
    }
}

// The groups and names match builder/app/src/lib/planner/poi-kinds.ts.
enum PlannerPreviewPlaceCategory: String, CaseIterable, Identifiable {
    case hotel, camp, shelter, shop, food, water, toilets, bike, pharmacy, station, viewpoint, peak

    var id: String { rawValue }
    var title: String {
        switch self {
        case .hotel: "Lodging"
        case .camp: "Campsites"
        case .shelter: "Shelters"
        case .shop: "Food shops"
        case .food: "Eating"
        case .water: "Water"
        case .toilets: "Toilets"
        case .bike: "Bike"
        case .pharmacy: "Pharmacy"
        case .station: "Stations"
        case .viewpoint: "Viewpoints"
        case .peak: "Peaks and passes"
        }
    }
    var symbol: String {
        switch self {
        case .hotel: "bed.double"
        case .camp: "tent"
        case .shelter: "house"
        case .shop: "cart"
        case .food: "fork.knife"
        case .water: "drop"
        case .toilets: "toilet"
        case .bike: "bicycle"
        case .pharmacy: "cross.case"
        case .station: "tram"
        case .viewpoint: "eye"
        case .peak: "mountain.2"
        }
    }

    static let groups: [(title: String, categories: [Self])] = [
        ("Sleep", [.hotel, .camp, .shelter]),
        ("Eat & drink", [.shop, .food]),
        ("Water", [.water, .toilets]),
        ("Fix & go", [.bike, .pharmacy, .station]),
        ("See", [.viewpoint, .peak]),
    ]

    static func category(for place: PlannerPreviewPlace) -> Self? {
        switch place.kind.rawValue {
        case "cafe": .food
        case "camping": .camp
        case "town": nil
        default: Self(rawValue: place.kind.rawValue)
        }
    }
}

struct PlannerPreviewLayerPanel: View {
    @Binding var network: PlannerPreviewNetwork
    @Binding var hidden: Set<PlannerPreviewPlaceCategory>
    @Binding var highlighted: Set<PlannerPreviewPlaceCategory>
    let onDone: () -> Void
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Label("Map layers", systemImage: "square.3.layers.3d")
                    .font(.headline)
                Spacer(minLength: 12)
                Button("Done", action: onDone)
                    .font(.body.weight(.semibold)).foregroundStyle(OBCTheme.tint).frame(minHeight: 44)
            }
            .padding(.horizontal, 20).padding(.bottom, 12)

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    networkSection
                    Divider().overlay(OBCTheme.hairline)
                    placesSection
                }
                .padding(.horizontal, 20).padding(.bottom, 24)
            }
            .scrollBounceBehavior(.basedOnSize)
        }
        .foregroundStyle(OBCTheme.ink)
        .tint(OBCTheme.tint)
    }

    private var networkSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Route networks").font(.subheadline.weight(.semibold))
            Picker("Route network", selection: $network) {
                ForEach(PlannerPreviewNetwork.allCases, id: \.self) { option in
                    Text(option.title).tag(option)
                }
            }
            .pickerStyle(.segmented)
            if network != .none {
                LazyVGrid(columns: [GridItem(.flexible(), alignment: .leading), GridItem(.flexible(), alignment: .leading)], alignment: .leading, spacing: 8) {
                    networkKey("National / intl.", rank: 3)
                    networkKey("Regional", rank: 2)
                    networkKey("Local", rank: 1)
                    networkKey("Unspecified", rank: 0)
                }
            }
        }
    }

    private var placesSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Places").font(.subheadline.weight(.semibold))
            ForEach(PlannerPreviewPlaceCategory.groups, id: \.title) { group in
                Text(group.title)
                    .font(.caption.weight(.semibold)).foregroundStyle(OBCTheme.secondary)
                    .padding(.top, 12).padding(.bottom, 2)
                VStack(spacing: 0) {
                    ForEach(group.categories) { category in placeRow(category) }
                }
            }
        }
    }

    /// A toggle shows or hides the type. The eye spotlights it: those places stay on the map at
    /// every zoom, ringed in amber.
    private func placeRow(_ category: PlannerPreviewPlaceCategory) -> some View {
        let shown = !hidden.contains(category)
        let marked = highlighted.contains(category)
        return HStack(spacing: 8) {
            Image(systemName: category.symbol)
                .font(.subheadline).foregroundStyle(OBCTheme.secondary)
                .frame(width: 24).accessibilityHidden(true)
            Text(category.title).font(.subheadline)
            Spacer(minLength: 4)
            Button {
                if marked { highlighted.remove(category) } else { highlighted.insert(category) }
            } label: {
                Image(systemName: marked ? "eye.fill" : "eye")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(marked ? OBCTheme.surface : OBCTheme.secondary)
                    .frame(width: 30, height: 30)
                    .background(marked ? OBCTheme.ink : OBCTheme.fill, in: Circle())
                    .frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!shown)
            .opacity(shown ? 1 : 0.35)
            .accessibilityLabel("Show \(category.title.lowercased()) at every zoom")
            .accessibilityValue(marked ? "On" : "Off")
            Toggle(category.title, isOn: Binding(get: { shown }, set: { on in
                if on { hidden.remove(category) } else { hidden.insert(category); highlighted.remove(category) }
            }))
            .labelsHidden().tint(OBCTheme.tint)
        }
        .frame(minHeight: 44)
    }

    private func networkKey(_ title: String, rank: Int) -> some View {
        HStack(spacing: 7) {
            Capsule().fill(Color(uiColor: PlannerPreviewNetworkStyle.color(rank: rank, traits: UITraitCollection(userInterfaceStyle: colorScheme == .dark ? .dark : .light))))
                .frame(width: 18, height: 3)
            Text(title).font(.caption).foregroundStyle(OBCTheme.secondary)
        }
    }
}
#endif
