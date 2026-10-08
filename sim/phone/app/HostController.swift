import CoreGraphics
import Foundation
import OBCHost
import QuartzCore
import UIKit

/// The host's life on the phone: the card under Application Support, the display link that
/// advances it, and the file actions that feed it.
///
/// Every call on the C surface is main thread only, which this actor is. The one exception is the
/// map import: it copies a whole map, so it runs detached with the host closed and reads its own
/// thread's error slot.
@MainActor
@Observable
final class HostController {
    enum State {
        /// No card, or a card with no map: the import screen.
        case needsMap
        case running
        case failed(String)
    }

    private(set) var state: State = .needsMap
    /// The host's current screen, refreshed when it changes — the dev sheet's live readout.
    private(set) var screen = ""
    /// A finished action's message, shown once and dismissed.
    var notice: String?
    /// A dropped map waiting for the rider to confirm it replaces the card's map.
    var offeredMap: URL?
    /// A map copy is in flight and the host is closed until it lands.
    private(set) var isImporting = false
    /// Physical pixels per panel pixel. One panel pixel is one point by default.
    var pixelScale = Int(UIScreen.main.scale)
    #if DEBUG
        /// Where the developer sheet says the phone is. Nothing writes it to disk: a launch is
        /// always on the real GPS, so a forgotten override can never look like broken hardware.
        private(set) var pretendPlace: PretendPlace?
    #endif

    let frameWidth = Int(obc_ios_frame_width())
    let frameHeight = Int(obc_ios_frame_height())
    /// The view the display link blits into, owned here so no frame travels through SwiftUI.
    let canvas = UIImageView()

    private var host: OpaquePointer?
    /// The interned name behind `screen`, as the host handed it over.
    private var screenName: UnsafePointer<CChar>?
    private var link: CADisplayLink?
    private var isActive = true
    private var epoch = CACurrentMediaTime()
    private let sensors = PhoneSensors()
    private let sound = SoundPlayer()
    private let support: URL
    private let documents: URL
    private static let deviceRGB = CGColorSpaceCreateDeviceRGB()

    /// The 32 GiB sparse card. Application Support, not Documents: it is not a rider's file.
    var card: URL { support.appending(path: "card.obc") }
    /// Where a committed ride's GPX lands, and what the Files app shows.
    var rides: URL { documents.appending(path: "rides", directoryHint: .isDirectory) }
    private var settings: URL { support.appending(path: "settings.bin") }

    init() {
        let files = FileManager.default
        support = files.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        documents = files.urls(for: .documentDirectory, in: .userDomainMask)[0]
        canvas.layer.magnificationFilter = .nearest
        canvas.contentMode = .scaleToFill
        canvas.isOpaque = true
        canvas.backgroundColor = .black
        // The layout decides the panel's size. Left alone, a `UIImageView` refuses to be drawn
        // smaller than its image, and every scale below the physical one is exactly that.
        canvas.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        canvas.setContentCompressionResistancePriority(.defaultLow, for: .vertical)
    }

    /// The panel at the chosen scale, in points.
    var panelSize: CGSize {
        let scale = UIScreen.main.scale
        return CGSize(
            width: CGFloat(frameWidth * pixelScale) / scale,
            height: CGFloat(frameHeight * pixelScale) / scale)
    }

    /// Why the last call on this thread failed; empty when none has.
    var hostError: String { String(cString: obc_ios_last_error()) }

    // MARK: Lifecycle

    /// Mount whatever the phone already has. A ready card opens. A phone with no map takes the one
    /// map in Documents when there is exactly one — a first launch after an AirDrop is then a
    /// launch, not a launch and a tap. Anything else asks for a map.
    func start() {
        guard host == nil, !isImporting else { return }
        let files = FileManager.default
        try? files.createDirectory(at: support, withIntermediateDirectories: true)
        try? files.createDirectory(at: rides, withIntermediateDirectories: true)
        switch obc_ios_card_state(card.path) {
        case 2: open()
        case 0, 1: if let only = loneMap() { useAsMap(only) } else { state = .needsMap }
        default: state = .failed(hostError)
        }
    }

