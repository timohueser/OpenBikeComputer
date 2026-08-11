import Foundation

/// The weather service as the app can honestly describe it (WX13): when the baker last published,
/// what it published, and who the mosaic must credit.
///
/// Every word of provenance the app shows comes from here, and here comes from the manifest. There is
/// no provider table in the app: a source added by a baker deploy shows up on this screen on the next
/// manifest read, on a phone that shipped before that source existed, and a source removed stops
/// being credited the same way. That is the epic's law ("adding a source is a baker deploy") enforced
/// by having nothing else to render from.
///
/// It describes **one dataset**, because there is one (#1244). The per-product rows this type used to
/// carry — tier, bbox, a staleness deadline each — described a choice the client no longer makes, and
/// a screen that still listed them would be inviting a rider to wonder which one they got.
public struct WeatherServiceStatus: Equatable, Sendable {
    /// The published generation. Provenance and a cache key, never a switch.
    public var generation: String
    /// The manifest's own `generated_at` — when the baker published this state of the world.
    public var generatedAt: Date
    /// When this app last read the manifest (the client's cache may have served it).
    public var observedAt: Date
    /// The upstream run/observation time the generation is built from.
    public var referenceTime: Date
    /// After this the generation must not be used at all — expiry is a hard stop, never a quiet
    /// downgrade, and never a dry map.
    public var staleAfter: Date
    /// When the baker is due to publish the next generation.
    public var nextGenerationExpectedAt: Date
    /// The lattice's stated ground resolution.
    public var cellSizeMetres: UInt16
    public var frameCount: Int
    /// The furthest genuine frame timestamp; `nil` for a document with no usable frames.
    public var latestFrameValidAt: Date?
    /// Every credit that must be displayed, in manifest order.
    public var attributions: [WeatherAttribution]
    /// Manifest frames this build could not make sense of and skipped. Never fatal, never silent.
    public var skippedFrames: Int

    public init(
        generation: String, generatedAt: Date, observedAt: Date, referenceTime: Date,
        staleAfter: Date, nextGenerationExpectedAt: Date, cellSizeMetres: UInt16,
        frameCount: Int, latestFrameValidAt: Date?, attributions: [WeatherAttribution],
        skippedFrames: Int
    ) {
        self.generation = generation
        self.generatedAt = generatedAt
        self.observedAt = observedAt
        self.referenceTime = referenceTime
        self.staleAfter = staleAfter
        self.nextGenerationExpectedAt = nextGenerationExpectedAt
        self.cellSizeMetres = cellSizeMetres
        self.frameCount = frameCount
        self.latestFrameValidAt = latestFrameValidAt
        self.attributions = attributions
        self.skippedFrames = skippedFrames
    }

    init(manifest: WeatherManifestV2, observedAt: Date) {
        self.init(
            generation: manifest.generation, generatedAt: manifest.generatedAt,
            observedAt: observedAt, referenceTime: manifest.referenceTime,
            staleAfter: manifest.freshness.staleAfter,
            nextGenerationExpectedAt: manifest.freshness.nextGenerationExpectedAt,
            cellSizeMetres: manifest.lattice.cellSizeMetres, frameCount: manifest.frames.count,
            latestFrameValidAt: manifest.frames.map(\.validAt).max(),
            attributions: manifest.attributions, skippedFrames: manifest.skippedFrames)
    }

    /// Usable only while `now` has not passed the generation's deadline.
    public func isFresh(at now: Date) -> Bool { now <= staleAfter }
}

/// Reading the service's health and credits — the manifest, and nothing else.
///
/// A separate seam from ``PrecipitationGridProvider`` because it asks a different question and must
/// be answerable without one: this call carries **no corridor and no coordinate**, so opening the
/// weather screen tells the CDN a phone asked for a global document, exactly as much as it learns
/// from any other manifest read.
public protocol WeatherServiceStatusProviding: Sendable {
    func serviceStatus(now: Date) async throws -> WeatherServiceStatus
}
