import Foundation
import OBCWeatherWire

/// The parsed `wx/v1/manifest.json` — the frozen WX5 delivery contract
/// (`host/obc-wx-bake/schema/manifest.schema.json`, `specs/OBCG_Spec.md` §10).
///
/// This is the *whole* of the client's selection knowledge. There is no country table, no region
/// list and no product allow-list anywhere in the app: coverage is `bbox_udeg`, preference is
/// `tier`, usability is `staleness_deadline`, and credit is `attribution`. Adding Austria or a new
/// upstream source is a baker deploy that this parser will happily read on a phone that shipped
/// before it existed.
public struct WeatherServiceManifest: Equatable, Sendable {
    /// The only document version this build understands. A different one is an outage, not
    /// something to guess at.
    public static let supportedVersion = 1

    public var version: Int
    public var generatedAt: Date
    public var products: [WeatherServiceProduct]

    public init(version: Int, generatedAt: Date, products: [WeatherServiceProduct]) {
        self.version = version
        self.generatedAt = generatedAt
        self.products = products
    }
}

public struct WeatherServiceProduct: Equatable, Sendable {
    /// Provenance and cache-key material only. Never a switch: the moment code branches on this
    /// string, "add a region without an app release" is over.
    public var id: String
    public var tier: WeatherTier
    /// Where the *whole timeline* is answerable — the intersection of the frames' windows.
    public var bounds: WeatherBoundingBox
    public var cellLatitudeMicrodegrees: UInt32
    public var cellLongitudeMicrodegrees: UInt32
    public var nominalCellMetres: UInt16
    public var referenceTime: Date
    public var generatedAt: Date
    /// The moment this product must stop being used if no fresher manifest replaced it.
    public var stalenessDeadline: Date
    public var attribution: WeatherAttribution
    public var frames: [WeatherServiceFrame]

    public init(
        id: String, tier: WeatherTier, bounds: WeatherBoundingBox,
        cellLatitudeMicrodegrees: UInt32, cellLongitudeMicrodegrees: UInt32,
        nominalCellMetres: UInt16, referenceTime: Date, generatedAt: Date,
        stalenessDeadline: Date, attribution: WeatherAttribution, frames: [WeatherServiceFrame]
    ) {
        self.id = id
        self.tier = tier
        self.bounds = bounds
        self.cellLatitudeMicrodegrees = cellLatitudeMicrodegrees
        self.cellLongitudeMicrodegrees = cellLongitudeMicrodegrees
        self.nominalCellMetres = nominalCellMetres
        self.referenceTime = referenceTime
        self.generatedAt = generatedAt
        self.stalenessDeadline = stalenessDeadline
        self.attribution = attribution
        self.frames = frames
    }

    /// Usable only while `now` has not passed the deadline. Expiry is a hard stop, never a quiet
    /// downgrade: expired rain must not produce an alert or a dry claim.
    public func isFresh(at now: Date) -> Bool { now <= stalenessDeadline }
}

public enum WeatherSourceClass: String, Equatable, Sendable {
    case observation
    case forecast
}

public struct WeatherServiceFrame: Equatable, Sendable {
    public var offsetMinutes: UInt32
    /// The genuine upstream validity time. A latent observation keeps its old timestamp.
    public var validAt: Date
    public var sourceClass: WeatherSourceClass
    /// The immutable object key. Immutable is why a frame is cached by key and **never**
    /// revalidated: the bytes behind a key cannot change.
    public var key: String
    public var byteLength: Int
    public var objectCRC32: UInt32
    public var geometry: WeatherFrameGeometry

    public init(
        offsetMinutes: UInt32, validAt: Date, sourceClass: WeatherSourceClass, key: String,
        byteLength: Int, objectCRC32: UInt32, geometry: WeatherFrameGeometry
    ) {
        self.offsetMinutes = offsetMinutes
        self.validAt = validAt
        self.sourceClass = sourceClass
        self.key = key
        self.byteLength = byteLength
        self.objectCRC32 = objectCRC32
        self.geometry = geometry
    }
}

/// The frame's exact OBCG geometry, restated by the manifest so corridor page arithmetic is
/// plannable before a single byte is fetched — and verifiable against the header once it is.
public struct WeatherFrameGeometry: Equatable, Sendable {
    public var southMicrodegrees: Int32
    public var westMicrodegrees: Int32
    public var cellLatitudeMicrodegrees: UInt32
    public var cellLongitudeMicrodegrees: UInt32
    public var width: UInt32
    public var height: UInt32
    public var cellSizeMetres: UInt16
    public var tileEdge: UInt16
    public var entriesPerPage: UInt16