    #if DEBUG
        /// Pretend to be somewhere else, or `nil` to go back to the phone's own GPS.
        func pretend(_ place: PretendPlace?) {
            pretendPlace = place
            sensors.pretend = place?.coordinate
        }
    #endif

    /// Close the host and give the phone its screen timeout back. Safe to call twice.
    func close() {
        link?.invalidate()
        link = nil
        sound.pause()
        sensors.detach()
        if let host { obc_ios_close(host) }
        host = nil
        canvas.image = nil
        screen = ""
        screenName = nil
        UIApplication.shared.isIdleTimerDisabled = false
    }

    /// Pause the loop while the scene is not active. The host itself stays open.
    func setActive(_ active: Bool) {
        isActive = active
        link?.isPaused = !active
        guard link != nil else { return }
        if active {
            sound.start()
        } else {
            sound.pause()
        }
    }

    /// Delete the card. The next import creates a new one; nothing is imported automatically here,
    /// or a reset would silently undo itself.
    func resetCard() {
        // A map import is writing this card from another thread.
        guard !isImporting else { return }
        close()
        let card = card
        guard FileManager.default.fileExists(atPath: card.path) else {
            state = .needsMap
            return
        }
        do {
            try FileManager.default.removeItem(at: card)
        } catch {
            // The card is still there and the host is now closed. Say so, rather than offer an
            // import screen over a card that was never reset.
            state = .failed("Could not reset the card: \(error.localizedDescription)")
            return
        }
        state = .needsMap
    }

