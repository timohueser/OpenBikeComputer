import SwiftUI

struct ReplayProfileView: View {
    let model: ReplayPlayerModel
    let height: CGFloat

    var body: some View {
        GeometryReader { geometry in
            Canvas { context, size in draw(in: &context, size: size) }
                .contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                    model.seek(Double(value.location.x / max(geometry.size.width, 1)) * model.content.totalDistance)
                })
                .overlay(alignment: .bottomLeading) { photoMarkers(width: geometry.size.width) }
        }
        .frame(height: height)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Elevation profile, distance along ride")
        .accessibilityValue("\(model.distance / 1_000, specifier: "%.1f") of \(model.content.totalDistance / 1_000, specifier: "%.1f") kilometers")
        .accessibilityHint("Swipe up or down to change distance. Playback pauses.")
        .accessibilityAdjustableAction { direction in
            let step = max(1, model.content.totalDistance / 100)
            switch direction {
            case .increment: model.seek(model.distance + step)
            case .decrement: model.seek(model.distance - step)
            @unknown default: break
            }
        }
        .accessibilityIdentifier("replay.profile")
    }

    private func photoMarkers(width: CGFloat) -> some View {
        ZStack(alignment: .bottomLeading) {
            ForEach(model.content.photos.filter { $0.thumbnailData != nil }, id: \.id) { photo in
                photoButton(photo)
                    .offset(x: markerPosition(photo, width: width))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func photoButton(_ photo: ReplayPhoto) -> some View {
        Button {
            model.seek(photo.distance)
            model.showPhoto(photo)
        } label: {
            Image(systemName: "photo.fill")
                .font(.caption)
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(OBCTheme.ink)
        .accessibilityLabel("Ride photo at \(photo.distance / 1_000, specifier: "%.1f") kilometers")
    }

    private func markerPosition(_ photo: ReplayPhoto, width: CGFloat) -> CGFloat {
        let position = width * CGFloat(photo.distance / max(1, model.content.totalDistance)) - 22
        return min(max(0, position), max(0, width - 44))
    }

    private func draw(in context: inout GraphicsContext, size: CGSize) {
        let total = max(1, model.content.totalDistance)
        let elevations = model.content.points.compactMap(\.elevation)
        let low = elevations.min() ?? 0
        let span = max(1, (elevations.max() ?? low) - low)
        let floor = size.height - 22
        var line = Path()
        var area = Path()
        var previousX: CGFloat?
        func finish() {
            if let previousX {
                area.addLine(to: CGPoint(x: previousX, y: floor))
                area.closeSubpath()
            }
        }
        for point in model.content.points {
            guard let elevation = point.elevation else { finish(); previousX = nil; continue }
            let x = size.width * point.distance / total
            let y = floor - (floor - 8) * (elevation - low) / span
            if point.segmentStart || previousX == nil {
                finish()
                line.move(to: CGPoint(x: x, y: y))
                area.move(to: CGPoint(x: x, y: floor))
            } else { line.addLine(to: CGPoint(x: x, y: y)) }
            area.addLine(to: CGPoint(x: x, y: y))
            previousX = x
        }
        finish()
        context.fill(area, with: .color(OBCTheme.profileFill))
        context.stroke(line, with: .color(OBCTheme.ride), lineWidth: 2)
        for day in model.content.days.dropFirst() {
            let x = size.width * day.distance / total
            var boundary = Path()
            boundary.move(to: CGPoint(x: x, y: 0))
            boundary.addLine(to: CGPoint(x: x, y: floor))
            context.stroke(boundary, with: .color(OBCTheme.secondary), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
        }
        let x = size.width * model.distance / total
        var cursor = Path()
        cursor.move(to: CGPoint(x: x, y: 0))
        cursor.addLine(to: CGPoint(x: x, y: size.height))
        context.stroke(cursor, with: .color(OBCTheme.ink), lineWidth: 2)
        if elevations.isEmpty {
            context.draw(Text("Elevation unavailable").font(.caption).foregroundStyle(OBCTheme.secondary),
                         at: CGPoint(x: size.width / 2, y: floor / 2))
        }
    }
}
