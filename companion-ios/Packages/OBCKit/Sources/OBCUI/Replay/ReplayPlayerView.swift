import SwiftUI

public struct ReplayPlayerView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.accessibilityReduceMotion) private var reducedMotion
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @State private var model: ReplayPlayerModel
    @State private var rendererVisible = true
    #if canImport(UIKit)
    @State private var originalIdleTimer: Bool?
    #endif

    public init(content: ReplayContent) { _model = State(initialValue: ReplayPlayerModel(content: content)) }

    private var usesSideDock: Bool {
        verticalSizeClass == .compact && dynamicTypeSize.isAccessibilitySize
    }

    private var playerLayout: AnyLayout {
        usesSideDock ? AnyLayout(HStackLayout(spacing: 0)) : AnyLayout(VStackLayout(spacing: 0))
    }

    public var body: some View {
        GeometryReader { geometry in
            VStack(spacing: 0) {
                header
                // Keep the renderer alive when the control dock changes axes.
                playerLayout {
                    terrain.frame(maxWidth: .infinity, maxHeight: .infinity)
                    if usesSideDock {
                        ScrollView { controls }
                            .accessibilityIdentifier("replay.controls")
                            .frame(width: min(360, geometry.size.width * 0.42))
                            .background(OBCTheme.surface)
                    } else {
                        controls.fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
        }
        .foregroundStyle(OBCTheme.ink)
        .background(OBCTheme.page)
        .tint(OBCTheme.tint)
        .onAppear {
            model.reducedMotion = reducedMotion
            #if canImport(UIKit)
            originalIdleTimer = UIApplication.shared.isIdleTimerDisabled
            #endif
        }
        .onChange(of: model.playing) { _, playing in updateIdleTimer(playing: playing) }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active {
                if !rendererVisible { model.retry(); rendererVisible = true }
            } else {
                model.suspend()
                rendererVisible = false
                updateIdleTimer(playing: false)
            }
        }
        .onChange(of: reducedMotion) { _, value in
            model.reducedMotion = value
            model.retry()
        }
        .onChange(of: model.day) { _, day in
            if let day { announce(day) }
        }
        .onChange(of: model.photo?.id) { _, id in
            if id != nil { announce("Ride photo. Continue resumes playback.") }
        }
        .onDisappear { model.suspend(); updateIdleTimer(playing: false) }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("replay.player")
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 12) {
                closeButton
                if dynamicTypeSize.isAccessibilitySize && verticalSizeClass != .compact {
                    if let day = model.day { dayLabel(day) }
                } else {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(model.content.title).font(.headline).lineLimit(1)
                        if let day = model.day { dayLabel(day) }
                    }
                }
                Spacer(minLength: 0)
                overviewButton
            }
            if dynamicTypeSize.isAccessibilitySize && verticalSizeClass != .compact {
                Text(model.content.title)
                    .font(.headline)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 4)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .fixedSize(horizontal: false, vertical: true)
        .background(OBCTheme.surface)
    }

    private var closeButton: some View {
        Button { dismiss() } label: {
            Image(systemName: "xmark")
                .font(.title3)
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(Rectangle())
        }
        .accessibilityLabel("Close")
        .accessibilityIdentifier("replay.close")
    }

    private func dayLabel(_ day: String) -> some View {
        Text(day).font(.caption).lineLimit(1).foregroundStyle(OBCTheme.secondary)
    }

    private var cameraDescription: String {
        model.camera == .overview ? "Overview" : model.camera == .adjusted ? "Following · Your angle" : "Following"
    }

    private var overviewButton: some View {
        Button {
            model.setCamera(model.camera == .overview ? "follow" : "overview")
        } label: {
            adaptiveLabel(model.camera == .overview ? "Follow rider" : "Overview",
                          symbol: model.camera == .overview ? "location" : "map")
                .font(.subheadline)
                .fixedSize(horizontal: true, vertical: true)
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(Rectangle())
        }
        .disabled(model.phase != .ready)
        .accessibilityValue(cameraDescription)
        .accessibilityIdentifier("replay.overview")
    }

    @ViewBuilder private func adaptiveLabel(_ title: String, symbol: String) -> some View {
        if dynamicTypeSize.isAccessibilitySize {
            Label(title, systemImage: symbol).labelStyle(.iconOnly)
        } else {
            Label(title, systemImage: symbol)
        }
    }

    private var terrain: some View {
        ZStack {
            OBCTheme.sketchGround
            #if canImport(UIKit)
            if rendererVisible { ReplayWebView(model: model).id(model.generation) }
            #endif
            if model.phase == .ready {
                VStack {
                    HStack {
                        if !dynamicTypeSize.isAccessibilitySize {
                            Text(cameraDescription)
                                .font(.caption)
                                .padding(8)
                                .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
                        }
                        Spacer()
                        if model.camera == .adjusted {
                            Button { model.setCamera("auto") } label: {
                                adaptiveLabel("Reset camera", symbol: "arrow.counterclockwise")
                                    .font(.subheadline)
                                    .padding(.horizontal, 12)
                                    .frame(minWidth: 44, minHeight: 44)
                                    .contentShape(Rectangle())
                            }
                            .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusSmall))
                        }
                    }
                    Spacer(minLength: 0)
                }
                .padding(12)
            }
            if let photo = model.photo, let data = photo.thumbnailData, let image = Image(photoData: data) {
                VStack(spacing: 8) {
                    image.resizable().scaledToFit()
                        .frame(maxHeight: verticalSizeClass == .compact ? 60 : 180)
                        .accessibilityLabel("Photo from this ride")
                    Button { model.continuePhoto() } label: {
                        Label("Continue", systemImage: "play.fill")
                            .frame(maxWidth: .infinity, minHeight: 44)
                            .contentShape(Rectangle())
                    }
                }
                .padding(12)
                .frame(maxWidth: 320)
                .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
                .padding(.horizontal, 24)
                .padding(.vertical, verticalSizeClass == .compact ? 8 : 56)
            }
            status
        }
        .clipped()
    }

    @ViewBuilder private var status: some View {
        switch model.phase {
        case .loading:
            VStack(spacing: 12) {
                ProgressView()
                Text("Loading terrain").font(.headline).multilineTextAlignment(.center)
            }
            .padding(16)
            .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .accessibilityElement(children: .combine)
        case .failed(let message):
            ScrollView {
                VStack(spacing: 12) {
                    Text("Map unavailable").font(.headline)
                    Text(message).font(.subheadline).multilineTextAlignment(.center)
                    Button { model.retry() } label: {
                        Label("Try again", systemImage: "arrow.clockwise")
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                    }
                }
                .padding(24)
            }
            .frame(maxWidth: 320)
            .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .padding(20)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("replay.error")
        case .ready: EmptyView()
        }
    }

    private var controls: some View {
        VStack(spacing: 8) {
            ViewThatFits(in: .horizontal) {
                HStack { distanceLabel; Spacer(); elevationLabel }
                VStack(alignment: .leading) { distanceLabel; elevationLabel }
            }
            .font(.subheadline.monospacedDigit())
            .fixedSize(horizontal: false, vertical: true)
            ReplayProfileView(model: model, height: verticalSizeClass == .compact ? 48 : 86)
                .disabled(model.phase != .ready)
            HStack(spacing: 16) {
                if model.content.photos.contains(where: { $0.thumbnailData != nil }) {
                    Menu {
                        ForEach(model.content.photos.filter { $0.thumbnailData != nil }, id: \.id) { photo in
                            Button("Photo at \(photo.distance / 1_000, specifier: "%.1f") km") {
                                model.seek(photo.distance)
                                model.showPhoto(photo)
                            }
                        }
                    } label: {
                        Image(systemName: "photo.on.rectangle").frame(minWidth: 44, minHeight: 44)
                    }
                    .accessibilityLabel("Ride photos")
                }
                Button {
                    model.togglePlayback()
                } label: {
                    adaptiveLabel(model.playing ? "Pause" : model.distance >= model.content.totalDistance ? "Replay" : "Play",
                                  symbol: model.playing ? "pause.fill" : "play.fill")
                        .font(.headline)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.vertical, 10)
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .foregroundStyle(OBCTheme.onAmber)
                .background(OBCTheme.amber, in: RoundedRectangle(cornerRadius: OBCTheme.controlRadius))
                .accessibilityIdentifier("replay.play")
                Menu {
                    ForEach([0.5, 1.0, 2.0], id: \.self) { speed in
                        Button("\(speed.formatted())×") { model.setSpeed(speed) }
                    }
                } label: {
                    Text("\(model.speed.formatted())×").monospacedDigit().frame(minWidth: 44, minHeight: 44)
                }
                .accessibilityLabel("Playback speed")
                .accessibilityValue("\(model.speed.formatted()) times")
            }
            .disabled(model.phase != .ready)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, verticalSizeClass == .compact ? 8 : 16)
        .background(OBCTheme.surface)
    }

    private var distanceLabel: some View {
        Text("\(model.distance / 1_000, specifier: "%.1f") / \(model.content.totalDistance / 1_000, specifier: "%.1f") km")
    }

    private var elevationLabel: some View {
        Text(model.elevation.map { "\(Int($0.rounded())) m" } ?? "Elevation unavailable")
            .foregroundStyle(OBCTheme.secondary)
    }

    private func updateIdleTimer(playing: Bool) {
        #if canImport(UIKit)
        UIApplication.shared.isIdleTimerDisabled = playing ? true : (originalIdleTimer ?? false)
        #endif
    }

    private func announce(_ text: String) {
        #if canImport(UIKit)
        UIAccessibility.post(notification: .announcement, argument: text)
        #endif
    }
}