    public init(
        southMicrodegrees: Int32, westMicrodegrees: Int32, cellLatitudeMicrodegrees: UInt32,
        cellLongitudeMicrodegrees: UInt32, width: UInt32, height: UInt32, cellSizeMetres: UInt16,
        tileEdge: UInt16, entriesPerPage: UInt16
    ) {
        self.southMicrodegrees = southMicrodegrees
        self.westMicrodegrees = westMicrodegrees
        self.cellLatitudeMicrodegrees = cellLatitudeMicrodegrees
        self.cellLongitudeMicrodegrees = cellLongitudeMicrodegrees
        self.width = width
        self.height = height
        self.cellSizeMetres = cellSizeMetres
        self.tileEdge = tileEdge
        self.entriesPerPage = entriesPerPage
    }

    public var bounds: WeatherBoundingBox {
        WeatherBoundingBox(
            southMicrodegrees: Int64(southMicrodegrees),
            westMicrodegrees: Int64(westMicrodegrees),
            northMicrodegrees: Int64(southMicrodegrees)
                + Int64(height) * Int64(cellLatitudeMicrodegrees),
            eastMicrodegrees: Int64(westMicrodegrees)
                + Int64(width) * Int64(cellLongitudeMicrodegrees))
    }

    /// Everything the OBCG header must agree with. A frame whose fetched header contradicts the
    /// manifest is refused: one of the two is lying, and neither is worth guessing about.
    func agrees(with header: OBCGridHeader) -> Bool {
        header.southLatitudeMicrodegrees == southMicrodegrees
            && header.westLongitudeMicrodegrees == westMicrodegrees
            && header.cellLatitudeStrideMicrodegrees == cellLatitudeMicrodegrees
            && header.cellLongitudeStrideMicrodegrees == cellLongitudeMicrodegrees
            && header.width == width && header.height == height
            && header.cellSizeMetres == cellSizeMetres && header.tileEdge == tileEdge
            && header.entriesPerPage == entriesPerPage
    }
}

/// Why a manifest document could not be used at all. Every case degrades to a service outage —
/// the hourly forecast still works, and the rider is told there is no rain map.
public enum WeatherManifestError: Error, Equatable, Sendable {
    case malformed
    case unsupportedVersion(Int)
}

public extension WeatherServiceManifest {
    /// Parse and validate a manifest document.
    ///
    /// Two different strictnesses, deliberately:
    ///
    /// - the **document** is strict — bad JSON or an unknown `version` is an outage, because a
    ///   client that guesses at a document shape it does not know will eventually guess wrong about
    ///   coverage;
    /// - a **product entry** is lenient — an entry this build cannot make sense of is skipped and
    ///   counted, never fatal. One malformed product must not cost a rider every other region, and
    ///   an entry with fields we have never heard of is exactly what forward compatibility looks
    ///   like.
    static func parse(_ data: Data) throws -> (manifest: WeatherServiceManifest, skippedProducts: Int) {
        let document: Document
        do {
            document = try JSONDecoder().decode(Document.self, from: data)
        } catch {
            throw WeatherManifestError.malformed
        }
        guard document.version == supportedVersion else {
            throw WeatherManifestError.unsupportedVersion(document.version)
        }
        guard let generatedAt = RFC3339.parse(document.generated_at) else {
            throw WeatherManifestError.malformed
        }
        var products: [WeatherServiceProduct] = []
        var skipped = 0
        for entry in document.products {
            if let product = entry.entry?.validated() {
                products.append(product)
            } else {
                skipped += 1
            }
        }
        return (
            WeatherServiceManifest(
                version: document.version, generatedAt: generatedAt, products: products),
            skipped)
    }

    // MARK: - Wire shape

    private struct Document: Decodable {
        var version: Int
        var generated_at: String
        var products: [LenientProductEntry]
    }

    /// One element of `products[]`, decoded so that a *broken element* cannot fail the array.
    ///
    /// This is what makes "entry-lenient" real rather than merely semantic. Decoding
    /// `[ProductEntry]` directly means one entry with `"tier": "radar"`, or one missing a required
    /// key, throws out of the array decode and takes the whole manifest with it — every region
    /// offline because of one bad product. Catching inside the element keeps the failure the size
    /// of the thing that failed; the *document* stays strict, and the skip is counted.
    private struct LenientProductEntry: Decodable {
        var entry: ProductEntry?

        init(from decoder: any Decoder) throws {
            entry = try? ProductEntry(from: decoder)
        }
    }

    private struct ProductEntry: Decodable {
        var id: String
        var tier: UInt8
        var bbox_udeg: BboxEntry
        var cell: CellEntry
        var reference_time: String
        var generated_at: String
        var staleness_deadline: String
        var attribution: AttributionEntry
        var frames: [FrameEntry]

