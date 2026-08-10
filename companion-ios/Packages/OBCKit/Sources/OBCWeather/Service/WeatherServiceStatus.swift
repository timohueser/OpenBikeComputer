import Foundation

/// The weather service as the app can honestly describe it (WX13): when the baker last published,
/// what it published, and who each product must credit.
///
/// Every word of provenance the app shows comes from here, and here comes from the manifest. There
/// is no provider table in the app: a source added by a baker deploy shows up on this screen on the
/// next manifest read, on a phone that shipped before that source existed, and a source removed
/// stops being credited the same way. That is the epic's law ("adding a source is a baker deploy")
/// enforced by having nothing else to render from.
public struct WeatherServiceStatus: Equatable, Sendable {
    /// The manifest's own `generated_at` — when the baker published this state of the world.
    public var generatedAt: Date
    /// When this app last read the manifest (a 60 s client cache may have served it).
    public var observedAt: Date
    public var products: [WeatherServiceProductStatus]
    /// Manifest entries this build could not make sense of and skipped. Never fatal, never silent.
    public var skippedProducts: Int

    public init(
        generatedAt: Date, observedAt: Date, products: [WeatherServiceProductStatus],
        skippedProducts: Int
    ) {
        self.generatedAt = generatedAt
        self.observedAt = observedAt
        self.products = products
        self.skippedProducts = skippedProducts
    }

    /// The products whose staleness deadline `now` has passed — the honest "service data stale
    /// since …" state, stated per product rather than as one global verdict, because the baker can
    /// be behind on Germany and current everywhere else.
    public func staleProducts(at now: Date) -> [WeatherServiceProductStatus] {
        products.filter { !$0.isFresh(at: now) }
    }

    /// Every credit line that must be displayed, de-duplicated, in manifest order. Two products
    /// from one upstream (DWD radar and a DWD model, say) credit it once.
    public var attributions: [WeatherAttribution] {
        var seen: Set<WeatherAttribution> = []
        var ordered: [WeatherAttribution] = []
        for product in products where seen.insert(product.attribution).inserted {
            ordered.append(product.attribution)
        }
        return ordered
    }
}

/// One published product, reduced to what a rider-facing screen may state about it.
public struct WeatherServiceProductStatus: Equatable, Sendable {
    /// Provenance and cache-key material only — never a switch (the app has no product allow-list).
    public var id: String
    public var tier: WeatherTier
    public var nominalCellMetres: UInt16
    /// The upstream run/observation time the product is built from.
    public var referenceTime: Date
    /// When the baker produced this entry.
    public var generatedAt: Date
    /// After this, the product must not be used at all — expiry is a hard stop, never a quiet
    /// downgrade.
    public var stalenessDeadline: Date
    public var attribution: WeatherAttribution
    public var frameCount: Int
    /// The furthest genuine frame timestamp; `nil` for a product with no frames (which the manifest
    /// parser already refuses, so this is belt-and-braces).
    public var latestFrameValidAt: Date?

    public init(
        id: String, tier: WeatherTier, nominalCellMetres: UInt16, referenceTime: Date,
        generatedAt: Date, stalenessDeadline: Date, attribution: WeatherAttribution,
        frameCount: Int, latestFrameValidAt: Date?
    ) {
        self.id = id
        self.tier = tier
        self.nominalCellMetres = nominalCellMetres
        self.referenceTime = referenceTime
        self.generatedAt = generatedAt
        self.stalenessDeadline = stalenessDeadline
        self.attribution = attribution
        self.frameCount = frameCount
        self.latestFrameValidAt = latestFrameValidAt
    }

    public func isFresh(at now: Date) -> Bool { now <= stalenessDeadline }

    init(product: WeatherServiceProduct) {
        self.init(
            id: product.id, tier: product.tier, nominalCellMetres: product.nominalCellMetres,
            referenceTime: product.referenceTime, generatedAt: product.generatedAt,
            stalenessDeadline: product.stalenessDeadline, attribution: product.attribution,
            frameCount: product.frames.count,
            latestFrameValidAt: product.frames.map(\.validAt).max())
    }
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
