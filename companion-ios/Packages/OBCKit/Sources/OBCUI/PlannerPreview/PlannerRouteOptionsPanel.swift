#if os(iOS)
import OBCDomain
import OBCPlanner
import SwiftUI

/// The Route options list, and the Use button of the previewed option under it.
struct PlannerRouteOptionsPanel: View {
    let options: PlannerRouteOptions
    let model: PlannerPreviewModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) { content }
                .padding(.horizontal, 16).padding(.bottom, 12)
        }.scrollBounceBehavior(.basedOnSize)
    }

    @ViewBuilder private var content: some View {
        if case .failed(let message) = options.status, options.isCurrent(model) {
            Text(message).font(.subheadline).foregroundStyle(OBCTheme.secondary)
            Button("Try again") { Task { await options.load(model) } }.frame(minHeight: 44)
        } else if options.status != .ready || !options.isCurrent(model) {
            ProgressView("Finding other routes…").font(.subheadline).frame(maxWidth: .infinity, minHeight: 44)
        } else if let yours = options.yours {
            OBCGroupedSection {
                ForEach(options.items) { item in
                    PlannerRouteOptionRow(item: item, yours: yours, selected: (options.previewed ?? yours.id) == item.id,
                                          label: options.accessibilityLabel(item), showsDivider: item.id != options.items.last?.id) {
                        options.select(item.id)
                    }
                }
            }
            if let preview = options.preview {
                Button("Use \(preview.name)") { options.use(model) }
                    .buttonStyle(.obcPrimary).accessibilityIdentifier("planner.options.use")
            }
            if options.items.count == 1 {
                Text("No other route is clearly better.").font(.subheadline).foregroundStyle(OBCTheme.secondary)
            }
        }
    }
}

/// One option: its letter, name and difference from your route, then its distance and climb. At accessibility text
/// sizes the figures go under the name.
private struct PlannerRouteOptionRow: View {
    let item: PlannerRouteOptions.Item
    let yours: PlannerRouteOptions.Item
    let selected: Bool
    let label: String
    let showsDivider: Bool
    let action: () -> Void
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        let path = item.option.path, stacked = typeSize.isAccessibilitySize
        let layout = stacked ? AnyLayout(VStackLayout(alignment: .leading, spacing: 6)) : AnyLayout(HStackLayout(spacing: 12))
        Button(action: action) {
            HStack(alignment: .top, spacing: 12) {
                PlannerRouteOptionLetter(letter: item.id, yours: item.id == yours.id)
                layout {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(item.name).font(.callout.weight(.semibold)).foregroundStyle(OBCTheme.ink)
                        Text(item.id == yours.id ? PlannerPreviewPreset(item.option.preference).title
                             : PlannerRouteOptions.difference(path, from: yours.option.path))
                            .font(.subheadline.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
                        if let unknown = PlannerRouteOptions.unknownSurface(item.option.unknownSurfaceMeters) {
                            Text(unknown).font(.caption.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    VStack(alignment: stacked ? .leading : .trailing, spacing: 3) {
                        Text(OBCFormat.distance(meters: path.distance)).font(.callout.monospacedDigit()).foregroundStyle(OBCTheme.ink)
                        Text.obcClimb(meters: path.ascent).font(.subheadline.monospacedDigit()).foregroundStyle(OBCTheme.secondary)
                    }.fixedSize()
                }
            }
            .padding(.vertical, 12).padding(.horizontal, 16).frame(minHeight: 52)
            .background(selected && item.id != yours.id ? OBCTheme.fill : .clear)
            .overlay(alignment: .bottom) {
                if showsDivider { OBCTheme.hairline.frame(height: 1).padding(.leading, 56) }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
        .accessibilityIdentifier("planner.options.\(item.id)")
    }
}

/// A in the route colour; each option in ink.
private struct PlannerRouteOptionLetter: View {
    let letter: String
    let yours: Bool
    var body: some View {
        Text(letter).font(.system(.footnote, weight: .bold)).dynamicTypeSize(...DynamicTypeSize.xLarge).foregroundStyle(OBCTheme.surface)
            .frame(minWidth: 26, minHeight: 26)
            .background(yours ? OBCTheme.route : OBCTheme.ink, in: Circle())
            .accessibilityHidden(true)
    }
}

/// What the planner map draws while an option is previewed.
extension PlannerRouteOptions {
    /// Your route as a thin dotted line under the option in ink; nil without a preview.
    var strokes: [MapStroke]? {
        guard let preview, let yours else { return nil }
        return [MapStroke(coordinates: yours.option.path.points.map(\.coordinate), color: OBCTheme.route, width: 3, cased: false,
                          dash: [0.1, 2]),
                MapStroke(coordinates: preview.option.path.points.map(\.coordinate), color: OBCTheme.ink, width: 4.5,
                          casingColor: OBCTheme.surface)]
    }

    var pins: [PlannerPreviewMapPin] {
        guard let places = letterPlaces, let preview, let yours else { return [] }
        return [PlannerPreviewMapPin(id: "option-\(yours.id)", title: "\(yours.id), \(yours.name)", coordinate: places.yours,
                                     kind: .option(yours.id, yours: true)),
                PlannerPreviewMapPin(id: "option-\(preview.id)", title: "\(preview.id), \(preview.name)", coordinate: places.preview,
                                     kind: .option(preview.id, yours: false))]
    }
}
#endif