        func validated() -> WeatherServiceProduct? {
            // Tier 0 is "invalid" in the OBCG registry; a product without frames answers nothing.
            guard tier != 0, !frames.isEmpty else { return nil }
            guard let reference = RFC3339.parse(reference_time),
                  let generated = RFC3339.parse(generated_at),
                  let deadline = RFC3339.parse(staleness_deadline)
            else { return nil }
            let bounds = bbox_udeg.bounds
            guard bounds.isWellFormed, cell.lat_udeg > 0, cell.lon_udeg > 0 else { return nil }
            var validFrames: [WeatherServiceFrame] = []
            for frame in frames {
                guard let validated = frame.validated() else { return nil }
                validFrames.append(validated)
            }
            // Strictly increasing genuine timestamps: OBCW §5 requires it of the bundle, and a
            // manifest that cannot supply it is not something to sort into shape.
            for index in 1..<Swift.max(1, validFrames.count) where
                validFrames[index].validAt <= validFrames[index - 1].validAt { return nil }
            // The bbox is defined as the intersection of the frames' windows — the region where the
            // whole timeline is answerable. A product claiming more than its own frames cover is
            // claiming coverage it cannot deliver, and selection would hand back a rain map with
            // holes in it, so the entry is refused rather than trusted.
            for frame in validFrames where !frame.geometry.bounds.contains(bounds) { return nil }
            return WeatherServiceProduct(
                id: id, tier: WeatherTier(rawValue: tier), bounds: bounds,
                cellLatitudeMicrodegrees: cell.lat_udeg, cellLongitudeMicrodegrees: cell.lon_udeg,
                nominalCellMetres: cell.nominal_m, referenceTime: reference,
                generatedAt: generated, stalenessDeadline: deadline,
                attribution: WeatherAttribution(text: attribution.text, url: attribution.url),
                frames: validFrames)
        }
    }

    private struct BboxEntry: Decodable {
        var south_udeg: Int64
        var west_udeg: Int64
        var north_udeg: Int64
        var east_udeg: Int64

        var bounds: WeatherBoundingBox {
            WeatherBoundingBox(
                southMicrodegrees: south_udeg, westMicrodegrees: west_udeg,
                northMicrodegrees: north_udeg, eastMicrodegrees: east_udeg)
        }
    }

    private struct CellEntry: Decodable {
        var lat_udeg: UInt32
        var lon_udeg: UInt32
        var nominal_m: UInt16
    }

    private struct AttributionEntry: Decodable {
        var text: String
        var url: String
    }

    private struct FrameEntry: Decodable {
        var offset_min: UInt32
        var valid_at: String
        var source_class: String
        var key: String
        var bytes: UInt64
        var object_crc32: String
        var geometry: GeometryEntry

        func validated() -> WeatherServiceFrame? {
            guard let validAt = RFC3339.parse(valid_at),
                  let sourceClass = WeatherSourceClass(rawValue: source_class),
                  let crc = UInt32(object_crc32.dropFirst(2), radix: 16),
                  object_crc32.hasPrefix("0x"), bytes > 0, bytes <= UInt64(Int32.max),
                  !key.isEmpty, !key.hasPrefix("/"), !key.contains(".."),
                  let geometry = geometry.validated()
            else { return nil }
            return WeatherServiceFrame(
                offsetMinutes: offset_min, validAt: validAt, sourceClass: sourceClass, key: key,
                byteLength: Int(bytes), objectCRC32: crc, geometry: geometry)
        }
    }

    private struct GeometryEntry: Decodable {
        var south_udeg: Int32
        var west_udeg: Int32
        var cell_lat_udeg: UInt32
        var cell_lon_udeg: UInt32
        var width: UInt32
        var height: UInt32
        var cell_size_m: UInt16
        var tile_edge: UInt16
        var entries_per_page: UInt16

        func validated() -> WeatherFrameGeometry? {
            // The same bounds OBCG §1/§3 put on a header. Checking them here means corridor
            // arithmetic never runs on numbers the fetched header would have rejected anyway.
            guard cell_lat_udeg > 0, cell_lon_udeg > 0, width > 0, height > 0,
                  width <= OBCGridCodec.maximumGridDimension,
                  height <= OBCGridCodec.maximumGridDimension,
                  UInt64(width) * UInt64(height) <= OBCGridCodec.maximumGridCells,
                  cell_size_m > 0,
                  tile_edge >= OBCGridCodec.minimumTileEdge, tile_edge <= OBCGridCodec.maximumTileEdge,
                  tile_edge.nonzeroBitCount == 1,
                  entries_per_page > 0, entries_per_page <= OBCGridCodec.maximumEntriesPerPage
            else { return nil }
            let geometry = WeatherFrameGeometry(
                southMicrodegrees: south_udeg, westMicrodegrees: west_udeg,
                cellLatitudeMicrodegrees: cell_lat_udeg, cellLongitudeMicrodegrees: cell_lon_udeg,
                width: width, height: height, cellSizeMetres: cell_size_m, tileEdge: tile_edge,
                entriesPerPage: entries_per_page)
            guard geometry.bounds.isWellFormed else { return nil }
            return geometry
        }
    }
}
