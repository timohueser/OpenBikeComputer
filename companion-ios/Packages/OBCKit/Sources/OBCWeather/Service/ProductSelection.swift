import Foundation

/// The outcome of manifest-only product selection.
public enum ProductSelectionOutcome: Equatable, Sendable {
    case selected(WeatherServiceProduct)
    case none(NoRainMapReason)
}

/// Product selection: a pure function of manifest data, the corridor bbox and the clock.
///
/// Every rule below is *data*, and that is the point of the whole architecture. There is no country
/// check, no `if productID == "dwd-rv"`, and no hard-coded region — a German corridor picks the
/// radar product because the radar product's bbox covers it and its deadline has not passed, and a
/// corridor in a country the project has never thought about picks the worldwide floor for exactly
/// the same reason.
public enum ProductSelection {
    /// How far the device clock may lead the manifest before freshness arithmetic stops being
    /// trustworthy. Beyond it the client still selects (the manifest's own ordering is intact) but
    /// says so in diagnostics rather than silently trusting or silently discarding a product.
    public static let clockSkewTolerance: TimeInterval = 15 * 60

    public static func select(
        from manifest: WeatherServiceManifest, corridor: WeatherCorridor, now: Date
    ) -> (outcome: ProductSelectionOutcome, expired: [String]) {
        let covering = manifest.products.filter { $0.bounds.contains(corridor.bounds) }
        guard !covering.isEmpty else { return (.none(.corridorNotCovered), []) }

        let fresh = covering.filter { $0.isFresh(at: now) }
        guard !fresh.isEmpty else {
            // Covered, but nothing usable. The rider is told the rain map is out of date; expired
            // frames are never shown, never alerted on, and never read as "dry".
            let latest = covering.map(\.stalenessDeadline).max() ?? now
            return (.none(.allCoveringProductsExpired(latestDeadline: latest)), covering.map(\.id))
        }

        // A candidate must also have a frame inside the two-hour question. Freshness and frame
        // availability are *different* facts: a product entry can sit inside its staleness deadline
        // while its newest frame is hours old — a baker that stopped mid-cycle, an upstream run that
        // never landed. Without this check such a product shadows a perfectly usable lower tier and
        // the corridor ends up with no rain map at all. It costs nothing: frame timestamps are
        // manifest data, so nothing is fetched to find out.
        let answerable = fresh.filter { !frames(of: $0, now: now).isEmpty }
        guard !answerable.isEmpty else {
            return (.none(.noFramesInWindow), covering.filter { !$0.isFresh(at: now) }.map(\.id))
        }

        // Highest tier wins (tier 1 radar beats 2 model beats 3 floor, and an unknown tier orders
        // by its number). Ties break on the newer upstream run, then on id — so two equally good
        // products always pick the same one, which keeps a bundle reproducible.
        let best = answerable.min { lhs, rhs in
            if lhs.tier != rhs.tier { return lhs.tier < rhs.tier }
            if lhs.referenceTime != rhs.referenceTime { return lhs.referenceTime > rhs.referenceTime }
            return lhs.id < rhs.id
        }
        guard let best else { return (.none(.corridorNotCovered), []) }
        let expired = covering.filter { !$0.isFresh(at: now) }.map(\.id)
        return (.selected(best), expired)
    }

    /// Frames worth fetching for a two-hour question, in wire order.
    ///
    /// Both ends are honest rather than convenient: a genuinely latent observation (IMERG-shaped,
    /// hours old) is kept with its real timestamp because it is the best *observation* there is,
    /// and nothing beyond the two-hour horizon is fetched because nothing beyond it is asked.
    /// Timestamps are never re-stamped to make a frame look current.
    public static func frames(
        of product: WeatherServiceProduct, now: Date,
        horizon: TimeInterval = WeatherCorridor.horizon,
        maximumObservationAge: TimeInterval = 6 * 3_600
    ) -> [WeatherServiceFrame] {
        product.frames.filter { frame in
            frame.validAt <= now.addingTimeInterval(horizon)
                && frame.validAt >= now.addingTimeInterval(-maximumObservationAge)
        }
    }

    /// True when the manifest claims to have been produced meaningfully after this device thinks
    /// "now" is. Reported, never compensated for.
    public static func clockSkewSuspected(manifest: WeatherServiceManifest, now: Date) -> Bool {
        manifest.generatedAt.timeIntervalSince(now) > clockSkewTolerance
    }
}