    private func open() {
        guard let handle = obc_ios_open(card.path, settings.path, rides.path) else {
            state = .failed(hostError)
            return
        }
        excludeCardFromBackup()
        host = handle
        state = .running
        sensors.attach(to: handle)
        epoch = CACurrentMediaTime()
        let link = CADisplayLink(target: self, selector: #selector(step))
        link.isPaused = !isActive
        link.add(to: .main, forMode: .common)
        self.link = link
        if isActive { sound.start() }
        UIApplication.shared.isIdleTimerDisabled = true
    }

    /// The card is a 32 GiB sparse file: iCloud must never try to copy it.
    private func excludeCardFromBackup() {
        var card = card
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? card.setResourceValues(values)
    }

    // MARK: The loop

    @objc private func step(_ link: CADisplayLink) {
        guard let host else { return }
        if obc_ios_tick(host, (CACurrentMediaTime() - epoch) * 1000) {
            canvas.image = frameImage(host)
        }
        var length: UInt32 = 0
        if let samples = obc_ios_take_sound(host, sound.sampleRate, &length) {
            sound.play(samples, count: Int(length))
        }
        // The host interns the screen names, so the pointer alone says whether it changed. Building
        // a String every frame would allocate 60 times a second to answer "still the Map".
        let raw = obc_ios_screen(host)
        if raw != screenName {
            screenName = raw
            screen = raw.map { String(cString: $0) } ?? ""
        }
    }

    /// One frame as an image over a copy: the host's buffer is valid only until its next call, and
    /// nothing on the way to the screen resamples it.
    private func frameImage(_ host: OpaquePointer) -> UIImage? {
        guard let bytes = obc_ios_frame(host) else { return nil }
        let pixels = Data(bytes: bytes, count: frameWidth * frameHeight * 4)
        guard let provider = CGDataProvider(data: pixels as CFData) else { return nil }
        let layout = CGBitmapInfo(
            rawValue: CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.noneSkipLast.rawValue)
        guard let image = CGImage(
            width: frameWidth, height: frameHeight, bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: frameWidth * 4, space: Self.deviceRGB, bitmapInfo: layout,
            provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
        else { return nil }
        return UIImage(cgImage: image)
    }

    /// One button edge from a touch pad.
    func button(_ button: ObcButton, down: Bool) {
        guard let host else { return }
        obc_ios_button(host, button, down)
    }

    // MARK: Files

    /// What is in Documents for the host to take, by name. The exports directory is not one.
    func inbox() -> [URL] {
        // Hidden files are skipped so a half-copied file staged by `copy` never looks importable.
        let files = (try? FileManager.default.contentsOfDirectory(
            at: documents, includingPropertiesForKeys: [.isDirectoryKey], options: [.skipsHiddenFiles])) ?? []
        return files
            .filter { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == false }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    /// The rides the host has exported.
    func exportedRides() -> [URL] {
        let files = (try? FileManager.default.contentsOfDirectory(at: rides, includingPropertiesForKeys: nil)) ?? []
        return files.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    /// A file from AirDrop, Files or the share sheet: copy it into Documents, then offer a map and
    /// import a route.
    func receive(_ url: URL) {
        let documents = documents
        Task {
            guard let file = await Task.detached(priority: .userInitiated, operation: {
                Self.copy(url, into: documents)
            }).value else {
                notice = "Could not read \(url.lastPathComponent)"
                return
            }
            if file.pathExtension.caseInsensitiveCompare("obcm") == .orderedSame {
                offeredMap = file
            } else {
                importRoute(file)
            }
        }
    }

    /// Put `url` in Documents under security-scoped access. A file that is already there — Files
    /// opens ours in place — is taken as it is: the symlinks have to be resolved first, or the
    /// `/private` prefix makes one file look like two.
    private nonisolated static func copy(_ url: URL, into documents: URL) -> URL? {
        let files = FileManager.default
        let destination = documents.appending(path: url.lastPathComponent)
        guard url.resolvingSymlinksInPath() != destination.resolvingSymlinksInPath() else {
            return destination
        }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        // Stage, then replace. A copy straight over the destination would delete a working map
        // before it knows the new one arrives whole.
        let staged = documents.appending(path: ".incoming-\(UUID().uuidString)")
        do {
            try files.copyItem(at: url, to: staged)
            guard files.fileExists(atPath: destination.path) else {
                try files.moveItem(at: staged, to: destination)
                return destination
            }
            return try files.replaceItemAt(destination, withItemAt: staged) ?? destination
        } catch {
            try? files.removeItem(at: staged)
            return nil
        }
    }

    /// Import one route into the open card.
    func importRoute(_ url: URL) {
        guard let host else {
            notice = "Import a map first."
            return
        }
        notice = obc_ios_import_route(host, url.path) == 0 ? "Imported \(url.lastPathComponent)" : hostError
    }

    /// Put a map on the card. The host closes first — the map is the one object a running device
    /// cannot have replaced under it — and the copy runs off the main thread, where a whole map
    /// belongs.
    func useAsMap(_ url: URL) {
        guard !isImporting else { return }
        offeredMap = nil
        // The import screen goes up before the host goes down, or the copy runs behind a dead
        // panel and four pads that answer nothing.
        state = .needsMap
        close()
        isImporting = true
        let card = card.path
        let map = url.path
        Task.detached(priority: .userInitiated) {
            // The error slot is thread-local, so it is read here and not after the hop.
            let failure = obc_ios_import_map(card, map) == 0 ? nil : String(cString: obc_ios_last_error())
            await self.finishMapImport(failure)
        }
    }

    private func finishMapImport(_ failure: String?) {
        isImporting = false
        if let failure {
            state = .failed(failure)
        } else {
            open()
        }
    }

    /// Delete a dropped file. Never during a map import: the file being copied onto the card may
    /// be this one.
    func delete(_ url: URL) {
        guard !isImporting else { return }
        do {
            try FileManager.default.removeItem(at: url)
        } catch {
            notice = "Could not delete \(url.lastPathComponent): \(error.localizedDescription)"
        }
    }

    /// The single map in Documents, when that is what is there.
    private func loneMap() -> URL? {
        let maps = inbox().filter { $0.pathExtension.caseInsensitiveCompare("obcm") == .orderedSame }
        return maps.count == 1 ? maps[0] : nil
    }
}
